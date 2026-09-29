//! Spec 006 — remote files and diffs (seam: the FileProvider contract of the SFTP provider over
//! fixtures — a real `sftp-server` on pipes, a scripted SFTP peer for hostile/short replies —
//! plus one native E2E remote read → disconnection → reconnection).
//!
//! The backend modules are compiled here through `#[path]`: composing them into
//! `src-tauri/src/lib.rs` belongs to spec 007. The module tree mirrors the future crate
//! (`crate::files::{local, sftp}`, `crate::connections`, `crate::bridge::ssh`).
//!
//! AC-006-01: Local and SSH with the same path and different contents — the SSH tree lists and
//!            reads only on the SSH host, tabs carry the host badge and Somente leitura, and no
//!            save command exists.
//! AC-006-02: a remote file read before the connection drops shows its cached content as
//!            Desatualizado; reconnecting invalidates old handles/cursors and allows a reload,
//!            never treating the cache as live state.
//! AC-006-03: two remote text snapshots (≤ 2 MiB) are compared by their identified contents;
//!            binary files, paths without permission or a missing SFTP subsystem produce a
//!            localized error while the Herdr terminals stay usable.
//!
//! Fixture values are distinct on purpose: two endpoints (`…a` / `…b`), boots `boot-a-1` /
//! `boot-a-2`, generations 1 / 2 and local/remote contents that differ at the same path.

#[allow(dead_code)]
#[path = "../src/connections/mod.rs"]
mod connections;

mod theme {
    pub use herdr_desktop::theme::*;
}

#[allow(dead_code)]
#[path = "../src/bridge"]
mod bridge {
    pub mod ssh;
}

#[allow(dead_code)]
#[path = "../src/files"]
mod files {
    pub mod local;
    pub mod sftp;
}

#[allow(dead_code)]
#[path = "../../scripts/feature-harness/native.rs"]
mod native_harness;

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../../scripts/feature-harness/window.rs"]
mod window_harness;

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use connections::ssh_options::{
    build_sftp_subsystem, build_ssh, noninteractive_options, IsolatedSshConfig, ProfileId,
    RemoteHerdrCommand, SshIdentity,
};
use connections::state::LinkPhase;
use files::sftp::guard::{FrameGuard, FrameStats};
use files::sftp::{
    OpenSshSftpConnector, RemoteFileTarget, RemoteFilesConfig, RemoteFilesState, RemoteHostLink,
    RemoteLinks, SftpConnector, SftpTransport, MAX_FRAME_BYTES, MAX_PENDING_OPERATIONS,
    MIN_FRAME_BYTES, OPERATION_DEADLINE, PROVIDER_ID,
};
use herdr_client::{FileKind, FileProvider, FileUri, LiveIdentity, RuntimeError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SESSION_A: &str = "hd006-remote-a";
const SESSION_B: &str = "hd006-remote-b";

fn endpoint_a() -> String {
    "0123456789abcdef0123456789abcdef".to_owned()
}

fn endpoint_b() -> String {
    "fedcba9876543210fedcba9876543210".to_owned()
}

// ---------------------------------------------------------------------------------------
// Links (what the connection hub of spec 003 reports)
// ---------------------------------------------------------------------------------------

#[derive(Default)]
struct FakeLinks {
    hosts: Mutex<BTreeMap<String, RemoteHostLink>>,
    revision: AtomicU64,
}

fn link(endpoint: &str, label: &str, session: &str, generation: u64, boot: &str) -> RemoteHostLink {
    RemoteHostLink {
        ssh: SshIdentity::new(
            ProfileId::parse(endpoint).unwrap(),
            "tester@127.0.0.1",
            Some(2222),
            session,
        )
        .unwrap(),
        label: label.to_owned(),
        phase: LinkPhase::Online,
        live: Some(LiveIdentity {
            endpoint: endpoint.to_owned(),
            session: session.to_owned(),
            connection_generation: generation,
            boot_id: boot.to_owned(),
        }),
    }
}

impl FakeLinks {
    fn with(links: Vec<RemoteHostLink>) -> Arc<Self> {
        let this = Arc::new(Self::default());
        for l in links {
            this.set(l);
        }
        this
    }

    fn set(&self, link: RemoteHostLink) {
        self.hosts
            .lock()
            .unwrap()
            .insert(link.ssh.endpoint_id().to_owned(), link);
        self.revision.fetch_add(1, Ordering::SeqCst);
    }

    fn lose(&self, endpoint: &str) {
        let mut hosts = self.hosts.lock().unwrap();
        let host = hosts.get_mut(endpoint).unwrap();
        host.phase = LinkPhase::Reconnecting;
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
}

impl RemoteLinks for FakeLinks {
    fn link(&self, endpoint: &str) -> Result<RemoteHostLink, RuntimeError> {
        self.hosts
            .lock()
            .unwrap()
            .get(endpoint)
            .cloned()
            .ok_or_else(|| RuntimeError::new("file_host_unknown", "host remoto desconhecido"))
    }

    fn links(&self) -> Vec<RemoteHostLink> {
        self.hosts.lock().unwrap().values().cloned().collect()
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }

    fn wait_changed(&self, _since: u64, _timeout: Duration) -> u64 {
        self.revision()
    }
}

fn target_of(link: &RemoteHostLink) -> RemoteFileTarget {
    let live = link.live.clone().unwrap();
    RemoteFileTarget {
        endpoint: live.endpoint,
        session: live.session,
        connection_generation: live.connection_generation,
        boot_id: live.boot_id,
    }
}

fn ruri(endpoint: &str, path: &str) -> FileUri {
    FileUri::remote(PROVIDER_ID, endpoint, path)
}

fn block<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

fn state_with(
    links: Arc<FakeLinks>,
    connector: Arc<dyn SftpConnector>,
    roots: &[(&str, &Path)],
) -> Arc<RemoteFilesState> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (endpoint, root) in roots {
        map.entry((*endpoint).to_owned())
            .or_default()
            .push(root.display().to_string());
    }
    Arc::new(
        RemoteFilesState::new(RemoteFilesConfig {
            links,
            connector,
            roots: map,
            deadline: OPERATION_DEADLINE,
        })
        .unwrap(),
    )
}

fn err(result: Result<impl std::fmt::Debug, RuntimeError>) -> RuntimeError {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(error) => error,
    }
}

static OP: AtomicU64 = AtomicU64::new(1);

fn op() -> String {
    format!("op-{}", OP.fetch_add(1, Ordering::SeqCst))
}

// ---------------------------------------------------------------------------------------
// Connectors: real sftp-server processes on pipes, optionally in a mount namespace where the
// same path holds different content (the "remote host"), and a scripted SFTP peer.
// ---------------------------------------------------------------------------------------

fn sftp_server_bin() -> &'static str {
    [
        "/usr/lib/ssh/sftp-server",
        "/usr/libexec/sftp-server",
        "/usr/lib/openssh/sftp-server",
        "/usr/libexec/openssh/sftp-server",
    ]
    .into_iter()
    .find(|p| Path::new(p).exists())
    .expect("sftp-server binary (OpenSSH) is required by the provider contract tests")
}

struct ProcessConnector {
    program: String,
    args: Vec<String>,
    opens: AtomicUsize,
    pids: Mutex<Vec<u32>>,
}

impl ProcessConnector {
    fn new(program: &str, args: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            program: program.to_owned(),
            args,
            opens: AtomicUsize::new(0),
            pids: Mutex::new(Vec::new()),
        })
    }

    /// `sftp-server` directly on pipes: same filesystem as the test.
    fn plain() -> Arc<Self> {
        Self::new(sftp_server_bin(), Vec::new())
    }

    /// `sftp-server` inside a user+mount namespace where `remote` is bind-mounted over `root`:
    /// the same absolute path shows the "remote host" content, and a nested user namespace
    /// drops capabilities so permissions behave as for the real user.
    #[cfg(target_os = "linux")]
    fn namespaced(remote: &Path, root: &Path) -> Arc<Self> {
        let uid = String::from_utf8(
            std::process::Command::new("id")
                .arg("-u")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let gid = String::from_utf8(
            std::process::Command::new("id")
                .arg("-g")
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let script = format!(
            "mount --bind \"$1\" \"$2\" && exec unshare --user --map-user={} --map-group={} {}",
            uid.trim(),
            gid.trim(),
            sftp_server_bin()
        );
        Self::new(
            "unshare",
            vec![
                "-rm".into(),
                "sh".into(),
                "-c".into(),
                script,
                "sftp-remote".into(),
                remote.display().to_string(),
                root.display().to_string(),
            ],
        )
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    fn last_pid(&self) -> u32 {
        *self
            .pids
            .lock()
            .unwrap()
            .last()
            .expect("a channel was opened")
    }
}

impl SftpConnector for ProcessConnector {
    fn open(&self, _link: &RemoteHostLink) -> Result<SftpTransport, RuntimeError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let mut command = tokio::process::Command::new(&self.program);
        command.args(&self.args);
        let transport = SftpTransport::spawn(command)
            .map_err(|_| RuntimeError::new("test_spawn_failed", "spawn failed"))?;
        self.pids.lock().unwrap().push(transport.pid().unwrap());
        Ok(transport)
    }
}

/// Process id of the real `sftp-server` behind a channel (the namespaced variant forks
/// through `unshare`/`sh`, so look for the descendant named sftp-server).
#[cfg(target_os = "linux")]
fn server_pid(root: u32) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut frontier = vec![root];
        while let Some(pid) = frontier.pop() {
            if std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .is_ok_and(|c| c.trim() == "sftp-server")
            {
                return pid;
            }
            if let Ok(children) =
                std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
            {
                frontier.extend(
                    children
                        .split_whitespace()
                        .filter_map(|c| c.parse::<u32>().ok()),
                );
            }
        }
        assert!(Instant::now() < deadline, "no sftp-server under pid {root}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: u32) -> bool {
    // A reaped process leaves no /proc entry; a zombie still counts as gone for the channel.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| {
            !s.rsplit_once(')')
                .is_some_and(|(_, t)| t.trim_start().starts_with('Z'))
        })
        .unwrap_or(false)
}

fn wait_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        assert!(Instant::now() < deadline, "process {pid} still alive");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn signal(pid: u32, sig: &str) {
    assert!(std::process::Command::new("kill")
        .args([sig, &pid.to_string()])
        .status()
        .unwrap()
        .success());
}

// --- scripted SFTP peer (test fixture only; the product never implements a codec) --------

const FXP_INIT: u8 = 1;
const FXP_VERSION: u8 = 2;
const FXP_OPEN: u8 = 3;
const FXP_CLOSE: u8 = 4;
const FXP_READ: u8 = 5;
const FXP_LSTAT: u8 = 7;
const FXP_OPENDIR: u8 = 11;
const FXP_READDIR: u8 = 12;
const FXP_REALPATH: u8 = 16;
const FXP_STAT: u8 = 17;
const FXP_STATUS: u8 = 101;
const FXP_HANDLE: u8 = 102;
const FXP_DATA: u8 = 103;
const FXP_NAME: u8 = 104;
const FXP_ATTRS: u8 = 105;

#[derive(Clone)]
enum PeerNode {
    File {
        content: Vec<u8>,
        reported_size: u64,
    },
    Dir {
        batches: Vec<Vec<Vec<u8>>>,
    },
}

