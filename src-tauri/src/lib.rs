//! Herdr Desktop — Tauri host.
//!
//! The WebView receives validated DTOs over IPC commands and one bounded channel of
//! frame events. It never sees sockets, credentials, environment or a shell. Commands
//! are registered per module (`COMMANDS`) and assembled here in one handler.
//!
//! Composition (007): one [`DesktopServices`] owns the single `ConnectionsState` hub, the
//! selection and its surface, hosted agents and projects, and the Local/remote file providers.
//! [`configure`] + [`handler`] mount the product; a native harness can mount the same product
//! with its own extra command by dispatching to [`handler`] (no second registry).

pub mod agent_kinds;
pub mod bridge;
pub mod connections;
pub mod files;
pub mod heap;
pub mod locale;
pub mod project_store;
pub mod terminal;
pub mod theme;

use std::path::PathBuf;
use std::sync::Arc;

use herdr_client::bootstrap::BootstrapConfig;
use herdr_client::{RuntimeError, SurfaceGeometry};
use tauri::{Manager, Runtime};

use bridge::agent_commands::AgentsState;
use bridge::composition::{ComposedSurface, SurfaceConfig};
use bridge::project_hosts::HostedProjectGateways;
use bridge::selection::SelectionState;
use bridge::terminal_actions::{TauriEffects, TerminalActions};
use connections::commands::{ConnectionsConfig, ConnectionsState};
use connections::profiles::herdr_state_dir;
use connections::ssh_options::IsolatedSshConfig;
use files::local::FilesState;
use files::sftp::{OpenSshSftpConnector, RemoteFilesConfig, RemoteFilesState, OPERATION_DEADLINE};
use project_store::ProjectsState;

/// Every IPC command exposed to the WebView, by module. Tests assert this registry
/// matches the handler so a module cannot expose a command silently.
pub fn command_registry() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("terminal", terminal::COMMANDS),
        ("selection", bridge::composition::COMMANDS),
        ("terminal_actions", bridge::terminal_actions::COMMANDS),
        ("connections", connections::commands::COMMANDS),
        ("agents", bridge::agent_commands::COMMANDS),
        ("projects", project_store::COMMANDS),
        ("workspace", bridge::workspace_commands::COMMANDS),
        ("files", files::local::COMMANDS),
        ("remote_files", files::sftp::COMMANDS),
        ("home", bridge::home::COMMANDS),
        ("theme", theme::COMMANDS),
        ("ui_trace", bridge::ui_trace::COMMANDS),
        ("locale", locale::COMMANDS),
        ("agent_kinds", agent_kinds::COMMANDS),
    ]
}

/// Initial geometry of new connections until the window reports its own (same as the WebView's
/// default).
pub const DEFAULT_GEOMETRY: SurfaceGeometry = SurfaceGeometry {
    cols: 80,
    rows: 24,
    cell_width_px: 9,
    cell_height_px: 18,
};

/// Configuration of the composed desktop.
#[derive(Debug, Clone)]
pub struct DesktopConfig {
    pub bootstrap: BootstrapConfig,
    /// Desktop preferences (connections, projects). `None` = the desktop application config
    /// directory resolved by Tauri at setup, never the engine's config directory.
    pub prefs_dir: Option<PathBuf>,
    pub herdr_state_dir: PathBuf,
    /// Test-only isolated SSH configuration (never set by the WebView).
    pub isolated_ssh: Option<IsolatedSshConfig>,
    pub geometry: SurfaceGeometry,
}

impl DesktopConfig {
    /// Product configuration: engine state dir from the environment lookup, preferences resolved
    /// at setup, the system OpenSSH configuration.
    pub fn new(bootstrap: BootstrapConfig, env: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            bootstrap,
            prefs_dir: None,
            herdr_state_dir: herdr_state_dir(env),
            isolated_ssh: None,
            geometry: DEFAULT_GEOMETRY,
        }
    }

    pub fn connections(&self, prefs_dir: PathBuf) -> ConnectionsConfig {
        ConnectionsConfig {
            prefs_dir,
            herdr_config_dir: self.bootstrap.config_dir.clone(),
            herdr_state_dir: self.herdr_state_dir.clone(),
            local_session: self.bootstrap.session.clone(),
            local_auto_start: self.bootstrap.auto_start,
            herdr_bin: self.bootstrap.herdr_bin.clone(),
            isolated_ssh: self.isolated_ssh.clone(),
            geometry: self.geometry,
        }
    }
}

/// Every managed state of the window, sharing one hub and one selection.
pub struct DesktopServices {
    pub connections: ConnectionsState,
    pub selection: SelectionState,
    pub surface: ComposedSurface,
    pub agents: AgentsState,
    pub projects: ProjectsState,
    pub files: FilesState,
    pub remote_files: RemoteFilesState,
    /// Legacy 001 terminal commands (approved harness; `session_start` is explicit Local only).
    pub terminal: terminal::TerminalState,
}

