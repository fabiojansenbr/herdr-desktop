//! Spec 005 — local files (seam: provider over temporary directories plus one native E2E
//! open → edit → external change → conflict).
//!
//! The backend module is compiled here through `#[path]`: composing it into
//! `src-tauri/src/lib.rs` belongs to spec 007.
//!
//! AC-005-01: reading UTF-8 up to 2 MiB (inclusive) produces an immutable snapshot; saving is
//!            explicit and rewrites the same file; beyond the limit or non-text returns
//!            `file_too_large` / `binary_unsupported` without a snapshot. CRLF and BOM survive.
//! AC-005-02: a buffer whose file changed externally is never written over the new version:
//!            save reports a content conflict, keeps the previous snapshot and a recovery copy
//!            can preserve the user's text.
//! AC-005-03: listing is paged (≤128 entries, opaque cursor, never silently truncated) and
//!            snapshots are scoped to the state (host/boot boundary) that read them.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use files_local::{
    FilesState, LineEnding, LocalFileProvider, RecoveryCopy, SaveOutcome, TextSnapshot,
    MAX_PAGE_ENTRIES, MAX_TEXT_BYTES, PROVIDER_ID, RECOVERY_MARKER,
};
use herdr_client::{FileCapabilities, FileKind, FileProvider, FileStat, FileUri};

#[allow(dead_code)]
#[path = "../src/files/local.rs"]
mod files_local;

