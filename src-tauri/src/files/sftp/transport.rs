//! Byte transports of an SFTP channel (private helpers of spec 006).
//!
//! Production uses the stdin/stdout pipes of `ssh … -s -- target sftp` built by the 003 seam
//! (`connections::ssh_options::build_sftp_subsystem`). Deadline and cancellation never rely on
//! blocking reads: the codec runs on tokio pipes, the provider races each operation against a
//! timer, and killing uses `Child::start_kill` + `wait` (SIGKILL / TerminateProcess), which is
//! portable by construction. Native macOS/Windows proof belongs to spec 008.

use std::io;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use herdr_client::bootstrap::INHERITED_SESSION_VARS;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

const STDERR_LIMIT: usize = 16 * 1024;

pub type BoxedWriter = Box<dyn AsyncWrite + Send + Unpin>;
pub type BoxedReader = Box<dyn AsyncRead + Send + Unpin>;

/// Pipes of one SFTP channel and, for process transports, the process that owns them.
pub struct SftpTransport {
    pub(crate) writer: BoxedWriter,
    pub(crate) reader: BoxedReader,
    pub(crate) process: Option<SftpProcess>,
}

impl SftpTransport {
    /// In-memory or already-connected streams (no process to kill or classify).
    pub fn from_streams(writer: BoxedWriter, reader: BoxedReader) -> Self {
        Self {
            writer,
            reader,
            process: None,
        }
    }

    /// Spawns `command` with piped stdio. Must run inside the provider's tokio runtime. The
    /// caller's Herdr session variables are removed and stderr is captured (bounded) only to
    /// classify failures; it is never shown.
    pub fn spawn(mut command: tokio::process::Command) -> io::Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for var in INHERITED_SESSION_VARS {
            command.env_remove(var);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let writer = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("stdin pipe missing"))?;
        let reader = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("stdout pipe missing"))?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_task = child.stderr.take().map(|mut pipe| {
            let sink = stderr.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut sink = sink.lock().expect("stderr sink");
                    let room = STDERR_LIMIT.saturating_sub(sink.len());
                    sink.extend_from_slice(&buf[..n.min(room)]);
                }
            })
        });
        let pid = child.id();
        Ok(Self {
            writer: Box::new(writer),
            reader: Box::new(reader),
            process: Some(SftpProcess {
                child,
                pid,
                stderr,
                stderr_task,
            }),
        })
    }

    /// Operating system id of the transport process, when there is one.
    pub fn pid(&self) -> Option<u32> {
        self.process.as_ref().and_then(|p| p.pid)
    }
}

/// The OpenSSH (or test) process behind a channel.
pub struct SftpProcess {
    child: tokio::process::Child,
    pid: Option<u32>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

/// How a process ended, for classification only.
pub struct ProcessEnd {
    /// `Some(code)` when it exited; `None` when killed by a signal or still running.
    pub code: Option<i32>,
    pub stderr: String,
}

impl SftpProcess {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Resolves when the process exits (the status is collected again by [`Self::end`]).
    pub async fn wait_exit(&mut self) {
        let _ = self.child.wait().await;
    }

    /// Kills and reaps the process (portable: SIGKILL / TerminateProcess).
    pub async fn kill(&mut self) {
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
    }

    /// Waits up to `timeout` for the exit and the end of stderr; kills the process otherwise.
    pub async fn end(&mut self, timeout: Duration) -> ProcessEnd {
        let code = match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(Ok(status)) => status.code(),
            _ => {
                self.kill().await;
                None
            }
        };
        if let Some(task) = self.stderr_task.take() {
            let _ = tokio::time::timeout(Duration::from_millis(500), task).await;
        }
        let stderr =
            String::from_utf8_lossy(&self.stderr.lock().expect("stderr sink")).into_owned();
        ProcessEnd { code, stderr }
    }
}
