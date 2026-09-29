//! Contracts of the spec 007 remote fixture plus ONE explicitly ignored smoke of real processes.
//!
//! Normal tests need no engine binary, sshd or previous build: they use fake engines written to
//! a temp dir. The smoke requires the pinned reference engine, the legacy engine and OpenSSH.

#[path = "../../tests/fidelity-native/remote.rs"]
pub mod remote;

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use herdr_client::SessionName;
use herdr_desktop::connections::hub::ApiLane;
use herdr_desktop::connections::ssh_options::{
    build_sftp, remote_command_line, RemoteHerdrCommand,
};
use serde_json::{json, Value};

use remote::{
    compute_sha256, disposable, forced_command_script, get_proc_starttime, is_proc_alive, may_stop,
    pair_problems, pinned_sha256_matches, port_listening, remote_exec_cmd, shell_quote,
    terminate_proc, verify_ledger_clean, HostFixture, HostNamespace, Ledger, LedgerEntry,
    RemoteFixture, RemoteFixtureConfig, RemoteFixtureGuard, StartupStage, TrackedProcess,
    TrackedSession, LEGACY_ENGINE_PATH, REFERENCE_ENGINE_PATH, REFERENCE_ENGINE_SHA256,
    SESSION_PREFIX, SHARED_PANE,
};

fn write_exec(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------------------------------
// Contract tests (deterministic, no machine prerequisites)
// ---------------------------------------------------------------------------------------

#[test]
fn contract_refuses_non_disposable_or_invalid_session_prefix() {
    let created = vec![
        format!("{SESSION_PREFIX}valid-1"),
        format!("{SESSION_PREFIX}valid-2"),
    ];
    for bad in [
        "default",
        "normal-session",
        "hd006-remote-a",
        "hd007-",
        "",
        "   ",
    ] {
        assert!(disposable(bad).is_err(), "{bad:?} accepted");
        assert!(!may_stop(bad, &created), "{bad:?} stoppable");
    }
    for evil in [
        "hd007-evil;rm -rf /",
        "hd007-evil$(whoami)",
        "hd007-evil`id`",
        "hd007-evil'name'",
    ] {
        assert!(disposable(evil).is_err(), "{evil:?} accepted");
    }
    let not_created = format!("{SESSION_PREFIX}foreign-run");
    assert!(disposable(&not_created).is_ok());
    assert!(!may_stop(&not_created, &created));
    assert!(may_stop(&created[0], &created));
}

#[test]
fn contract_params_escaped_without_insecure_interpolation() {
    for raw in [
        "simple",
        "single'quote",
        "double\"quotes\"",
        "; rm -rf / ;",
        "$USER and ${HOME}",
        "$(whoami) and `id`",
        "newlines\nand\ttabs",
        "UTF-8: Olá, café, 🚀, 日本語",
    ] {
        let quoted = shell_quote(raw);
        let out = Command::new("sh")
            .args(["-c", &format!("printf '%s' {quoted}")])
            .output()
            .expect("run sh");
        assert_eq!(String::from_utf8_lossy(&out.stdout), raw);
        let cmd = remote_exec_cmd(raw, "status server --json");
        assert!(cmd.starts_with("exec herdr --session '"));
        assert!(cmd.ends_with("' status server --json"));
    }
}

/// Would catch: a forced command that passes arbitrary/injected command lines through, runs
/// another session, or lets the caller's HERDR_SOCKET_PATH/HOME/PATH reach the engine.
#[test]
fn contract_forced_command_accepts_only_product_lines_with_private_namespace() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = tmp.path().join("fake-engine");
    write_exec(
        &engine,
        "#!/bin/sh\nprintf 'args=%s\\n' \"$*\"\nprintf 'XDG_CONFIG_HOME=%s\\nHOME=%s\\nPATH=%s\\nHERDR_CONFIG_PATH=%s\\nSOCKET=%s\\n' \"$XDG_CONFIG_HOME\" \"$HOME\" \"$PATH\" \"$HERDR_CONFIG_PATH\" \"${HERDR_SOCKET_PATH-unset}\"\n",
    );
    let sftp = tmp.path().join("fake-sftp");
    write_exec(&sftp, "#!/bin/sh\necho sftp-ran\n");
    let ns = HostNamespace::new(tmp.path().join("h-ssh"), &engine);
    std::fs::create_dir_all(ns.log_dir()).unwrap();
    let session = SessionName::parse("hd007-remote-contract-ssh").unwrap();
    let script = forced_command_script(&ns, &session, Some(&sftp)).unwrap();
    let script_path = tmp.path().join("forced.sh");
    write_exec(&script_path, &script);

    let run = |original: &str| {
        Command::new("sh")
            .arg(&script_path)
            .env("SSH_ORIGINAL_COMMAND", original)
            .env("HERDR_SOCKET_PATH", "/tmp/user-default.sock")
            .env("HOME", "/home/not-private")
            .output()
            .unwrap()
    };

    let api = run(&remote_command_line(
        &session,
        RemoteHerdrCommand::ApiBridge,
    ));
    assert_eq!(api.status.code(), Some(0));
    let text = String::from_utf8_lossy(&api.stdout);
    assert!(
        text.contains("args=--session hd007-remote-contract-ssh remote-api-bridge\n"),
        "{text}"
    );
    assert!(text.contains(&format!("XDG_CONFIG_HOME={}\n", ns.config_home().display())));
    assert!(text.contains(&format!("HOME={}\n", ns.home().display())));
    assert!(text.contains(&format!("PATH={}:/usr/bin:/bin\n", ns.bin_dir().display())));
    assert!(text.contains(&format!(
        "HERDR_CONFIG_PATH={}\n",
        ns.config_file().display()
    )));
    assert!(text.contains("SOCKET=unset\n"), "{text}");

    let status = run(&remote_command_line(
        &session,
        RemoteHerdrCommand::ServerStatus,
    ));
    assert!(String::from_utf8_lossy(&status.stdout)
        .contains("args=--session hd007-remote-contract-ssh status server --json\n"));
    let sftp_out = run(&sftp.display().to_string());
    assert_eq!(String::from_utf8_lossy(&sftp_out.stdout), "sftp-ran\n");

    let other = SessionName::parse("hd007-remote-other").unwrap();
    for rejected in [
        format!(
            "{}; touch {}",
            remote_command_line(&session, RemoteHerdrCommand::ApiBridge),
            tmp.path().join("pwned").display()
        ),
        remote_command_line(&other, RemoteHerdrCommand::ApiBridge),
        "exec herdr --session default remote-api-bridge".to_string(),
        "sh -c id".to_string(),
        String::new(),
    ] {
        let out = run(&rejected);
        assert_eq!(out.status.code(), Some(126), "accepted {rejected:?}");
        assert!(out.stdout.is_empty(), "engine ran for {rejected:?}");
    }
    assert!(!tmp.path().join("pwned").exists());
    let log = std::fs::read_to_string(ns.forced_command_log()).unwrap();
    assert_eq!(log.lines().filter(|l| l.starts_with("accept ")).count(), 3);
    assert_eq!(log.lines().filter(|l| l.starts_with("reject ")).count(), 5);
}

