//! Spec 016 — zero-config bootstrap (AC-016-01/02).
//!
//! The window must use the same configuration directory and the same `default` session the
//! `herdr` CLI uses, and start that session detached (same form as the TUI bootstrap) when it
//! is not running. These tests never start a real engine and never touch `~/.config/herdr`:
//! the configuration comes from an explicit environment lookup and the engine is a fake
//! executable in a temporary directory; the socket is a `UnixListener` of the test itself.

#![cfg(unix)]

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use herdr_client::bootstrap::{
    bootstrap_from_env, ensure_session_running, server_timeout_error, start_session_detached,
    ENV_CONFIG_DIR, ENV_HERDR_BIN, ENV_SESSION, SESSION_START_TIMEOUT,
};
use herdr_client::{SessionName, SessionPaths};

/// Environment lookup over explicit pairs (never the process environment).
fn env_of(pairs: Vec<(&'static str, String)>) -> impl Fn(&str) -> Option<String> {
    move |key| {
        pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone())
    }
}

/// `path` as the environment lookup sees it.
fn as_env_value(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn write_fake_herdr(dir: &Path) -> PathBuf {
    let path = dir.join("fake-herdr");
    std::fs::write(
        &path,
        "#!/bin/sh\nprintf '%s' \"$*\" > \"${0%/*}/fake-herdr.argv\"\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn argv_file(fake: &Path) -> PathBuf {
    fake.parent().unwrap().join("fake-herdr.argv")
}

fn wait_for_file(path: &Path) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "fake herdr never wrote {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Binds a listener for `paths.client_socket` after `delay` and keeps it alive until the end
/// of the test, emulating the engine becoming ready a moment after the detached start.
fn bind_client_socket_later(paths: &SessionPaths, delay: Duration) -> mpsc::Receiver<()> {
    let (ready_tx, ready_rx) = mpsc::channel();
    let socket = paths.client_socket.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = UnixListener::bind(&socket).expect("bind fake client socket");
        ready_tx.send(()).unwrap();
        // Held until the receiver is dropped by the test.
        std::thread::sleep(Duration::from_secs(10));
        let _ = listener;
    });
    ready_rx
}

// AC-016-01: no HERDR_DESKTOP_* variable → the CLI's default config dir and the `default`
// session, with the automatic start enabled.
#[test]
fn zero_config_resolves_the_cli_default_directory_and_the_default_session() {
    let home = tempfile::tempdir().unwrap();
    let env = env_of(vec![("HOME", as_env_value(home.path()))]);
    let config = bootstrap_from_env(&env).unwrap();

    assert_eq!(
        config.config_dir,
        home.path().join(".config").join("herdr"),
        "same directory resolution as the CLI"
    );
    assert_eq!(config.session.unwrap().as_str(), "default");
    assert!(config.auto_start, "zero-config starts the default session");
    assert_eq!(config.herdr_bin, PathBuf::from("herdr"));

    // The default session lives directly in the config dir (ports `data_dir_for(None)`).
    let paths = SessionPaths::for_session(&config.config_dir, &SessionName::default_session());
    assert_eq!(paths.data_dir, config.config_dir);
    assert_eq!(
        paths.client_socket,
        config.config_dir.join("herdr-client.sock")
    );
    assert_eq!(paths.api_socket, config.config_dir.join("herdr.sock"));
}

// Parity with the CLI resolution: XDG_CONFIG_HOME wins over HOME.
#[test]
fn zero_config_honours_xdg_config_home_like_the_cli() {
    let xdg = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let env = env_of(vec![
        ("XDG_CONFIG_HOME", as_env_value(xdg.path())),
        ("HOME", as_env_value(home.path())),
    ]);
    let config = bootstrap_from_env(&env).unwrap();
    assert_eq!(config.config_dir, xdg.path().join("herdr"));
}