#[derive(Clone, Default)]
struct PeerScript {
    nodes: BTreeMap<Vec<u8>, PeerNode>,
    /// Maximum bytes per DATA reply (short reads).
    short_read: Option<usize>,
    /// DATA replies carry this many bytes more than the requested length.
    oversize_data: usize,
    /// Any request naming this path gets STATUS with a non-UTF-8 message.
    bad_status_on: Option<Vec<u8>>,
    /// Any request naming this path gets a frame header announcing this length.
    frame_len_on: Option<(Vec<u8>, u32)>,
    /// Requests naming this path are answered only after `release` is notified.
    hang_on: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeerRequest {
    connection: usize,
    kind: u8,
    path: Vec<u8>,
    offset: u64,
    len: u32,
}

struct FakePeer {
    script: PeerScript,
    log: Mutex<Vec<PeerRequest>>,
    opens: AtomicUsize,
    release: tokio::sync::Notify,
    released: std::sync::atomic::AtomicBool,
}

impl FakePeer {
    fn new(script: PeerScript) -> Arc<Self> {
        Arc::new(Self {
            script,
            log: Mutex::new(Vec::new()),
            opens: AtomicUsize::new(0),
            release: tokio::sync::Notify::new(),
            released: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.release.notify_waiters();
    }

    fn requests(&self) -> Vec<PeerRequest> {
        self.log.lock().unwrap().clone()
    }
}

struct PeerConnector(Arc<FakePeer>);

impl SftpConnector for PeerConnector {
    fn open(&self, _link: &RemoteHostLink) -> Result<SftpTransport, RuntimeError> {
        let connection = self.0.opens.fetch_add(1, Ordering::SeqCst) + 1;
        let (client, server) = tokio::io::duplex(4 * 1024 * 1024);
        let (client_read, client_write) = tokio::io::split(client);
        tokio::spawn(serve_peer(self.0.clone(), server, connection));
        Ok(SftpTransport::from_streams(
            Box::new(client_write),
            Box::new(client_read),
        ))
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_str(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u32(out, bytes.len() as u32);
    out.extend_from_slice(bytes);
}

fn attrs(out: &mut Vec<u8>, size: u64, dir: bool) {
    put_u32(out, 0x1 | 0x4 | 0x8);
    out.extend_from_slice(&size.to_be_bytes());
    put_u32(out, if dir { 0o040755 } else { 0o100644 });
    put_u32(out, 1_700_000_000);
    put_u32(out, 1_700_000_000);
}

fn frame(kind: u8, id: u32, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_u32(&mut out, (body.len() + 5) as u32);
    out.push(kind);
    put_u32(&mut out, id);
    out.extend_from_slice(body);
    out
}

fn status(id: u32, code: u32, message: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    put_u32(&mut body, code);
    put_str(&mut body, message);
    put_str(&mut body, b"");
    frame(FXP_STATUS, id, &body)
}

struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn u32(&mut self) -> u32 {
        let (head, tail) = self.0.split_at(4);
        self.0 = tail;
        u32::from_be_bytes(head.try_into().unwrap())
    }
    fn u64(&mut self) -> u64 {
        let (head, tail) = self.0.split_at(8);
        self.0 = tail;
        u64::from_be_bytes(head.try_into().unwrap())
    }
    fn bytes(&mut self) -> Vec<u8> {
        let len = self.u32() as usize;
        let (head, tail) = self.0.split_at(len);
        self.0 = tail;
        head.to_vec()
    }
}

/// Open handles of one peer connection: handle → (path, next READDIR batch).
type PeerHandles = BTreeMap<Vec<u8>, (Vec<u8>, usize)>;

async fn serve_peer(peer: Arc<FakePeer>, stream: tokio::io::DuplexStream, connection: usize) {
    let (mut reader, writer) = tokio::io::split(stream);
    let writer = Arc::new(tokio::sync::Mutex::new(writer));
    let handles: Arc<Mutex<PeerHandles>> = Arc::default();
    let mut next_handle = 0u32;
    loop {
        let mut len = [0u8; 4];
        if reader.read_exact(&mut len).await.is_err() {
            return;
        }
        let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
        if reader.read_exact(&mut body).await.is_err() {
            return;
        }
        let kind = body[0];
        if kind == FXP_INIT {
            let mut out = Vec::new();
            put_u32(&mut out, 5);
            out.push(FXP_VERSION);
            put_u32(&mut out, 3);
            writer.lock().await.write_all(&out).await.unwrap();
            continue;
        }
        let mut c = Cursor(&body[1..]);
        let id = c.u32();
        let mut request = PeerRequest {
            connection,
            kind,
            path: Vec::new(),
            offset: 0,
            len: 0,
        };
        match kind {
            FXP_OPEN | FXP_LSTAT | FXP_STAT | FXP_OPENDIR | FXP_REALPATH => {
                request.path = c.bytes();
            }
            FXP_READ => {
                let handle = c.bytes();
                request.path = handles
                    .lock()
                    .unwrap()
                    .get(&handle)
                    .map(|h| h.0.clone())
                    .unwrap_or_default();
                request.offset = c.u64();
                request.len = c.u32();
                request.path.extend_from_slice(b"#");
                request.path.extend_from_slice(&handle);
            }
            FXP_READDIR | FXP_CLOSE => {
                let handle = c.bytes();
                request.path = handles
                    .lock()
                    .unwrap()
                    .get(&handle)
                    .map(|h| h.0.clone())
                    .unwrap_or_default();
                request.path.extend_from_slice(b"#");
                request.path.extend_from_slice(&handle);
            }
            _ => {}
        }
        peer.log.lock().unwrap().push(request.clone());
        let path = request
            .path
            .split(|b| *b == b'#')
            .next()
            .unwrap_or_default()
            .to_vec();
        let script = &peer.script;

        if script
            .frame_len_on
            .as_ref()
            .is_some_and(|(p, _)| *p == path)
        {
            let (_, announced) = script.frame_len_on.clone().unwrap();
            let mut out = announced.to_be_bytes().to_vec();
            out.extend_from_slice(&[FXP_STATUS, 0, 0, 0, 0]);
            let _ = writer.lock().await.write_all(&out).await;
            continue;
        }
        if script.bad_status_on.as_ref() == Some(&path) {
            let reply = status(
                id,
                3,
                &[0xff, 0xfe, b'S', b'E', b'G', b'R', b'E', b'D', b'O'],
            );
            writer.lock().await.write_all(&reply).await.unwrap();
            continue;
        }

        let reply = match kind {
            FXP_REALPATH => {
                let mut b = Vec::new();
                put_u32(&mut b, 1);
                put_str(&mut b, &path);
                put_str(&mut b, b"");
                put_u32(&mut b, 0);
                frame(FXP_NAME, id, &b)
            }
            FXP_STAT | FXP_LSTAT => match script.nodes.get(&path) {
                Some(PeerNode::File { reported_size, .. }) => {
                    let mut b = Vec::new();
                    attrs(&mut b, *reported_size, false);
                    frame(FXP_ATTRS, id, &b)
                }
                Some(PeerNode::Dir { .. }) => {
                    let mut b = Vec::new();
                    attrs(&mut b, 4096, true);
                    frame(FXP_ATTRS, id, &b)
                }
                None => status(id, 2, b"No such file"),
            },
            FXP_OPEN | FXP_OPENDIR => match script.nodes.get(&path) {
                Some(_) => {
                    next_handle += 1;
                    let handle = format!("h{next_handle}").into_bytes();
                    handles
                        .lock()
                        .unwrap()
                        .insert(handle.clone(), (path.clone(), 0));
                    let mut b = Vec::new();
                    put_str(&mut b, &handle);
                    frame(FXP_HANDLE, id, &b)
                }
                None => status(id, 2, b"No such file"),
            },
            FXP_READ => match script.nodes.get(&path) {
                Some(PeerNode::File { content, .. }) => {
                    let start = request.offset as usize;
                    if start >= content.len() {
                        status(id, 1, b"EOF")
                    } else {
                        let mut n = (request.len as usize).min(content.len() - start);
                        if let Some(max) = script.short_read {
                            n = n.min(max);
                        }
                        let mut data = content[start..start + n].to_vec();
                        if script.oversize_data > 0 {
                            data = vec![b'x'; request.len as usize + script.oversize_data];
                        }
                        let mut b = Vec::new();
                        put_str(&mut b, &data);
                        frame(FXP_DATA, id, &b)
                    }
                }
                _ => status(id, 4, b"failure"),
            },
            FXP_READDIR => {
                let handle = request
                    .path
                    .split(|b| *b == b'#')
                    .nth(1)
                    .unwrap_or_default()
                    .to_vec();
                let batch = {
                    let mut table = handles.lock().unwrap();
                    let entry = table.get_mut(&handle).unwrap();
                    let index = entry.1;
                    entry.1 += 1;
                    match script.nodes.get(&path) {
                        Some(PeerNode::Dir { batches }) => batches.get(index).cloned(),
                        _ => None,
                    }
                };
                match batch {
                    Some(names) => {
                        let mut b = Vec::new();
                        put_u32(&mut b, names.len() as u32);
                        for name in names {
                            put_str(&mut b, &name);
                            put_str(&mut b, b"");
                            attrs(&mut b, 1, false);
                        }
                        frame(FXP_NAME, id, &b)
                    }
                    None => status(id, 1, b"EOF"),
                }
            }
            FXP_CLOSE => status(id, 0, b"OK"),
            _ => status(id, 8, b"unsupported"),
        };
        if script.hang_on.as_ref() == Some(&path) && !peer.released.load(Ordering::SeqCst) {
            let peer = peer.clone();
            let writer = writer.clone();
            tokio::spawn(async move {
                let notified = peer.release.notified();
                if !peer.released.load(Ordering::SeqCst) {
                    notified.await;
                }
                let _ = writer.lock().await.write_all(&reply).await;
            });
            continue;
        }
        if writer.lock().await.write_all(&reply).await.is_err() {
            return;
        }
    }
}

fn peer_state(
    script: PeerScript,
) -> (
    Arc<FakePeer>,
    Arc<FakeLinks>,
    Arc<RemoteFilesState>,
    RemoteHostLink,
) {
    let peer = FakePeer::new(script);
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let links = FakeLinks::with(vec![l.clone()]);
    let state = state_with(
        links.clone(),
        Arc::new(PeerConnector(peer.clone())),
        &[(&endpoint_a(), Path::new("/raiz"))],
    );
    (peer, links, state, l)
}

// ---------------------------------------------------------------------------------------
// Fixture trees
// ---------------------------------------------------------------------------------------

struct Trees {
    _temp: tempfile::TempDir,
    root: PathBuf,
    remote: PathBuf,
    outside: PathBuf,
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// `root` is the local project; `remote` is what the namespaced server shows at `root`.
fn trees() -> Trees {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("projeto");
    let remote = temp.path().join("host-remoto");
    let outside = temp.path().join("fora");
    write(&root.join("notas.txt"), b"conteudo LOCAL\n");
    write(&root.join("so-local.txt"), b"apenas local\n");
    write(&outside.join("segredo.txt"), b"fora da raiz\n");
    write(
        &remote.join("notas.txt"),
        b"conteudo REMOTO linha 1\r\nlinha 2\r\n",
    );
    write(
        &remote.join("sub/um.txt"),
        b"dentro do subdiretorio remoto\n",
    );
    // Local: `escape` leaves the root; remote: `escape` is a regular file inside the root.
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    write(&remote.join("escape"), b"arquivo remoto chamado escape\n");
    // Local: `vinculo` is a regular file; remote: `vinculo` points outside the root.
    write(&root.join("vinculo"), b"arquivo local vinculo\n");
    std::os::unix::fs::symlink(outside.join("segredo.txt"), remote.join("vinculo")).unwrap();
    std::os::unix::fs::symlink("notas.txt", remote.join("atalho.txt")).unwrap();
    Trees {
        _temp: temp,
        root,
        remote,
        outside,
    }
}

// =======================================================================================
// AC-006-01 — only the SSH host, badge/read-only, no save
// =======================================================================================

/// Would catch: the SFTP provider falling back to the local filesystem for an identical path
/// (listing/reading local content), or writing anything while reading.
#[cfg(target_os = "linux")]
#[test]
fn same_path_is_listed_and_read_only_on_the_ssh_host() {
    let t = trees();
    let connector = ProcessConnector::namespaced(&t.remote, &t.root);
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let state = state_with(
        FakeLinks::with(vec![l.clone()]),
        connector.clone(),
        &[(&endpoint_a(), &t.root)],
    );
    let target = target_of(&l);
    let root = t.root.display().to_string();
    let local_before = std::fs::read(t.root.join("notas.txt")).unwrap();

    let page = block(state.list(&op(), &target, &ruri(&endpoint_a(), &root), None)).unwrap();
    let mut names: Vec<String> = page.page.entries.iter().map(|e| e.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        ["atalho.txt", "escape", "notas.txt", "sub", "vinculo"],
        "remote tree, not the local one"
    );
    assert!(page
        .page
        .entries
        .iter()
        .all(|e| e.uri.provider == PROVIDER_ID
            && e.uri.host.as_deref() == Some(endpoint_a().as_str())));
    assert_eq!(
        page.page
            .entries
            .iter()
            .find(|e| e.name == "sub")
            .unwrap()
            .kind,
        FileKind::Directory
    );
    assert_eq!(
        page.page
            .entries
            .iter()
            .find(|e| e.name == "atalho.txt")
            .unwrap()
            .kind,
        FileKind::Symlink
    );
    assert_eq!(page.connection.endpoint, endpoint_a());
    assert_eq!(page.connection.connection_generation, 1);
    assert_eq!(page.connection.boot_id, "boot-a-1");

    let file = format!("{root}/notas.txt");
    let snapshot = block(state.read(&op(), &target, &ruri(&endpoint_a(), &file))).unwrap();
    assert_eq!(
        snapshot.snapshot.content,
        "conteudo REMOTO linha 1\nlinha 2\n"
    );
    assert_eq!(snapshot.snapshot.eol, files::local::LineEnding::Crlf);
    assert_eq!(snapshot.snapshot.size, 34);
    assert_eq!(snapshot.snapshot.uri, ruri(&endpoint_a(), &file));
    assert_ne!(
        snapshot.snapshot.content.as_bytes(),
        local_before.as_slice()
    );
    assert_eq!(
        std::fs::read(t.root.join("notas.txt")).unwrap(),
        local_before,
        "local file untouched"
    );
    assert_eq!(
        connector.opens(),
        1,
        "one persistent channel for both operations"
    );

    // The provider is read-only: no write capability and writing is refused before any I/O.
    let provider = state.provider(target.clone());
    assert!(!provider.capabilities().write);
    assert!(
        provider.capabilities().read
            && provider.capabilities().list
            && provider.capabilities().stat
    );
    let refused = provider
        .write(&ruri(&endpoint_a(), &file), b"sobrescrever")
        .unwrap_err();
    assert_eq!(refused.code, "write_unsupported");
    assert_eq!(connector.opens(), 1);
    assert_eq!(
        std::fs::read(t.remote.join("notas.txt")).unwrap(),
        b"conteudo REMOTO linha 1\r\nlinha 2\r\n"
    );
}

/// Would catch: a local URI, a URI of another host or a non-POSIX path being served (or being
/// redirected to the local provider) instead of refused before a channel is opened.
#[test]
fn foreign_or_unsupported_uris_are_refused_before_any_io() {
    let connector = ProcessConnector::plain();
    let a = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let b = link(&endpoint_b(), "Servidor B", SESSION_B, 7, "boot-b-7");
    let state = state_with(
        FakeLinks::with(vec![a.clone(), b]),
        connector.clone(),
        &[(&endpoint_a(), Path::new("/srv/projeto"))],
    );
    let target = target_of(&a);
    let cases = [
        (FileUri::local("/srv/projeto/notas.txt"), "file_uri_invalid"),
        (
            FileUri {
                provider: PROVIDER_ID.into(),
                host: None,
                path: "/srv/projeto/notas.txt".into(),
            },
            "file_host_mismatch",
        ),
        (
            ruri(&endpoint_b(), "/srv/projeto/notas.txt"),
            "file_host_mismatch",
        ),
        (
            ruri(&endpoint_a(), "srv/projeto/notas.txt"),
            "remote_path_unsupported",
        ),
        (
            ruri(&endpoint_a(), "C:\\projeto\\notas.txt"),
            "remote_path_unsupported",
        ),
        (
            ruri(&endpoint_a(), "\\\\servidor\\share\\notas.txt"),
            "remote_path_unsupported",
        ),
        (
            ruri(&endpoint_a(), "/srv/projeto/\0notas.txt"),
            "remote_path_unsupported",
        ),
    ];
    for (uri, code) in cases {
        let error = err(block(state.read(&op(), &target, &uri)));
        assert_eq!(error.code, code, "{uri:?}");
        assert_eq!(error.endpoint.as_deref(), Some(endpoint_a().as_str()));
        let error = err(block(state.list(&op(), &target, &uri, None)));
        assert_eq!(error.code, code, "{uri:?}");
    }
    // Host B is online but has no authorized root: nothing is served.
    let b_target = RemoteFileTarget {
        endpoint: endpoint_b(),
        session: SESSION_B.into(),
        connection_generation: 7,
        boot_id: "boot-b-7".into(),
    };
    assert_eq!(
        err(block(state.list(
            &op(),
            &b_target,
            &ruri(&endpoint_b(), "/"),
            None
        )))
        .code,
        "root_not_authorized"
    );
    assert_eq!(connector.opens(), 0, "no channel for refused requests");
}

/// Would catch: a save/write command exposed to the WebView, a command without handler or a
/// bridge invoking something the backend does not declare.
#[test]
fn remote_commands_are_read_only_limited_and_match_the_frontend_bridge() {
    let source = include_str!("../src/files/sftp.rs");
    for command in files::sftp::COMMANDS {
        assert!(
            source.contains(&format!("#[tauri::command]\npub async fn {command}("))
                || source.contains(&format!("#[tauri::command]\npub fn {command}(")),
            "{command} has no #[tauri::command] handler"
        );
        for word in [
            "save", "write", "recovery", "delete", "rename", "shell", "exec", "spawn", "upload",
        ] {
            assert!(!command.contains(word), "{command} looks like {word}");
        }
    }
    assert_eq!(
        source.matches("#[tauri::command]").count(),
        files::sftp::COMMANDS.len()
    );
    let bridge = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../src/files/remote.ts"
    ))
    .unwrap();
    let mut invoked: Vec<&str> = bridge
        .split("invoke<")
        .skip(1)
        .filter_map(|rest| rest.split('"').nth(1))
        .collect();
    invoked.sort_unstable();
    invoked.dedup();
    let mut declared = files::sftp::COMMANDS.to_vec();
    declared.sort_unstable();
    assert_eq!(invoked, declared);
    assert!(
        !files::sftp::COMMANDS
            .iter()
            .any(|c| files::local::COMMANDS.contains(c)),
        "remote reuses no local command name"
    );
}

/// Would catch: the SFTP channel built with its own argv (accept-new host keys, prompts, user
/// config) instead of the 003 seam, or a remote shell command instead of the subsystem.
#[test]
fn sftp_subsystem_command_comes_from_the_003_seam() {
    let identity = SshIdentity::new(
        ProfileId::parse(&endpoint_a()).unwrap(),
        "dev@host.example",
        Some(2222),
        SESSION_A,
    )
    .unwrap();
    let isolated = IsolatedSshConfig {
        identity_file: PathBuf::from("/tmp/hd006/keys/id"),
        user_known_hosts_file: PathBuf::from("/tmp/hd006/known_hosts"),
    };
    let command = build_sftp_subsystem(&identity, Some(&isolated));
    assert_eq!(command.program, "ssh");
    let mut expected = noninteractive_options();
    expected.extend(
        [
            "-F",
            "none",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "IdentityAgent=none",
            "-o",
            "GlobalKnownHostsFile=none",
            "-o",
            "UserKnownHostsFile=/tmp/hd006/known_hosts",
            "-i",
            "/tmp/hd006/keys/id",
            "-T",
            "-p",
            "2222",
            "-s",
            "--",
            "dev@host.example",
            "sftp",
        ]
        .map(String::from),
    );
    assert_eq!(command.args, expected);
    assert!(command
        .args
        .contains(&"StrictHostKeyChecking=yes".to_owned()));
    assert!(command.args.contains(&"BatchMode=yes".to_owned()));
    assert!(!command
        .args
        .iter()
        .any(|a| a.contains("accept-new") || a.contains("herdr")));
    // Same options as the Herdr lanes of 003, only the tail differs.
    let herdr = build_ssh(&identity, Some(&isolated), RemoteHerdrCommand::ServerStatus);
    let shared = herdr.args.iter().position(|a| a == "-T").unwrap();
    assert_eq!(command.args[..shared], herdr.args[..shared]);

    let plain = build_sftp_subsystem(
        &SshIdentity::new(
            ProfileId::parse(&endpoint_b()).unwrap(),
            "host.example",
            None,
            SESSION_B,
        )
        .unwrap(),
        None,
    );
    assert!(!plain.args.contains(&"-F".to_owned()) && !plain.args.contains(&"-p".to_owned()));
    assert_eq!(
        plain.args[plain.args.len() - 4..],
        ["-s", "--", "host.example", "sftp"].map(String::from)
    );
    // The production connector uses exactly this builder.
    let connector = OpenSshSftpConnector::new(Some(isolated.clone()));
    assert_eq!(connector.command(&identity), command);
}

/// Would catch: resolving `..` or symlinks with the local filesystem (the local tree differs),
/// or accepting a server-resolved path outside the authorized root.
#[cfg(target_os = "linux")]
#[test]
fn root_traversal_and_symlinks_are_resolved_by_the_server() {
    let t = trees();
    let connector = ProcessConnector::namespaced(&t.remote, &t.root);
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let state = state_with(
        FakeLinks::with(vec![l.clone()]),
        connector,
        &[(&endpoint_a(), &t.root)],
    );
    let target = target_of(&l);
    let root = t.root.display().to_string();
    let read = |path: String| block(state.read(&op(), &target, &ruri(&endpoint_a(), &path)));

    // Locally `escape` leaves the root; on the server it is a regular file inside.
    assert_eq!(
        read(format!("{root}/escape")).unwrap().snapshot.content,
        "arquivo remoto chamado escape\n"
    );
    // Locally `vinculo` is a regular file; on the server it points outside the root.
    assert_eq!(
        err(read(format!("{root}/vinculo"))).code,
        "path_outside_root"
    );
    // Traversal is resolved by the server and refused.
    let outside = t.outside.join("segredo.txt").display().to_string();
    assert_eq!(
        err(read(format!("{root}/sub/../../fora/segredo.txt"))).code,
        "path_outside_root"
    );
    assert_eq!(err(read(outside.clone())).code, "path_outside_root");
    assert_eq!(
        err(block(state.list(
            &op(),
            &target,
            &ruri(&endpoint_a(), &t.outside.display().to_string()),
            None
        )))
        .code,
        "path_outside_root"
    );
    // A symlink inside the root to a file inside the root is served.
    assert_eq!(
        read(format!("{root}/atalho.txt")).unwrap().snapshot.content,
        "conteudo REMOTO linha 1\nlinha 2\n"
    );
    // Missing, directory-as-file and file-as-directory are errors on the resource.
    assert_eq!(
        err(read(format!("{root}/nao-existe.txt"))).code,
        "file_not_found"
    );
    assert_eq!(err(read(format!("{root}/sub"))).code, "not_a_file");
    assert_eq!(
        err(block(state.list(
            &op(),
            &target,
            &ruri(&endpoint_a(), &format!("{root}/notas.txt")),
            None
        )))
        .code,
        "not_a_directory"
    );
    let stat = block(state.stat(
        &op(),
        &target,
        &ruri(&endpoint_a(), &format!("{root}/notas.txt")),
    ))
    .unwrap();
    assert_eq!(
        (stat.kind, stat.size, stat.read_only),
        (FileKind::File, 34, true)
    );
}

/// Would catch: UTF-8 special names mangled (shell/batch quoting, lossy conversion) or not
/// reopenable through the URI returned by the listing.
#[test]
fn utf8_special_names_are_listed_byte_exact_and_reopened() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("nomes");
    let names = [
        "com espaco.txt",
        "aspas'simples.txt",
        "aspas\"duplas.txt",
        "glob*[a-z]?.txt",
        "barra\\invertida.txt",
        "nova\nlinha.txt",
        "tab\tulacao.txt",
        "日本語 ファイル.txt",
        "emoji-🐑.txt",
        "-comeca-com-hifen.txt",
        ".oculto",
        "acentuação.txt",
    ];
    let long = format!("{}.txt", "n".repeat(241));
    for name in names.iter().copied().chain([long.as_str()]) {
        write(&root.join(name), name.as_bytes());
    }
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let state = state_with(
        FakeLinks::with(vec![l.clone()]),
        ProcessConnector::plain(),
        &[(&endpoint_a(), &root)],
    );
    let target = target_of(&l);
    let page = block(state.list(
        &op(),
        &target,
        &ruri(&endpoint_a(), &root.display().to_string()),
        None,
    ))
    .unwrap();
    let mut got: Vec<String> = page.page.entries.iter().map(|e| e.name.clone()).collect();
    got.sort();
    let mut expected: Vec<String> = names
        .iter()
        .map(|n| n.to_string())
        .chain([long.clone()])
        .collect();
    expected.sort();
    assert_eq!(got, expected);
    assert!(page.page.next_cursor.is_none());
    for entry in &page.page.entries {
        assert_eq!(entry.uri.path, format!("{}/{}", root.display(), entry.name));
        let snapshot = block(state.read(&op(), &target, &entry.uri)).unwrap();
        assert_eq!(
            snapshot.snapshot.content, entry.name,
            "reopened by the returned uri"
        );
    }
}

// =======================================================================================
// AC-006-02 — identity, stale cache, invalidated handles, reload on a new channel
// =======================================================================================

/// Would catch: validating generation/boot/session after touching the transport, or serving a
/// host that is not online (cache treated as live state).
#[test]
fn stale_identity_or_offline_host_is_refused_before_any_io() {
    let connector = ProcessConnector::plain();
    let current = link(&endpoint_a(), "Servidor A", SESSION_A, 2, "boot-a-2");
    let links = FakeLinks::with(vec![current.clone()]);
    let state = state_with(
        links.clone(),
        connector.clone(),
        &[(&endpoint_a(), Path::new("/"))],
    );
    let file = ruri(&endpoint_a(), "/etc/hostname");
    let live = target_of(&current);
    let stale = |f: &dyn Fn(&mut RemoteFileTarget)| {
        let mut t = live.clone();
        f(&mut t);
        t
    };
    for (target, code) in [
        (
            stale(&|t| t.connection_generation = 1),
            "target_generation_stale",
        ),
        (
            stale(&|t| t.boot_id = "boot-a-1".into()),
            "target_boot_stale",
        ),
        (
            stale(&|t| t.session = SESSION_B.into()),
            "target_session_mismatch",
        ),
        (stale(&|t| t.endpoint = endpoint_b()), "file_host_unknown"),
    ] {
        assert_eq!(err(block(state.read(&op(), &target, &file))).code, code);
        assert_eq!(err(block(state.stat(&op(), &target, &file))).code, code);
    }
    // Listing hosts is presentation only: it never opens a channel.
    let hosts = state.hosts();
    assert_eq!(hosts.hosts.len(), 1);
    let host = &hosts.hosts[0];
    assert_eq!(
        (
            host.online,
            host.connection_generation,
            host.boot_id.as_deref()
        ),
        (true, Some(2), Some("boot-a-2"))
    );
    assert_eq!(host.label, "Servidor A");
    assert!(!host.capabilities.write && host.capabilities.read);
    assert_eq!(host.roots, ["/"]);
    links.lose(&endpoint_a());
    let error = err(block(state.read(&op(), &live, &file)));
    assert_eq!(error.code, "host_unavailable");
    assert!(error.retryable);
    assert!(!state.hosts().hosts[0].online);
    assert_eq!(connector.opens(), 0, "nothing reached the transport");
}

/// Would catch: reusing the channel, its handles or its paging cursors after the connection
/// was renewed, or tagging a new read with the old connection identity.
#[cfg(target_os = "linux")]
#[test]
fn reconnection_invalidates_channel_and_cursors_and_reload_uses_a_new_channel() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("grande");
    for i in 0..300 {
        write(&root.join(format!("f{i:03}.txt")), b"x");
    }
    write(&root.join("f000.txt"), b"leitura antes da queda\n");
    let connector = ProcessConnector::plain();
    let before = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let links = FakeLinks::with(vec![before.clone()]);
    let state = state_with(links.clone(), connector.clone(), &[(&endpoint_a(), &root)]);
    let dir = ruri(&endpoint_a(), &root.display().to_string());
    let file = ruri(&endpoint_a(), &format!("{}/f000.txt", root.display()));