/// Would catch: cleanup reporting a session as stopped although its stop could not even run.
#[test]
fn contract_idempotent_double_cleanup_reports_unverified_stop_as_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let mut guard = RemoteFixtureGuard::new(tmp.path().to_path_buf());
    guard.tracked_processes.push(TrackedProcess {
        pid: 99_999_999,
        starttime: 12345,
        name: "dummy".into(),
    });
    let name = format!("{SESSION_PREFIX}cleanup-1");
    guard.created_sessions.push(TrackedSession {
        name: name.clone(),
        namespace: HostNamespace::new(tmp.path().join("ns"), tmp.path().join("missing-engine")),
        project_root: tmp.path().to_path_buf(),
        log_file: tmp.path().join("server.log"),
        pid: None,
        starttime: None,
        boot_id: "b".into(),
        pane_id: SHARED_PANE.into(),
        agent_resolution: Default::default(),
    });
    let first = guard.cleanup();
    assert!(first.stopped_sessions.is_empty(), "{first:?}");
    assert_eq!(first.failures.len(), 1, "{first:?}");
    assert!(first.failures[0].contains(&name));
    assert_eq!(guard.cleanup(), Default::default());
}

/// Would catch: stop() returning Ok while the engine still lists the session as running.
#[test]
fn contract_stop_is_not_reported_when_namespace_still_lists_session_running() {
    let tmp = tempfile::tempdir().unwrap();
    let name = format!("{SESSION_PREFIX}still-running");
    let engine = tmp.path().join("fake-engine");
    write_exec(
        &engine,
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  'session stop {name}') exit 0 ;;\n  'session list --json') printf '{{\"sessions\":[{{\"name\":\"{name}\",\"running\":true}}]}}' ;;\n  *) exit 1 ;;\nesac\n"
        ),
    );
    let mut sleeper = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = sleeper.id();
    let st = get_proc_starttime(pid).unwrap();
    let mut guard = RemoteFixtureGuard::new(tmp.path().to_path_buf());
    guard.created_sessions.push(TrackedSession {
        name: name.clone(),
        namespace: HostNamespace::new(tmp.path().join("ns"), &engine),
        project_root: tmp.path().to_path_buf(),
        log_file: tmp.path().join("server.log"),
        pid: Some(pid),
        starttime: Some(st),
        boot_id: "b".into(),
        pane_id: SHARED_PANE.into(),
        agent_resolution: Default::default(),
    });
    let report = guard.cleanup();
    let _ = sleeper.wait();
    assert!(report.stopped_sessions.is_empty(), "{report:?}");
    assert_eq!(report.failures.len(), 1);
    assert!(
        report.failures[0].contains("still lists it running"),
        "{report:?}"
    );
    assert!(!is_proc_alive(pid, st), "owned server left alive");
}

