#[allow(dead_code)]
#[path = "../../tests/fidelity-native/display.rs"]
mod display;

#[allow(dead_code)]
#[path = "../../tests/fidelity-native/pointer_virtual.rs"]
mod pointer_virtual;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../tests/fidelity-native/supervisor.rs"]
mod supervisor;

use std::path::{Path, PathBuf};

use pointer_virtual::{
    ComparisonReport, MethodObservation, RecordedEvent, VirtualPointerClient, BTN_LEFT, BTN_MIDDLE,
    BTN_RIGHT, HTML_TEST_PAGE, WINDOW_RUNNER_PY,
};

#[test]
fn test_button_constants() {
    assert_eq!(BTN_LEFT, 272);
    assert_eq!(BTN_RIGHT, 273);
    assert_eq!(BTN_MIDDLE, 274);
}

#[test]
fn test_virtual_pointer_connect_rejects_relative_path() {
    let result = VirtualPointerClient::connect(Path::new("relative/socket"));
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(
        err.contains("not absolute"),
        "expected not absolute, got: {err}"
    );
}

#[test]
fn test_recorded_event_deserialization() {
    let json_str = r#"{"type":"pointerdown","button":0,"client_x":400.0,"client_y":300.0,"delta_y":null,"is_trusted":true}"#;
    let ev: RecordedEvent = serde_json::from_str(json_str).expect("deserialize");
    assert_eq!(ev.event_type, "pointerdown");
    assert_eq!(ev.button, Some(0));
    assert_eq!(ev.client_x, 400.0);
    assert_eq!(ev.client_y, 300.0);
    assert!(ev.delta_y.is_none());
    assert!(ev.is_trusted);
}

#[test]
fn test_comparison_report_markdown_table() {
    let report = ComparisonReport {
        observations: vec![
            MethodObservation {
                method: "swaymsg seat cursor".into(),
                injection: "set/press/release".into(),
                observed_events: vec![],
                pointerdown_count: 0,
                pointerup_count: 0,
                wheel_count: 0,
                status: "FALHA (0 eventos)".into(),
            },
            MethodObservation {
                method: "zwlr_virtual_pointer_v1".into(),
                injection: "motion_absolute/button/axis".into(),
                observed_events: vec![RecordedEvent {
                    event_type: "pointerdown".into(),
                    button: Some(0),
                    client_x: 400.0,
                    client_y: 300.0,
                    delta_y: None,
                    is_trusted: true,
                }],
                pointerdown_count: 1,
                pointerup_count: 0,
                wheel_count: 0,
                status: "SUCESSO".into(),
            },
        ],
    };
    let table = report.markdown_table();
    assert!(table.contains("swaymsg seat cursor"));
    assert!(table.contains("zwlr_virtual_pointer_v1"));
    assert!(table.contains("SUCESSO"));
}

#[test]
fn test_html_and_runner_templates() {
    assert!(HTML_TEST_PAGE.contains("pointerdown"));
    assert!(HTML_TEST_PAGE.contains("pointerup"));
    assert!(HTML_TEST_PAGE.contains("wheel"));
    assert!(WINDOW_RUNNER_PY.contains("WebKit2"));
    assert!(WINDOW_RUNNER_PY.contains("notify::title"));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "native pointer virtual proof under private sway display; runs under flock native-measurement.lock"]
