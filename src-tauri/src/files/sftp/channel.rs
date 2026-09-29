//! One SFTP channel per qualified connection (private helper of spec 006).
//!
//! A channel is the `openssh-sftp-client` session over one transport, tagged with the connection
//! identity it was opened for (endpoint, session, generation, boot). Errors of a single resource
//! (missing file, permission) keep the channel; transport/codec failures kill it, invalidate its
//! paging cursors and are classified without echoing any peer text. Reads never use the crate's
//! whole-file helpers: the offset is set explicitly before every READ (short DATA replies would
//! otherwise skip bytes) and DATA longer than requested is refused.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::SeekFrom;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::BytesMut;
use futures_core::Stream;
use herdr_client::{FileEntry, FileKind, FileStat, FileUri, RuntimeError};
use openssh_sftp_client::error::SftpErrorKind;
use openssh_sftp_client::fs::{DirEntry, ReadDir};
use openssh_sftp_client::metadata::MetaData;
use openssh_sftp_client::{Error as SftpError, Sftp, SftpOptions};
use tokio::io::AsyncSeekExt;

use super::guard::{FrameGuard, FrameStats, KillSwitch, WriteGuard};
use super::posix;
use super::transport::{ProcessEnd, SftpProcess, SftpTransport};
use super::{errors, MAX_PAGE_ENTRIES, PROVIDER_ID};
use crate::connections::failure::{classify_ssh_failure, ConnectFailure};

/// SSH_FXP_NAME: the packet type carrying directory entries (and realpath answers).
pub const FXP_NAME: u8 = 104;
/// Open paging cursors kept per channel; the oldest is closed beyond this.
pub const MAX_CURSORS: usize = 16;
/// Largest READ asked for; the crate lowers it to the negotiated limit (OpenSSH: 261120).
pub const READ_REQUEST_BYTES: u64 = 262_144;

/// Connection identity a channel was opened for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelKey {
    pub endpoint: String,
    pub session: String,
    pub generation: u64,
    pub boot_id: String,
}

/// Failure of one channel operation.
pub enum OpError {
    /// Error reported by the crate (resource status or transport/codec failure).
    Sftp(SftpError),
    /// Already classified error on the resource; the channel stays usable.
    Resource(RuntimeError),
    /// The peer broke the protocol contract (e.g. DATA larger than requested).
    Protocol,
}

impl From<SftpError> for OpError {
    fn from(error: SftpError) -> Self {
        Self::Sftp(error)
    }
}

impl From<RuntimeError> for OpError {
    fn from(error: RuntimeError) -> Self {
        Self::Resource(error)
    }
}

struct DirCursor {
    dir: FileUri,
    path: String,
    stream: Pin<Box<ReadDir>>,
    peeked: Option<DirEntry>,
}

#[derive(Default)]
struct Cursors {
    map: HashMap<String, DirCursor>,
    order: VecDeque<String>,
}

pub struct Channel {
    pub id: u64,
    pub key: ChannelKey,
    pid: Option<u32>,
    sftp: Mutex<Option<Sftp>>,
    process: tokio::sync::Mutex<Option<SftpProcess>>,
    stats: Arc<FrameStats>,
    kill: Arc<KillSwitch>,
    dead: AtomicBool,
    death: tokio::sync::OnceCell<RuntimeError>,
    roots: tokio::sync::Mutex<HashMap<String, String>>,
    cursors: tokio::sync::Mutex<Cursors>,
}

/// Snapshot of a channel for diagnostics and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelInfo {
    pub id: u64,
    pub pid: Option<u32>,
    pub generation: u64,
    pub boot_id: String,
    pub alive: bool,
    pub rejected_frame: Option<u32>,
    pub frames: BTreeMap<u8, u64>,
}

impl ChannelInfo {
    pub fn frames_of(&self, kind: u8) -> u64 {
        self.frames.get(&kind).copied().unwrap_or(0)
    }
}