#[allow(dead_code)]
#[path = "../../scripts/feature-harness/native.rs"]
mod native_harness;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../scripts/feature-harness/window.rs"]
mod window_harness;

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("raiz");
        std::fs::create_dir_all(&root).unwrap();
        Self { _temp: temp, root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, bytes).unwrap();
        path
    }

    fn bytes(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.path(rel)).unwrap()
    }

    fn uri(&self, rel: &str) -> FileUri {
        FileUri::local(self.path(rel).to_string_lossy().into_owned())
    }

    fn uri_of(&self, path: &Path) -> FileUri {
        FileUri::local(path.to_string_lossy().into_owned())
    }

    fn state(&self) -> FilesState {
        FilesState::new(vec![self.root.clone()]).unwrap()
    }

    fn provider(&self) -> LocalFileProvider {
        LocalFileProvider::new(vec![self.root.clone()]).unwrap()
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn saved(outcome: SaveOutcome) -> TextSnapshot {
    match outcome {
        SaveOutcome::Saved { snapshot } => snapshot,
        other => panic!("expected Saved, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------------
// AC-005-01 — read, snapshot identity, explicit save, encoding, limits
// ---------------------------------------------------------------------------------------

/// Would catch: a snapshot id reused between reads, content normalised away (CRLF), a relative
/// path accepted, or a stale snapshot mutated when the file changes on disk.
#[test]
fn read_produces_immutable_snapshots_with_distinct_ids_and_uri() {
    let f = Fixture::new();
    f.write("notas.txt", b"linha um\nlinha dois\n");
    let state = f.state();

    let first = state.read(&f.uri("notas.txt")).unwrap();
    let second = state.read(&f.uri("notas.txt")).unwrap();

    assert_ne!(first.id, second.id, "each read gets its own opaque id");
    assert_eq!(first.content, "linha um\nlinha dois\n");
    assert_eq!(first.eol, LineEnding::Lf);
    assert!(!first.bom);
    assert_eq!(first.size, 20);
    assert_eq!(
        first.uri,
        FileUri {
            provider: PROVIDER_ID.into(),
            host: None,
            path: f.path("notas.txt").to_string_lossy().into_owned(),
        }
    );

    // A later external change never mutates an already issued snapshot.
    f.write("notas.txt", b"trocado\n");
    assert_eq!(
        state.snapshot(&first.id).unwrap().content,
        "linha um\nlinha dois\n"
    );
    assert_eq!(
        state.snapshot(&second.id).unwrap().content,
        "linha um\nlinha dois\n"
    );

    let caps: FileCapabilities = f.provider().capabilities();
    assert!(caps.list && caps.read && caps.stat && caps.write);
}

/// Would catch: saving that drops a BOM or converts CRLF to LF (or vice versa), or a save
/// that does not rewrite the original path.
#[test]
fn save_preserves_crlf_and_bom() {
    let f = Fixture::new();
    f.write("crlf.txt", b"\xEF\xBB\xBFum\r\ndois\r\n");
    let state = f.state();
    let base = state.read(&f.uri("crlf.txt")).unwrap();
    assert!(base.bom);
    assert_eq!(base.eol, LineEnding::Crlf);
    assert_eq!(base.content, "um\ndois\n");

    // Unmodified round trip is byte identical.
    let same = saved(state.save(&base.id, &base.content).unwrap());
    assert_eq!(f.bytes("crlf.txt"), b"\xEF\xBB\xBFum\r\ndois\r\n");
    assert!(same.bom);
    assert_eq!(same.eol, LineEnding::Crlf);

    // Edited buffer keeps the file's CRLF and BOM.
    let edited = saved(state.save(&same.id, "um\ndois\neditado\n").unwrap());
    assert_eq!(
        f.bytes("crlf.txt"),
        b"\xEF\xBB\xBFum\r\ndois\r\neditado\r\n"
    );
    assert_eq!(edited.content, "um\ndois\neditado\n");
    assert!(edited.bom);
    assert_eq!(edited.eol, LineEnding::Crlf);

    // A plain LF file without BOM stays LF without BOM.
    f.write("lf.txt", b"a\nb\n");
    let lf = state.read(&f.uri("lf.txt")).unwrap();
    let lf_saved = saved(state.save(&lf.id, "a\nb\nc\n").unwrap());
    assert_eq!(f.bytes("lf.txt"), b"a\nb\nc\n");
    assert!(!lf_saved.bom);
    assert_eq!(lf_saved.eol, LineEnding::Lf);
}

/// Would catch: a NUL or invalid UTF-8 accepted as text, or a snapshot created for a file the
/// editor can never render.
#[test]
fn binary_or_nul_files_are_refused_without_a_snapshot() {
    let f = Fixture::new();
    f.write("nul.txt", b"abc\x00def");
    f.write("latin1.txt", b"caf\xE9");
    let state = f.state();

    for name in ["nul.txt", "latin1.txt"] {
        let error = state.read(&f.uri(name)).unwrap_err();
        assert_eq!(error.code, "binary_unsupported", "{name}");
        assert!(!error.message.contains('/'), "{name}");
    }

    assert_eq!(
        state
            .save("00000000-0000-4000-8000-000000000000", "x")
            .unwrap_err()
            .code,
        "snapshot_unknown",
        "refused reads never registered a snapshot"
    );
    assert_eq!(f.bytes("nul.txt"), b"abc\x00def");
}

/// Would catch: an off-by-one limit (> vs >=), or reading a large file before checking its size.
#[test]
fn two_mib_limit_is_inclusive() {
    let f = Fixture::new();
    let exact = vec![b'a'; MAX_TEXT_BYTES as usize];
    f.write("exato.txt", &exact);
    let state = f.state();
    let snapshot = state.read(&f.uri("exato.txt")).unwrap();
    assert_eq!(snapshot.size, MAX_TEXT_BYTES);
    assert_eq!(snapshot.content.len() as u64, MAX_TEXT_BYTES);

    let bigger = vec![b'a'; MAX_TEXT_BYTES as usize + 1];
    f.write("acima.txt", &bigger);
    let error = state.read(&f.uri("acima.txt")).unwrap_err();
    assert_eq!(error.code, "file_too_large");

    // Saving a buffer larger than the limit is refused as well.
    assert_eq!(
        state
            .save(&snapshot.id, &"a".repeat(MAX_TEXT_BYTES as usize + 1))
            .unwrap_err()
            .code,
        "file_too_large"
    );
}

/// Would catch: symlinks followed outside the authorized root, `..` escapes, relative paths,
/// a URI of another provider or host falling back to the local filesystem.
#[cfg(unix)]
#[test]
fn roots_are_enforced_and_foreign_uris_never_fall_back_to_local() {
    let f = Fixture::new();
    let other = Fixture::new();
    f.write("alvo.txt", b"dentro\n");
    let outside = other.write("fora.txt", b"fora\n");
    std::os::unix::fs::symlink(f.path("alvo.txt"), f.path("atalho.txt")).unwrap();
    std::os::unix::fs::symlink(&outside, f.path("escape.txt")).unwrap();
    let state = f.state();

    // Inside symlinks are listed as symlinks and resolve while reading.
    let page = state.list(&f.uri_of(&f.root), None).unwrap();
    let atalho = page
        .entries
        .iter()
        .find(|e| e.name == "atalho.txt")
        .unwrap();
    assert_eq!(atalho.kind, FileKind::Symlink);
    assert_eq!(
        state.read(&f.uri("atalho.txt")).unwrap().content,
        "dentro\n"
    );

    // Symlink pointing outside the root is refused, never followed silently.
    assert_eq!(
        state.read(&f.uri("escape.txt")).unwrap_err().code,
        "path_outside_root"
    );
    assert_eq!(
        state.read(&f.uri_of(&outside)).unwrap_err().code,
        "path_outside_root"
    );
    assert_eq!(
        state.read(&f.uri("../fora.txt")).unwrap_err().code,
        "path_outside_root"
    );
    assert_eq!(
        state
            .read(&FileUri::local("relativo.txt"))
            .unwrap_err()
            .code,
        "file_uri_invalid"
    );

    // Distinct hosts never reach the local provider (no fallback to the local host).
    for host in ["boot-alpha", "boot-beta"] {
        let remote = FileUri::remote("local", host, f.path("alvo.txt").to_string_lossy());
        let error = state.read(&remote).unwrap_err();
        assert_eq!(error.code, "file_host_unsupported", "{host}");
        assert_eq!(error.endpoint.as_deref(), Some(PROVIDER_ID));
    }
    let foreign = FileUri {
        provider: "sftp".into(),
        host: None,
        path: f.path("alvo.txt").to_string_lossy().into_owned(),
    };
    assert_eq!(state.read(&foreign).unwrap_err().code, "file_uri_invalid");
}

/// Would catch: listing reporting a wrong kind, size, mtime or read_only flag.
#[test]
fn stat_reports_kind_size_mtime_and_read_only() {
    let f = Fixture::new();
    f.write("doc.txt", b"abc");
    f.write("sub/um.txt", b"y");
    let state = f.state();

    let stat: FileStat = state.stat(&f.uri("doc.txt")).unwrap();
    assert_eq!(stat.kind, FileKind::File);
    assert_eq!(stat.size, 3);
    assert!(stat.modified_unix_ms.is_some());
    assert!(!stat.read_only);

    let dir: FileStat = state.stat(&f.uri_of(&f.root)).unwrap();
    assert_eq!(dir.kind, FileKind::Directory);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(f.path("doc.txt"), std::fs::Permissions::from_mode(0o444))
            .unwrap();
        let read_only = state.stat(&f.uri("doc.txt")).unwrap();
        assert!(read_only.read_only);
        std::fs::set_permissions(f.path("doc.txt"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
    }
}

// ---------------------------------------------------------------------------------------
// AC-005-02 — external change, conflict, recovery copy
// ---------------------------------------------------------------------------------------

/// The core of AC-005-02. Would catch: saving over a newer external version, a conflict keyed
/// on timestamps/ids instead of content, or a conflict that discards the previous snapshot.
#[test]
fn external_change_before_save_is_a_conflict_that_never_overwrites() {
    let f = Fixture::new();
    f.write("doc.txt", b"versao base\n");
    let state = f.state();
    let base = state.read(&f.uri("doc.txt")).unwrap();

    f.write("doc.txt", b"versao externa\nmais nova\n");
    let outcome = state
        .save(&base.id, "buffer do usuario\neditado\n")
        .unwrap();

    let SaveOutcome::Conflict { current, message } = outcome else {
        panic!("expected Conflict, got {outcome:?}");
    };
    let current = current.expect("the disk version is readable text");
    assert_eq!(current.content, "versao externa\nmais nova\n");
    assert_ne!(
        current.id, base.id,
        "the disk version gets a fresh snapshot id"
    );
    assert!(!message.is_empty());

    assert_eq!(f.bytes("doc.txt"), b"versao externa\nmais nova\n");
    assert!(
        !f.bytes("doc.txt")
            .windows(b"buffer".len())
            .any(|window| window == b"buffer"),
        "the user buffer never reaches the disk on a conflict"
    );
    assert_eq!(state.snapshot(&base.id).unwrap().content, "versao base\n");

    // The disk version can be reloaded explicitly.
    let reloaded = state.read(&f.uri("doc.txt")).unwrap();
    assert_eq!(reloaded.content, "versao externa\nmais nova\n");
}

/// Would catch: conflict detection based on the snapshot id or mtime instead of the real content
/// (a different id over identical content must not conflict).
#[test]
fn different_id_or_mtime_over_identical_content_is_not_a_conflict() {
    let f = Fixture::new();
    f.write("doc.txt", b"versao base\n");
    let state = f.state();
    let base = state.read(&f.uri("doc.txt")).unwrap();

    f.write("doc.txt", b"versao base\n");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(f.path("doc.txt"))
        .unwrap();
    file.set_modified(SystemTime::now() + Duration::from_secs(3600))
        .unwrap();
    drop(file);
    let current = state.read(&f.uri("doc.txt")).unwrap();
    assert_ne!(current.id, base.id);
    assert_ne!(current.modified_unix_ms, base.modified_unix_ms);

    let snapshot = saved(state.save(&base.id, "versao base editada\n").unwrap());
    assert_eq!(f.bytes("doc.txt"), b"versao base editada\n");
    assert_eq!(snapshot.content, "versao base editada\n");
}

/// Would catch: a save that writes in place (truncating on failure), leaves temp siblings, or
/// registers a snapshot for an unknown id.
#[test]
fn save_is_atomic_and_unknown_snapshots_write_nothing() {
    let f = Fixture::new();
    f.write("doc.txt", b"base\n");
    let state = f.state();
    let before = f.bytes("doc.txt");

    assert_eq!(
        state.save("no-such-snapshot", "x\n").unwrap_err().code,
        "snapshot_unknown"
    );
    assert_eq!(f.bytes("doc.txt"), before);
    assert_eq!(names(f.root()), ["doc.txt"]);

    let base = state.read(&f.uri("doc.txt")).unwrap();
    saved(state.save(&base.id, "novo\n").unwrap());
    assert_eq!(f.bytes("doc.txt"), b"novo\n");
    assert_eq!(names(f.root()), ["doc.txt"], "no temp files left behind");
}

/// Would catch: a missing recovery copy, a copy written over the original, or a second copy
/// overwriting the first one.
#[test]
fn recovery_copy_preserves_the_user_text_next_to_the_file() {
    let f = Fixture::new();
    f.write("doc.txt", b"base\n");
    let state = f.state();
    let base = state.read(&f.uri("doc.txt")).unwrap();
    f.write("doc.txt", b"versao externa\n");
    assert!(matches!(
        state.save(&base.id, "texto do usuario\n").unwrap(),
        SaveOutcome::Conflict { .. }
    ));

    let copy: RecoveryCopy = state
        .save_recovery(&base.id, "texto do usuario\nnao perder\n")
        .unwrap();
    let copy_path = PathBuf::from(&copy.path);
    assert!(copy_path.starts_with(f.root()));
    assert!(copy_path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(&format!("doc.txt{RECOVERY_MARKER}")));
    assert_eq!(
        std::fs::read(&copy_path).unwrap(),
        b"texto do usuario\nnao perder\n"
    );
    assert!(copy.bytes > 0);
    assert_eq!(
        f.bytes("doc.txt"),
        b"versao externa\n",
        "original untouched"
    );

    let second = state.save_recovery(&base.id, "outro texto\n").unwrap();
    assert_ne!(copy.path, second.path);
    assert_eq!(
        std::fs::read(&copy_path).unwrap(),
        b"texto do usuario\nnao perder\n",
        "the first recovery copy is preserved"
    );
    assert_eq!(
        state
            .save_recovery("no-such-snapshot", "x")
            .unwrap_err()
            .code,
        "snapshot_unknown"
    );
}

/// The watcher/compare path: restoring the disk version must be explicit and produce a new
/// snapshot; nothing else clears the dirty buffer.
#[test]
fn a_file_deleted_externally_conflicts_without_losing_the_buffer() {
    let f = Fixture::new();
    f.write("doc.txt", b"base\n");
    let state = f.state();
    let base = state.read(&f.uri("doc.txt")).unwrap();
    std::fs::remove_file(f.path("doc.txt")).unwrap();

    let outcome = state.save(&base.id, "texto do usuario\n").unwrap();
    let SaveOutcome::Conflict { current, message } = outcome else {
        panic!("expected Conflict, got {outcome:?}");
    };
    assert!(current.is_none());
    assert!(message.contains("no longer exists"), "{message}");
    let copy = state.save_recovery(&base.id, "texto do usuario\n").unwrap();
    assert!(PathBuf::from(copy.path).exists());
}

// ---------------------------------------------------------------------------------------
// AC-005-03 — pagination and snapshot scoping
// ---------------------------------------------------------------------------------------

/// Would catch: silent truncation, pages larger than 128, a non-deterministic or non-opaque
/// cursor, missing entries between pages, duplicated entries, or `FileProvider::list` truncated.
#[test]
fn pagination_is_complete_deterministic_and_never_silently_truncated() {
    let f = Fixture::new();
    for i in 0..300 {
        f.write(&format!("f{i:03}.txt"), b"x");
    }
    f.write("sub/um.txt", b"y");
    let provider = f.provider();
    let dir = f.uri_of(&f.root);

    let page1 = provider.list_page(&dir, None, MAX_PAGE_ENTRIES).unwrap();
    assert_eq!(page1.entries.len(), 128);
    let cursor1 = page1.next_cursor.clone().expect("more entries exist");
    assert_ne!(
        cursor1,
        page1.entries.last().unwrap().name,
        "cursor is opaque, not a raw entry name"
    );

    let page2 = provider
        .list_page(&dir, Some(&cursor1), MAX_PAGE_ENTRIES)
        .unwrap();
    let again = provider
        .list_page(&dir, Some(&cursor1), MAX_PAGE_ENTRIES)
        .unwrap();
    assert_eq!(page2.entries, again.entries, "same cursor, same page");
    assert_eq!(page2.entries.len(), 128);
    let page3 = provider
        .list_page(&dir, page2.next_cursor.as_deref(), MAX_PAGE_ENTRIES)
        .unwrap();
    assert!(page3.next_cursor.is_none(), "last page closes the cursor");

    let mut all: Vec<String> = page1
        .entries
        .iter()
        .chain(&page2.entries)
        .chain(&page3.entries)
        .map(|entry| entry.name.clone())
        .collect();
    assert_eq!(all.len(), 301, "300 files + sub");
    all.sort();
    let mut deduped = all.clone();
    deduped.dedup();
    assert_eq!(all, deduped, "no duplicates between pages");
    assert!(page3
        .entries
        .iter()
        .any(|e| e.name == "sub" && e.kind == FileKind::Directory));

    let first_page_names: Vec<String> = page1.entries.iter().map(|e| e.name.clone()).collect();
    let mut sorted = first_page_names.clone();
    sorted.sort();
    assert_eq!(first_page_names, sorted, "pages follow the sorted order");

    let small = provider.list_page(&dir, None, 10).unwrap();
    assert_eq!(small.entries.len(), 10);
    assert!(small.next_cursor.is_some());
    let huge = provider.list_page(&dir, None, 10_000).unwrap();
    assert_eq!(
        huge.entries.len(),
        MAX_PAGE_ENTRIES,
        "page is capped at 128"
    );
    assert!(huge.next_cursor.is_some());

    assert_eq!(
        provider.list(&dir).unwrap().len(),
        301,
        "FileProvider::list is not truncated by the page cap"
    );
}

/// Would catch: a directory listed with an invalid cursor, a file listed as a directory, or an
/// empty page reported as complete when entries remain.
#[test]
fn listing_negative_cases() {
    let f = Fixture::new();
    f.write("doc.txt", b"x");
    let state = f.state();
    assert_eq!(
        state.list(&f.uri("doc.txt"), None).unwrap_err().code,
        "not_a_directory"
    );
    assert_eq!(
        state.list(&f.uri_of(&f.root), Some("zz")).unwrap_err().code,
        "invalid_cursor"
    );
    assert_eq!(
        state.list(&f.uri("nao-existe"), None).unwrap_err().code,
        "file_not_found"
    );
    assert_eq!(
        state
            .list(&FileUri::remote("local", "boot-alpha", "/x"), None)
            .unwrap_err()
            .code,
        "file_host_unsupported"
    );
}

/// Would catch: snapshots readable/writable from another state (another "boot"/host) or release
/// not actually freeing them.
#[test]
fn snapshots_are_scoped_to_the_state_that_read_them() {
    let a = Fixture::new();
    let b = Fixture::new();
    a.write("doc.txt", b"a\n");
    b.write("doc.txt", b"b\n");
    let state_a = a.state();
    let state_b = b.state();
    let snapshot = state_a.read(&a.uri("doc.txt")).unwrap();

    assert_eq!(
        state_b.save(&snapshot.id, "x\n").unwrap_err().code,
        "snapshot_unknown"
    );
    assert_eq!(b.bytes("doc.txt"), b"b\n");

    state_a.release(std::slice::from_ref(&snapshot.id)).unwrap();
    assert!(state_a.snapshot(&snapshot.id).is_none());
    assert_eq!(
        state_a.save(&snapshot.id, "x\n").unwrap_err().code,
        "snapshot_unknown"
    );
}

// ---------------------------------------------------------------------------------------
// IPC surface of the module
// ---------------------------------------------------------------------------------------

/// Would catch: a module command without a handler, a generic shell/fs command exposed to the
/// WebView, or the frontend bridge invoking a name the backend does not declare.
#[test]
fn file_commands_are_limited_and_match_the_frontend_bridge() {
    let source = include_str!("../src/files/local.rs");
    let forbidden = [
        "shell", "exec", "spawn", "open_url", "http", "fetch", "generic",
    ];
    for command in files_local::COMMANDS {
        assert!(
            source.contains(&format!("#[tauri::command]\npub fn {command}(")),
            "{command} has no #[tauri::command] handler"
        );
        for word in forbidden {
            assert!(!command.contains(word), "{command} looks like {word}");
        }
    }
    assert_eq!(
        source.matches("#[tauri::command]").count(),
        files_local::COMMANDS.len()
    );

    let bridge = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../src/files/bridge.ts"
    ))
    .unwrap();
    let mut invoked: Vec<&str> = bridge
        .split("invoke<")
        .skip(1)
        .filter_map(|rest| rest.split('"').nth(1))
        .collect();
    invoked.sort_unstable();
    let mut declared: Vec<&str> = files_local::COMMANDS.to_vec();
    declared.sort_unstable();
    assert_eq!(invoked, declared);
}

// ---------------------------------------------------------------------------------------
// Native E2E: real Tauri window with the explorer/editor and the real backend/IPC.
// GUI 1: list (paged) → open → type → save (CRLF kept) → large/binary messages → dirty close
// cancel; the parent writes an external version while the GUI 2 buffer is dirty → conflict →
// compare → save copy → reload.
// ---------------------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod e2e {
    use super::files_local;
    use super::native_harness::{self, PhaseRun};
    use super::window_harness;
    use serde_json::{json, Value};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    const WINDOW_PHASE_TEST: &str = "e2e::e2e_files_window";

    /// One GUI process: a Tauri window over the built frontend whose page runs
    /// `src/features/files/e2e.ts` against the real file commands.
    #[test]
    #[ignore = "window phase of e2e_files_flow; fails when run outside the harness"]
    fn e2e_files_window() {
        let phase = native_harness::current_phase();
        let params: Value =
            serde_json::from_str(&native_harness::required("HERDR_DESKTOP_E2E_PARAMS")).unwrap();
        let root = PathBuf::from(params["root"].as_str().expect("params.root"));
        let state = files_local::FilesState::new(vec![root]).expect("authorized root");
        let builder =
            tauri::Builder::default()
                .manage(state)
                .invoke_handler(tauri::generate_handler![
                    files_local::files_list,
                    files_local::files_read,
                    files_local::files_stat,
                    files_local::files_save,
                    files_local::files_save_recovery,
                    files_local::files_release,
                    window_harness::harness_report,
                ]);
        window_harness::run_feature_window(
            tauri::generate_context!(),
            builder,
            "files",
            &phase,
            params,
            PathBuf::from(native_harness::required(native_harness::RESULT_ENV)),
            Duration::from_secs(120),
        );
        panic!("the harness window returned without reporting done");
    }

    /// Like `native_harness::run_phase`, but lets the parent act while the child window runs:
    /// when the page reports `step: "dirty"` the callback fires (and writes the external
    /// version) before the child continues to its own assertions.
    fn run_phase_with_progress(
        phase_test: &str,
        phase: &str,
        work_dir: &Path,
        env: &[(&str, &str)],
        on_progress: impl FnOnce(&Value),
    ) -> PhaseRun {
        let exe = std::env::current_exe().expect("current test executable");
        let result_path: PathBuf = work_dir.join(format!("phase-{phase}.json"));
        let _ = std::fs::remove_file(&result_path);
        let mut command = std::process::Command::new(exe);
        command
            .args([
                phase_test,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(native_harness::PHASE_ENV, phase)
            .env(native_harness::RESULT_ENV, &result_path);
        for (key, value) in env {
            command.env(key, value);
        }
        let child = command.spawn().expect("spawn phase process");
        let pid = child.id();
        let deadline = Instant::now() + Duration::from_secs(90);
        let mut on_progress = Some(on_progress);
        let mut child = child;
        loop {
            if let Some(callback) = on_progress.take() {
                if let Ok(raw) = std::fs::read_to_string(&result_path) {
                    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                        if value.get("step").and_then(Value::as_str) == Some("dirty") {
                            callback(&value);
                        } else {
                            on_progress = Some(callback);
                        }
                    } else {
                        on_progress = Some(callback);
                    }
                } else {
                    on_progress = Some(callback);
                }
            }
            if child.try_wait().expect("poll phase process").is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "phase {phase} did not finish in 90s"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let output = child.wait_with_output().expect("wait phase process");
        let stderr = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.status.success(),
            "phase {phase} failed ({}):\n{stderr}",
            output.status
        );
        let raw = std::fs::read_to_string(&result_path)
            .unwrap_or_else(|e| panic!("phase {phase} wrote no result ({e}):\n{stderr}"));
        let result = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("phase {phase} result is not JSON ({e}): {raw}"));
        PhaseRun {
            pid,
            result,
            stderr,
        }
    }

    fn assert_null(error: &Value, context: &str) {
        assert!(error.is_null(), "{context}: {error}");
    }

    /// Build metadata written by `vite.config.ts` (`dist/.vite/module-chunks.json`): for every
    /// emitted chunk, the source modules it contains. The window embeds the same `dist/`.
    fn module_chunks() -> std::collections::BTreeMap<String, Vec<String>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist/.vite/module-chunks.json");
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "module metadata missing at {} ({e}); build the frontend first",
                path.display()
            )
        });
        let value: Value = serde_json::from_str(&raw).expect("module metadata is JSON");
        let chunks: std::collections::BTreeMap<String, Vec<String>> =
            serde_json::from_value(value["chunks"].clone()).expect("chunks: file -> modules");
        assert!(!chunks.is_empty(), "module metadata lists no chunk");
        chunks
    }

    /// Editor and language modules: the CodeMirror/Lezer packages (and their private
    /// dependencies) plus the lazy editor entry point in `src/editor/`.
    fn is_editor_module(module: &str) -> bool {
        const PACKAGES: [&str; 6] = [
            "node_modules/@codemirror/",
            "node_modules/@lezer/",
            "node_modules/@marijn/find-cluster-break/",
            "node_modules/style-mod/",
            "node_modules/w3c-keyname/",
            "node_modules/crelt/",
        ];
        module.starts_with("src/editor/") || PACKAGES.iter().any(|p| module.contains(p))
    }

    /// Maps the scripts reported by one page probe to source modules. Every loaded script
    /// must be a known chunk of this build (a stale or foreign map fails instead of counting
    /// zero). Returns (all modules loaded, editor/language modules loaded).
    fn probe_modules(
        chunks: &std::collections::BTreeMap<String, Vec<String>>,
        probe: &Value,
        context: &str,
    ) -> (Vec<String>, Vec<String>) {
        let scripts = probe["scripts"]
            .as_array()
            .unwrap_or_else(|| panic!("{context}: probe without scripts: {probe}"));
        assert!(
            !scripts.is_empty(),
            "{context}: probe saw no script: {probe}"
        );
        let mut all = Vec::new();
        for script in scripts {
            let path = script.as_str().expect("script pathname");
            let modules = chunks.get(path.trim_start_matches('/')).unwrap_or_else(|| {
                panic!("{context}: loaded script {path} is not a chunk of dist")
            });
            all.extend(modules.iter().cloned());
        }
        all.sort();
        all.dedup();
        let editor = all
            .iter()
            .filter(|m| is_editor_module(m))
            .cloned()
            .collect();
        (all, editor)
    }

    #[test]
    #[ignore = "needs the disposable session created by scripts/feature-harness/session.sh; run by just check-spec 005"]
    fn e2e_files_flow() {
        let session = native_harness::required("HERDR_DESKTOP_E2E_SESSION");
        assert!(
            session.starts_with("hd005-"),
            "never run against a non-disposable session: {session}"
        );
        let report = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_REPORT"));
        let work = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_DIR")).join("work");
        std::fs::create_dir_all(&report).unwrap();

        let root = work.join("raiz");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("notas.txt"), b"linha um\r\nlinha dois\r\n").unwrap();
        std::fs::write(root.join("outro.txt"), b"outro arquivo\n").unwrap();
        std::fs::write(root.join("sub").join("um.txt"), b"dentro do subdiretorio\n").unwrap();
        std::fs::write(
            root.join("grande.txt"),
            vec![b'a'; files_local::MAX_TEXT_BYTES as usize + 1],
        )
        .unwrap();
        std::fs::write(root.join("bin.dat"), b"dados\x00binarios\xFF").unwrap();
        for i in 0..130 {
            std::fs::write(root.join(format!("p{i:03}.txt")), b"pagina\n").unwrap();
        }

        let herdr_config = herdr_client::session::herdr_config_dir(&|k| std::env::var(k).ok());
        let endpoints = herdr_config.join("endpoints.json");
        let endpoints_before = std::fs::read(&endpoints).ok();

        let params = json!({
            "root": root.display().to_string(),
            "session": session,
        })
        .to_string();
        let env = [("HERDR_DESKTOP_E2E_PARAMS", params.as_str())];

        let mut log = std::fs::File::create(report.join("e2e-files.log")).unwrap();
        let mut note = |line: String| {
            eprintln!("{line}");
            writeln!(log, "{line}").unwrap();
        };
        note(format!(
            "session={session} root={} (temporary work dir, removed at the end)",
            root.display()
        ));

        // --- GUI 1: paged listing, open, edit, explicit save, limits, dirty close cancel ------
        let gui1 = native_harness::run_phase(WINDOW_PHASE_TEST, "create", &report, &env);
        let c = &gui1.result;
        note(format!("GUI 1 (pid {}) window report: {c}", gui1.pid));
        assert_null(&c["error"], "GUI 1");

        // AC-005-03: modules actually loaded by the page, not DOM presence. Mounted and still
        // idle for >= 2000 ms with nothing open → zero editor/language modules; the same probe
        // after opening a text file detects the editor.
        let chunks = module_chunks();
        let editor_chunks: Vec<&String> = chunks
            .iter()
            .filter(|(_, modules)| modules.iter().any(|m| is_editor_module(m)))
            .map(|(file, _)| file)
            .collect();
        assert!(
            !editor_chunks.is_empty(),
            "the build has no editor/language chunk to detect"
        );
        let probes = &c["module_probe"];
        let (mounted_all, mounted_editor) =
            probe_modules(&chunks, &probes["mounted"], "mounted probe");
        let (idle_all, idle_editor) = probe_modules(&chunks, &probes["idle"], "idle probe");
        let (_, opened_editor) = probe_modules(&chunks, &probes["opened"], "opened probe");
        for (name, all) in [("mounted", &mounted_all), ("idle", &idle_all)] {
            assert!(
                all.iter()
                    .any(|m| m == "src/components/FilesWorkspace.svelte"),
                "{name} probe must see the lazily loaded files feature chunk: {all:?}"
            );
        }
        let idle_ms = probes["idle"]["at_ms"].as_i64().unwrap()
            - probes["mounted"]["at_ms"].as_i64().unwrap();
        assert!(idle_ms >= 2000, "idle window too short: {idle_ms} ms");
        assert_eq!(
            probes["mounted"]["tabs"],
            json!(0),
            "nothing open while mounted"
        );
        assert_eq!(probes["idle"]["tabs"], json!(0), "nothing open while idle");
        assert_eq!(
            mounted_editor,
            Vec::<String>::new(),
            "editor/language modules loaded at mount with no file open"
        );
        assert_eq!(
            idle_editor,
            Vec::<String>::new(),
            "editor/language modules loaded during {idle_ms} ms idle with no file open"
        );
        assert!(
            opened_editor.iter().any(|m| m == "src/editor/editor.ts")
                && opened_editor
                    .iter()
                    .any(|m| m.contains("node_modules/@codemirror/view/")),
            "opening a text file must be detected by the same probe: {opened_editor:?}"
        );
        note(format!(
            "module probe: mounted {} modules / 0 editor; idle {idle_ms} ms {} modules / 0 editor; opened {} editor modules",
            mounted_all.len(),
            idle_all.len(),
            opened_editor.len()
        ));
        assert_eq!(c["editor_absent_before_open"], json!(true));
        assert!(c["empty_state_text"]
            .as_str()
            .unwrap_or("")
            .contains("Nenhum arquivo aberto"));
        assert_eq!(c["initial_rows"], json!(files_local::MAX_PAGE_ENTRIES));
        assert_eq!(c["load_more_visible"], json!(true));
        assert_eq!(
            c["rows_after_more"],
            json!(135),
            "root has 130 p*.txt + 4 files + sub"
        );
        assert_eq!(c["load_more_gone"], json!(true));
        assert_eq!(c["subdir_expanded"], json!(true));
        assert_eq!(
            c["tab_names"],
            json!(["notas.txt"]),
            "open file creates one tab"
        );
        assert_eq!(c["editor_loaded_after_open"], json!(true));
        assert_eq!(c["typed"]["dirty"], json!(true));
        assert_eq!(c["saved"]["dirty"], json!(false));
        assert!(c["saved"]["editor_text"]
            .as_str()
            .unwrap_or("")
            .contains("OBS usuario"));
        assert_eq!(c["large_error"]["code"], "file_too_large");
        assert_eq!(c["large_error"]["editor_absent"], json!(true));
        assert_eq!(c["binary_error"]["code"], "binary_unsupported");
        assert_eq!(c["binary_error"]["editor_absent"], json!(true));
        assert_eq!(c["close_prompt_shown"], json!(true));
        assert_eq!(c["close_cancelled"]["text_kept"], json!(true));
        assert_eq!(c["close_cancelled"]["dirty"], json!(true));

        // Disk after GUI 1: the explicit save rewrote the same file preserving CRLF.
        let saved_text = std::fs::read_to_string(root.join("notas.txt")).unwrap();
        assert!(saved_text.contains("OBS usuario"), "{saved_text}");
        assert!(saved_text.contains("linha um\r\n"), "{saved_text}");
        assert!(
            !saved_text.replace("\r\n", "\n").contains('\r'),
            "CRLF preserved end to end: {saved_text:?}"
        );
        assert_eq!(
            std::fs::read(root.join("grande.txt")).unwrap().len() as u64,
            files_local::MAX_TEXT_BYTES + 1
        );

        // --- GUI 2: dirty buffer + external change → conflict → compare → save copy → reload ---
        let gui2 = run_phase_with_progress(WINDOW_PHASE_TEST, "conflict", &report, &env, |_| {
            std::fs::write(root.join("notas.txt"), b"versao externa\nagente\n").unwrap();
        });
        let r = &gui2.result;
        note(format!("GUI 2 (pid {}) window report: {r}", gui2.pid));
        assert_null(&r["error"], "GUI 2");
        assert_eq!(
            r["opened"]["text"],
            saved_text.replace("\r\n", "\n").as_str(),
            "the editor shows the saved text with normalised line endings"
        );
        assert_eq!(
            r["dirty_attr"],
            json!("true"),
            "tab stays dirty after the conflict"
        );
        assert_eq!(r["dirty"], json!(true));
        assert_eq!(r["external_notice"], json!(true));
        assert_eq!(r["conflict"]["buffer_kept"], json!(true));
        assert!(r["conflict"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("mudou no disco"));
        assert_eq!(
            r["conflict"]["options"],
            json!(["Recarregar", "Comparar", "Salvar cópia"])
        );
        assert_eq!(r["diff"]["origins"].as_array().map(Vec::len), Some(2));
        assert!(
            r["diff"]["added"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap_or("").contains("linha do conflito")),
            "{}",
            r["diff"]
        );
        assert!(
            r["diff"]["removed"]
                .as_array()
                .unwrap()
                .iter()
                .any(|line| line.as_str().unwrap_or("").contains("versao externa")),
            "{}",
            r["diff"]
        );

        // AC-005-01: both diff bases over three distinct texts. Original (read snapshot)
        // = saved text of GUI 1; disk = external version; buffer = original without "linha
        // dois" plus "linha do conflito".
        let origin_id = |diff: &Value, prefix: &str| -> String {
            let origins = diff["origins"].as_array().expect("origins");
            assert_eq!(origins.len(), 2, "{diff}");
            assert_eq!(origins[1], json!("buffer atual"), "{diff}");
            let base = origins[0].as_str().unwrap();
            base.strip_prefix(prefix)
                .unwrap_or_else(|| panic!("base origin must start with {prefix:?}: {diff}"))
                .to_string()
        };
        let disk = &r["diff"];
        assert_eq!(disk["base"], json!("disk"), "{disk}");
        let disk_id = origin_id(disk, "versão no disco ");
        assert_eq!(
            disk["removed"],
            json!(["versao externa", "agente"]),
            "{disk}"
        );
        assert_eq!(
            disk["added"],
            json!(["linha um", "", "OBS usuario", "", "linha do conflito"]),
            "{disk}"
        );
        assert_eq!(disk["summary"], json!("+5 −2"), "{disk}");
        let original = &r["diff_original"];
        assert_eq!(original["base"], json!("original"), "{original}");
        let original_id = origin_id(original, "conteúdo-base ");
        assert_eq!(original["removed"], json!(["linha dois"]), "{original}");
        assert_eq!(
            original["added"],
            json!(["", "linha do conflito"]),
            "{original}"
        );
        assert_eq!(original["summary"], json!("+2 −1"), "{original}");
        assert!(
            !disk_id.is_empty() && disk_id != original_id,
            "the two bases are distinct snapshots: {disk_id} / {original_id}"
        );
        assert_eq!(r["reloaded"]["dirty"], json!(false));
        assert_eq!(r["reloaded"]["text"], "versao externa\nagente\n");

        // Disk after GUI 2: the conflict never overwrote the external version; reload adopted it
        // and the recovery copy preserved the user's buffer.
        assert_eq!(
            std::fs::read(root.join("notas.txt")).unwrap(),
            b"versao externa\nagente\n"
        );
        let recovery = r["recovery"].as_str().expect("recovery path in the report");
        let recovery_path = PathBuf::from(recovery);
        assert!(recovery_path.starts_with(&root), "{recovery}");
        assert!(
            recovery_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains(files_local::RECOVERY_MARKER),
            "{recovery}"
        );
        let recovery_text = std::fs::read_to_string(&recovery_path).unwrap();
        assert!(
            recovery_text.contains("linha do conflito"),
            "{recovery_text}"
        );
        assert!(!recovery_text.contains("versao externa"), "{recovery_text}");

        // The files feature never writes engine persistence.
        assert_eq!(
            std::fs::read(&endpoints).ok(),
            endpoints_before,
            "endpoints.json changed"
        );
        assert!(
            !herdr_config.join("projects.json").exists()
                && !herdr_config.join("files.json").exists(),
            "no desktop store inside the engine config dir"
        );
        note("endpoints.json unchanged; no store inside the engine config dir".into());

        std::fs::write(
            report.join("e2e-files-summary.json"),
            serde_json::to_string_pretty(&json!({
                "session": session,
                "root": root.display().to_string(),
                "gui_processes": { "create": gui1.pid, "conflict": gui2.pid },
                "listing": {
                    "initial_rows": c["initial_rows"],
                    "rows_after_more": c["rows_after_more"],
                },
                "save": {
                    "path": root.join("notas.txt").display().to_string(),
                    "crlf_preserved": true,
                    "editor_text": c["saved"]["editor_text"],
                },
                "limits": { "large": c["large_error"], "binary": c["binary_error"] },
                "close_cancel": c["close_cancelled"],
                "conflict": r["conflict"],
                "diff": r["diff"],
                "diff_original": r["diff_original"],
                "module_probe": c["module_probe"],
                "recovery": recovery,
                "reloaded": r["reloaded"],
                "endpoints_json_unchanged": true,
            }))
            .unwrap(),
        )
        .unwrap();
    }
}
