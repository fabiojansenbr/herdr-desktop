//! Durable `events.subscribe` byte streams of one host's JSON API (007).
//!
//! Local: a connection to the session's API socket (the socket of `ApiClient`). SSH: one
//! long-lived `remote-api-bridge` process built by the profile's `build_ssh`, whose stdio the
//! engine forwards to the remote API socket. The subscription protocol itself (request line,
//! event lines) stays with the agents watcher; this module only opens, bounds and ends streams.
//!
//! Every stream reads with a periodic timeout (`WouldBlock`) so a stopped watcher ends, and a
//! [`Fenced`] stream reports its end as soon as its connection is no longer the current one.

use std::io::{self, Read, Write};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError};
use std::time::Duration;

use herdr_client::api::ApiClient;
use herdr_client::RuntimeError;

use super::agent_commands::EventStream;
use super::ssh::{SshChild, SshRunner};
pub use crate::connections::commands::EventSource;
use crate::connections::ssh_options::OpenSshCommand;

/// Wake-up cadence of a read with no data.
pub const EVENT_READ_TICK: Duration = Duration::from_millis(250);
/// Size of one chunk read from the SSH process.
pub const EVENT_CHUNK_BYTES: usize = 8 * 1024;
/// Chunks held for an idle watcher before the SSH process output stops being read.
pub const EVENT_BUFFER_CHUNKS: usize = 32;

/// Opens one stream of `source`; the caller writes the subscription request.
pub fn open_event_stream(
    source: &EventSource,
    endpoint: &str,
) -> Result<Box<dyn EventStream>, RuntimeError> {
    match source {
        EventSource::Local(api) => open_local(api, endpoint),
        EventSource::Ssh { command, runner } => {
            let stream = SshEventStream::spawn(runner.as_ref(), command).map_err(|error| {
                RuntimeError::from_io_kind(error.kind(), "remote events are unavailable")
                    .with_endpoint(endpoint)
            })?;
            Ok(Box::new(stream))
        }
    }
}

#[cfg(unix)]
fn open_local(api: &ApiClient, endpoint: &str) -> Result<Box<dyn EventStream>, RuntimeError> {
    let open = || -> io::Result<std::os::unix::net::UnixStream> {
        let stream = std::os::unix::net::UnixStream::connect(api.socket_path())?;
        stream.set_read_timeout(Some(EVENT_READ_TICK))?;
        Ok(stream)
    };
    open()
        .map(|s| Box::new(s) as Box<dyn EventStream>)
        .map_err(|error| {
            RuntimeError::from_io_kind(error.kind(), "Herdr events are unavailable")
                .with_endpoint(endpoint)
        })
}

#[cfg(not(unix))]
fn open_local(api: &ApiClient, endpoint: &str) -> Result<Box<dyn EventStream>, RuntimeError> {
    // Windows named pipe (unproven until 008): blocking reads, so a stopped watcher ends at its
    // next event or when the pipe closes.
    herdr_client::local::connect_local_stream(api.socket_path())
        .map(|s| Box::new(s) as Box<dyn EventStream>)
        .map_err(|error| {
            RuntimeError::from_io_kind(error.kind(), "Herdr events are unavailable")
                .with_endpoint(endpoint)
        })
}

/// `remote-api-bridge` process as a stream. A pump thread moves stdout into a bounded channel;
/// when the watcher is idle the channel fills and the process output is no longer read. Dropping
/// the stream kills the process, which also ends the pump.
struct SshEventStream {
    child: Box<dyn SshChild>,
    stdin: Box<dyn Write + Send>,
    chunks: Receiver<Vec<u8>>,
    current: Vec<u8>,
    offset: usize,
}

impl SshEventStream {
    fn spawn(runner: &dyn SshRunner, command: &OpenSshCommand) -> io::Result<Self> {
        let mut child = runner.spawn(command)?;
        let (Some(stdin), Some(mut stdout)) = (child.take_stdin(), child.take_stdout()) else {
            child.kill();
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "ssh has no stdio for events",
            ));
        };
        let (tx, chunks) = sync_channel::<Vec<u8>>(EVENT_BUFFER_CHUNKS);
        let pump = std::thread::Builder::new()
            .name("herdr-desktop-ssh-events".into())
            .spawn(move || {
                let mut buf = vec![0u8; EVENT_CHUNK_BYTES];
                loop {
                    match stdout.read(&mut buf) {
                        Ok(0) => return,
                        Ok(n) => {
                            if tx.send(buf[..n].to_vec()).is_err() {
                                return;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(_) => return,
                    }
                }
            });
        if let Err(error) = pump {
            child.kill();
            return Err(error);
        }
        Ok(Self {
            child,
            stdin,
            chunks,
            current: Vec::new(),
            offset: 0,
        })
    }
}

impl Read for SshEventStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.offset >= self.current.len() {
            match self.chunks.recv_timeout(EVENT_READ_TICK) {
                Ok(chunk) => {
                    self.current = chunk;
                    self.offset = 0;
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock))
                }
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let n = buf.len().min(self.current.len() - self.offset);
        buf[..n].copy_from_slice(&self.current[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}

impl Write for SshEventStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdin.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush()
    }
}

impl Drop for SshEventStream {
    fn drop(&mut self) {
        self.child.kill();
    }
}

/// Stream that ends (reads return 0) once `current` reports that its connection is no longer
/// the one it was opened for; checked before handing out any byte.
pub struct Fenced {
    inner: Box<dyn EventStream>,
    current: Box<dyn Fn() -> bool + Send>,
}

impl Fenced {
    pub fn new(inner: Box<dyn EventStream>, current: Box<dyn Fn() -> bool + Send>) -> Self {
        Self { inner, current }
    }
}

impl Read for Fenced {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !(self.current)() {
            return Ok(0);
        }
        let read = self.inner.read(buf)?;
        if read > 0 && !(self.current)() {
            return Ok(0);
        }
        Ok(read)
    }
}

impl Write for Fenced {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if !(self.current)() {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