    let first = block(state.list(&op(), &target_of(&before), &dir, None)).unwrap();
    let cursor = first.page.next_cursor.clone().expect("more pages");
    let cached = block(state.read(&op(), &target_of(&before), &file)).unwrap();
    let old_channel = state.channel(&endpoint_a()).unwrap();
    let old_pid = server_pid(connector.last_pid());
    assert_eq!(cached.connection.channel, old_channel.id);

    // The hub reports a renewed connection (new generation, same boot) and the file changes.
    write(&root.join("f000.txt"), b"leitura depois da reconexao\n");
    let after = link(&endpoint_a(), "Servidor A", SESSION_A, 2, "boot-a-1");
    links.set(after.clone());

    assert_eq!(
        err(block(state.list(
            &op(),
            &target_of(&before),
            &dir,
            Some(&cursor)
        )))
        .code,
        "target_generation_stale"
    );
    assert_eq!(
        err(block(state.read(&op(), &target_of(&before), &file))).code,
        "target_generation_stale"
    );
    assert_eq!(
        err(block(state.list(
            &op(),
            &target_of(&after),
            &dir,
            Some(&cursor)
        )))
        .code,
        "cursor_stale"
    );
    wait_gone(old_pid);

    let reloaded = block(state.read(&op(), &target_of(&after), &file)).unwrap();
    let new_channel = state.channel(&endpoint_a()).unwrap();
    assert_ne!(new_channel.id, old_channel.id);
    assert_eq!(reloaded.connection.channel, new_channel.id);
    assert_eq!(reloaded.connection.connection_generation, 2);
    assert_ne!(reloaded.snapshot.id, cached.snapshot.id);
    assert_eq!(reloaded.snapshot.content, "leitura depois da reconexao\n");
    assert_eq!(
        cached.snapshot.content, "leitura antes da queda\n",
        "the cached snapshot is immutable"
    );
    assert_eq!(connector.opens(), 2);
}

/// Would catch: retrying (replaying) the failed operation automatically on a dead channel, or
/// never opening another channel after a loss.
#[cfg(target_os = "linux")]
#[test]
fn lost_channel_fails_the_operation_and_the_next_one_opens_another_channel() {
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a.txt"), b"texto\n");
    let connector = ProcessConnector::plain();
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let state = state_with(
        FakeLinks::with(vec![l.clone()]),
        connector.clone(),
        &[(&endpoint_a(), temp.path())],
    );
    let file = ruri(
        &endpoint_a(),
        &temp.path().join("a.txt").display().to_string(),
    );
    block(state.stat(&op(), &target_of(&l), &file)).unwrap();
    let pid = connector.last_pid();
    signal(pid, "-KILL");
    wait_gone(pid);
    let error = err(block(state.read(&op(), &target_of(&l), &file)));
    assert_eq!(error.code, "connection_lost");
    assert!(error.retryable);
    assert_eq!(
        connector.opens(),
        1,
        "the failed read was not replayed on another channel"
    );
    assert_eq!(
        block(state.read(&op(), &target_of(&l), &file))
            .unwrap()
            .snapshot
            .content,
        "texto\n"
    );
    assert_eq!(connector.opens(), 2);
}

// =======================================================================================
// AC-006-03 — snapshots, limits, localized errors, channel kept alive
// =======================================================================================

/// Would catch: reusing an id for two reads, truncating at 2 MiB, or reading past the limit
/// instead of refusing it.
#[test]
fn two_snapshots_are_identified_and_the_2_mib_limit_is_inclusive() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let pattern: Vec<u8> = (0..files::sftp::MAX_TEXT_BYTES as usize)
        .map(|i| {
            if i % 100 == 99 {
                b'\n'
            } else {
                b'a' + (i % 26) as u8
            }
        })
        .collect();
    write(&root.join("exato.txt"), &pattern);
    let mut over = pattern.clone();
    over.push(b'z');
    write(&root.join("excede.txt"), &over);
    write(&root.join("v.txt"), b"primeira\nleitura\n");
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let state = state_with(
        FakeLinks::with(vec![l.clone()]),
        ProcessConnector::plain(),
        &[(&endpoint_a(), root)],
    );
    let t = target_of(&l);
    let at = |name: &str| ruri(&endpoint_a(), &root.join(name).display().to_string());

