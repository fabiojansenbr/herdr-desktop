//! Stable, sanitized errors of the remote files provider (private helper of spec 006).
//!
//! Messages are composed here in English (spec 071: the WebView translates them by `code`); no
//! peer STATUS text, OpenSSH stderr, remote path, credential or crate detail is ever included.
//! Every error names the endpoint it belongs to.

use std::time::Duration;

use herdr_client::RuntimeError;

fn error(endpoint: &str, code: &str, message: &str) -> RuntimeError {
    RuntimeError::new(code, message).with_endpoint(endpoint)
}

fn retryable(endpoint: &str, code: &str, message: &str) -> RuntimeError {
    error(endpoint, code, message).retryable()
}

pub fn uri_invalid(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "file_uri_invalid",
        "the URI does not belong to the remote files provider (SFTP)",
    )
}

pub fn host_mismatch(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "file_host_mismatch",
        "the file belongs to another host; nothing was read and there is no fallback to the local host",
    )
}

pub fn host_unknown(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "file_host_unknown",
        "unknown SSH host for remote files",
    )
}

pub fn host_unavailable(endpoint: &str) -> RuntimeError {
    retryable(
        endpoint,
        "host_unavailable",
        "the SSH host is not connected; cached content is not live state",
    )
}

pub fn path_unsupported(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "remote_path_unsupported",
        "unsupported remote path: use an absolute POSIX path (starting with /)",
    )
}

pub fn root_not_authorized(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "root_not_authorized",
        "no project root was authorized for this host",
    )
}

pub fn outside_root(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "path_outside_root",
        "the server resolved the path outside this project's authorized roots",
    )
}

pub fn operation_id_invalid(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "operation_id_invalid",
        "invalid operation identifier",
    )
}

pub fn operation_duplicate(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "operation_duplicate",
        "an operation with this identifier is already pending",
    )
}

pub fn queue_full(endpoint: &str, limit: usize) -> RuntimeError {
    retryable(
        endpoint,
        "sftp_queue_full",
        &format!("this host's remote files queue is full ({limit} pending operations); try again shortly"),
    )
}

pub fn timeout(endpoint: &str, deadline: Duration) -> RuntimeError {
    retryable(
        endpoint,
        "timeout",
        &format!(
            "the remote file operation exceeded {} ms; this host's SFTP channel was closed and the terminals were preserved",
            deadline.as_millis()
        ),
    )
}

pub fn cancelled(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "operation_cancelled",
        "the operation was cancelled; this host's SFTP channel was closed, and the open content and the terminals were preserved",
    )
}

pub fn channel_renewed(endpoint: &str) -> RuntimeError {
    retryable(
        endpoint,
        "connection_renewed",
        "the connection to the host was renewed; the previous SFTP channel was closed",
    )
}

pub fn connection_lost(endpoint: &str) -> RuntimeError {
    retryable(
        endpoint,
        "connection_lost",
        "the SFTP channel was closed; a new operation opens another channel (nothing is repeated automatically)",
    )
}

/// TASK-006-05: a working SSH terminal does not imply the SFTP subsystem.
pub fn sftp_unavailable(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "sftp_unavailable",
        "this host's SSH works, but the SFTP subsystem is not available; the Herdr terminals stay usable. Enable the sftp subsystem in the host's sshd to see files",
    )
}

pub fn frame_rejected(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "sftp_frame_rejected",
        "the SFTP server sent a frame beyond the accepted limit; the channel was closed",
    )
}

pub fn protocol(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "sftp_protocol_error",
        "the SFTP server sent an invalid response; the channel was closed",
    )
}

pub fn name_unsupported(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "remote_name_unsupported",
        "the directory holds a file name outside UTF-8, unsupported in this version; the listing was not shown",
    )
}

pub fn not_found(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "file_not_found",
        "resource not found on the remote host",
    )
}

pub fn permission_denied(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "permission_denied",
        "permission denied on the remote host",
    )
}

pub fn operation_unsupported(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "sftp_operation_unsupported",
        "this host's SFTP server does not support this operation",
    )
}

pub fn remote_io(endpoint: &str) -> RuntimeError {
    error(endpoint, "remote_io_error", "I/O error on the remote host")
}

pub fn not_a_file(endpoint: &str) -> RuntimeError {
    error(endpoint, "not_a_file", "the path is not a file")
}

pub fn not_a_directory(endpoint: &str) -> RuntimeError {
    error(endpoint, "not_a_directory", "the path is not a directory")
}

pub fn too_large(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "file_too_large",
        "the file is over 2 MiB; the editor was not loaded",
    )
}

pub fn binary_unsupported(endpoint: &str) -> RuntimeError {
    error(
        endpoint,
        "binary_unsupported",
        "the file is not UTF-8 text (or contains NUL); the editor was not loaded",
    )
}

pub fn invalid_cursor(endpoint: &str) -> RuntimeError {
    error(endpoint, "invalid_cursor", "invalid paging cursor")
}

pub fn cursor_stale(endpoint: &str) -> RuntimeError {
    retryable(
        endpoint,
        "cursor_stale",
        "the listing belongs to a closed channel; reload the folder",
    )
}

pub fn runtime_failed() -> RuntimeError {
    RuntimeError::new(
        "sftp_runtime_failed",
        "could not start the remote files runtime",
    )
}
