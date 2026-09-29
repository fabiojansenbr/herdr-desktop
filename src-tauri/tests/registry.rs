//! Command registry per module (CONTRATOS: "registro de comandos por módulo"). Would catch:
//! a module exposing a command that is not registered in `lib.rs` (or vice versa), and any
//! generic shell/fs/http command sneaking into the WebView surface.

#[test]
fn every_module_command_is_registered_exactly_once_in_the_handler() {
    let lib = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    let handler_start = lib
        .find("generate_handler![")
        .expect("invoke handler present");
    let handler_end = lib[handler_start..].find(']').unwrap() + handler_start;
    let handler = &lib[handler_start..handler_end];
    let registered: Vec<&str> = handler
        .split(',')
        .map(str::trim)
        .filter(|s| s.contains("::"))
        .map(|s| s.rsplit("::").next().unwrap())
        .collect();

    let mut declared = Vec::new();
    for (module, commands) in herdr_desktop::command_registry() {
        for command in commands {
            assert!(
                registered.contains(command),
                "{module}::{command} is declared but not registered in lib.rs"
            );
            declared.push(*command);
        }
    }
    let mut sorted = registered.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        registered.len(),
        "duplicate registration in handler"
    );
    for command in &registered {
        assert!(
            declared.contains(command),
            "{command} is registered but no module declares it"
        );
    }
    assert_eq!(declared.len(), 94);
}

/// Would catch: a module command added, dropped or renamed without updating this literal list
/// (no wildcard), or a module missing from the composed registry.
#[test]
fn composed_registry_is_exactly_the_explicit_module_command_list() {
    let expected: &[(&str, &[&str])] = &[
        (
            "terminal",
            &[
                "terminal_status",
                "terminal_connect",
                "terminal_input",
                "terminal_resize",
                "terminal_focus",
                "terminal_detach",
                "session_start",
            ],
        ),
        (
            "selection",
            &[
                "selection_get",
                "selection_set",
                "surface_attach",
                "surface_input",
                "surface_resize",
                "surface_focus",
                "surface_status",
                "surface_detach",
                "surface_interest",
            ],
        ),
        (
            "terminal_actions",
            &[
                "surface_pane_focus",
                "surface_scroll",
                "surface_copy_selection",
                "surface_open_link",
                "surface_paste_clipboard",
                "surface_focus_host",
            ],
        ),
        (
            "connections",
            &[
                "connections_list",
                "connections_watch",
                "connection_profile_save",
                "connection_profiles_import",
                "connection_connect",
                "connection_cancel",
                "connection_disconnect",
                "connection_reconnect",
                "connection_remove",
                "connection_send_text",
                "connection_workspaces",
                "connections_set_connect_on_open",
            ],
        ),
        (
            "agents",
            &[
                "agents_connect",
                "agents_overview",
                "agents_detach",
                "agent_start",
                "agent_autonomy_flags",
                "agent_prompt",
                "agent_open_attention",
                "pane_split",
                "pane_focus",
                "pane_set_split_ratio",
                "pane_input",
                "pane_rename",
                "pane_swap",
                "pane_input_set",
                "tab_create",
                "tab_focus",
                "tab_close",
                "tab_rename",
                "pane_zoom",
                "pane_close",
            ],
        ),
        (
            "projects",
            &[
                "projects_list",
                "project_create",
                "collection_create",
                "collection_add_project",
                "collection_remove_project",
                "collection_move_project",
                "collection_move",
                "project_open",
                "project_pick_folder",
                "group_create",
                "group_assign",
                "workspace_pref_set",
                "group_rename",
                "group_set_color",
                "group_set_collapsed",
                "group_delete",
                "recent_folder_add",
            ],
        ),
        (
            "workspace",
            &[
                "workspace_focus",
                "workspace_create",
                "workspace_rename",
                "workspace_close",
                "host_tab_close",
            ],
        ),
        (
            "files",
            &[
                "files_list",
                "files_read",
                "files_stat",
                "files_save",
                "files_save_recovery",
                "files_release",
            ],
        ),
        (
            "remote_files",
            &[
                "remote_files_hosts",
                "remote_files_watch",
                "remote_files_list",
                "remote_files_read",
                "remote_files_stat",
                "remote_files_cancel",
            ],
        ),
        ("home", &["system_user", "pane_read"]),
        ("theme", &["theme_current"]),
        ("ui_trace", &["ui_trace"]),
        ("locale", &["app_locale"]),
        ("agent_kinds", &["agent_kinds_available"]),
    ];
    let actual = herdr_desktop::command_registry();
    assert_eq!(actual.len(), expected.len());
    for ((module, commands), (want_module, want)) in actual.iter().zip(expected) {
        assert_eq!(module, want_module);
        assert_eq!(commands, want, "{module}");
    }
}

