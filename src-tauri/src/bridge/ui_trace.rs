//! Debug trace of the WebView UI (spec 028 r3). One additive command writes the line the WebView
//! sends to the host's stderr in debug builds (`[ui] ...`); a release build ignores it. The line
//! is bounded and carries only what the WebView formats itself (coordinates, element names, menu
//! state) — never clipboard content, credentials or frame data.

/// Commands this module exposes to the WebView (kept in sync with `lib.rs` by a test).
pub const COMMANDS: &[&str] = &["ui_trace"];

/// Longest line accepted; a runaway WebView trace never floods the debug log.
pub const MAX_UI_TRACE_LINE_BYTES: usize = 4096;

/// Prefix of every line written to stderr.
pub const UI_TRACE_PREFIX: &str = "[ui] ";

/// Payload written for one accepted line: only a debug build with a bounded line produces one.
pub fn ui_trace_payload(line: &str, debug: bool) -> Option<String> {
    (debug && line.len() <= MAX_UI_TRACE_LINE_BYTES).then(|| format!("{UI_TRACE_PREFIX}{line}"))
}

/// Writes one WebView debug line to stderr (debug builds only; a no-op in release).
#[tauri::command]
pub fn ui_trace(line: String) {
    if let Some(payload) = ui_trace_payload(&line, cfg!(debug_assertions)) {
        eprintln!("{payload}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_debug_line_is_prefixed_and_a_release_line_is_dropped() {
        assert_eq!(
            ui_trace_payload("contextmenu client=(10,20) pane=w1:p1", true).as_deref(),
            Some("[ui] contextmenu client=(10,20) pane=w1:p1")
        );
        assert_eq!(ui_trace_payload("anything", false), None);
    }

    #[test]
    fn the_line_is_bounded_by_the_host_limit() {
        assert!(ui_trace_payload(&"x".repeat(MAX_UI_TRACE_LINE_BYTES), true).is_some());
        assert_eq!(
            ui_trace_payload(&"x".repeat(MAX_UI_TRACE_LINE_BYTES + 1), true),
            None
        );
    }
}