/// Would catch: an engine spawned by `TrackedSession::start` surviving a startup error.
#[test]
fn contract_session_start_error_after_spawn_kills_owned_server_and_ledger_has_it() {
    let tmp = tempfile::tempdir().unwrap();
    let pid_file = tmp.path().join("server.pid");
    let engine = tmp.path().join("fake-engine");
    write_exec(
        &engine,
        &format!(
            "#!/bin/sh\ncase \"$*\" in\n  *' server') echo $$ > {}; exec sleep 30 ;;\n  *) exit 1 ;;\nesac\n",
            shell_quote(&pid_file.display().to_string())
        ),
    );
    let agent = tmp.path().join("agent.sh");
    write_exec(&agent, "#!/bin/sh\nexit 0\n");
    let ns = HostNamespace::new(tmp.path().join("h-loc"), &engine);
    let ledger = Ledger::in_workdir(tmp.path());
    let name = format!("{SESSION_PREFIX}1-abc123-loc");
    let err = TrackedSession::start(
        &name,
        &ns,
        tmp.path(),
        &agent,
        &ledger,
        Duration::from_millis(800),
    )
    .unwrap_err();
    assert!(err.contains("not ready"), "{err}");
    let entries = ledger.read().unwrap();
    let [LedgerEntry::Server {
        pid,
        starttime,
        session,
        ..
    }] = entries.as_slice()
    else {
        panic!("ledger: {entries:?}");
    };
    assert_eq!(session, &name);
    let reported: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(reported, *pid, "ledger holds the spawned server pid");
    assert!(
        !is_proc_alive(*pid, *starttime),
        "server survived start error"
    );
    // Helper was installed before the engine started.
    assert!(ns.agent_helper().is_file());
}

#[test]
fn contract_cleans_up_tracked_process_on_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id();
    let starttime = get_proc_starttime(pid).unwrap();
    let mut guard = RemoteFixtureGuard::new(tmp.path().to_path_buf());
    guard.tracked_processes.push(TrackedProcess {
        pid,
        starttime,
        name: "sleep".into(),
    });
    let report = guard.cleanup();
    let _ = child.wait();
    assert!(report.killed_pids.contains(&pid));
    assert!(!is_proc_alive(pid, starttime));
}

