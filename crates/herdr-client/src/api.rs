//! Newline-delimited JSON API client (reference: `src/api/client.rs`). One request per
//! connection, request id, `result` or `error` body. Used for runtime actions that are
//! not part of the visual endpoint lane (e.g. `pane.process_info`, `workspace.create`).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Deserialize;

use crate::contracts::RuntimeError;
use crate::local::{connect_local_stream, set_stream_timeouts};

const API_TIMEOUT: Duration = Duration::from_secs(10);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Mirror of `api::schema::ErrorBody`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WireResponse {
    Success {
        id: String,
        result: serde_json::Value,
    },
    Error {
        id: String,
        error: ErrorBody,
    },
}

#[derive(Debug, Clone)]
pub struct ApiClient {
    socket_path: PathBuf,
    endpoint: String,
}

impl ApiClient {
    pub fn new(socket_path: &Path, endpoint: &str) -> Self {
        Self {
            socket_path: socket_path.to_path_buf(),
            endpoint: endpoint.to_owned(),
        }
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Sends `{"id","method","params"}` and returns the `result` object.
    pub fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RuntimeError> {
        let id = format!("desktop:{}", NEXT_ID.fetch_add(1, Ordering::Relaxed));
        let request = serde_json::json!({ "id": id, "method": method, "params": params });
        let mut stream = connect_local_stream(&self.socket_path).map_err(|error| {
            RuntimeError::from_io_kind(error.kind(), "the Herdr API is unavailable")
                .with_endpoint(self.endpoint.clone())
        })?;
        set_stream_timeouts(&stream, API_TIMEOUT).map_err(|error| {
            RuntimeError::from_io_kind(error.kind(), "the Herdr API is unavailable")
                .with_endpoint(self.endpoint.clone())
        })?;
        let mut line = serde_json::to_string(&request).map_err(|_| {
            RuntimeError::new("serialization_error", "could not serialize the request")
        })?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .and_then(|_| stream.flush())
            .map_err(|error| {
                RuntimeError::from_io_kind(error.kind(), "could not send the request")
                    .with_endpoint(self.endpoint.clone())
            })?;
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        let read = reader.read_line(&mut response).map_err(|error| {
            RuntimeError::from_io_kind(error.kind(), "could not read the response")
                .with_endpoint(self.endpoint.clone())
        })?;
        if read == 0 || response.trim().is_empty() {
            return Err(
                RuntimeError::new("empty_response", "empty response from the API")
                    .retryable()
                    .with_endpoint(self.endpoint.clone()),
            );
        }
        match serde_json::from_str::<WireResponse>(&response) {
            Ok(WireResponse::Success { id: got, result }) if got == id => Ok(result),
            Ok(WireResponse::Success { .. }) => Err(RuntimeError::new(
                "response_id_mismatch",
                "the response id does not match the request",
            )
            .with_endpoint(self.endpoint.clone())),
            Ok(WireResponse::Error { id: got, error }) => {
                let mut runtime = RuntimeError::new(error.code, error.message)
                    .with_endpoint(self.endpoint.clone());
                if got != id {
                    runtime.code = format!("{}:id_mismatch", runtime.code);
                }
                Err(runtime)
            }
            Err(_) => Err(RuntimeError::new(
                "protocol_error",
                "the API response is not valid JSON",
            )
            .with_endpoint(self.endpoint.clone())),
        }
    }

    /// `pane.process_info` → shell pid of the pane, if reported.
    pub fn pane_shell_pid(&self, pane_id: &str) -> Result<Option<u32>, RuntimeError> {
        let result = self.request(
            "pane.process_info",
            serde_json::json!({ "pane_id": pane_id }),
        )?;
        Ok(result
            .pointer("/process_info/shell_pid")
            .and_then(serde_json::Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()))
    }
}