fn is_format(error: &SftpError) -> bool {
    match error {
        SftpError::FormatError(_) => true,
        SftpError::RecursiveErrors(r) => {
            is_format(&r.original_error) || is_format(&r.occuring_error)
        }
        SftpError::RecursiveErrors3(r) => {
            is_format(&r.err1) || is_format(&r.err2) || is_format(&r.err3)
        }
        _ => false,
    }
}

fn is_protocol(error: &SftpError) -> bool {
    matches!(
        error,
        SftpError::InvalidResponse(_)
            | SftpError::HandleTooLong
            | SftpError::UnsupportedSftpProtocol { .. }
            | SftpError::SftpServerHelloMsgTooLong { .. }
            | SftpError::InvalidResponseId { .. }
            | SftpError::BufferTooLong(_)
    )
}

/// Resource-level errors keep the channel; everything else is fatal for it.
pub fn resource_error(error: &SftpError, endpoint: &str) -> Option<RuntimeError> {
    match error {
        SftpError::SftpError(kind, _) => Some(match kind {
            SftpErrorKind::NoSuchFile => errors::not_found(endpoint),
            SftpErrorKind::PermDenied => errors::permission_denied(endpoint),
            SftpErrorKind::OpUnsupported => errors::operation_unsupported(endpoint),
            _ => errors::remote_io(endpoint),
        }),
        SftpError::UnsupportedExtension(_) => Some(errors::operation_unsupported(endpoint)),
        _ => None,
    }
}

fn classify_handshake(
    endpoint: &str,
    stats: &FrameStats,
    error: Option<&SftpError>,
    end: Option<ProcessEnd>,
) -> RuntimeError {
    if stats.rejected().is_some() {
        return errors::frame_rejected(endpoint);
    }
    match end {
        Some(end) => {
            let lower = end.stderr.to_ascii_lowercase();
            if lower.contains("subsystem request failed") || end.code == Some(127) {
                return errors::sftp_unavailable(endpoint);
            }
            match classify_ssh_failure(endpoint, end.code, &end.stderr) {
                ConnectFailure::Attention { error, .. } => error,
                ConnectFailure::Transient(error) => error,
            }
        }
        None if error.is_some_and(|e| is_format(e) || is_protocol(e)) => errors::protocol(endpoint),
        None => errors::connection_lost(endpoint),
    }
}

fn file_kind(metadata: &MetaData) -> FileKind {
    match metadata.file_type() {
        Some(t) if t.is_dir() => FileKind::Directory,
        Some(t) if t.is_symlink() => FileKind::Symlink,
        Some(t) if t.is_file() => FileKind::File,
        _ => FileKind::Other,
    }
}

fn modified_ms(metadata: &MetaData) -> Option<u64> {
    metadata
        .modified()
        .map(|t| t.as_duration().as_millis() as u64)
}

async fn next_entry(stream: &mut Pin<Box<ReadDir>>) -> Option<Result<DirEntry, SftpError>> {
    std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
}

/// Raw bytes of a remote file (at most `cap`, refused beyond it).
pub struct RemoteBytes {
    pub bytes: Vec<u8>,
    pub metadata: MetaData,
}

