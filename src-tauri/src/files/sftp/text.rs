//! Text decoding of remote reads (private helper of spec 006).
//!
//! Same public semantics as the local provider of spec 005 (`files::local::TextSnapshot`):
//! NUL or invalid UTF-8 is not text, a valid BOM and CRLF are kept as metadata and the content
//! is normalised to `\n`. Kept private here because `local.rs` does not export its decoder.

use crate::files::local::LineEnding;

pub struct Decoded {
    pub content: String,
    pub bom: bool,
    pub eol: LineEnding,
}

/// `None` when the bytes are not UTF-8 text (NUL or invalid sequences).
pub fn decode(bytes: &[u8]) -> Option<Decoded> {
    if bytes.contains(&0) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let (bom, body) = match text.strip_prefix('\u{FEFF}') {
        Some(body) => (true, body),
        None => (false, text),
    };
    let eol = if body.contains("\r\n") {
        LineEnding::Crlf
    } else {
        LineEnding::Lf
    };
    let content = if eol == LineEnding::Crlf {
        body.replace("\r\n", "\n")
    } else {
        body.to_owned()
    };
    Some(Decoded { content, bom, eol })
}