impl DesktopServices {
    /// Builds the services; connects and starts nothing.
    pub fn new(config: &DesktopConfig, prefs_dir: PathBuf) -> Result<Self, RuntimeError> {
        let connections = ConnectionsState::new(config.connections(prefs_dir.clone()));
        let selection = SelectionState::new(connections.clone());
        let surface = ComposedSurface::new(
            selection.clone(),
            SurfaceConfig {
                local_config_dir: config.bootstrap.config_dir.clone(),
                local_session: config.bootstrap.session.clone(),
                local_auto_start: config.bootstrap.auto_start,
                surface_trace: config.bootstrap.surface_trace.clone(),
            },
        );
        let agents = AgentsState::hosted(selection.agents_host());
        let files = FilesState::empty();
        let remote_files = RemoteFilesState::new(RemoteFilesConfig {
            links: Arc::new(connections.clone()),
            connector: Arc::new(OpenSshSftpConnector::new(config.isolated_ssh.clone())),
            roots: Default::default(),
            deadline: OPERATION_DEADLINE,
        })?;
        let projects = ProjectsState::with_gateways(
            prefs_dir,
            config.bootstrap.config_dir.clone(),
            Arc::new(HostedProjectGateways::new(
                selection.clone(),
                files.clone(),
                remote_files.root_authorizer(),
            )),
        );
        Ok(Self {
            terminal: terminal::TerminalState::new(config.bootstrap.clone()),
            connections,
            selection,
            surface,
            agents,
            projects,
            files,
            remote_files,
        })
    }

    pub fn manage<R: Runtime, M: Manager<R>>(self, manager: &M) {
        manager.manage(self.connections);
        manager.manage(self.selection);
        manager.manage(self.surface);
        manager.manage(self.agents);
        manager.manage(self.projects);
        manager.manage(self.files);
        manager.manage(self.remote_files);
        manager.manage(self.terminal);
    }
}

/// Resolves the preferences directory and manages every service on `app` (call from `setup`).
pub fn install<R: Runtime>(
    app: &tauri::App<R>,
    config: &DesktopConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let prefs_dir = match config.prefs_dir.clone() {
        Some(dir) => dir,
        None => app.path().app_config_dir()?,
    };
    let services = DesktopServices::new(config, prefs_dir)?;
    let connections = services.connections.clone();
    let theme = theme::load_theme_from(theme::omarchy_theme_dir().as_deref());
    theme::apply_native_background(app.handle(), &theme);
    if let Some(dir) = theme::omarchy_theme_dir() {
        theme::spawn_watch(app.handle().clone(), dir);
    }
    // Terminal actions write the clipboard / open links through the native plugins of this app.
    let actions = TerminalActions::composed(
        services.surface.clone(),
        Arc::new(TauriEffects::new(app.handle().clone())),
    );
    // Native folder picker for "Abrir projeto": Rust side only, same as clipboard.
    services
        .projects
        .set_folder_picker(Arc::new(TauriFolderPicker {
            app: app.handle().clone(),
        }));
    services.manage(app);
    app.manage(actions);
    // Spec 058 (AC-058-02): the window is installed, so the hosts that were connected when the
    // app was last used are dialed on a thread of their own, through the same path as the
    // Conectar button. The setup returns right away; nothing here waits for a remote host.
    let _ = connections.resume_connected_hosts();
    // One `malloc_trim` after the window settled (spec 009). Never periodic: hidden panes keep
    // their zero periodic repaint and idle CPU is untouched.
    heap::spawn_startup_trim(heap::STARTUP_TRIM_AFTER);
    Ok(())
}

/// Tauri dialog plugin: directory-only fixed (AC-016-03), title translated by the front (071).
struct TauriFolderPicker<R: Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: Runtime> project_store::FolderPicker for TauriFolderPicker<R> {
    fn pick_folder(&self, title: &str) -> Result<Option<String>, RuntimeError> {
        use tauri_plugin_dialog::DialogExt;
        match self
            .app
            .dialog()
            .file()
            .set_title(title)
            .blocking_pick_folder()
        {
            None => Ok(None),
            Some(path) => path
                .into_path()
                .map(|p| Some(p.to_string_lossy().into_owned()))
                .map_err(|_| {
                    RuntimeError::new(
                        "invalid_project_root",
                        "the chosen path is not a valid folder",
                    )
                }),
        }
    }
}

/// Closing the window releases only this GUI's clients and channels, off the GUI thread; engines,
/// PTYs, agents and remote servers keep running.
pub fn release_clients<R: Runtime>(app: &tauri::AppHandle<R>) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("herdr-desktop-close".into())
        .spawn(move || {
            if let Some(surface) = app.try_state::<ComposedSurface>() {
                surface.detach();
            }
            if let Some(agents) = app.try_state::<AgentsState>() {
                agents.detach();
            }
            if let Some(terminal) = app.try_state::<terminal::TerminalState>() {
                terminal.detach();
            }
            if let Some(connections) = app.try_state::<ConnectionsState>() {
                connections.detach_all();
            }
        });
}