// AC-016-01 edge: HERDR_DESKTOP_SESSION defined keeps the named-session behavior (no
// automatic start); HERDR_DESKTOP_CONFIG_DIR and HERDR_DESKTOP_HERDR_BIN stay valid overrides.
#[test]
fn overrides_keep_the_named_session_without_automatic_start() {
    let home = tempfile::tempdir().unwrap();
    let override_dir = tempfile::tempdir().unwrap();
    let env = env_of(vec![
        ("HOME", as_env_value(home.path())),
        (ENV_SESSION, "hd016-work".into()),
        (ENV_CONFIG_DIR, as_env_value(override_dir.path())),
        (ENV_HERDR_BIN, "/opt/herdr/bin/herdr".into()),
    ]);
    let config = bootstrap_from_env(&env).unwrap();
    assert_eq!(config.session.unwrap().as_str(), "hd016-work");
    assert!(!config.auto_start, "an explicit session never autostarts");
    assert_eq!(config.config_dir, override_dir.path());
    assert_eq!(config.herdr_bin, PathBuf::from("/opt/herdr/bin/herdr"));
}

// The name `default` is a valid session name (the desktop now addresses it); an empty
// override is the same as no override.
#[test]
fn the_default_name_is_accepted_and_empty_values_fall_back_to_zero_config() {
    assert!(SessionName::parse("default").is_ok());
    assert!(SessionName::parse("hd016-x").is_ok());
    assert!(SessionName::parse("bad/name").is_err());

    let home = tempfile::tempdir().unwrap();
    let env = env_of(vec![
        ("HOME", as_env_value(home.path())),
        (ENV_SESSION, "   ".into()),
    ]);
    let config = bootstrap_from_env(&env).unwrap();
    assert_eq!(config.session.unwrap().as_str(), "default");
    assert!(config.auto_start);
}

// Spec 067 (AC-067-04): the real-window harnesses launch the app with `HERDR_DESKTOP_LOCALE=pt`
// next to `HERDR_DESKTOP_SESSION`. Would catch: the locale override reaching the bootstrap (a
// window that resolved a session or a configuration directory from the language).
#[test]
fn the_harness_locale_override_does_not_change_the_bootstrap() {
    let home = tempfile::tempdir().unwrap();
    let env = env_of(vec![
        ("HOME", as_env_value(home.path())),
        ("HERDR_DESKTOP_LOCALE", "pt".into()),
    ]);
    let config = bootstrap_from_env(&env).unwrap();
    assert_eq!(config.session.unwrap().as_str(), "default");
    assert!(config.auto_start);
    assert_eq!(config.config_dir, home.path().join(".config/herdr"));
}

// AC-016-02: `herdr` absent from PATH is one literal line, retryable, no engine started.
#[test]
fn a_missing_herdr_is_the_literal_line_and_nothing_is_started() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("herdr-not-installed");
    let session = SessionName::parse("hd016-missing").unwrap();
    let paths = SessionPaths::for_session(dir.path(), &session);

    let error = ensure_session_running(
        &missing,
        dir.path(),
        &session,
        None,
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(error.code, "herdr_not_found");
    assert_eq!(error.message, "herdr was not found in PATH — install Herdr");
    assert!(error.retryable);
    assert_eq!(error.endpoint.as_deref(), Some("local"));
    assert!(!paths.data_dir.exists(), "nothing was created");
}

// AC-016-01: an absent session is started detached with the TUI's form (`herdr server`,
// no `--session` for the default) and waited for until the client socket accepts.
#[test]
fn an_absent_default_session_is_started_detached_and_waited_for() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("herdr-config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let fake = write_fake_herdr(dir.path());
    let session = SessionName::default_session();
    let paths = SessionPaths::for_session(&config_dir, &session);
    let ready = bind_client_socket_later(&paths, Duration::from_millis(150));

    ensure_session_running(&fake, &config_dir, &session, None, Duration::from_secs(5))
        .expect("the fake server became ready inside the budget");

    assert_eq!(wait_for_file(&argv_file(&fake)), "server");
    ready.recv_timeout(Duration::from_secs(1)).unwrap();
}

// A named session keeps `--session <name>` (the previous behavior of the detached start).
#[test]
fn a_named_session_is_started_with_the_session_argument() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_herdr(dir.path());
    let session = SessionName::parse("hd016-named").unwrap();

    start_session_detached(&fake, &session, None).unwrap();
    assert_eq!(
        wait_for_file(&argv_file(&fake)),
        "--session hd016-named server"
    );
}