    let exact = block(state.read(&op(), &t, &at("exato.txt"))).unwrap();
    assert_eq!(exact.snapshot.size, 2_097_152);
    assert_eq!(exact.snapshot.content.as_bytes(), pattern.as_slice());
    let handles_before = state.channel(&endpoint_a()).unwrap().frames_of(FXP_HANDLE);
    assert_eq!(
        err(block(state.read(&op(), &t, &at("excede.txt")))).code,
        "file_too_large"
    );
    assert_eq!(
        state.channel(&endpoint_a()).unwrap().frames_of(FXP_HANDLE),
        handles_before,
        "refused by size before opening"
    );

    let first = block(state.read(&op(), &t, &at("v.txt"))).unwrap();
    write(&root.join("v.txt"), b"primeira\nsegunda\n");
    let second = block(state.read(&op(), &t, &at("v.txt"))).unwrap();
    assert_ne!(first.snapshot.id, second.snapshot.id);
    assert_eq!(first.snapshot.uri, second.snapshot.uri);
    assert_eq!(
        (
            first.snapshot.content.as_str(),
            second.snapshot.content.as_str()
        ),
        ("primeira\nleitura\n", "primeira\nsegunda\n")
    );
    assert_eq!(first.connection, second.connection);
}

/// Would catch: an error on one file invalidating the channel (or the host), or a peer message
/// (English OS text, path) echoed to the WebView.
#[cfg(target_os = "linux")]
#[test]
fn binary_permission_and_missing_errors_stay_on_the_resource() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    write(&root.join("bin.dat"), b"dados\x00binarios");
    write(&root.join("latin1.txt"), b"caf\xe9\n");
    write(
        &root.join("bom-crlf.txt"),
        "\u{feff}um\r\ndois\r\n".as_bytes(),
    );
    write(&root.join("segredo.txt"), b"sem permissao\n");
    std::fs::create_dir_all(root.join("fechado")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        root.join("segredo.txt"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    std::fs::set_permissions(root.join("fechado"), std::fs::Permissions::from_mode(0o000)).unwrap();
    let connector = ProcessConnector::plain();
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let state = state_with(
        FakeLinks::with(vec![l.clone()]),
        connector.clone(),
        &[(&endpoint_a(), root)],
    );
    let t = target_of(&l);
    let at = |name: &str| ruri(&endpoint_a(), &root.join(name).display().to_string());

    for (name, code) in [
        ("bin.dat", "binary_unsupported"),
        ("latin1.txt", "binary_unsupported"),
        ("segredo.txt", "permission_denied"),
    ] {
        let error = err(block(state.read(&op(), &t, &at(name))));
        assert_eq!(error.code, code, "{name}");
        assert!(!error.retryable);
        assert!(
            !error.message.contains(&root.display().to_string())
                && !error.message.contains("Permission denied"),
            "{error:?}"
        );
    }
    assert_eq!(
        err(block(state.list(&op(), &t, &at("fechado"), None))).code,
        "permission_denied"
    );
    let bom = block(state.read(&op(), &t, &at("bom-crlf.txt"))).unwrap();
    assert_eq!(
        (
            bom.snapshot.content.as_str(),
            bom.snapshot.bom,
            bom.snapshot.eol
        ),
        ("um\ndois\n", true, files::local::LineEnding::Crlf)
    );
    assert_eq!(connector.opens(), 1, "errors never replaced the channel");
    std::fs::set_permissions(
        root.join("segredo.txt"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    std::fs::set_permissions(root.join("fechado"), std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// Would catch: treating a working SSH as proof of SFTP, collapsing host-key/auth/unreachable
/// into one error, or echoing OpenSSH stderr (TASK-006-05).
#[test]
fn missing_sftp_subsystem_and_ssh_failures_are_classified_from_the_process() {
    let cases = [
        (
            "subsystem request failed on channel 0",
            "sftp_unavailable",
            false,
        ),
        (
            "Host key verification failed.",
            "ssh_host_key_unknown",
            false,
        ),
        (
            "tester@127.0.0.1: Permission denied (publickey).",
            "ssh_authentication_required",
            false,
        ),
        (
            "ssh: connect to host 127.0.0.1 port 2: Connection refused",
            "ssh_unreachable",
            true,
        ),
    ];
    for (stderr, code, retryable) in cases {
        let connector = ProcessConnector::new(
            "sh",
            vec!["-c".into(), format!("echo '{stderr}' >&2; exit 255")],
        );
        let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
        let state = state_with(
            FakeLinks::with(vec![l.clone()]),
            connector,
            &[(&endpoint_a(), Path::new("/srv"))],
        );
        let error = err(block(state.list(
            &op(),
            &target_of(&l),
            &ruri(&endpoint_a(), "/srv"),
            None,
        )));
        assert_eq!(
            (error.code.as_str(), error.retryable),
            (code, retryable),
            "{stderr}"
        );
        assert!(
            !error.message.contains("channel 0") && !error.message.contains("127.0.0.1"),
            "{error:?}"
        );
        if code == "sftp_unavailable" {
            assert!(
                error.message.contains("SSH")
                    && error.message.contains("SFTP")
                    && error.message.contains("terminals"),
                "{}",
                error.message
            );
        }
    }
}

// =======================================================================================
// Transport contract: framing, offsets, DATA size, peer errors, names, paging
// =======================================================================================

struct CountingReader {
    data: std::io::Cursor<Vec<u8>>,
}

impl tokio::io::AsyncRead for CountingReader {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let position = self.data.position() as usize;
        let remaining = &self.data.get_ref()[position..];
        let n = remaining.len().min(buf.remaining()).min(7); // tiny reads exercise reassembly
        buf.put_slice(&remaining[..n]);
        self.data.set_position((position + n) as u64);
        std::task::Poll::Ready(Ok(()))
    }
}

/// Would catch: a frame length outside 5..=1048576 reaching the codec (allocation of a hostile
/// length) or an off-by-one at either inclusive edge.
#[test]
fn frame_guard_rejects_lengths_outside_5_to_1048576_before_the_body() {
    assert_eq!((MIN_FRAME_BYTES, MAX_FRAME_BYTES), (5, 1_048_576));
    let run = |len: u32, body: usize| {
        let mut bytes = len.to_be_bytes().to_vec();
        bytes.extend(std::iter::repeat_n(0x65u8, body));
        let stats = Arc::new(FrameStats::default());
        let mut guard = FrameGuard::new(
            CountingReader {
                data: std::io::Cursor::new(bytes),
            },
            stats.clone(),
        );
        let mut out = Vec::new();
        let result = block(guard.read_to_end(&mut out));
        (result, out, stats, guard.into_inner().data.position())
    };
    for len in [0u32, 1, 4, 1_048_577, 64 * 1024 * 1024] {
        let (result, out, stats, consumed) = run(len, 32);
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{len}");
        assert!(
            out.is_empty(),
            "no byte of a rejected frame reaches the codec ({len})"
        );
        assert_eq!(consumed, 4, "only the length prefix was read ({len})");
        assert_eq!(stats.rejected(), Some(len));
    }
    for len in [5u32, 1_048_576] {
        let (result, out, stats, _) = run(len, len as usize);
        result.unwrap();
        assert_eq!(out.len(), len as usize + 4, "{len}");
        assert_eq!(stats.rejected(), None);
        assert_eq!(stats.frames_of(0x65), 1);
    }
}

/// Would catch: a hostile peer frame accepted, its text echoed, or the rejection killing more
/// than the affected channel.
#[test]
fn hostile_frame_lengths_kill_only_the_channel_and_the_next_operation_recovers() {
    for announced in [64 * 1024 * 1024, 0, 1_048_577] {
        let mut script = PeerScript::default();
        script
            .nodes
            .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
        script.nodes.insert(
            b"/raiz/ok.txt".to_vec(),
            PeerNode::File {
                content: b"ok\n".to_vec(),
                reported_size: 3,
            },
        );
        script.frame_len_on = Some((b"/raiz/hostil".to_vec(), announced));
        let (peer, _links, state, l) = peer_state(script);
        let started = Instant::now();
        let error = err(block(state.stat(
            &op(),
            &target_of(&l),
            &ruri(&endpoint_a(), "/raiz/hostil"),
        )));
        assert_eq!(error.code, "sftp_frame_rejected", "{announced}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!error.message.contains(&announced.to_string()));
        assert_eq!(
            block(state.read(&op(), &target_of(&l), &ruri(&endpoint_a(), "/raiz/ok.txt")))
                .unwrap()
                .snapshot
                .content,
            "ok\n"
        );
        assert_eq!(
            peer.opens.load(Ordering::SeqCst),
            2,
            "recovery on another channel"
        );
    }
}

/// Would catch: the library's offset (advanced by the requested length) used after short DATA
/// replies — the read would skip bytes and still succeed.
#[test]
fn short_data_replies_are_read_with_explicit_offsets() {
    let content: Vec<u8> = (0..100_000)
        .map(|i| {
            if i % 50 == 49 {
                b'\n'
            } else {
                b'a' + (i % 23) as u8
            }
        })
        .collect();
    let mut script = PeerScript::default();
    script
        .nodes
        .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
    script.nodes.insert(
        b"/raiz/curto.txt".to_vec(),
        PeerNode::File {
            content: content.clone(),
            reported_size: content.len() as u64,
        },
    );
    script.short_read = Some(1000);
    let (peer, _links, state, l) = peer_state(script);
    let snapshot = block(state.read(
        &op(),
        &target_of(&l),
        &ruri(&endpoint_a(), "/raiz/curto.txt"),
    ))
    .unwrap();
    assert_eq!(snapshot.snapshot.content.as_bytes(), content.as_slice());
    let reads: Vec<(u64, u32)> = peer
        .requests()
        .into_iter()
        .filter(|r| r.kind == FXP_READ)
        .map(|r| (r.offset, r.len))
        .collect();
    let mut expected_offset = 0;
    for (offset, _) in reads.iter().take(100) {
        assert_eq!(
            *offset, expected_offset,
            "offset follows the bytes received: {reads:?}"
        );
        expected_offset += 1000;
    }
    assert!(reads.len() >= 100);
}

/// Would catch: DATA longer than requested accepted (unbounded growth past the cap) or a file
/// that grows after STAT read past 2 MiB.
#[test]
fn oversized_data_and_growth_after_stat_are_refused() {
    let mut script = PeerScript::default();
    script
        .nodes
        .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
    script.nodes.insert(
        b"/raiz/a.txt".to_vec(),
        PeerNode::File {
            content: b"abcdef\n".to_vec(),
            reported_size: 7,
        },
    );
    script.oversize_data = 16;
    let (_peer, _links, state, l) = peer_state(script);
    assert_eq!(
        err(block(state.read(
            &op(),
            &target_of(&l),
            &ruri(&endpoint_a(), "/raiz/a.txt")
        )))
        .code,
        "sftp_protocol_error"
    );

    let mut script = PeerScript::default();
    script
        .nodes
        .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
    let grown = vec![b'g'; files::sftp::MAX_TEXT_BYTES as usize + 10];
    script.nodes.insert(
        b"/raiz/cresce.txt".to_vec(),
        PeerNode::File {
            content: grown,
            reported_size: 10,
        },
    );
    script.nodes.insert(
        b"/raiz/grande.txt".to_vec(),
        PeerNode::File {
            content: b"x".to_vec(),
            reported_size: files::sftp::MAX_TEXT_BYTES + 1,
        },
    );
    let (peer, _links, state, l) = peer_state(script);
    assert_eq!(
        err(block(state.read(
            &op(),
            &target_of(&l),
            &ruri(&endpoint_a(), "/raiz/cresce.txt")
        )))
        .code,
        "file_too_large"
    );
    assert_eq!(
        err(block(state.read(
            &op(),
            &target_of(&l),
            &ruri(&endpoint_a(), "/raiz/grande.txt")
        )))
        .code,
        "file_too_large"
    );
    let opened: Vec<Vec<u8>> = peer
        .requests()
        .into_iter()
        .filter(|r| r.kind == FXP_OPEN)
        .map(|r| r.path)
        .collect();
    assert_eq!(
        opened,
        vec![b"/raiz/cresce.txt".to_vec()],
        "grande.txt refused by STAT without OPEN"
    );
    let largest = peer
        .requests()
        .into_iter()
        .filter(|r| r.kind == FXP_READ)
        .map(|r| r.offset + r.len as u64)
        .max()
        .unwrap();
    assert!(
        largest <= files::sftp::MAX_TEXT_BYTES + 1 + 20480,
        "reading stopped at the cap: {largest}"
    );
}

/// Would catch: a STATUS with a non-UTF-8 message crashing the provider, being echoed, or
/// leaving a dead channel in place for the next operation.
#[test]
fn status_with_non_utf8_message_is_sanitized_and_recovers_on_another_channel() {
    let mut script = PeerScript::default();
    script
        .nodes
        .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
    script.nodes.insert(
        b"/raiz/ok.txt".to_vec(),
        PeerNode::File {
            content: b"ok\n".to_vec(),
            reported_size: 3,
        },
    );
    script.bad_status_on = Some(b"/raiz/status".to_vec());
    let (peer, _links, state, l) = peer_state(script);
    let error = err(block(state.stat(
        &op(),
        &target_of(&l),
        &ruri(&endpoint_a(), "/raiz/status"),
    )));
    assert_eq!(error.code, "sftp_protocol_error");
    assert!(
        !error.message.contains("SEGREDO") && !error.message.contains('\u{fffd}'),
        "{error:?}"
    );
    assert_eq!(
        block(state.read(&op(), &target_of(&l), &ruri(&endpoint_a(), "/raiz/ok.txt")))
            .unwrap()
            .snapshot
            .content,
        "ok\n"
    );
    assert_eq!(peer.opens.load(Ordering::SeqCst), 2);
}

fn good_names(prefix: &str, n: usize) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| format!("{prefix}{i:03}.txt").into_bytes())
        .collect()
}

/// Would catch: a lossy or silently partial listing when a later batch carries a non-UTF-8
/// name, or the broken channel taking another directory (or host) down with it.
#[test]
fn mixed_directory_fails_explicitly_after_more_than_one_batch_and_another_directory_recovers() {
    let mut late = good_names("a", 100);
    let mut batch3 = good_names("c", 10);
    batch3.push(b"latin1-\xe9.txt".to_vec());
    let mut early_batch2 = good_names("b", 20);
    early_batch2.push(b"latin1-\xe9.txt".to_vec());
    let mut script = PeerScript::default();
    script
        .nodes
        .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
    script.nodes.insert(
        b"/raiz/tardio".to_vec(),
        PeerNode::Dir {
            batches: vec![std::mem::take(&mut late), good_names("b", 100), batch3],
        },
    );
    script.nodes.insert(
        b"/raiz/cedo".to_vec(),
        PeerNode::Dir {
            batches: vec![good_names("a", 100), early_batch2],
        },
    );
    script.nodes.insert(
        b"/raiz/outro".to_vec(),
        PeerNode::Dir {
            batches: vec![good_names("o", 3)],
        },
    );
    let (peer, _links, state, l) = peer_state(script);
    let t = target_of(&l);

    // Bad name in batch 2 of the first page: explicit error after one full batch arrived.
    let error = err(block(state.list(
        &op(),
        &t,
        &ruri(&endpoint_a(), "/raiz/cedo"),
        None,
    )));
    assert_eq!(error.code, "remote_name_unsupported");
    assert!(!error.message.contains("latin1"));
    let recovered =
        block(state.list(&op(), &t, &ruri(&endpoint_a(), "/raiz/outro"), None)).unwrap();
    assert_eq!(recovered.page.entries.len(), 3);
    assert_eq!(peer.opens.load(Ordering::SeqCst), 2);

    // Bad name in batch 3: page 1 is complete and says more remain; page 2 fails explicitly.
    let first = block(state.list(&op(), &t, &ruri(&endpoint_a(), "/raiz/tardio"), None)).unwrap();
    assert_eq!(first.page.entries.len(), 128);
    assert!(first
        .page
        .entries
        .iter()
        .all(|e| !e.name.contains('\u{fffd}')));
    let cursor = first
        .page
        .next_cursor
        .clone()
        .expect("page 1 is not presented as complete");
    let before = state.channel(&endpoint_a()).unwrap();
    let readdirs = || {
        peer.requests()
            .into_iter()
            .filter(|r| r.kind == FXP_READDIR && r.path.starts_with(b"/raiz/tardio#"))
            .count()
    };
    assert_eq!(readdirs(), 2, "page 1 needed two READDIR batches");
    let error = err(block(state.list(
        &op(),
        &t,
        &ruri(&endpoint_a(), "/raiz/tardio"),
        Some(&cursor),
    )));
    assert_eq!(error.code, "remote_name_unsupported");
    assert_eq!(readdirs(), 3, "the failure came from the third batch");
    // The failed channel and its cursors are gone; another directory works on a new channel.
    assert_eq!(
        err(block(state.list(
            &op(),
            &t,
            &ruri(&endpoint_a(), "/raiz/tardio"),
            Some(&cursor)
        )))
        .code,
        "cursor_stale"
    );
    let again = block(state.list(&op(), &t, &ruri(&endpoint_a(), "/raiz/outro"), None)).unwrap();
    assert_eq!(again.page.entries.len(), 3);
    assert_ne!(state.channel(&endpoint_a()).unwrap().id, before.id);
}

/// Would catch: the real server's NAME decoding failure producing lossy names or a listing
/// presented as complete (the name may land in any batch on a real filesystem).
#[cfg(target_os = "linux")]
#[test]
fn real_server_mixed_directory_never_lists_lossy_or_complete() {
    use std::os::unix::ffi::OsStrExt;
    let temp = tempfile::tempdir().unwrap();
    let mixed = temp.path().join("misto");
    for i in 0..250 {
        write(&mixed.join(format!("n{i:03}.txt")), b"x");
    }
    write(
        &mixed.join(std::ffi::OsStr::from_bytes(b"latin1-\xe9.txt")),
        b"x",
    );
    write(&temp.path().join("limpo/um.txt"), b"um\n");
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let other = link(&endpoint_b(), "Servidor B", SESSION_B, 3, "boot-b-3");
    let connector = ProcessConnector::plain();
    let state = state_with(
        FakeLinks::with(vec![l.clone(), other.clone()]),
        connector.clone(),
        &[(&endpoint_a(), temp.path()), (&endpoint_b(), temp.path())],
    );
    let b_file = ruri(
        &endpoint_b(),
        &temp.path().join("limpo/um.txt").display().to_string(),
    );
    block(state.stat(&op(), &target_of(&other), &b_file)).unwrap();
    let b_channel = state.channel(&endpoint_b()).unwrap();

    let dir = ruri(&endpoint_a(), &mixed.display().to_string());
    let mut cursor: Option<String> = None;
    let mut delivered = 0;
    let outcome = loop {
        match block(state.list(&op(), &target_of(&l), &dir, cursor.as_deref())) {
            Ok(page) => {
                assert!(page
                    .page
                    .entries
                    .iter()
                    .all(|e| !e.name.contains('\u{fffd}')));
                delivered += page.page.entries.len();
                match page.page.next_cursor {
                    Some(next) => cursor = Some(next),
                    None => break Ok(delivered),
                }
            }
            Err(error) => break Err(error),
        }
    };
    let error = outcome.expect_err("a directory with a non-UTF-8 name is never complete");
    assert_eq!(error.code, "remote_name_unsupported");
    assert!(delivered < 251);
    let clean = block(state.list(
        &op(),
        &target_of(&l),
        &ruri(
            &endpoint_a(),
            &temp.path().join("limpo").display().to_string(),
        ),
        None,
    ))
    .unwrap();
    assert_eq!(clean.page.entries.len(), 1);
    assert_eq!(
        state.channel(&endpoint_b()).unwrap().id,
        b_channel.id,
        "the other host kept its channel"
    );
    assert!(block(state.stat(&op(), &target_of(&other), &b_file)).is_ok());
}

/// Would catch: pages above 128, a cursor that leaks names, a last page still announcing more
/// (exactly 128 entries), `.`/`..` listed, or a forged cursor accepted.
#[test]
fn paging_is_at_most_128_with_opaque_cursors_and_never_truncated() {
    let temp = tempfile::tempdir().unwrap();
    for i in 0..300 {
        write(
            &temp.path().join(format!("tres/arquivo-secreto-{i:03}.txt")),
            b"x",
        );
    }
    for i in 0..128 {
        write(&temp.path().join(format!("exato/e{i:03}")), b"x");
    }
    let l = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let state = state_with(
        FakeLinks::with(vec![l.clone()]),
        ProcessConnector::plain(),
        &[(&endpoint_a(), temp.path())],
    );
    let t = target_of(&l);
    let dir = ruri(
        &endpoint_a(),
        &temp.path().join("tres").display().to_string(),
    );
    let mut sizes = Vec::new();
    let mut all = std::collections::BTreeSet::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = block(state.list(&op(), &t, &dir, cursor.as_deref())).unwrap();
        sizes.push(page.page.entries.len());
        for entry in &page.page.entries {
            assert!(entry.name != "." && entry.name != "..");
            assert!(all.insert(entry.name.clone()), "duplicate {}", entry.name);
        }
        match page.page.next_cursor {
            Some(next) => {
                assert!(
                    !next.contains("secreto") && !next.contains(&temp.path().display().to_string())
                );
                cursor = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(sizes, [128, 128, 44]);
    assert_eq!(all.len(), 300);
    let exact = block(state.list(
        &op(),
        &t,
        &ruri(
            &endpoint_a(),
            &temp.path().join("exato").display().to_string(),
        ),
        None,
    ))
    .unwrap();
    assert_eq!(
        (exact.page.entries.len(), exact.page.next_cursor.clone()),
        (128, None)
    );
    assert_eq!(
        err(block(state.list(&op(), &t, &dir, Some("forjado")))).code,
        "invalid_cursor"
    );
    // A cursor is bound to its directory.
    let page = block(state.list(&op(), &t, &dir, None)).unwrap();
    let other = ruri(
        &endpoint_a(),
        &temp.path().join("exato").display().to_string(),
    );
    assert_eq!(
        err(block(state.list(
            &op(),
            &t,
            &other,
            page.page.next_cursor.as_deref()
        )))
        .code,
        "invalid_cursor"
    );
    // FileProvider::list is the full (never truncated) listing.
    assert_eq!(state.provider(t).list(&dir).unwrap().len(), 300);
}

// =======================================================================================
// Deadline, cancellation and the bounded queue
// =======================================================================================

/// Would catch: an operation without a total deadline (frozen server blocks forever), a
/// timeout that kills other hosts' channels or processes, or a dead channel kept for reuse.
#[cfg(target_os = "linux")]
#[test]
fn total_deadline_of_10000_ms_kills_only_the_affected_channel() {
    assert_eq!(OPERATION_DEADLINE, Duration::from_millis(10_000));
    let temp = tempfile::tempdir().unwrap();
    write(&temp.path().join("a.txt"), b"texto a\n");
    let connector_a = ProcessConnector::plain();
    let connector_b = ProcessConnector::plain();
    let a = link(&endpoint_a(), "Servidor A", SESSION_A, 1, "boot-a-1");
    let b = link(&endpoint_b(), "Servidor B", SESSION_B, 1, "boot-b-1");
    let state_a = state_with(
        FakeLinks::with(vec![a.clone()]),
        connector_a.clone(),
        &[(&endpoint_a(), temp.path())],
    );
    let state_b = state_with(
        FakeLinks::with(vec![b.clone()]),
        connector_b.clone(),
        &[(&endpoint_b(), temp.path())],
    );
    let file_a = ruri(
        &endpoint_a(),
        &temp.path().join("a.txt").display().to_string(),
    );
    let file_b = ruri(
        &endpoint_b(),
        &temp.path().join("a.txt").display().to_string(),
    );
    block(state_a.stat(&op(), &target_of(&a), &file_a)).unwrap();
    block(state_b.stat(&op(), &target_of(&b), &file_b)).unwrap();
    let frozen = connector_a.last_pid();
    let other = connector_b.last_pid();
    let mut terminal = std::process::Command::new("cat")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    signal(frozen, "-STOP");
    let started = Instant::now();
    let timed = {
        let state_a = state_a.clone();
        let target = target_of(&a);
        let file = file_a.clone();
        std::thread::spawn(move || block(state_a.read(&op(), &target, &file)))
    };
    std::thread::sleep(Duration::from_millis(500));
    // While A is frozen, B and the terminal keep working.
    assert_eq!(
        block(state_b.read(&op(), &target_of(&b), &file_b))
            .unwrap()
            .snapshot
            .content,
        "texto a\n"
    );
    let error = err(timed.join().unwrap());
    let elapsed = started.elapsed();
    assert_eq!(error.code, "timeout");
    assert!(error.retryable);
    assert!(
        elapsed >= Duration::from_millis(10_000) && elapsed < Duration::from_millis(11_500),
        "{elapsed:?}"
    );
    wait_gone(frozen);
    assert!(alive(other), "the other channel process survived");
    {
        use std::io::{BufRead, Write};
        let mut stdin = terminal.stdin.take().unwrap();
        stdin.write_all(b"terminal vivo\n").unwrap();
        let mut line = String::new();
        std::io::BufReader::new(terminal.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "terminal vivo\n");
        drop(stdin);
    }
    assert!(terminal.wait().unwrap().success());
    assert_eq!(
        block(state_a.read(&op(), &target_of(&a), &file_a))
            .unwrap()
            .snapshot
            .content,
        "texto a\n"
    );
    assert_eq!(connector_a.opens(), 2);
    assert_eq!(connector_b.opens(), 1);
}

/// Would catch: cancellation that waits for the peer, kills other channels, or replays the
/// cancelled request when the next operation opens a new channel.
#[test]
fn cancel_in_flight_ends_only_that_channel_and_never_replays() {
    let mut script = PeerScript::default();
    script
        .nodes
        .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
    script.nodes.insert(
        b"/raiz/lento.txt".to_vec(),
        PeerNode::File {
            content: b"lento\n".to_vec(),
            reported_size: 6,
        },
    );
    script.nodes.insert(
        b"/raiz/ok.txt".to_vec(),
        PeerNode::File {
            content: b"ok\n".to_vec(),
            reported_size: 3,
        },
    );
    script.hang_on = Some(b"/raiz/lento.txt".to_vec());
    let (peer, _links, state, l) = peer_state(script);
    let t = target_of(&l);
    block(state.stat(&op(), &t, &ruri(&endpoint_a(), "/raiz/ok.txt"))).unwrap();
    let id = op();
    let pending = {
        let (state, t, id) = (state.clone(), t.clone(), id.clone());
        std::thread::spawn(move || {
            block(state.read(&id, &t, &ruri(&endpoint_a(), "/raiz/lento.txt")))
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.pending(&endpoint_a()) == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(!state.cancel("op-desconhecida"));
    let started = Instant::now();
    assert!(state.cancel(&id));
    let error = err(pending.join().unwrap());
    assert_eq!(error.code, "operation_cancelled");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(state.pending(&endpoint_a()), 0);
    peer.release();
    assert_eq!(
        block(state.read(&op(), &t, &ruri(&endpoint_a(), "/raiz/ok.txt")))
            .unwrap()
            .snapshot
            .content,
        "ok\n"
    );
    let replayed = peer
        .requests()
        .into_iter()
        .filter(|r| r.connection == 2 && r.path.starts_with(b"/raiz/lento.txt"))
        .count();
    assert_eq!(
        replayed, 0,
        "the cancelled read was not sent again on the new channel"
    );
    assert_eq!(peer.opens.load(Ordering::SeqCst), 2);
}

/// Would catch: an unbounded queue, a 33rd operation that blocks or kills the pending ones, or
/// duplicate/malformed operation ids accepted.
#[test]
fn queue_holds_32_pending_operations_and_refuses_the_33rd_without_killing_them() {
    assert_eq!(MAX_PENDING_OPERATIONS, 32);
    let mut script = PeerScript::default();
    script
        .nodes
        .insert(b"/raiz".to_vec(), PeerNode::Dir { batches: vec![] });
    script.nodes.insert(
        b"/raiz/lento.txt".to_vec(),
        PeerNode::File {
            content: b"lento\n".to_vec(),
            reported_size: 6,
        },
    );
    script.hang_on = Some(b"/raiz/lento.txt".to_vec());
    let (peer, _links, state, l) = peer_state(script);
    let t = target_of(&l);
    block(state.stat(&op(), &t, &ruri(&endpoint_a(), "/raiz"))).unwrap();
    let workers: Vec<_> = (0..32)
        .map(|i| {
            let (state, t) = (state.clone(), t.clone());
            std::thread::spawn(move || {
                block(state.stat(
                    &format!("fila-{i}"),
                    &t,
                    &ruri(&endpoint_a(), "/raiz/lento.txt"),
                ))
            })
        })
        .collect();
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.pending(&endpoint_a()) < 32 {
        assert!(
            Instant::now() < deadline,
            "pending {}",
            state.pending(&endpoint_a())
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    let refused = err(block(state.stat(&op(), &t, &ruri(&endpoint_a(), "/raiz"))));
    assert_eq!(refused.code, "sftp_queue_full");
    assert!(refused.retryable);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(
        err(block(state.stat(
            "fila-3",
            &t,
            &ruri(&endpoint_a(), "/raiz")
        )))
        .code,
        "operation_duplicate"
    );
    for bad in ["", "com espaço", &"x".repeat(65)] {
        assert_eq!(
            err(block(state.stat(bad, &t, &ruri(&endpoint_a(), "/raiz")))).code,
            "operation_id_invalid"
        );
    }
    assert_eq!(state.pending(&endpoint_a()), 32);
    peer.release();
    for worker in workers {
        assert_eq!(worker.join().unwrap().unwrap().size, 6);
    }
    assert_eq!(
        peer.opens.load(Ordering::SeqCst),
        1,
        "the refusal killed nothing"
    );
}

// =======================================================================================
// Native E2E: real Tauri window with the local files workspace (005) and the remote files
// workspace (006) over the real IPC bridges and backends, against two disposable unprivileged
// sshd instances (own port, keys, authorized_keys and known_hosts; one with the SFTP subsystem
// in a mount namespace where the project path holds the "remote" content, one without SFTP),
// the gate's disposable local Herdr session and a disposable remote Herdr session reached
// through SSH. Flow: same path Local vs SSH → read-only remote tab with host badge → binary /
// permission / missing SFTP errors on the resource while terminals keep working → sshd
// interrupted: cached tab Desatualizado → sshd back: new generation, reload on a new channel,
// diff of the two reads → GUI closed; engines, shells and files preserved.
// =======================================================================================

#[cfg(target_os = "linux")]
mod e2e {
    use super::connections::commands::{self, ConnectionsConfig, ConnectionsState};
    use super::connections::ssh_options::IsolatedSshConfig;
    use super::files::local as files_local;
    use super::files::sftp::{
        self, OpenSshSftpConnector, RemoteFilesConfig, RemoteFilesState, OPERATION_DEADLINE,
    };
    use super::native_harness::{self, PHASE_ENV, RESULT_ENV};
    use super::window_harness;
    use herdr_client::{
        ConnectOptions, LocalGateway, RuntimeGateway, SessionName, SurfaceGeometry,
    };
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    const WINDOW_PHASE_TEST: &str = "e2e::e2e_remote_files_window";
    const ENDPOINT_A: &str = "0006aaaa0006aaaa0006aaaa0006aaaa";
    const ENDPOINT_B: &str = "0006bbbb0006bbbb0006bbbb0006bbbb";
    const LABEL_A: &str = "Servidor SSH";
    const LABEL_B: &str = "SSH sem SFTP";
    const SESSION_VARS: &[&str] = &[
        "HERDR_SOCKET_PATH",
        "HERDR_CLIENT_SOCKET_PATH",
        "HERDR_SESSION",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
        "HERDR_PANE_ID",
    ];

    /// One GUI process: a Tauri window over the built frontend whose page runs
    /// `src/features/remote-files/e2e.ts` against the real commands of 003, 005 and 006.
    #[test]
    #[ignore = "window phase of e2e_remote_files_flow; fails when run outside the harness"]
    fn e2e_remote_files_window() {
        let phase = native_harness::current_phase();
        let params: Value =
            serde_json::from_str(&native_harness::required("HERDR_DESKTOP_E2E_PARAMS")).unwrap();
        let env = |k: &str| std::env::var(k).ok();
        let isolated = IsolatedSshConfig {
            identity_file: PathBuf::from(native_harness::required(
                "HERDR_DESKTOP_E2E_SSH_IDENTITY",
            )),
            user_known_hosts_file: PathBuf::from(native_harness::required(
                "HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS",
            )),
        };
        let connections = ConnectionsState::new(ConnectionsConfig {
            prefs_dir: PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_PREFS")),
            herdr_config_dir: herdr_client::session::herdr_config_dir(&env),
            herdr_state_dir: PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_STATE")),
            local_session: Some(
                SessionName::parse(&native_harness::required("HERDR_DESKTOP_E2E_SESSION")).unwrap(),
            ),
            local_auto_start: false,
            herdr_bin: "herdr".into(),
            isolated_ssh: Some(isolated.clone()),
            geometry: SurfaceGeometry {
                cols: 100,
                rows: 30,
                cell_width_px: 9,
                cell_height_px: 18,
            },
        });
        let root = params["root"].as_str().unwrap().to_owned();
        let remote_files = RemoteFilesState::new(RemoteFilesConfig {
            links: Arc::new(connections.clone()),
            connector: Arc::new(OpenSshSftpConnector::new(Some(isolated))),
            roots: BTreeMap::from([
                (ENDPOINT_A.to_owned(), vec![root.clone()]),
                (ENDPOINT_B.to_owned(), vec![root.clone()]),
            ]),
            deadline: OPERATION_DEADLINE,
        })
        .unwrap();
        let local_files = files_local::FilesState::new(vec![PathBuf::from(&root)]).unwrap();
        let closing = connections.clone();
        let builder = tauri::Builder::default()
            .manage(connections)
            .manage(remote_files)
            .manage(local_files)
            .on_window_event(move |_window, event| {
                if matches!(
                    event,
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                ) {
                    closing.detach_all();
                }
            })
            .invoke_handler(tauri::generate_handler![
                commands::connections_list,
                commands::connections_watch,
                commands::connection_connect,
                commands::connection_send_text,
                sftp::remote_files_hosts,
                sftp::remote_files_watch,
                sftp::remote_files_list,
                sftp::remote_files_read,
                sftp::remote_files_stat,
                sftp::remote_files_cancel,
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
            "remote-files",
            &phase,
            params,
            PathBuf::from(native_harness::required(RESULT_ENV)),
            Duration::from_secs(300),
        );
        panic!("the harness window returned without reporting done");
    }

    // --- helpers ------------------------------------------------------------------------

    fn run(cmd: &mut Command, what: &str) -> std::process::Output {
        let out = cmd
            .output()
            .unwrap_or_else(|e| panic!("{what}: could not start ({e})"));
        assert!(
            out.status.success(),
            "{what} failed ({}): {}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn herdr_bin() -> String {
        std::env::var("HERDR_DESKTOP_HERDR_BIN").unwrap_or_else(|_| "herdr".into())
    }

    fn herdr_plain(config: &Path) -> Command {
        let mut command = Command::new(herdr_bin());
        for var in SESSION_VARS {
            command.env_remove(var);
        }
        command.env("HERDR_CONFIG_PATH", config);
        command
    }

    fn herdr(config: &Path, session: &str) -> Command {
        let mut command = herdr_plain(config);
        command.arg("--session").arg(session);
        command
    }

    fn session_running(session: &str) -> bool {
        let out = Command::new(herdr_bin())
            .args(["session", "list"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).lines().any(|l| {
            let mut cols = l.split_whitespace();
            cols.next() == Some(session) && cols.next() == Some("running")
        })
    }

    fn pane_text(config: &Path, session: &str, pane: &str) -> String {
        let out = herdr(config, session)
            .args(["pane", "read", pane, "--source", "recent", "--lines", "400"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// Boot id and shell pid observed through a separate metadata-only connection.
    fn observe(session: &str) -> (String, u32) {
        let env = |k: &str| std::env::var(k).ok();
        let config_dir = herdr_client::session::herdr_config_dir(&env);
        let mut gateway = LocalGateway::new(&config_dir, SessionName::parse(session).unwrap());
        gateway
            .connect(ConnectOptions {
                geometry: SurfaceGeometry {
                    cols: 80,
                    rows: 24,
                    cell_width_px: 9,
                    cell_height_px: 18,
                },
                surface_active: false,
            })
            .expect("observer attach");
        let events = gateway.take_event_stream().unwrap();
        std::thread::spawn(move || while events.recv().is_ok() {});
        let deadline = Instant::now() + Duration::from_secs(20);
        let boot = loop {
            if let Some(identity) = gateway.identity() {
                break identity.boot_id;
            }
            assert!(Instant::now() < deadline, "observer got no boot id");
            std::thread::sleep(Duration::from_millis(20));
        };
        let mut pid = None;
        for _ in 0..100 {
            pid = gateway.api().pane_shell_pid("w1:p1").unwrap();
            if pid.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        gateway.detach();
        (boot, pid.expect("w1:p1 shell pid"))
    }

    fn reap(mut child: std::process::Child) {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }

    fn process_alive(pid: u32) -> bool {
        super::alive(pid)
    }

    fn cmdline(pid: u32) -> String {
        std::fs::read(format!("/proc/{pid}/cmdline"))
            .map(|b| String::from_utf8_lossy(&b).replace('\0', " "))
            .unwrap_or_default()
    }

    fn comm(pid: u32) -> String {
        std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .map(|c| c.trim().to_owned())
            .unwrap_or_default()
    }

    fn processes() -> BTreeMap<u32, (u32, String)> {
        let mut map = BTreeMap::new();
        for entry in std::fs::read_dir("/proc").unwrap().flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some(tail) = stat.rsplit_once(')').map(|(_, t)| t.to_owned()) else {
                continue;
            };
            let ppid = tail
                .split_whitespace()
                .nth(1)
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            map.insert(pid, (ppid, cmdline(pid)));
        }
        map
    }

    fn descendants(root: u32) -> Vec<u32> {
        let table = processes();
        let mut found = vec![root];
        let mut i = 0;
        while i < found.len() {
            let parent = found[i];
            found.extend(
                table
                    .iter()
                    .filter(|(_, (ppid, _))| *ppid == parent)
                    .map(|(pid, _)| *pid),
            );
            i += 1;
        }
        found.remove(0);
        found
    }

    fn kill(pid: u32) {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status();
    }

    fn port_listening(port: u16) -> bool {
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    // --- disposable sshd ------------------------------------------------------------------

    struct SshKeys {
        base: PathBuf,
    }

    impl SshKeys {
        fn create(base: &Path) -> Self {
            for dir in ["etc", "keys"] {
                std::fs::create_dir_all(base.join(dir)).unwrap();
            }
            for (key, comment) in [
                (base.join("etc/ssh_host_ed25519_key"), "hd006-host"),
                (base.join("keys/id_ed25519"), "hd006-client"),
            ] {
                run(
                    Command::new("ssh-keygen")
                        .args(["-q", "-t", "ed25519", "-N", "", "-C", comment, "-f"])
                        .arg(&key),
                    "ssh-keygen",
                );
            }
            std::fs::copy(
                base.join("keys/id_ed25519.pub"),
                base.join("etc/authorized_keys"),
            )
            .unwrap();
            Self {
                base: base.to_path_buf(),
            }
        }

        fn identity(&self) -> PathBuf {
            self.base.join("keys/id_ed25519")
        }

        fn known_hosts(&self) -> PathBuf {
            self.base.join("etc/known_hosts")
        }

        fn trust(&self, port: u16) {
            let host_pub =
                std::fs::read_to_string(self.base.join("etc/ssh_host_ed25519_key.pub")).unwrap();
            let key: Vec<&str> = host_pub.split_whitespace().take(2).collect();
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.known_hosts())
                .unwrap();
            writeln!(file, "[127.0.0.1]:{port} {} {}", key[0], key[1]).unwrap();
        }
    }

    impl Drop for SshKeys {
        fn drop(&mut self) {
            for key in ["keys/id_ed25519", "etc/ssh_host_ed25519_key"] {
                let _ = Command::new("shred")
                    .arg("-u")
                    .arg(self.base.join(key))
                    .status();
            }
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    struct Sshd {
        base: PathBuf,
        port: u16,
        pid: Option<u32>,
    }

    impl Sshd {
        /// `subsystem`: the `Subsystem sftp` command, or `None` for an sshd without SFTP.
        fn create(name: &str, keys: &SshKeys, subsystem: Option<&Path>) -> Self {
            let base = keys.base.join(name);
            for dir in ["run", "log"] {
                std::fs::create_dir_all(base.join(dir)).unwrap();
            }
            let port = free_port();
            let user = String::from_utf8(run(Command::new("id").arg("-un"), "id -un").stdout)
                .unwrap()
                .trim()
                .to_owned();
            let k = keys.base.display();
            let b = base.display();
            let sftp_line = subsystem
                .map(|path| format!("Subsystem sftp {}\n", path.display()))
                .unwrap_or_default();
            std::fs::write(
                base.join("sshd_config"),
                format!(
                    "Port {port}\nListenAddress 127.0.0.1\nAddressFamily inet\nHostKey {k}/etc/ssh_host_ed25519_key\nPidFile {b}/run/sshd.pid\nAuthorizedKeysFile {k}/etc/authorized_keys\nAllowUsers {user}\nPubkeyAuthentication yes\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin no\nUsePAM no\nStrictModes no\nX11Forwarding no\nAllowAgentForwarding no\nAllowTcpForwarding no\nPermitTunnel no\nLoginGraceTime 30\nLogLevel VERBOSE\n{sftp_line}PerSourcePenalties no\n"
                ),
            )
            .unwrap();
            keys.trust(port);
            let sshd = Self {
                base,
                port,
                pid: None,
            };
            run(
                Command::new(sshd.binary())
                    .arg("-t")
                    .arg("-f")
                    .arg(sshd.config()),
                "sshd -t",
            );
            sshd
        }

        fn binary(&self) -> &'static str {
            ["/usr/bin/sshd", "/usr/sbin/sshd"]
                .into_iter()
                .find(|p| Path::new(p).exists())
                .expect("sshd binary")
        }

        fn config(&self) -> PathBuf {
            self.base.join("sshd_config")
        }

        fn start(&mut self) {
            let _ = std::fs::remove_file(self.base.join("run/sshd.pid"));
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.base.join("log/sshd.log"))
                .unwrap();
            Command::new("setsid")
                .arg(self.binary())
                .args(["-D", "-e", "-f"])
                .arg(self.config())
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .map(reap)
                .expect("start sshd");
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let pid = std::fs::read_to_string(self.base.join("run/sshd.pid"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());
                if let Some(pid) = pid {
                    if port_listening(self.port) {
                        assert!(
                            cmdline(pid).contains(&self.config().display().to_string()),
                            "pid {pid} is not our sshd"
                        );
                        self.pid = Some(pid);
                        return;
                    }
                }
                assert!(Instant::now() < deadline, "sshd did not start");
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// Descendant `sftp-server` processes currently served by this sshd.
        fn sftp_servers(&self) -> Vec<u32> {
            self.pid
                .map(|pid| {
                    descendants(pid)
                        .into_iter()
                        .filter(|p| comm(*p) == "sftp-server")
                        .collect()
                })
                .unwrap_or_default()
        }

        /// Kills the listener and every connection it serves (not the Herdr servers).
        fn stop(&mut self) -> Vec<String> {
            let Some(pid) = self.pid.take() else {
                return Vec::new();
            };
            assert!(cmdline(pid).contains(&self.config().display().to_string()));
            let children = descendants(pid);
            let killed: Vec<String> = children.iter().map(|p| cmdline(*p)).collect();
            kill(pid);
            for child in &children {
                kill(*child);
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while port_listening(self.port)
                || process_alive(pid)
                || children.iter().any(|c| process_alive(*c))
            {
                assert!(Instant::now() < deadline, "sshd did not stop");
                std::thread::sleep(Duration::from_millis(50));
            }
            killed
        }
    }

    impl Drop for Sshd {
        fn drop(&mut self) {
            self.stop();
        }
    }

    // --- disposable remote session ----------------------------------------------------------

    struct RemoteSession {
        name: String,
        config: PathBuf,
        work: PathBuf,
        log: PathBuf,
    }

    impl RemoteSession {
        fn start(&self) {
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.log)
                .unwrap();
            let mut command = Command::new("setsid");
            command.arg(herdr_bin());
            for var in SESSION_VARS {
                command.env_remove(var);
            }
            command
                .env("HERDR_CONFIG_PATH", &self.config)
                .env("HERDR_STARTUP_CWD", &self.work)
                .args(["--session", &self.name, "server"])
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .map(reap)
                .expect("start remote herdr session");
            let deadline = Instant::now() + Duration::from_secs(15);
            while !herdr(&self.config, &self.name)
                .args(["pane", "list"])
                .output()
                .is_ok_and(|o| o.status.success())
            {
                assert!(Instant::now() < deadline, "remote session did not start");
                std::thread::sleep(Duration::from_millis(100));
            }
            let panes: Value = serde_json::from_slice(
                &run(
                    herdr(&self.config, &self.name).args(["pane", "list"]),
                    "pane list",
                )
                .stdout,
            )
            .unwrap();
            if panes["result"]["panes"]
                .as_array()
                .is_none_or(|p| p.is_empty())
            {
                run(
                    herdr(&self.config, &self.name)
                        .args(["workspace", "create", "--focus", "--cwd"])
                        .arg(&self.work),
                    "workspace create",
                );
            }
            let _ = herdr(&self.config, &self.name)
                .args([
                    "pane",
                    "wait-output",
                    "--timeout",
                    "10000",
                    "--regex",
                    "[$#>%] ?$",
                    "w1:p1",
                ])
                .output();
        }

        fn stop(&self) {
            let _ = herdr_plain(&self.config)
                .args(["session", "stop", &self.name])
                .output();
            let deadline = Instant::now() + Duration::from_secs(10);
            while session_running(&self.name) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    impl Drop for RemoteSession {
        fn drop(&mut self) {
            self.stop();
            let _ = herdr_plain(&self.config)
                .args(["session", "delete", &self.name])
                .output();
        }
    }

    fn read_report(path: &Path) -> Option<Value> {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
    }

    fn our_ssh_clients(identity: &Path) -> Vec<String> {
        let needle = identity.display().to_string();
        processes()
            .into_values()
            .map(|(_, cmd)| cmd)
            .filter(|cmd| cmd.starts_with("ssh ") && cmd.contains(&needle))
            .collect()
    }

    fn tree_bytes(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current).unwrap().flatten() {
                let path = entry.path();
                let meta = std::fs::symlink_metadata(&path).unwrap();
                let rel = path.strip_prefix(dir).unwrap().display().to_string();
                if meta.is_dir() {
                    stack.push(path);
                } else if meta.permissions().mode_readable() {
                    out.insert(rel, std::fs::read(&path).unwrap());
                } else {
                    out.insert(
                        rel,
                        format!("mode {:o}", meta.permissions().mode_bits()).into_bytes(),
                    );
                }
            }
        }
        out
    }

    trait ModeExt {
        fn mode_readable(&self) -> bool;
        fn mode_bits(&self) -> u32;
    }

    impl ModeExt for std::fs::Permissions {
        fn mode_readable(&self) -> bool {
            use std::os::unix::fs::PermissionsExt;
            self.mode() & 0o400 != 0
        }
        fn mode_bits(&self) -> u32 {
            use std::os::unix::fs::PermissionsExt;
            self.mode() & 0o7777
        }
    }

    fn texts(value: &Value) -> Vec<String> {
        value
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| v.as_str().unwrap_or_default().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    #[ignore = "needs the disposable session created by scripts/feature-harness/session.sh and local sshd instances; run by just check-spec 006"]
    fn e2e_remote_files_flow() {
        use std::os::unix::fs::PermissionsExt;

        let local = native_harness::required("HERDR_DESKTOP_E2E_SESSION");
        let pane = native_harness::required("HERDR_DESKTOP_E2E_PANE");
        let dir = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_DIR"));
        let report = PathBuf::from(native_harness::required("HERDR_DESKTOP_E2E_REPORT"));
        assert!(
            local.starts_with("hd006-"),
            "never run against a non-disposable session: {local}"
        );
        assert_eq!(pane, "w1:p1", "premise: local pane id");
        std::fs::create_dir_all(&report).unwrap();
        let config = dir.join("config.toml");
        let stamp = std::process::id();
        let mut log = std::fs::File::create(report.join("e2e-remote-files.log")).unwrap();
        let mut note = |line: String| {
            eprintln!("{line}");
            writeln!(log, "{line}").unwrap();
        };

        // Fixture: the project path is the same on both sides; the SSH host shows other content.
        let root = dir.join("work/projeto");
        let remote_tree = dir.join("host-remoto");
        let local_notes = b"conteudo LOCAL desta maquina\n".to_vec();
        let remote_notes_1 = b"linha um remota\nlinha dois remota\nlinha tres\n".to_vec();
        let remote_notes_2 =
            b"linha um remota\nlinha dois ALTERADA\nlinha tres\nlinha quatro\n".to_vec();
        std::fs::create_dir_all(root.join("so-local")).unwrap();
        std::fs::write(root.join("notas.txt"), &local_notes).unwrap();
        std::fs::write(
            root.join("so-local/local.txt"),
            b"apenas neste computador\n",
        )
        .unwrap();
        std::fs::create_dir_all(remote_tree.join("sub")).unwrap();
        std::fs::write(remote_tree.join("notas.txt"), &remote_notes_1).unwrap();
        std::fs::write(remote_tree.join("sub/remoto.txt"), b"apenas no host SSH\n").unwrap();
        std::fs::write(remote_tree.join("bin.dat"), b"dados\x00binarios\xff").unwrap();
        std::fs::write(
            remote_tree.join("segredo.txt"),
            b"sem permissao de leitura\n",
        )
        .unwrap();
        std::fs::set_permissions(
            remote_tree.join("segredo.txt"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        let local_before = tree_bytes(&root);

        // Disposable SSH: keys, sshd A (SFTP in a mount namespace), sshd B (no SFTP).
        let keys = SshKeys::create(&dir.join("ssh"));
        let uid = String::from_utf8(run(Command::new("id").arg("-u"), "id -u").stdout).unwrap();
        let gid = String::from_utf8(run(Command::new("id").arg("-g"), "id -g").stdout).unwrap();
        let wrapper = keys.base.join("etc/sftp-remoto.sh");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\n# Test-only SFTP subsystem: the project path shows the remote tree (mount namespace).\nexec unshare -rm sh -c 'mount --bind \"$1\" \"$2\" && exec unshare --user --map-user={} --map-group={} {}' sftp-remoto '{}' '{}'\n",
                uid.trim(),
                gid.trim(),
                super::sftp_server_bin(),
                remote_tree.display(),
                root.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut sshd_a = Sshd::create("sshd-a", &keys, Some(&wrapper));
        let mut sshd_b = Sshd::create("sshd-b", &keys, None);
        sshd_a.start();
        sshd_b.start();
        let known_hosts_before = std::fs::read(keys.known_hosts()).unwrap();

        let remote = RemoteSession {
            name: format!("{local}-r"),
            config: config.clone(),
            work: dir.join("work-remote"),
            log: dir.join("remote-server.log"),
        };
        std::fs::create_dir_all(&remote.work).unwrap();
        remote.start();
        let (local_boot, local_shell) = observe(&local);
        let (remote_boot, remote_shell) = observe(&remote.name);
        assert_ne!(local_boot, remote_boot, "premise: distinct engines");
        note(format!(
            "local={local} boot={local_boot} shell={local_shell}; remote={} boot={remote_boot} shell={remote_shell}; sshd A 127.0.0.1:{} (sftp in mount namespace) pid {:?}; sshd B 127.0.0.1:{} (no sftp) pid {:?}; root={}",
            remote.name, sshd_a.port, sshd_a.pid, sshd_b.port, sshd_b.pid, root.display()
        ));

        let user = String::from_utf8(run(Command::new("id").arg("-un"), "id").stdout)
            .unwrap()
            .trim()
            .to_owned();
        let prefs = tempfile::tempdir().unwrap();
        std::fs::write(
            prefs.path().join("connections.json"),
            serde_json::to_vec_pretty(&json!({
                "version": 1,
                "profiles": [
                    {"id": ENDPOINT_A, "label": LABEL_A, "target": format!("{user}@127.0.0.1"), "port": sshd_a.port, "session": remote.name},
                    {"id": ENDPOINT_B, "label": LABEL_B, "target": format!("{user}@127.0.0.1"), "port": sshd_b.port, "session": remote.name},
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let state_dir = dir.join("tui-state");
        std::fs::create_dir_all(&state_dir).unwrap();
        let markers = json!({
            "a_errors": format!("HD006_A_ERR_{stamp}"),
            "b_nosftp": format!("HD006_B_NOSFTP_{stamp}"),
            "local": format!("HD006_LOCAL_{stamp}"),
            "a_reconnected": format!("HD006_A_RECON_{stamp}"),
        });
        let params = json!({
            "root": root.display().to_string(),
            "endpoint_a": ENDPOINT_A,
            "endpoint_b": ENDPOINT_B,
            "label_a": LABEL_A,
            "label_b": LABEL_B,
            "markers": markers,
        });

        // GUI process.
        let result_path = report.join("e2e-remote-files-window.json");
        let _ = std::fs::remove_file(&result_path);
        let window_log = std::fs::File::create(report.join("e2e-remote-files-window.log")).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                WINDOW_PHASE_TEST,
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PHASE_ENV, "flow")
            .env(RESULT_ENV, &result_path)
            .env("HERDR_DESKTOP_E2E_PARAMS", params.to_string())
            .env("HERDR_DESKTOP_E2E_PREFS", prefs.path())
            .env("HERDR_DESKTOP_E2E_STATE", &state_dir)
            .env("HERDR_DESKTOP_E2E_SESSION", &local)
            .env("HERDR_DESKTOP_E2E_SSH_IDENTITY", keys.identity())
            .env("HERDR_DESKTOP_E2E_SSH_KNOWN_HOSTS", keys.known_hosts())
            .stdout(window_log.try_clone().unwrap())
            .stderr(window_log)
            .spawn()
            .expect("spawn GUI phase");
        let gui_pid = child.id();
        note(format!("GUI pid {gui_pid}"));

        let mut handled_read = false;
        let mut handled_stale = false;
        let mut sftp_servers_while_online = Vec::new();
        let mut killed_on_interrupt = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(330);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline, "GUI phase did not finish");
            let step = read_report(&result_path)
                .and_then(|r| r["step"].as_str().map(str::to_owned))
                .unwrap_or_default();
            if step == "read" && !handled_read {
                handled_read = true;
                sftp_servers_while_online = sshd_a.sftp_servers();
                std::fs::write(remote_tree.join("notas.txt"), &remote_notes_2).unwrap();
                killed_on_interrupt = sshd_a.stop();
                note(format!(
                    "SSH A interrupted after the first read: remote notas.txt changed; sshd A and {} connection processes stopped (sftp-server while online: {:?})",
                    killed_on_interrupt.len(), sftp_servers_while_online
                ));
            }
            if step == "stale" && !handled_stale {
                handled_stale = true;
                sshd_a.start();
                note(format!("sshd A back on pid {:?}", sshd_a.pid));
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let r = read_report(&result_path).expect("final window report");
        note(format!("GUI exited {status}; window report: {r}"));
        assert!(status.success(), "GUI phase failed: {r}");
        assert!(r["error"].is_null(), "{r}");
        assert!(
            handled_read && handled_stale,
            "coordination steps not reached"
        );

        let root_str = root.display().to_string();
        let notes_path = format!("{root_str}/notas.txt");

        // Empty state: the selected host offline shows onboarding, nothing listed.
        assert_eq!(r["empty"]["onboarding"], "host_offline", "{}", r["empty"]);
        assert_eq!(r["empty"]["refresh_disabled"], true);
        assert_eq!(r["empty"]["entries"], json!(0));

        // AC-006-01: same path, different contents; only the SSH host is listed/read remotely.
        let remote_names = texts(&r["remote_tree"]["names"]);
        assert!(
            remote_names.contains(&"sub".to_owned())
                && remote_names.contains(&"bin.dat".to_owned()),
            "{remote_names:?}"
        );
        assert!(
            !remote_names.contains(&"so-local".to_owned()),
            "local-only entry listed on SSH: {remote_names:?}"
        );
        let local_names = texts(&r["local_tree"]["names"]);
        assert!(
            local_names.contains(&"so-local".to_owned())
                && !local_names.contains(&"bin.dat".to_owned()),
            "{local_names:?}"
        );
        assert_eq!(r["remote_tab"]["path"], notes_path.as_str());
        assert_eq!(
            r["local_tab"]["path"],
            notes_path.as_str(),
            "same path on both sides"
        );
        assert_eq!(
            r["remote_tab"]["text"],
            String::from_utf8(remote_notes_1.clone()).unwrap()
        );
        assert_eq!(
            r["local_tab"]["text"],
            String::from_utf8(local_notes.clone()).unwrap()
        );
        let tab_badge = r["remote_tab"]["badge"].as_str().unwrap();
        assert!(
            tab_badge.contains(LABEL_A) && tab_badge.contains("Somente leitura"),
            "{tab_badge}"
        );
        assert!(!tab_badge.contains("Desatualizado"), "{tab_badge}");
        let header_badge = r["remote_tree"]["header_badge"].as_str().unwrap();
        assert!(
            header_badge.contains(LABEL_A) && header_badge.contains("Somente leitura"),
            "{header_badge}"
        );
        assert_eq!(
            r["remote_tab"]["save_buttons"],
            json!(0),
            "no save in the remote workspace"
        );
        assert_eq!(r["remote_tab"]["editor_read_only"], true);
        assert_eq!(r["remote_tab"]["editable"], "false");
        assert_eq!(
            r["local_tab"]["save_buttons"],
            json!(1),
            "the local workspace keeps its save"
        );
        assert_eq!(
            r["local_tab"]["typed_changed"], true,
            "the typing probe edits an editable editor"
        );
        assert_eq!(
            r["remote_tab"]["typed_ignored"], true,
            "typing into the remote editor changed nothing"
        );

        // AC-006-03: errors stay on the resource; missing SFTP explained; terminals usable.
        assert_eq!(r["errors"]["binary"]["code"], "binary_unsupported");
        assert_eq!(r["errors"]["permission"]["code"], "permission_denied");
        assert_eq!(r["errors"]["host_a_online"], true);
        assert_eq!(r["errors"]["tree_entries"], json!(remote_names.len()));
        let nosftp = &r["nosftp"];
        assert_eq!(nosftp["code"], "sftp_unavailable");
        assert!(
            nosftp["hint"]
                .as_str()
                .unwrap()
                .contains("SSH funcionando não garante SFTP"),
            "{nosftp}"
        );
        assert_eq!(
            nosftp["host_online"], true,
            "SSH to host B works while SFTP is missing"
        );
        assert_eq!(nosftp["tabs_kept"], json!(3));
        for (session, marker) in [
            (&remote.name, &markers["a_errors"]),
            (&remote.name, &markers["b_nosftp"]),
            (&remote.name, &markers["a_reconnected"]),
            (&local, &markers["local"]),
        ] {
            let text = pane_text(&config, session, "w1:p1");
            assert!(
                text.contains(marker.as_str().unwrap()),
                "{marker} missing in {session} w1:p1"
            );
        }
        assert!(
            !pane_text(&config, &local, "w1:p1").contains(markers["a_errors"].as_str().unwrap())
        );
        note("terminals: SSH A marker after file errors, SSH B marker with SFTP missing, local marker and SSH A marker after reconnection all reached their panes".into());

        // AC-006-02: cached read shown as Desatualizado while lost; renewed connection requires a
        // reload, which reads the new content on a new channel; diff of the two identified reads.
        assert!(
            !sftp_servers_while_online.is_empty(),
            "the remote tab was read through sftp-server of sshd A"
        );
        let stale = &r["stale"];
        assert_eq!(stale["host_online"], false);
        assert_eq!(stale["reason"], "host_offline");
        assert_eq!(
            stale["text"],
            String::from_utf8(remote_notes_1.clone()).unwrap(),
            "cache kept, not live"
        );
        assert!(stale["badge"].as_str().unwrap().contains("Desatualizado"));
        assert_eq!(stale["reload_disabled"], true);
        let renewed = &r["renewed"];
        assert_eq!(renewed["reason"], "connection_renewed");
        assert!(
            renewed["generation"].as_u64().unwrap()
                > r["remote_tab"]["generation"].as_u64().unwrap()
        );
        assert_eq!(
            renewed["text"],
            String::from_utf8(remote_notes_1.clone()).unwrap(),
            "no silent refresh"
        );
        assert_eq!(renewed["tree_stale"], true);
        let reloaded = &r["reloaded"];
        assert_eq!(
            reloaded["text"],
            String::from_utf8(remote_notes_2.clone()).unwrap()
        );
        assert_eq!(reloaded["stale"], false);
        assert_ne!(reloaded["label"], r["remote_tab"]["label"]);
        assert!(reloaded["label"]
            .as_str()
            .unwrap()
            .ends_with(&format!("geração {}", renewed["generation"])));
        let diff = &r["diff"];
        assert_eq!(
            diff["sources"],
            json!([r["remote_tab"]["label"], reloaded["label"]])
        );
        assert_eq!(diff["removed"], json!(["linha dois remota"]));
        assert_eq!(
            diff["added"],
            json!(["linha dois ALTERADA", "linha quatro"])
        );
        assert_eq!(diff["summary"], "+2 −1");
        assert_eq!(r["tree_after_refresh"]["stale"], false);

        // GUI closed: no SSH client or sftp-server left; engines/shells alive; files untouched.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !our_ssh_clients(&keys.identity()).is_empty() || !sshd_a.sftp_servers().is_empty() {
            assert!(
                Instant::now() < deadline,
                "ssh clients {:?} / sftp-server {:?} left behind",
                our_ssh_clients(&keys.identity()),
                sshd_a.sftp_servers()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!process_alive(gui_pid));
        assert!(session_running(&local) && session_running(&remote.name));
        assert!(
            process_alive(local_shell) && process_alive(remote_shell),
            "a shell died"
        );
        assert_eq!(observe(&local).0, local_boot);
        assert_eq!(
            observe(&remote.name).0,
            remote_boot,
            "the GUI restarted the remote server"
        );
        assert_eq!(
            tree_bytes(&root),
            local_before,
            "the local project was not modified"
        );
        std::fs::set_permissions(
            remote_tree.join("segredo.txt"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(remote_tree.join("notas.txt")).unwrap(),
            remote_notes_2
        );
        assert_eq!(
            std::fs::read(remote_tree.join("segredo.txt")).unwrap(),
            b"sem permissao de leitura\n"
        );
        assert_eq!(
            std::fs::read(keys.known_hosts()).unwrap(),
            known_hosts_before,
            "known_hosts untouched"
        );
        note("after GUI exit: no ssh client or sftp-server left; sessions running; shells alive; boots unchanged; local tree, remote files and known_hosts byte-identical (except the harness change)".into());

        std::fs::write(
            report.join("e2e-remote-files-summary.json"),
            serde_json::to_vec_pretty(&json!({
                "local_session": local,
                "remote_session": remote.name,
                "local_boot": local_boot,
                "remote_boot": remote_boot,
                "root": root_str,
                "endpoints": { "sftp": ENDPOINT_A, "no_sftp": ENDPOINT_B },
                "gui_pid": gui_pid,
                "empty": r["empty"],
                "remote_tree": r["remote_tree"],
                "local_tree": r["local_tree"],
                "remote_tab": r["remote_tab"],
                "local_tab": r["local_tab"],
                "errors": r["errors"],
                "nosftp": r["nosftp"],
                "sftp_servers_while_online": sftp_servers_while_online,
                "processes_stopped_with_sshd_a": killed_on_interrupt.len(),
                "stale": r["stale"],
                "renewed": r["renewed"],
                "reloaded": r["reloaded"],
                "diff": r["diff"],
                "terminal_markers": markers,
            }))
            .unwrap(),
        )
        .unwrap();
        drop(remote);
        drop(sshd_a);
        drop(sshd_b);
        drop(keys);
    }
}