/// Would catch: prefix/empty digest acceptance and machine-dependent assertions.
#[test]
fn contract_fixture_config_validation() {
    assert!(RemoteFixtureConfig::new("relative/path")
        .validate_structure()
        .unwrap_err()
        .contains("absolute"));
    let tmp = tempfile::tempdir().unwrap();
    assert!(RemoteFixtureConfig::new(tmp.path())
        .with_session_prefix("invalid-prefix-")
        .validate_structure()
        .unwrap_err()
        .contains("session_prefix"));
    assert!(RemoteFixtureConfig::new(tmp.path())
        .validate_structure()
        .is_ok());
    let long = PathBuf::from(format!("/tmp/{}", "x".repeat(60)));
    assert!(RemoteFixtureConfig::new(long)
        .validate_structure()
        .unwrap_err()
        .contains("too long"));
    assert!(RemoteFixtureConfig::new("/tmp/with space")
        .validate_structure()
        .is_err());

    let missing = RemoteFixtureConfig::new(tmp.path())
        .with_reference_binary(tmp.path().join("absent-herdr"))
        .validate()
        .unwrap_err();
    assert!(missing.contains("does not exist"), "{missing}");
    let fake = tmp.path().join("fake_herdr");
    std::fs::write(&fake, b"#!/bin/sh\necho fake\n").unwrap();
    let mismatch = RemoteFixtureConfig::new(tmp.path())
        .with_reference_binary(&fake)
        .validate()
        .unwrap_err();
    assert!(mismatch.contains("sha256 mismatch"), "{mismatch}");

    assert!(pinned_sha256_matches(REFERENCE_ENGINE_SHA256));
    assert!(!pinned_sha256_matches(""));
    assert!(!pinned_sha256_matches(&REFERENCE_ENGINE_SHA256[..12]));
    assert!(!pinned_sha256_matches(&format!(
        "{REFERENCE_ENGINE_SHA256}00"
    )));
}

#[test]
fn contract_pending_process_guard_cleans_up_on_induced_error() {
    let mut child = Command::new("sleep").arg("60").spawn().unwrap();
    let pid = child.id();
    let starttime = get_proc_starttime(pid).unwrap();
    fn fails_after_spawn(pid: u32, starttime: u64) -> Result<(), &'static str> {
        let _guard = remote::PendingProcessGuard::new(pid, starttime);
        Err("induced error after spawn")
    }
    assert!(fails_after_spawn(pid, starttime).is_err());
    let _ = child.wait();
    assert!(!is_proc_alive(pid, starttime));
}

#[test]
fn contract_fixture_roots_and_pair_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let local = HostFixture {
        session: format!("{SESSION_PREFIX}local-1"),
        pane_id: SHARED_PANE.into(),
        root: tmp.path().join("root-a"),
        boot_id: "boot-local-123".into(),
    };
    let ssh = HostFixture {
        session: format!("{SESSION_PREFIX}ssh-1"),
        pane_id: SHARED_PANE.into(),
        root: tmp.path().join("root-b"),
        boot_id: "boot-ssh-456".into(),
    };
    assert!(pair_problems(&local, &ssh).is_empty());
    let mut b = ssh.clone();
    b.session = local.session.clone();
    assert!(!pair_problems(&local, &b).is_empty());
    let mut b = ssh.clone();
    b.boot_id = local.boot_id.clone();
    assert!(!pair_problems(&local, &b).is_empty());
    let mut b = ssh.clone();
    b.pane_id = "w1:p2".into();
    assert!(!pair_problems(&local, &b).is_empty());
    let mut b = ssh.clone();
    b.root = local.root.join("nested");
    assert!(!pair_problems(&local, &b).is_empty());
}

#[test]
fn contract_process_starttime_tracking() {
    let my_pid = std::process::id();
    let my_st = get_proc_starttime(my_pid).unwrap();
    assert!(is_proc_alive(my_pid, my_st));
    assert!(get_proc_starttime(99_999_999).is_none());
    assert!(!is_proc_alive(my_pid, my_st + 1000));
    assert!(!terminate_proc(my_pid, my_st + 1000));
}

// ---------------------------------------------------------------------------------------
// Smoke of real processes (disposable sshd + engine sessions); run explicitly
// ---------------------------------------------------------------------------------------

struct Report {
    lines: Vec<String>,
}

impl Report {
    fn note(&mut self, line: impl Into<String>) {
        let line = line.into();
        eprintln!("{line}");
        self.lines.push(line);
    }
}