#[test]
fn no_generic_shell_fs_or_http_commands_are_exposed() {
    let forbidden = [
        "shell",
        "exec",
        "spawn",
        "open_url",
        "read_file",
        "write_file",
        "http",
        "fetch",
    ];
    for (module, commands) in herdr_desktop::command_registry() {
        for command in commands {
            for word in forbidden {
                assert!(
                    !command.contains(word),
                    "{module}::{command} looks like a generic {word} command; IPC must stay limited"
                );
            }
        }
    }
    let capability = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/capabilities/default.json"
    ))
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&capability).unwrap();
    let permissions: Vec<&str> = json["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert_eq!(
        permissions,
        [
            "core:default",
            "core:window:allow-close",
            "core:window:allow-minimize",
            "core:window:allow-toggle-maximize",
            "core:window:allow-start-dragging"
        ],
        "only core IPC permissions are granted (window chrome of spec 017, no shell/fs/http)"
    );
    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json")).unwrap(),
    )
    .unwrap();
    let csp = config["app"]["security"]["csp"].as_str().unwrap();
    assert!(
        csp.starts_with("default-src 'self'"),
        "CSP must not be disabled: {csp}"
    );
}

/// Spec 007 r10 (Planner decision): after the connection modal closes, the host returns keyboard
/// focus to the WebView (GUI r9: the WebView lost GTK widget focus when the modal left) through one
/// narrow command. Would catch: a command taking WebView arguments (a window label, a target), focus
/// applied to a window other than the invoking product window, or a failed focus reported as done.
mod focus_host {
    use std::cell::Cell;

    use herdr_client::RuntimeError;
    use herdr_desktop::bridge::terminal_actions::{
        focus_host, HostFocusWindow, PRODUCT_WINDOW_LABEL,
    };

    struct Window {
        label: &'static str,
        fail: bool,
        focused: Cell<u32>,
    }

    impl Window {
        fn new(label: &'static str) -> Self {
            Self {
                label,
                fail: false,
                focused: Cell::new(0),
            }
        }
    }

    impl HostFocusWindow for Window {
        fn label(&self) -> &str {
            self.label
        }
        fn set_focus(&self) -> Result<(), RuntimeError> {
            self.focused.set(self.focused.get() + 1);
            if self.fail {
                Err(RuntimeError::new("focus_host_failed", "fake"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn focuses_the_invoking_product_window_once() {
        let w = Window::new("main");
        assert_eq!(focus_host(&w), Ok(()));
        assert_eq!(w.focused.get(), 1);
    }

    #[test]
    fn refuses_any_other_window_without_focusing_it() {
        for label in ["connections", "Main", "main ", "", "main-2"] {
            let w = Window::new(label);
            let e = focus_host(&w).unwrap_err();
            assert_eq!(e.code, "focus_host_refused", "{label:?}");
            assert_eq!(w.focused.get(), 0, "{label:?}");
        }
    }

    #[test]
    fn a_failed_focus_is_an_error_not_a_success() {
        let w = Window {
            fail: true,
            ..Window::new("main")
        };
        assert_eq!(focus_host(&w).unwrap_err().code, "focus_host_failed");
        assert_eq!(w.focused.get(), 1, "never retried");
    }

    #[test]
    fn the_product_label_is_the_single_configured_window() {
        let config: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tauri.conf.json"))
                .unwrap(),
        )
        .unwrap();
        let windows = config["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0]["label"], PRODUCT_WINDOW_LABEL);
    }

    /// Only the window Tauri injects for the invoking webview; nothing supplied by the WebView.
    #[test]
    fn the_command_takes_no_webview_arguments() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bridge/terminal_actions.rs"
        ))
        .unwrap();
        let compact: String = source.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            compact.contains("#[tauri::command] pub async fn surface_focus_host<R: tauri::Runtime>( window: tauri::WebviewWindow<R>, ) -> Result<(), RuntimeError> { focus_host(&window) }"),
            "surface_focus_host signature changed"
        );
    }

    /// Decision r11 (GUI r10: `WebviewWindow::set_focus` = tao `present_with_time` on an already
    /// active toplevel; `document.hasFocus()` stayed false 88 ms after the ack). Would catch: the
    /// product window focused at toplevel level again instead of the WebView widget
    /// (`Webview::set_focus` → wry `grab_focus`), or the label read from another object.
    #[test]
    fn the_product_focuses_the_webview_widget_not_the_toplevel() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/bridge/terminal_actions.rs"
        ))
        .unwrap();
        let compact: String = source.split_whitespace().collect::<Vec<_>>().join(" ");
        let start = compact
            .find("impl<R: tauri::Runtime> HostFocusWindow for tauri::WebviewWindow<R> {")
            .expect("product impl of HostFocusWindow");
        let body: String = compact[start..].chars().take(400).collect();
        assert!(
            body.contains("fn set_focus(&self) -> Result<(), RuntimeError> { tauri::Webview::set_focus(self.as_ref())"),
            "{body}"
        );
        assert!(
            body.contains("fn label(&self) -> &str { tauri::WebviewWindow::label(self) }"),
            "{body}"
        );
        assert!(
            !compact.contains("tauri::WebviewWindow::set_focus"),
            "toplevel focus still used"
        );
    }
}
