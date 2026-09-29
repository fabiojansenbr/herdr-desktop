//! Bridges from the WebView/IPC layer to the runtime: agents (004), the SSH transport (003), the
//! composed selection of the window and its terminal actions (007).

pub mod agent_commands;
pub mod composition;
pub mod events;
// Home screen of the design (spec 012): system user and static pane snapshots.
pub mod home;
pub mod project_hosts;
pub mod selection;
pub mod ssh;
pub mod terminal_actions;
// Debug trace of the WebView UI to the host's stderr (spec 028 r3).
pub mod ui_trace;
// Engine workspace actions of the projects side bar (spec 025).
pub mod workspace_commands;