fn wait_until<T>(timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = probe() {
            return Some(v);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn count_prefix(text: &str, prefix: &str) -> usize {
    text.lines().filter(|l| l.starts_with(prefix)).count()
}

#[test]
#[ignore = "smoke of real processes (disposable sshd/sessions); run explicitly"]
fn smoke_remote_fixture_processes_lifecycle_and_bridge_discrimination() {
    let dest = std::env::var("HERDR_DESKTOP_E2E_REPORT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../evidencias/007/remote-fixture/r2/smoke-report.md")
        });
    assert!(
        !dest.exists(),
        "report destination {} already exists; previous evidence is never overwritten",
        dest.display()
    );
    let mut r = Report { lines: Vec::new() };
    r.note("# Smoke fixture remota 007 (processos reais; sem GUI; não é veredito de AC)");

    // 0. Full preflight is mandatory here.
    let hash = compute_sha256(Path::new(REFERENCE_ENGINE_PATH)).expect("reference hash");
    r.note(format!("reference={REFERENCE_ENGINE_PATH} sha256={hash}"));
    assert_eq!(hash, REFERENCE_ENGINE_SHA256);
    assert!(Path::new(LEGACY_ENGINE_PATH).exists());

    // 1. Real partial startup failure: everything spawned is in the ledger and is gone.
    let failed_dir = tempfile::tempdir().unwrap();
    let failed_cfg = RemoteFixtureConfig::new(failed_dir.path())
        .with_induced_failure(StartupStage::AfterSshSession);
    failed_cfg.validate().expect("full preflight");
    let err = RemoteFixture::start_pair(failed_cfg).expect_err("induced failure must fail");
    r.note(format!("## induced partial startup\nerror: {err}"));
    assert!(
        err.contains("induced startup failure at AfterSshSession"),
        "{err}"
    );
    let ledger = Ledger::in_workdir(failed_dir.path());
    r.note(format!(
        "ledger:\n{}",
        std::fs::read_to_string(&ledger.path).unwrap()
    ));
    let partial = verify_ledger_clean(&ledger);
    r.note(format!("ledger verification: {partial:?}"));
    assert_eq!((partial.servers, partial.sshd), (2, 1), "{partial:?}");
    assert!(
        partial.descendants >= 2,
        "pane shells captured: {partial:?}"
    );
    assert!(partial.is_clean(), "{partial:?}");
    drop(failed_dir);

    // 2. The pair.
    let tmp = tempfile::tempdir().unwrap();
    let mut fx =
        RemoteFixture::start_pair(RemoteFixtureConfig::new(tmp.path())).expect("start_pair");
    let local = fx.local_fixture();
    let ssh = fx.ssh_fixture();
    r.note(format!("## pair\nlocal={local:?}\nssh={ssh:?}"));
    assert_eq!(
        (local.pane_id.as_str(), ssh.pane_id.as_str()),
        (SHARED_PANE, SHARED_PANE)
    );
    assert!(pair_problems(&local, &ssh).is_empty());
    for (label, host) in [("local", &fx.local_session), ("ssh", &fx.ssh_session)] {
        r.note(format!(
            "{label} agent resolution: {:?}",
            host.agent_resolution
        ));
        assert_eq!(
            host.agent_resolution.resolved,
            host.namespace.agent_helper().display().to_string()
        );
    }

    // 3. SSH endpoint through the product connector: welcome + snapshot boot of the SSH host.
    let connector = fx.ssh_connector();
    let mut gateway = connector
        .connect(herdr_client::contracts::ConnectOptions {
            geometry: herdr_client::contracts::SurfaceGeometry {
                cols: 80,
                rows: 24,
                cell_width_px: 9,
                cell_height_px: 18,
            },
            surface_active: false,
        })
        .unwrap_or_else(|f| panic!("ssh connect: {:?}", f.error()));
    use herdr_client::RuntimeGateway;
    let live =
        wait_until(Duration::from_secs(10), || gateway.identity()).expect("ssh snapshot boot");
    let negotiated = gateway.negotiated().clone();
    r.note(format!(
        "## ssh endpoint (SshConnector over OpenSSH)\ngeneration={} server_version={} identity={live:?}\nmethods={:?}",
        negotiated.generation, negotiated.server_version, negotiated.methods
    ));
    assert_eq!(negotiated.generation, 1);
    assert_eq!(live.session, ssh.session);
    assert_eq!(
        live.boot_id, ssh.boot_id,
        "SSH endpoint reaches the SSH namespace"
    );
    assert_ne!(live.boot_id, local.boot_id);
    gateway.detach();
    drop(gateway);

    // 4. JSON API over SSH through the product lane.
    let lane = connector.api_lane();
    let mut ssh_api_calls = 0usize;
    let mut ssh_request = |method: &str, params: Value| {
        ssh_api_calls += 1;
        lane.request(method, params)
    };
    let panes = ssh_request("pane.list", json!({})).expect("pane.list over ssh");
    r.note(format!("ssh pane.list: {panes}"));
    let ssh_pane = panes["panes"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["pane_id"] == SHARED_PANE))
        .expect("w1:p1 on ssh")
        .clone();
    assert_eq!(ssh_pane["cwd"], ssh.root.display().to_string());
    let local_api = fx.host_api("local").unwrap();
    let local_panes = local_api
        .request("pane.list", json!({}))
        .expect("local pane.list");
    let local_pane = local_panes["panes"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["pane_id"] == SHARED_PANE))
        .expect("w1:p1 on local")
        .clone();
    assert_eq!(local_pane["cwd"], local.root.display().to_string());
    assert_ne!(local_pane["cwd"], ssh_pane["cwd"]);
    let manifests = ssh_request("server.agent_manifests", json!({})).expect("manifests");
    let pi_manifest = manifests["manifests"]
        .as_array()
        .and_then(|a| a.iter().find(|m| m["agent"] == "pi"))
        .expect("pi manifest on ssh host")
        .clone();
    r.note(format!("ssh pi manifest: {pi_manifest}"));
    assert_ne!(pi_manifest["source_kind"], "local override");
    assert!(pi_manifest["warning"].is_null());

    // 5. Legacy host: API unavailable, status probe (terminal path) still fine.
    fx.start_legacy_host().expect("legacy host");
    let legacy = fx.legacy_fixture().unwrap();
    let legacy_connector = fx.legacy_connector().unwrap();
    let legacy_probe = legacy_connector.probe();
    let legacy_api = legacy_connector.api_lane().request("pane.list", json!({}));
    r.note(format!(
        "## legacy\nfixture={legacy:?}\nprobe={:?}\napi={:?}",
        legacy_probe.as_ref().err().map(|f| f.error().clone()),
        legacy_api
    ));
    assert!(legacy_probe.is_ok());
    assert_eq!(legacy_api.unwrap_err().code, "remote_api_unsupported");

    // 6. SFTP through the product builder and the forced command.
    let got = tmp.path().join("sftp-notas.txt");
    let batch = tmp.path().join("sftp-batch.txt");
    std::fs::write(
        &batch,
        format!(
            "get {} {}\n",
            ssh.root.join("notas.txt").display(),
            got.display()
        ),
    )
    .unwrap();
    let built = build_sftp(
        &fx.ssh_profile.to_identity(),
        Some(&fx.isolated_ssh_config()),
    );
    let mut args = built.args.clone();
    let at = args.iter().position(|a| a == "--").unwrap();
    args.splice(at..at, ["-b".to_string(), batch.display().to_string()]);
    let sftp = Command::new(&built.program).args(&args).output().unwrap();
    r.note(format!(
        "## sftp exit={:?} stderr={}",
        sftp.status.code(),
        String::from_utf8_lossy(&sftp.stderr).trim()
    ));
    assert!(sftp.status.success());
    assert_eq!(
        std::fs::read_to_string(&got).unwrap(),
        std::fs::read_to_string(ssh.root.join("notas.txt")).unwrap()
    );

    // 7. Agent start + prompt through the SSH engine API, exactly once; nothing on Local.
    assert_eq!(
        fx.read_agent_log("ssh").unwrap(),
        "",
        "helper ran before agent.start"
    );
    assert_eq!(fx.read_agent_log("local").unwrap(), "");
    let agent_name = format!("hd007-fake-{}", std::process::id());
    let started = ssh_request(
        "agent.start",
        json!({ "name": agent_name, "kind": "pi", "pane_id": SHARED_PANE }),
    )
    .expect("agent.start over ssh");
    r.note(format!("## agent\nagent.start: {started}"));
    assert_eq!(started["agent"]["name"], agent_name.as_str());
    assert_eq!(started["argv"], json!(["pi"]));
    let ready = wait_until(Duration::from_secs(20), || {
        let info = ssh_request("agent.get", json!({ "target": SHARED_PANE })).ok()?;
        let a = &info["agent"];
        (a["name"] == agent_name.as_str() && a["agent"] == "pi" && a["launch_pending"] != true)
            .then(|| info.clone())
    })
    .expect("agent ready on ssh host");
    r.note(format!("agent.get ready: {ready}"));
    let prompt = "tarefa remota de fidelidade 007";
    let prompted = ssh_request(
        "agent.prompt",
        json!({ "target": SHARED_PANE, "text": prompt }),
    )
    .expect("agent.prompt over ssh");
    r.note(format!("agent.prompt: {prompted}"));
    let ssh_log = wait_until(Duration::from_secs(10), || {
        let text = fx.read_agent_log("ssh").ok()?;
        (count_prefix(&text, "state idle") >= 2).then_some(text)
    })
    .expect("prompt handled by helper");
    let local_log = fx.read_agent_log("local").unwrap();
    r.note(format!(
        "ssh agent log:\n{ssh_log}\nlocal agent log: {local_log:?}"
    ));
    assert_eq!(count_prefix(&ssh_log, "start "), 1);
    assert_eq!(
        count_prefix(
            &ssh_log,
            &format!("start session={} pane={SHARED_PANE} ", ssh.session)
        ),
        1
    );
    assert_eq!(count_prefix(&ssh_log, "prompt "), 1);
    assert_eq!(count_prefix(&ssh_log, &format!("prompt {prompt} ")), 1);
    assert_eq!(local_log, "", "no agent action reached Local");
    let ssh_agents = ssh_request("agent.list", json!({})).unwrap();
    let local_agents = local_api.request("agent.list", json!({})).unwrap();
    r.note(format!(
        "ssh agent.list: {ssh_agents}\nlocal agent.list: {local_agents}"
    ));
    assert_eq!(ssh_agents["agents"].as_array().map(Vec::len), Some(1));
    assert_eq!(local_agents["agents"].as_array().map(Vec::len), Some(0));
    let calls = ssh_api_calls;
    let forced_ssh = fx.read_forced_command_log("ssh").unwrap();
    r.note(format!(
        "forced command log (ssh), lane requests={calls}:\n{forced_ssh}"
    ));
    assert_eq!(count_prefix(&forced_ssh, "accept api-bridge"), calls);
    assert_eq!(count_prefix(&forced_ssh, "reject "), 0);
    assert!(count_prefix(&forced_ssh, "accept client-bridge") >= 1);
    assert_eq!(fx.read_forced_command_log("local").unwrap(), "");

    // 8. Cleanup verified per namespace, PID/starttime and port.
    let processes = fx.process_snapshot();
    let namespaces: Vec<(String, HostNamespace)> = [
        &fx.local_session,
        &fx.ssh_session,
        fx.legacy_session.as_ref().unwrap(),
    ]
    .iter()
    .map(|s| (s.name.clone(), s.namespace.clone()))
    .collect();
    let ports = [
        fx.ssh_profile.port,
        fx.legacy_ssh_profile.as_ref().unwrap().port,
    ];
    r.note(format!(
        "## cleanup\nprocesses before: {processes:?}\nports: {ports:?}"
    ));
    assert!(
        processes.len() >= 6,
        "servers, sshd and pane processes: {processes:?}"
    );
    for (name, ns) in &namespaces {
        assert!(
            ns.session_running(name).unwrap(),
            "{name} running before cleanup"
        );
    }
    let report = fx.cleanup();
    r.note(format!("cleanup report: {report:?}"));
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(report.stopped_sessions.len(), 3);
    let survivors: Vec<&TrackedProcess> = processes
        .iter()
        .filter(|p| is_proc_alive(p.pid, p.starttime))
        .collect();
    assert!(survivors.is_empty(), "{survivors:?}");
    for port in ports {
        assert!(!port_listening(port), "port {port} still listening");
    }
    for (name, ns) in &namespaces {
        let listing = ns.session_list_json().unwrap();
        r.note(format!("{name} namespace listing after: {listing}"));
        assert!(!ns.session_running(name).unwrap(), "{name} still running");
    }
    let main_ledger = verify_ledger_clean(&fx.guard.ledger);
    r.note(format!("ledger verification: {main_ledger:?}"));
    assert!(main_ledger.is_clean(), "{main_ledger:?}");
    r.note("RESULT: all smoke checks above passed");

    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .expect("create new report");
    writeln!(file, "{}", r.lines.join("\n\n")).unwrap();
}
