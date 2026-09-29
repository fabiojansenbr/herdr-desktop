//! Remote POSIX paths (private helpers of spec 006).
//!
//! Remote paths are strings in the host's POSIX convention, independent of the desktop's own
//! separator (a Windows client still sends `/a/b`). Nothing here touches the local filesystem:
//! `..` and symlinks are resolved by the SFTP server (`realpath`), and containment is checked on
//! the server's answer. A backslash is an ordinary file name byte on POSIX hosts.

/// Longest remote path accepted in a request.
pub const MAX_PATH_BYTES: usize = 4096;

/// Accepts only absolute POSIX paths without NUL. `C:\x`, `\\srv\share` and relative paths are
/// an unsupported convention, never reinterpreted.
pub fn is_supported(path: &str) -> bool {
    path.starts_with('/') && !path.contains('\0') && path.len() <= MAX_PATH_BYTES
}

/// `dir/name` with exactly one separator.
pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// True when the server-resolved `path` is `root` itself or below it (component-wise).
pub fn contains(root: &str, path: &str) -> bool {
    if root == "/" {
        return path.starts_with('/');
    }
    let root = root.trim_end_matches('/');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}
