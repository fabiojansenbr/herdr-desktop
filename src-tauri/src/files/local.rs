//! Local files (spec 005).
//!
//! - [`LocalFileProvider`]: a [`FileProvider`] restricted to explicitly authorized roots
//!   (absolute, canonicalized; symlinks and `..` are resolved and refused when they leave a
//!   root). It reads UTF-8 text up to the inclusive 2 MiB limit, pages directory listings
//!   (≤128 entries with an opaque cursor, never silently truncated) and writes through a
//!   sibling temp file + rename.
//! - [`FilesState`]: live text snapshots plus the paged listing used by the WebView. Every
//!   read produces an immutable snapshot with its own opaque id and [`FileUri`]; saving takes
//!   a snapshot id and compares the *content* on disk with the snapshot content before
//!   writing. Different ids (or a newer mtime) over identical content are not a conflict.
//! - Tauri commands (`COMMANDS`) are registered by the window composition (spec 007); until
//!   then they are compiled and exercised by `src-tauri/tests/files_local.rs`. Spec 006
//!   reuses [`FilePage`], [`TextSnapshot`], [`SaveOutcome`] and the provider/list_page API.
//!
//! Atomicity (TASK-005-05): saving checks the disk content and then renames a sibling temp
//! file over the target. That is an **optimistic check, not a CAS**: another process may
//! write between the check and the rename, and the rename wins. This module never locks a
//! file, never coordinates with external editors/agents and must not promise such a lock. On
//! a detected conflict nothing is written to the original path; the user's text stays in the
//! buffer and [`FilesState::save_recovery`] writes a preserved copy next to the file.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use herdr_client::{
    FileCapabilities, FileEntry, FileKind, FileProvider, FileStat, FileUri, RuntimeError,
};
use serde::{Deserialize, Serialize};

/// Provider id used in every [`FileUri`] this module serves.
pub const PROVIDER_ID: &str = "local";
/// Inclusive size limit for text files (2 MiB).
pub const MAX_TEXT_BYTES: u64 = 2_097_152;
/// Maximum number of entries in one [`FilePage`].
pub const MAX_PAGE_ENTRIES: usize = 128;
/// Live snapshots kept per state; the oldest are evicted.
pub const MAX_LIVE_SNAPSHOTS: usize = 128;
/// Marker inserted before the timestamp of a recovery copy file name.
pub const RECOVERY_MARKER: &str = ".herdr-recuperacao-";

// ---------------------------------------------------------------------------------------
// Paged listing and text snapshots
// ---------------------------------------------------------------------------------------

/// One page of a directory listing. `next_cursor` is `Some` exactly while entries remain, so
/// a consumer can never mistake a truncated response for the whole directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePage {
    pub uri: FileUri,
    pub entries: Vec<FileEntry>,
    /// Opaque continuation token; `None` on the last page.
    pub next_cursor: Option<String>,
}

/// Line separator detected in the file; re-applied when saving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    Lf,
    Crlf,
}

/// Immutable content read from one file. The id is opaque; the uri identifies the origin and
/// `content` is normalised to `\n` so the editor and the diff never see CRLF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSnapshot {
    pub id: String,
    pub uri: FileUri,
    pub content: String,
    pub bom: bool,
    pub eol: LineEnding,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_unix_ms: Option<u64>,
}

/// Result of an explicit save against a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SaveOutcome {
    /// The disk content still matched the snapshot; the buffer was written.
    Saved { snapshot: TextSnapshot },
    /// The disk content differs (or is no longer readable text): nothing was overwritten.
    /// `current` is a fresh snapshot of the disk version when it is readable text.
    Conflict {
        current: Option<TextSnapshot>,
        message: String,
    },
}

/// Preserved copy of a buffer that could not be saved over the original file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryCopy {
    pub path: String,
    pub uri: FileUri,
    pub bytes: u64,
}

// ---------------------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------------------

/// [`FileProvider`] over the local filesystem, limited to authorized roots.
#[derive(Debug)]
pub struct LocalFileProvider {
    roots: Vec<PathBuf>,
}