fn test_pointer_virtual_native() {
    use std::process::Command;
    use std::time::Duration;

    use display::{PrivateDisplay, Resources};
    use supervisor::Supervisor;

    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let user_uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1000);

    let pid = std::process::id();
    let runtime_dir = PathBuf::from(format!("/tmp/hd7P-{pid}"));
    let home_dir = PathBuf::from(format!("/tmp/hd7P-home-{pid}"));

    let _ = std::fs::remove_dir_all(&runtime_dir);
    let _ = std::fs::remove_dir_all(&home_dir);
    std::fs::create_dir_all(&runtime_dir).expect("create runtime_dir");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))
            .expect("chmod 0700 runtime_dir");
    }
    std::fs::create_dir_all(&home_dir).expect("create home_dir");

    let resources = Resources::under(Path::new("/home/user/Projects/herdr-desktop/.local"));

    let display = PrivateDisplay::new(
        runtime_dir.clone(),
        home_dir.clone(),
        resources,
        user,
        user_uid,
    )
    .expect("PrivateDisplay layout");

    let evidence_dir = PathBuf::from(
        std::env::var("POINTER_PREP_EVIDENCE_DIR")
            .unwrap_or_else(|_| "evidencias/007/pointer-prep".into()),
    );
    let _ = std::fs::create_dir_all(&evidence_dir);
    let proc_out = evidence_dir.join("processes");

    let mut sup = Supervisor::start(display, proc_out).expect("Supervisor start");
    let wayland_socket = sup.wayland.clone();
    assert!(
        wayland_socket.exists(),
        "wayland socket must exist after supervisor start"
    );

    // Find the sway IPC socket in runtime_dir
    let sway_ipc = std::fs::read_dir(&runtime_dir)
        .expect("read runtime_dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("sway-ipc"))
        })
        .expect("sway-ipc socket in runtime dir");

    // Write the test HTML and window runner into the private runtime dir
    let events_file = runtime_dir.join("events.jsonl");
    let html_file = runtime_dir.join("test_page.html");
    let win_script = runtime_dir.join("window_runner.py");

    std::fs::write(&html_file, HTML_TEST_PAGE).expect("write test_page.html");
    std::fs::write(&win_script, WINDOW_RUNNER_PY).expect("write window_runner.py");

    // Spawn the WebKitGTK test window inside the private display environment
    let mut win_launch = sup
        .display
        .window(Path::new("/usr/bin/python3"), &wayland_socket, false, &[])
        .expect("display.window");
    win_launch.args = vec![
        win_script.to_string_lossy().into_owned(),
        events_file.to_string_lossy().into_owned(),
        html_file.to_string_lossy().into_owned(),
    ];
    win_launch
        .env
        .push(("DBUS_SESSION_BUS_ADDRESS".into(), sup.bus.clone()));

    let mut win_child = win_launch.command().spawn().expect("spawn test window");
    let _win_pid = win_child.id();

    // Settle for window to map in sway
    std::thread::sleep(Duration::from_millis(1500));

    // Helper to read newly recorded events from events_file
    let read_events = |start_idx: usize| -> Vec<RecordedEvent> {
        if !events_file.exists() {
            return Vec::new();
        }
        let content = std::fs::read_to_string(&events_file).unwrap_or_default();
        content
            .lines()
            .skip(start_idx)
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<RecordedEvent>(l).ok())
            .collect()
    };

    let total_events = || -> usize {
        if !events_file.exists() {
            0
        } else {
            std::fs::read_to_string(&events_file)
                .unwrap_or_default()
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count()
        }
    };

    let mut observations = Vec::new();

    // -------------------------------------------------------------------------
    // Method 1: Current swaymsg seat cursor commands (set, press button1, release, press button4)
    // -------------------------------------------------------------------------
    let m1_start = total_events();
    let sway_swaymsg =
        Path::new("/home/user/Projects/herdr-desktop/.local/native-input/prefix/usr/bin/swaymsg");
    let sway_lib =
        Path::new("/home/user/Projects/herdr-desktop/.local/native-input/prefix/usr/lib");

    let run_swaymsg = |args: &[&str]| -> Result<String, String> {
        let mut cmd = Command::new(sway_swaymsg);
        cmd.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LD_LIBRARY_PATH", sway_lib)
            .arg("-s")
            .arg(&sway_ipc);
        cmd.args(args);
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "exit {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            ))
        }
    };

    let _ = run_swaymsg(&["seat", "seat0", "cursor", "set", "400", "300"]);
    let _ = run_swaymsg(&["seat", "seat0", "cursor", "press", "button1"]);
    let _ = run_swaymsg(&["seat", "seat0", "cursor", "release", "button1"]);
    let _ = run_swaymsg(&["seat", "seat0", "cursor", "press", "button4"]);
    std::thread::sleep(Duration::from_millis(500));

    let m1_events = read_events(m1_start);
    let m1_down = m1_events
        .iter()
        .filter(|e| e.event_type == "pointerdown")
        .count();
    let m1_up = m1_events
        .iter()
        .filter(|e| e.event_type == "pointerup")
        .count();
    let m1_wheel = m1_events.iter().filter(|e| e.event_type == "wheel").count();
    observations.push(MethodObservation {
        method: "swaymsg seat seat0 cursor (absoluto)".into(),
        injection: "set 400 300, press button1, release button1, press button4".into(),
        observed_events: m1_events.clone(),
        pointerdown_count: m1_down,
        pointerup_count: m1_up,
        wheel_count: m1_wheel,
        status: if m1_events.is_empty() {
            "FALHA: 0 eventos wl_pointer entregues à janela".into()
        } else {
            "EVENTOS ENTREGUES".into()
        },
    });

    // -------------------------------------------------------------------------
    // Method 2: Relative motion before press (seat cursor move)
    // -------------------------------------------------------------------------
    let m2_start = total_events();
    let _ = run_swaymsg(&["seat", "seat0", "cursor", "move", "10", "10"]);
    let _ = run_swaymsg(&["seat", "seat0", "cursor", "press", "button1"]);
    let _ = run_swaymsg(&["seat", "seat0", "cursor", "release", "button1"]);
    std::thread::sleep(Duration::from_millis(500));

    let m2_events = read_events(m2_start);
    let m2_down = m2_events
        .iter()
        .filter(|e| e.event_type == "pointerdown")
        .count();
    let m2_up = m2_events
        .iter()
        .filter(|e| e.event_type == "pointerup")
        .count();
    let m2_wheel = m2_events.iter().filter(|e| e.event_type == "wheel").count();
    observations.push(MethodObservation {
        method: "swaymsg seat seat0 cursor move (relativo)".into(),
        injection: "move 10 10, press button1, release button1".into(),
        observed_events: m2_events.clone(),
        pointerdown_count: m2_down,
        pointerup_count: m2_up,
        wheel_count: m2_wheel,
        status: if m2_events.is_empty() {
            "FALHA: 0 eventos wl_pointer entregues à janela".into()
        } else {
            "EVENTOS ENTREGUES".into()
        },
    });

    // -------------------------------------------------------------------------
    // Method 3: zwlr_virtual_pointer_v1
    // -------------------------------------------------------------------------
    let m3_start = total_events();
    let mut vp = VirtualPointerClient::connect(&wayland_socket)
        .expect("connect VirtualPointerClient to private Wayland display");

    vp.click(400, 300, 1280, 720, BTN_LEFT)
        .expect("virtual pointer click");
    vp.wheel_up(400, 300, 1280, 720, 1)
        .expect("virtual pointer wheel_up");
    std::thread::sleep(Duration::from_millis(500));
    drop(vp);

    let m3_events = read_events(m3_start);
    let m3_down = m3_events
        .iter()
        .filter(|e| e.event_type == "pointerdown")
        .count();
    let m3_up = m3_events
        .iter()
        .filter(|e| e.event_type == "pointerup")
        .count();
    let m3_wheel = m3_events.iter().filter(|e| e.event_type == "wheel").count();
    observations.push(MethodObservation {
        method: "zwlr_virtual_pointer_v1".into(),
        injection: "motion_absolute(400,300), button(BTN_LEFT), wheel_up(1)".into(),
        observed_events: m3_events.clone(),
        pointerdown_count: m3_down,
        pointerup_count: m3_up,
        wheel_count: m3_wheel,
        status: if m3_down >= 1 && m3_up >= 1 && m3_wheel >= 1 {
            "PROVADO: pointerdown, pointerup e wheel entregues com confiança à janela".into()
        } else {
            format!("INCOMPLETO: down={m3_down}, up={m3_up}, wheel={m3_wheel}")
        },
    });

    let report = ComparisonReport {
        observations: observations.clone(),
    };
    let markdown = report.markdown_table();
    println!("\n=== COMPARAÇÃO DE MÉTODOS DE INJEÇÃO DE PONTEIRO ===\n{markdown}");

    // Save evidence
    let report_path = evidence_dir.join("comparison_report.md");
    let report_json = evidence_dir.join("comparison_report.json");
    std::fs::write(&report_path, &markdown).expect("write comparison_report.md");
    std::fs::write(
        &report_json,
        serde_json::to_string_pretty(&report).expect("serialize report"),
    )
    .expect("write comparison_report.json");

    // Copy events file to evidence dir
    if events_file.exists() {
        let _ = std::fs::copy(&events_file, evidence_dir.join("events.jsonl"));
    }

    // Terminate window child process cleanly
    let _ = win_child.kill();
    let _ = win_child.wait();

    // Supervisor cleanup and proof
    let cleanup_log = sup.cleanup();
    let cleanup_log_path = evidence_dir.join("cleanup_proof.log");
    std::fs::write(&cleanup_log_path, cleanup_log.join("\n")).expect("write cleanup log");

    // Verify cleanup
    let leftover_count = cleanup_log
        .iter()
        .filter(|l| l.starts_with("LEFTOVER"))
        .count();
    assert_eq!(
        leftover_count, 0,
        "Supervisor cleanup must leave 0 leftover processes in private runtime"
    );
    assert!(
        cleanup_log
            .iter()
            .any(|l| l.contains("no process left with XDG_RUNTIME_DIR")),
        "cleanup log must confirm no process left with private XDG_RUNTIME_DIR"
    );
    assert!(
        cleanup_log
            .iter()
            .any(|l| l.contains("runtime dir removed: true")),
        "cleanup log must confirm runtime dir was removed"
    );

    // Verify method results
    assert_eq!(
        m1_events.len(),
        0,
        "swaymsg seat cursor method should reproduce zero events"
    );
    assert_eq!(
        m2_events.len(),
        0,
        "swaymsg seat cursor move relative should also yield zero events"
    );
    assert!(
        m3_down >= 1,
        "zwlr_virtual_pointer_v1 must deliver at least 1 pointerdown event"
    );
    assert!(
        m3_up >= 1,
        "zwlr_virtual_pointer_v1 must deliver at least 1 pointerup event"
    );
    assert!(
        m3_wheel >= 1,
        "zwlr_virtual_pointer_v1 must deliver at least 1 wheel event"
    );

    for ev in &m3_events {
        assert!(
            ev.is_trusted,
            "Events delivered by zwlr_virtual_pointer_v1 must have isTrusted=true"
        );
    }
}