impl Channel {
    /// Handshake over `transport`. A failure is classified from the frame guard and, for process
    /// transports, from the exit status and captured stderr (never shown).
    pub async fn open(
        id: u64,
        key: ChannelKey,
        transport: SftpTransport,
    ) -> Result<Arc<Self>, RuntimeError> {
        let SftpTransport {
            writer,
            reader,
            mut process,
        } = transport;
        let pid = process.as_ref().and_then(SftpProcess::pid);
        let stats = Arc::new(FrameStats::default());
        let kill = Arc::new(KillSwitch::default());
        let guarded = FrameGuard::with_kill(reader, stats.clone(), kill.clone());
        let handshake = Sftp::new(
            WriteGuard::new(writer, stats.clone()),
            guarded,
            SftpOptions::new(),
        );
        // A transport that ends (or sends a refused frame) before the hello completes fails the
        // handshake at once: the crate's own cleanup is not awaited.
        let outcome = {
            let ended = async {
                loop {
                    if stats.transport_ended() || stats.rejected().is_some() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            };
            let exited = async {
                match process.as_mut() {
                    Some(process) => process.wait_exit().await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                result = handshake => result.map_err(Some),
                _ = ended => Err(None),
                _ = exited => Err(None),
            }
        };
        match outcome {
            Ok(sftp) => Ok(Arc::new(Self {
                id,
                key,
                pid,
                sftp: Mutex::new(Some(sftp)),
                process: tokio::sync::Mutex::new(process),
                stats,
                kill,
                dead: AtomicBool::new(false),
                death: tokio::sync::OnceCell::new(),
                roots: tokio::sync::Mutex::new(HashMap::new()),
                cursors: tokio::sync::Mutex::new(Cursors::default()),
            })),
            Err(error) => {
                kill.kill();
                let end = match process.as_mut() {
                    Some(process) => Some(process.end(Duration::from_secs(3)).await),
                    None => None,
                };
                Err(classify_handshake(
                    &key.endpoint,
                    &stats,
                    error.as_ref(),
                    end,
                ))
            }
        }
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub fn info(&self) -> ChannelInfo {
        ChannelInfo {
            id: self.id,
            pid: self.pid,
            generation: self.key.generation,
            boot_id: self.key.boot_id.clone(),
            alive: !self.is_dead(),
            rejected_frame: self.stats.rejected(),
            frames: self.stats.by_type(),
        }
    }

    fn endpoint(&self) -> &str {
        &self.key.endpoint
    }

    /// Kills the transport, drops the codec and every cursor. Idempotent.
    pub async fn shutdown(&self) {
        self.dead.store(true, Ordering::SeqCst);
        self.kill.kill();
        drop(self.sftp.lock().expect("sftp slot").take());
        let cursors = std::mem::take(&mut *self.cursors.lock().await);
        drop(cursors);
        if let Some(process) = self.process.lock().await.as_mut() {
            process.kill().await;
        }
    }

    /// Ends the channel for `reason` (cancel, deadline, identity change, protocol breach).
    pub async fn close_with(&self, reason: RuntimeError) -> RuntimeError {
        let _ = self.death.set(reason.clone());
        self.shutdown().await;
        self.death.get().cloned().unwrap_or(reason)
    }

    /// Classifies a fatal crate error once for the channel and ends it. The codec's own errors
    /// are not inspected for text; the guards tell apart a rejected frame, a transport that
    /// ended (EOF, broken pipe, process gone) and a codec that refused what it received (the
    /// type of the last frame distinguishes a NAME batch from any other reply).
    pub async fn fail(&self, error: SftpError) -> RuntimeError {
        let classified = self
            .death
            .get_or_init(|| async {
                self.dead.store(true, Ordering::SeqCst);
                let endpoint = self.endpoint();
                if self.stats.rejected().is_some() {
                    errors::frame_rejected(endpoint)
                } else if self.stats.transport_ended() || self.kill.is_killed() {
                    errors::connection_lost(endpoint)
                } else if self.stats.last_type() == Some(FXP_NAME) {
                    errors::name_unsupported(endpoint)
                } else if is_format(&error)
                    || is_protocol(&error)
                    || matches!(error, SftpError::BackgroundTaskFailure(_))
                {
                    errors::protocol(endpoint)
                } else {
                    errors::connection_lost(endpoint)
                }
            })
            .await
            .clone();
        self.shutdown().await;
        classified
    }

    fn fs(&self) -> Result<openssh_sftp_client::fs::Fs, OpError> {
        self.sftp
            .lock()
            .expect("sftp slot")
            .as_ref()
            .map(Sftp::fs)
            .ok_or_else(|| OpError::Resource(errors::connection_lost(&self.key.endpoint)))
    }

    async fn canonical(&self, path: &str) -> Result<String, OpError> {
        let mut fs = self.fs()?;
        let resolved = fs.canonicalize(path).await?;
        resolved
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| OpError::Resource(errors::name_unsupported(self.endpoint())))
    }

    /// Resolves `path` on the server (symlinks, `..`) and requires it inside an authorized root,
    /// itself resolved on the server once per channel.
    async fn resolve(&self, path: &str, roots: &[String]) -> Result<String, OpError> {
        let resolved = self.canonical(path).await?;
        for root in roots {
            let cached = self.roots.lock().await.get(root).cloned();
            let canonical_root = match cached {
                Some(value) => value,
                None => {
                    let value = self.canonical(root).await?;
                    self.roots.lock().await.insert(root.clone(), value.clone());
                    value
                }
            };
            if posix::contains(&canonical_root, &resolved) {
                return Ok(resolved);
            }
        }
        Err(OpError::Resource(errors::outside_root(self.endpoint())))
    }

    pub async fn stat(&self, uri: &FileUri, roots: &[String]) -> Result<FileStat, OpError> {
        let path = self.resolve(&uri.path, roots).await?;
        let metadata = self.fs()?.metadata(&path).await?;
        Ok(FileStat {
            uri: uri.clone(),
            kind: file_kind(&metadata),
            size: metadata.len().unwrap_or(0),
            modified_unix_ms: modified_ms(&metadata),
            read_only: true,
        })
    }

    /// Reads at most `cap` bytes (inclusive), refusing larger files before opening them (STAT)
    /// and while reading (growth after STAT).
    pub async fn read_bytes(
        &self,
        uri: &FileUri,
        roots: &[String],
        cap: u64,
    ) -> Result<RemoteBytes, OpError> {
        let path = self.resolve(&uri.path, roots).await?;
        let mut fs = self.fs()?;
        let metadata = fs.metadata(&path).await?;
        if !metadata.file_type().is_some_and(|t| t.is_file()) {
            return Err(OpError::Resource(errors::not_a_file(self.endpoint())));
        }
        if metadata.len().unwrap_or(0) > cap {
            return Err(OpError::Resource(errors::too_large(self.endpoint())));
        }
        let mut options = {
            let slot = self.sftp.lock().expect("sftp slot");
            slot.as_ref()
                .map(Sftp::options)
                .ok_or_else(|| OpError::Resource(errors::connection_lost(self.endpoint())))?
        };
        let mut file = options.read(true).open(&path).await?;
        let mut bytes: Vec<u8> = Vec::with_capacity(metadata.len().unwrap_or(0).min(cap) as usize);
        let outcome: Result<(), OpError> = loop {
            let offset = bytes.len() as u64;
            let want = (cap + 1 - offset).min(READ_REQUEST_BYTES) as u32;
            // Explicit offset: the crate advances by the requested length, not the bytes received.
            if file.seek(SeekFrom::Start(offset)).await.is_err() {
                break Err(OpError::Protocol);
            }
            match file.read(want, BytesMut::new()).await {
                Ok(None) => break Ok(()),
                Ok(Some(data)) => {
                    // The crate may lower the request to the negotiated read length; the offset it
                    // advanced is the length actually requested from the peer.
                    let requested = file.offset().saturating_sub(offset);
                    if data.is_empty() || data.len() as u64 > requested {
                        break Err(OpError::Protocol);
                    }
                    bytes.extend_from_slice(&data);
                    if bytes.len() as u64 > cap {
                        break Err(OpError::Resource(errors::too_large(self.endpoint())));
                    }
                }
                Err(error) => break Err(OpError::Sftp(error)),
            }
        };
        match outcome {
            Ok(()) => {
                let _ = file.close().await;
                Ok(RemoteBytes { bytes, metadata })
            }
            Err(OpError::Resource(error)) => {
                let _ = file.close().await;
                Err(OpError::Resource(error))
            }
            Err(other) => Err(other),
        }
    }

    /// First page of a directory listing.
    pub async fn list_first(
        &self,
        uri: &FileUri,
        roots: &[String],
    ) -> Result<(Vec<FileEntry>, Option<String>), OpError> {
        let path = self.resolve(&uri.path, roots).await?;
        let mut fs = self.fs()?;
        let metadata = fs.metadata(&path).await?;
        if !metadata.file_type().is_some_and(|t| t.is_dir()) {
            return Err(OpError::Resource(errors::not_a_directory(self.endpoint())));
        }
        let dir = fs.open_dir(&path).await?;
        let cursor = DirCursor {
            dir: uri.clone(),
            path,
            stream: Box::pin(dir.read_dir()),
            peeked: None,
        };
        self.page(cursor).await
    }

    /// Next page for a cursor of this channel (tokens are single use).
    pub async fn list_next(
        &self,
        uri: &FileUri,
        token: &str,
    ) -> Result<(Vec<FileEntry>, Option<String>), OpError> {
        let cursor = {
            let mut cursors = self.cursors.lock().await;
            match cursors.map.get(token) {
                Some(cursor) if cursor.dir == *uri => {
                    cursors.order.retain(|t| t != token);
                    cursors.map.remove(token)
                }
                _ => None,
            }
        };
        match cursor {
            Some(cursor) => self.page(cursor).await,
            None => Err(OpError::Resource(errors::invalid_cursor(self.endpoint()))),
        }
    }

    fn entry(&self, dir: &str, entry: &DirEntry) -> Result<Option<FileEntry>, OpError> {
        let Some(name) = entry.filename().to_str() else {
            return Err(OpError::Resource(errors::name_unsupported(self.endpoint())));
        };
        if name == "." || name == ".." || name.is_empty() {
            return Ok(None);
        }
        let kind = match entry.file_type() {
            Some(t) if t.is_dir() => FileKind::Directory,
            Some(t) if t.is_symlink() => FileKind::Symlink,
            Some(t) if t.is_file() => FileKind::File,
            _ => FileKind::Other,
        };
        Ok(Some(FileEntry {
            uri: FileUri::remote(PROVIDER_ID, self.endpoint(), posix::join(dir, name)),
            name: name.to_owned(),
            kind,
        }))
    }

    /// Fills one page (≤ 128) and peeks one more entry, so `next_cursor` is `Some` exactly while
    /// entries remain. Polling stops once the page is full: no READDIR ahead of the user.
    async fn page(
        &self,
        mut cursor: DirCursor,
    ) -> Result<(Vec<FileEntry>, Option<String>), OpError> {
        let mut entries = Vec::new();
        let mut more = true;
        while entries.len() < MAX_PAGE_ENTRIES {
            let next = match cursor.peeked.take() {
                Some(entry) => Some(Ok(entry)),
                None => next_entry(&mut cursor.stream).await,
            };
            match next {
                None => {
                    more = false;
                    break;
                }
                Some(Err(error)) => return Err(OpError::Sftp(error)),
                Some(Ok(entry)) => {
                    if let Some(item) = self.entry(&cursor.path, &entry)? {
                        entries.push(item);
                    }
                }
            }
        }
        if more {
            more = false;
            while let Some(next) = next_entry(&mut cursor.stream).await {
                let entry = next.map_err(OpError::Sftp)?;
                if self.entry(&cursor.path, &entry)?.is_some() {
                    cursor.peeked = Some(entry);
                    more = true;
                    break;
                }
            }
        }
        if !more {
            return Ok((entries, None));
        }
        let token = format!("c{}-{}", self.id, uuid::Uuid::new_v4().simple());
        let mut cursors = self.cursors.lock().await;
        if self.is_dead() {
            return Err(OpError::Resource(errors::connection_lost(self.endpoint())));
        }
        cursors.map.insert(token.clone(), cursor);
        cursors.order.push_back(token.clone());
        while cursors.order.len() > MAX_CURSORS {
            if let Some(oldest) = cursors.order.pop_front() {
                cursors.map.remove(&oldest);
            }
        }
        Ok((entries, Some(token)))
    }
}

/// Channel id encoded in a cursor token, when the token has the provider's shape.
pub fn cursor_channel(token: &str) -> Option<u64> {
    let rest = token.strip_prefix('c')?;
    let (id, random) = rest.split_once('-')?;
    if random.len() != 32 || !random.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    id.parse().ok()
}