/// Product setup and window lifecycle on `builder` (the invoke handler is [`handler`]).
pub fn configure<R: Runtime>(
    builder: tauri::Builder<R>,
    config: DesktopConfig,
) -> tauri::Builder<R> {
    // Allocator policy before the host spawns its threads (spec 009): `mallopt` only governs the
    // arenas glibc creates after it. A platform that has no such policy reports it and goes on.
    let applied = heap::configure();
    if !applied.all() {
        tracing::debug!(?applied, "glibc heap policy not fully applied");
    }
    // The WebKit processes are spawned later, from this environment (spec 009).
    let inherited = heap::export_child_policy();
    tracing::debug!(?inherited, "glibc heap policy exported to child processes");
    builder
        // Native clipboard and default-browser opener for terminal actions (Rust side only: no
        // capability grants to the WebView, and no automatic opening of links clicked in it).
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        // Native folder picker for "Abrir projeto" (spec 016). Rust side only: no dialog
        // capability is granted to the WebView; the chosen path is validated by the project
        // store before it becomes a Local project.
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| install(app, &config))
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                release_clients(window.app_handle());
            }
        })
}

/// The product's one invoke handler (every module's `COMMANDS`).
pub fn handler<R: Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        terminal::terminal_status,
        terminal::terminal_connect,
        terminal::terminal_input,
        terminal::terminal_resize,
        terminal::terminal_focus,
        terminal::terminal_detach,
        terminal::session_start,
        bridge::composition::selection_get,
        bridge::composition::selection_set,
        bridge::composition::surface_attach,
        bridge::composition::surface_input,
        bridge::composition::surface_resize,
        bridge::composition::surface_focus,
        bridge::composition::surface_status,
        bridge::composition::surface_detach,
        bridge::composition::surface_interest,
        bridge::terminal_actions::surface_pane_focus,
        bridge::terminal_actions::surface_scroll,
        bridge::terminal_actions::surface_copy_selection,
        bridge::terminal_actions::surface_open_link,
        bridge::terminal_actions::surface_paste_clipboard,
        bridge::terminal_actions::surface_focus_host,
        connections::commands::connections_list,
        connections::commands::connections_watch,
        connections::commands::connection_profile_save,
        connections::commands::connection_profiles_import,
        connections::commands::connection_connect,
        connections::commands::connection_cancel,
        connections::commands::connection_disconnect,
        connections::commands::connection_reconnect,
        connections::commands::connection_remove,
        connections::commands::connection_send_text,
        connections::commands::connection_workspaces,
        connections::commands::connections_set_connect_on_open,
        bridge::agent_commands::agents_connect,
        bridge::agent_commands::agents_overview,
        bridge::agent_commands::agents_detach,
        bridge::agent_commands::agent_start,
        bridge::agent_commands::agent_autonomy_flags,
        bridge::agent_commands::agent_prompt,
        bridge::agent_commands::agent_open_attention,
        bridge::agent_commands::pane_split,
        bridge::agent_commands::pane_focus,
        bridge::agent_commands::pane_set_split_ratio,
        bridge::agent_commands::pane_input,
        bridge::agent_commands::pane_rename,
        bridge::agent_commands::pane_swap,
        bridge::agent_commands::pane_input_set,
        bridge::agent_commands::pane_zoom,
        bridge::agent_commands::pane_close,
        bridge::agent_commands::tab_create,
        bridge::agent_commands::tab_focus,
        bridge::agent_commands::tab_close,
        bridge::agent_commands::tab_rename,
        project_store::projects_list,
        project_store::project_create,
        project_store::collection_create,
        project_store::collection_add_project,
        project_store::collection_remove_project,
        project_store::collection_move_project,
        project_store::collection_move,
        project_store::project_open,
        project_store::project_pick_folder,
        project_store::group_create,
        project_store::group_assign,
        project_store::workspace_pref_set,
        project_store::group_rename,
        project_store::group_set_color,
        project_store::group_set_collapsed,
        project_store::group_delete,
        project_store::recent_folder_add,
        bridge::workspace_commands::workspace_focus,
        bridge::workspace_commands::workspace_create,
        bridge::workspace_commands::workspace_rename,
        bridge::workspace_commands::workspace_close,
        bridge::workspace_commands::host_tab_close,
        files::local::files_list,
        files::local::files_read,
        files::local::files_stat,
        files::local::files_save,
        files::local::files_save_recovery,
        files::local::files_release,
        files::sftp::remote_files_hosts,
        files::sftp::remote_files_watch,
        files::sftp::remote_files_list,
        files::sftp::remote_files_read,
        files::sftp::remote_files_stat,
        files::sftp::remote_files_cancel,
        bridge::home::system_user,
        bridge::home::pane_read,
        theme::theme_current,
        bridge::ui_trace::ui_trace,
        locale::app_locale,
        agent_kinds::agent_kinds_available,
    ]
}

/// The composed product builder.
pub fn builder(config: DesktopConfig) -> tauri::Builder<tauri::Wry> {
    configure(tauri::Builder::default(), config).invoke_handler(handler())
}

/// Runs the desktop with an already-validated bootstrap configuration.
pub fn run(config: BootstrapConfig) -> tauri::Result<()> {
    let config = DesktopConfig::new(config, &herdr_client::bootstrap::process_env);
    builder(config)
        .build(tauri::generate_context!())?
        .run(|_app, _event| {});
    Ok(())
}