impl LocalFileProvider {
    /// Authorizes `roots` (absolute; canonicalized). An empty or relative root is refused.
    pub fn new(roots: Vec<PathBuf>) -> Result<Self, RuntimeError> {
        let mut authorized: Vec<PathBuf> = Vec::new();
        for root in roots {
            if root.as_os_str().is_empty() || !root.is_absolute() {
                return Err(file_error(
                    "file_uri_invalid",
                    "the authorized root must be an absolute path",
                ));
            }
            let root = canonicalize_lenient(&root);
            if !authorized.contains(&root) {
                authorized.push(root);
            }
        }
        if authorized.is_empty() {
            return Err(file_error(
                "file_uri_invalid",
                "no authorized root was given",
            ));
        }
        Ok(Self { roots: authorized })
    }

    /// Paged listing: at most `limit` entries (clamped to [`MAX_PAGE_ENTRIES`]), sorted by
    /// name, continuing after the opaque cursor.
    pub fn list_page(
        &self,
        dir: &FileUri,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<FilePage, RuntimeError> {
        let mut entries = self.read_entries(dir)?;
        if let Some(cursor) = cursor {
            let after = decode_cursor(cursor)?;
            entries.retain(|entry| entry.name > after);
        }
        let take = limit.clamp(1, MAX_PAGE_ENTRIES);
        let next_cursor = if entries.len() > take {
            entries
                .get(take - 1)
                .map(|entry| encode_cursor(&entry.name))
        } else {
            None
        };
        entries.truncate(take);
        Ok(FilePage {
            uri: dir.clone(),
            entries,
            next_cursor,
        })
    }

    /// Reads one text file into an immutable snapshot. Refuses directories, files above
    /// [`MAX_TEXT_BYTES`] and non-UTF-8/NUL content before any snapshot exists.
    pub fn read_text(&self, file: &FileUri) -> Result<TextSnapshot, RuntimeError> {
        let path = self.resolve(file)?;
        let meta = std::fs::metadata(&path).map_err(|error| io_error(error, "open file"))?;
        if !meta.is_file() {
            return Err(not_a_file());
        }
        let size = meta.len();
        if size > MAX_TEXT_BYTES {
            return Err(file_too_large());
        }
        let modified = modified_unix_ms(&meta);
        let bytes = std::fs::read(&path).map_err(|error| io_error(error, "read file"))?;
        if bytes.len() as u64 > MAX_TEXT_BYTES {
            return Err(file_too_large());
        }
        let decoded = decode_text(&bytes)?;
        Ok(TextSnapshot {
            id: uuid::Uuid::new_v4().to_string(),
            uri: file.clone(),
            content: decoded.content,
            bom: decoded.bom,
            eol: decoded.eol,
            size: bytes.len() as u64,
            modified_unix_ms: modified,
        })
    }

    /// Writes `contents` atomically (sibling temp + rename), re-encoding per `bom`/`eol`.
    pub fn write_text(
        &self,
        file: &FileUri,
        contents: &str,
        bom: bool,
        eol: LineEnding,
    ) -> Result<(), RuntimeError> {
        self.write_bytes(file, &encode_text(contents, bom, eol))
    }

    /// Writes raw bytes atomically inside an authorized root.
    pub fn write_bytes(&self, file: &FileUri, contents: &[u8]) -> Result<(), RuntimeError> {
        if contents.len() as u64 > MAX_TEXT_BYTES {
            return Err(file_too_large());
        }
        let path = self.resolve(file)?;
        if path.is_dir() {
            return Err(not_a_file());
        }
        atomic_write_bytes(&path, contents)
    }

    /// Path of a new recovery copy next to `file` (never overwrites an existing copy).
    pub fn recovery_path(&self, file: &FileUri) -> Result<PathBuf, RuntimeError> {
        let path = self.resolve(file)?;
        let name = path
            .file_name()
            .ok_or_else(|| file_error("file_uri_invalid", "the file has no name"))?
            .to_string_lossy()
            .into_owned();
        let parent = path
            .parent()
            .ok_or_else(|| file_error("file_uri_invalid", "the file has no directory"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0);
        let mut candidate = parent.join(format!("{name}{RECOVERY_MARKER}{stamp}"));
        let mut suffix = 1;
        while candidate.exists() {
            candidate = parent.join(format!("{name}{RECOVERY_MARKER}{stamp}-{suffix}"));
            suffix += 1;
        }
        Ok(candidate)
    }

    /// Validates provider/host/path and resolves to the real (symlink-free) path, which must
    /// stay inside one authorized root.
    fn resolve(&self, uri: &FileUri) -> Result<PathBuf, RuntimeError> {
        if uri.provider != PROVIDER_ID {
            return Err(file_error(
                "file_uri_invalid",
                "the file's provider is not the local provider",
            ));
        }
        if uri.host.is_some() {
            return Err(file_error(
                "file_host_unsupported",
                "this local provider does not open remote host files; no fallback to the local host",
            ));
        }
        if uri.path.is_empty() {
            return Err(file_error("file_uri_invalid", "empty file path"));
        }
        let path = PathBuf::from(&uri.path);
        if !path.is_absolute() {
            return Err(file_error(
                "file_uri_invalid",
                "the file path must be absolute",
            ));
        }
        let resolved = canonicalize_lenient(&path);
        if self.roots.iter().any(|root| resolved.starts_with(root)) {
            Ok(resolved)
        } else {
            Err(file_error(
                "path_outside_root",
                "the file is outside this project's authorized roots",
            ))
        }
    }

    fn read_entries(&self, dir: &FileUri) -> Result<Vec<FileEntry>, RuntimeError> {
        let path = self.resolve(dir)?;
        let meta = std::fs::metadata(&path).map_err(|error| io_error(error, "open directory"))?;
        if !meta.is_dir() {
            return Err(not_a_directory());
        }
        let listing =
            std::fs::read_dir(&path).map_err(|error| io_error(error, "list directory"))?;
        let mut entries = Vec::new();
        for entry in listing {
            let entry = entry.map_err(|error| io_error(error, "list directory"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| io_error(error, "list directory"))?;
            entries.push(FileEntry {
                uri: FileUri {
                    provider: PROVIDER_ID.into(),
                    host: None,
                    path: path.join(entry.file_name()).to_string_lossy().into_owned(),
                },
                name: entry.file_name().to_string_lossy().into_owned(),
                kind: file_kind(&file_type),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

impl FileProvider for LocalFileProvider {
    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn capabilities(&self) -> FileCapabilities {
        FileCapabilities {
            list: true,
            read: true,
            stat: true,
            write: true,
        }
    }

    /// Full listing (never truncated). Paged consumers use [`LocalFileProvider::list_page`].
    fn list(&self, dir: &FileUri) -> Result<Vec<FileEntry>, RuntimeError> {
        self.read_entries(dir)
    }

    /// Refuses (instead of truncating) when the file exceeds `max_bytes` or the text limit.
    fn read(&self, file: &FileUri, max_bytes: u64) -> Result<Vec<u8>, RuntimeError> {
        let limit = max_bytes.min(MAX_TEXT_BYTES);
        let path = self.resolve(file)?;
        let meta = std::fs::metadata(&path).map_err(|error| io_error(error, "open file"))?;
        if !meta.is_file() {
            return Err(not_a_file());
        }
        if meta.len() > limit {
            return Err(file_too_large());
        }
        let bytes = std::fs::read(&path).map_err(|error| io_error(error, "read file"))?;
        if bytes.len() as u64 > limit {
            return Err(file_too_large());
        }
        Ok(bytes)
    }

    fn stat(&self, uri: &FileUri) -> Result<FileStat, RuntimeError> {
        let path = self.resolve(uri)?;
        let meta = std::fs::metadata(&path).map_err(|error| io_error(error, "stat file"))?;
        Ok(FileStat {
            uri: uri.clone(),
            kind: file_kind(&meta.file_type()),
            size: meta.len(),
            modified_unix_ms: modified_unix_ms(&meta),
            read_only: meta.permissions().readonly(),
        })
    }

    fn write(&self, file: &FileUri, contents: &[u8]) -> Result<(), RuntimeError> {
        self.write_bytes(file, contents)
    }
}

fn file_kind(file_type: &std::fs::FileType) -> FileKind {
    if file_type.is_symlink() {
        FileKind::Symlink
    } else if file_type.is_dir() {
        FileKind::Directory
    } else if file_type.is_file() {
        FileKind::File
    } else {
        FileKind::Other
    }
}

// ---------------------------------------------------------------------------------------
// Snapshots + paged state
// ---------------------------------------------------------------------------------------

/// Live snapshots and the provider for one authorized root set.
///
/// Snapshots are scoped to this state: an id issued by another state (another GUI "boot" or
/// host) is unknown here, and an unknown id can never write. Conflict detection still compares
/// real content, never ids.
///
/// Cloning shares the same roots and snapshots (one state for the WebView commands and the
/// projects service); it never duplicates them.
#[derive(Clone)]
pub struct FilesState {
    inner: Arc<Mutex<FilesInner>>,
}

struct FilesInner {
    provider: LocalFileProvider,
    snapshots: HashMap<String, TextSnapshot>,
    order: VecDeque<String>,
}

impl FilesState {
    pub fn new(roots: Vec<PathBuf>) -> Result<Self, RuntimeError> {
        Self::from_provider(LocalFileProvider::new(roots)?)
    }

    pub fn from_provider(provider: LocalFileProvider) -> Result<Self, RuntimeError> {
        Ok(Self {
            inner: Arc::new(Mutex::new(FilesInner {
                provider,
                snapshots: HashMap::new(),
                order: VecDeque::new(),
            })),
        })
    }

    /// State with no authorized root: every path is refused until a project root is
    /// authorized through [`FilesState::authorize_root`]. Nothing (cwd, home) by default.
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(Mutex::new(FilesInner {
                provider: LocalFileProvider { roots: Vec::new() },
                snapshots: HashMap::new(),
                order: VecDeque::new(),
            })),
        }
    }

    /// Adds one project root (Rust only; no WebView command). The root must be an absolute,
    /// existing directory; it is canonicalized (symlinks resolved) and must not be the
    /// filesystem root. Adding an already authorized root is a no-op. Live snapshots and the
    /// other roots are kept. Returns the canonical root.
    pub fn authorize_root(&self, root: impl AsRef<Path>) -> Result<PathBuf, RuntimeError> {
        let root = canonical_root(root.as_ref())?;
        self.with_inner(|inner| {
            if !inner.provider.roots.contains(&root) {
                inner.provider.roots.push(root.clone());
            }
            Ok(root)
        })
    }

    /// Canonical authorized roots, in authorization order.
    pub fn authorized_roots(&self) -> Vec<PathBuf> {
        self.inner
            .lock()
            .map(|inner| inner.provider.roots.clone())
            .unwrap_or_default()
    }

    fn with_inner<T>(
        &self,
        run: impl FnOnce(&mut FilesInner) -> Result<T, RuntimeError>,
    ) -> Result<T, RuntimeError> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| file_error("state_poisoned", "the files' internal state is invalid"))?;
        run(&mut inner)
    }

    /// One page (≤128 entries) of `uri`, continuing after `cursor`.
    pub fn list(&self, uri: &FileUri, cursor: Option<&str>) -> Result<FilePage, RuntimeError> {
        self.with_inner(|inner| inner.provider.list_page(uri, cursor, MAX_PAGE_ENTRIES))
    }

    /// Reads text and registers the snapshot.
    pub fn read(&self, uri: &FileUri) -> Result<TextSnapshot, RuntimeError> {
        self.with_inner(|inner| {
            let snapshot = inner.provider.read_text(uri)?;
            inner.remember(snapshot.clone());
            Ok(snapshot)
        })
    }

    pub fn stat(&self, uri: &FileUri) -> Result<FileStat, RuntimeError> {
        self.with_inner(|inner| inner.provider.stat(uri))
    }

    pub fn snapshot(&self, id: &str) -> Option<TextSnapshot> {
        self.inner
            .lock()
            .ok()
            .and_then(|inner| inner.snapshots.get(id).cloned())
    }

    /// Saves `content` against `snapshot_id`. Writes only when the disk content still equals
    /// the snapshot content; otherwise returns a conflict and touches nothing.
    pub fn save(&self, snapshot_id: &str, content: &str) -> Result<SaveOutcome, RuntimeError> {
        if content.len() as u64 > MAX_TEXT_BYTES {
            return Err(file_too_large());
        }
        self.with_inner(|inner| {
            let base = inner
                .snapshots
                .get(snapshot_id)
                .cloned()
                .ok_or_else(snapshot_unknown)?;
            let current = match inner.provider.read_text(&base.uri) {
                Ok(current) => current,
                Err(error)
                    if error.code == "file_too_large" || error.code == "binary_unsupported" =>
                {
                    return Ok(SaveOutcome::Conflict {
                        current: None,
                        message: format!(
                            "the version on disk is no longer editable text ({}); the buffer was preserved",
                            error.code
                        ),
                    });
                }
                Err(error) if error.code == "file_not_found" => {
                    return Ok(SaveOutcome::Conflict {
                        current: None,
                        message: "the file no longer exists on disk; the buffer was preserved"
                            .into(),
                    });
                }
                Err(error) => return Err(error),
            };
            if current.content != base.content {
                return Ok(SaveOutcome::Conflict {
                    current: Some(current),
                    message:
                        "the file changed on disk after it was read; reload, compare or save a copy"
                            .into(),
                });
            }
            inner
                .provider
                .write_text(&base.uri, content, current.bom, current.eol)?;
            let saved = inner.provider.read_text(&base.uri)?;
            inner.remember(saved.clone());
            Ok(SaveOutcome::Saved { snapshot: saved })
        })
    }

    /// Writes the user's buffer to a preserved recovery copy next to the file. The original is
    /// never touched; existing copies are never overwritten.
    pub fn save_recovery(
        &self,
        snapshot_id: &str,
        content: &str,
    ) -> Result<RecoveryCopy, RuntimeError> {
        if content.len() as u64 > MAX_TEXT_BYTES {
            return Err(file_too_large());
        }
        self.with_inner(|inner| {
            let base = inner
                .snapshots
                .get(snapshot_id)
                .cloned()
                .ok_or_else(snapshot_unknown)?;
            let path = inner.provider.recovery_path(&base.uri)?;
            let uri = FileUri::local(path.to_string_lossy().into_owned());
            let bytes = encode_text(content, base.bom, base.eol);
            inner.provider.write_bytes(&uri, &bytes)?;
            Ok(RecoveryCopy {
                path: path.to_string_lossy().into_owned(),
                uri,
                bytes: bytes.len() as u64,
            })
        })
    }

    /// Frees snapshots the WebView no longer needs (closed tabs, replaced bases).
    pub fn release(&self, ids: &[String]) -> Result<(), RuntimeError> {
        self.with_inner(|inner| {
            for id in ids {
                inner.snapshots.remove(id);
            }
            let FilesInner {
                snapshots, order, ..
            } = inner;
            order.retain(|id| snapshots.contains_key(id));
            Ok(())
        })
    }
}

impl FilesInner {
    fn remember(&mut self, snapshot: TextSnapshot) {
        let id = snapshot.id.clone();
        if self.snapshots.insert(id.clone(), snapshot).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > MAX_LIVE_SNAPSHOTS {
            if let Some(oldest) = self.order.pop_front() {
                self.snapshots.remove(&oldest);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Text encoding
// ---------------------------------------------------------------------------------------

struct Decoded {
    content: String,
    bom: bool,
    eol: LineEnding,
}

/// Validates UTF-8/NUL and normalises CRLF to `\n`, keeping BOM and line ending for saving.
fn decode_text(bytes: &[u8]) -> Result<Decoded, RuntimeError> {
    if bytes.contains(&0) {
        return Err(binary_unsupported());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| binary_unsupported())?;
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
    Ok(Decoded { content, bom, eol })
}

fn encode_text(content: &str, bom: bool, eol: LineEnding) -> Vec<u8> {
    let mut text = String::with_capacity(content.len() + 3);
    if bom {
        text.push('\u{FEFF}');
    }
    match eol {
        LineEnding::Lf => text.push_str(content),
        LineEnding::Crlf => text.push_str(&content.replace('\n', "\r\n")),
    }
    text.into_bytes()
}

// ---------------------------------------------------------------------------------------
// Cursor, paths and errors
// ---------------------------------------------------------------------------------------

fn encode_cursor(name: &str) -> String {
    let mut out = String::with_capacity(name.len() * 2);
    for byte in name.as_bytes() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn decode_cursor(value: &str) -> Result<String, RuntimeError> {
    if value.is_empty() || !value.len().is_multiple_of(2) || !value.is_ascii() {
        return Err(file_error("invalid_cursor", "invalid paging cursor"));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for index in (0..value.len()).step_by(2) {
        let byte = u8::from_str_radix(&value[index..index + 2], 16)
            .map_err(|_| file_error("invalid_cursor", "invalid paging cursor"))?;
        bytes.push(byte);
    }
    String::from_utf8(bytes).map_err(|_| file_error("invalid_cursor", "invalid paging cursor"))
}

/// Canonicalizes the longest existing ancestor and re-appends the rest, so containment also
/// works for paths that do not exist yet.
fn canonicalize_lenient(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                existing = parent.to_path_buf();
            }
            _ => break,
        }
    }
    let mut resolved = std::fs::canonicalize(&existing).unwrap_or(existing);
    for name in rest.into_iter().rev() {
        resolved.push(name);
    }
    resolved
}

/// Strict validation of a dynamically authorized root (see [`FilesState::authorize_root`]).
fn canonical_root(root: &Path) -> Result<PathBuf, RuntimeError> {
    let invalid = |message: &str| file_error("file_uri_invalid", message);
    if root.as_os_str().is_empty() || !root.is_absolute() {
        return Err(invalid("the authorized root must be an absolute path"));
    }
    let canonical = std::fs::canonicalize(root)
        .map_err(|_| invalid("the authorized root does not exist or cannot be resolved"))?;
    if !canonical.is_dir() {
        return Err(invalid("the authorized root must be a directory"));
    }
    if canonical.parent().is_none() {
        return Err(invalid("the filesystem root cannot be authorized"));
    }
    Ok(canonical)
}

fn modified_unix_ms(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|elapsed| elapsed.as_millis() as u64)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), RuntimeError> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| file_error("file_uri_invalid", "the destination has no directory"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| file_error("file_uri_invalid", "the destination has no name"))?;
    let tmp = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error(error, "write file"));
    }
    Ok(())
}

fn file_error(code: &str, message: &str) -> RuntimeError {
    RuntimeError::new(code, message).with_endpoint(PROVIDER_ID)
}

fn io_error(error: std::io::Error, context: &str) -> RuntimeError {
    let (code, label) = match error.kind() {
        std::io::ErrorKind::NotFound => ("file_not_found", "resource not found"),
        std::io::ErrorKind::PermissionDenied => ("permission_denied", "permission denied"),
        _ => ("file_io_error", "I/O error"),
    };
    file_error(code, &format!("{context}: {label}"))
}

fn file_too_large() -> RuntimeError {
    file_error(
        "file_too_large",
        "the file is over 2 MiB; open it outside the editor",
    )
}

fn binary_unsupported() -> RuntimeError {
    file_error(
        "binary_unsupported",
        "the file is not UTF-8 text (or contains NUL); the editor was not loaded",
    )
}

fn not_a_file() -> RuntimeError {
    file_error("not_a_file", "the path is not a file")
}

fn not_a_directory() -> RuntimeError {
    file_error("not_a_directory", "the path is not a directory")
}

fn snapshot_unknown() -> RuntimeError {
    file_error(
        "snapshot_unknown",
        "the source read is no longer available; reload the file",
    )
}

// ---------------------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------------------

/// Commands this module exposes to the WebView (registered by the window composition, 007).
/// Paths are always qualified by a [`FileUri`]: a remote host is refused, never redirected to
/// the local filesystem.
pub const COMMANDS: &[&str] = &[
    "files_list",
    "files_read",
    "files_stat",
    "files_save",
    "files_save_recovery",
    "files_release",
];

#[tauri::command]
pub fn files_list(
    state: tauri::State<'_, FilesState>,
    uri: FileUri,
    cursor: Option<String>,
) -> Result<FilePage, RuntimeError> {
    state.list(&uri, cursor.as_deref())
}

#[tauri::command]
pub fn files_read(
    state: tauri::State<'_, FilesState>,
    uri: FileUri,
) -> Result<TextSnapshot, RuntimeError> {
    state.read(&uri)
}

#[tauri::command]
pub fn files_stat(
    state: tauri::State<'_, FilesState>,
    uri: FileUri,
) -> Result<FileStat, RuntimeError> {
    state.stat(&uri)
}

#[tauri::command]
pub fn files_save(
    state: tauri::State<'_, FilesState>,
    snapshot_id: String,
    content: String,
) -> Result<SaveOutcome, RuntimeError> {
    state.save(&snapshot_id, &content)
}

#[tauri::command]
pub fn files_save_recovery(
    state: tauri::State<'_, FilesState>,
    snapshot_id: String,
    content: String,
) -> Result<RecoveryCopy, RuntimeError> {
    state.save_recovery(&snapshot_id, &content)
}

#[tauri::command]
pub fn files_release(
    state: tauri::State<'_, FilesState>,
    snapshot_ids: Vec<String>,
) -> Result<(), RuntimeError> {
    state.release(&snapshot_ids)
}