// AC-016-01 edge: a session already running is never started again.
#[test]
fn a_running_session_is_not_started_again() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("herdr-config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let fake = write_fake_herdr(dir.path());
    let session = SessionName::default_session();
    let paths = SessionPaths::for_session(&config_dir, &session);
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let _listener = UnixListener::bind(&paths.client_socket).unwrap();

    ensure_session_running(&fake, &config_dir, &session, None, Duration::from_secs(1)).unwrap();
    assert!(
        !argv_file(&fake).exists(),
        "an available session must not be started"
    );
}

// AC-016-02: a server that never becomes ready is the timeout literal (the product budget is
// 10 s), retryable, and the caller can retry the same bootstrap.
#[test]
fn a_start_that_never_becomes_ready_times_out_with_the_ten_second_line() {
    assert_eq!(SESSION_START_TIMEOUT, Duration::from_secs(10));
    let literal = server_timeout_error(SESSION_START_TIMEOUT);
    assert_eq!(literal.code, "server_start_timeout");
    assert_eq!(literal.message, "the server did not answer within 10 s");
    assert!(literal.retryable);

    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("herdr-config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let fake = write_fake_herdr(dir.path());
    let session = SessionName::default_session();

    let error = ensure_session_running(
        &fake,
        &config_dir,
        &session,
        None,
        Duration::from_millis(150),
    )
    .unwrap_err();
    assert_eq!(error.code, "server_start_timeout");
    assert_eq!(
        error.message,
        server_timeout_error(Duration::from_millis(150)).message
    );
    assert!(error.retryable);
    // The retry repeats the bootstrap: the fake engine is spawned again.
    let error_again = ensure_session_running(
        &fake,
        &config_dir,
        &session,
        None,
        Duration::from_millis(100),
    )
    .unwrap_err();
    assert_eq!(error_again.code, "server_start_timeout");
    assert_eq!(wait_for_file(&argv_file(&fake)), "server");
}

// A stale socket file (nobody listening) is not "running": the start is attempted.
#[test]
fn a_stale_socket_is_treated_as_an_absent_session() {
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("herdr-config");
    std::fs::create_dir_all(&config_dir).unwrap();
    let fake = write_fake_herdr(dir.path());
    let session = SessionName::default_session();
    let paths = SessionPaths::for_session(&config_dir, &session);
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    drop(UnixListener::bind(&paths.client_socket).unwrap());

    let _ = ensure_session_running(
        &fake,
        &config_dir,
        &session,
        None,
        Duration::from_millis(150),
    );
    assert_eq!(wait_for_file(&argv_file(&fake)), "server");
}

// The fake engine above is what the desktop starts; this guards the helper itself (the argv
// file is written by a real detached process, not by the test).
#[test]
fn the_fake_engine_really_is_a_detached_process() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_herdr(dir.path());
    let session = SessionName::parse("hd016-detach").unwrap();
    let pid = start_session_detached(&fake, &session, None).unwrap();
    assert!(pid > 0);
    // Reading the file also proves the child had its own cwd-independent script path.
    let line = wait_for_file(&argv_file(&fake));
    assert!(line.starts_with("--session hd016-detach"), "{line}");
    let output = std::process::Command::new("ps")
        .args(["-o", "sid=,pid=", "-p", &pid.to_string()])
        .output();
    if let Ok(output) = output {
        let text = String::from_utf8_lossy(&output.stdout);
        let mut parts = text.split_whitespace();
        if let (Some(sid), Some(child)) = (parts.next(), parts.next()) {
            assert_eq!(sid, child, "the engine must be its own session leader");
        }
    }
}

// Guards that the fake helper really is executable and its argv is visible to the test (a
// sanity check of the fixture, not of the product code).
#[test]
fn the_fake_engine_file_is_executable() {
    let dir = tempfile::tempdir().unwrap();
    let fake = write_fake_herdr(dir.path());
    let output = std::process::Command::new(&fake)
        .arg("--probe")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(wait_for_file(&argv_file(&fake)), "--probe");
}
