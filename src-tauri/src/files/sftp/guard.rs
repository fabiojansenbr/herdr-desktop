//! Inbound framing guard and kill switch for one SFTP channel (private helpers of spec 006).
//!
//! [`FrameGuard`] sits between the transport's stdout and the SFTP codec of
//! `openssh-sftp-client`. It only reads the u32 length prefix of each SFTP frame and refuses
//! lengths outside [`MIN_FRAME_BYTES`]..=[`MAX_FRAME_BYTES`] before any byte of that frame (or
//! an allocation of its announced size) reaches the codec. Bodies are passed through unchanged;
//! the first body byte (the packet type) is counted so a dead channel can be classified without
//! decoding anything. It is not a codec.
//!
//! [`KillSwitch`] ends the stream from our side (cancel/deadline/identity change) for any
//! transport, including in-memory ones, so the codec's background tasks stop portably.

use std::collections::BTreeMap;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Smallest valid SFTP frame body: type byte + u32 request id.
pub const MIN_FRAME_BYTES: u32 = 5;
/// Largest inbound SFTP frame accepted (1 MiB, inclusive).
pub const MAX_FRAME_BYTES: u32 = 1_048_576;

const HEADER: usize = 4;
const CHUNK: usize = 64 * 1024;

/// What crossed the guard on one channel.
#[derive(Debug, Default)]
pub struct FrameStats {
    by_type: Mutex<BTreeMap<u8, u64>>,
    last_type: Mutex<Option<u8>>,
    rejected: Mutex<Option<u32>>,
    ended: AtomicBool,
}

impl FrameStats {
    /// Length of the frame that was refused, if any.
    pub fn rejected(&self) -> Option<u32> {
        *self.rejected.lock().expect("frame stats")
    }

    /// Frames of one SFTP packet type that reached the codec.
    pub fn frames_of(&self, kind: u8) -> u64 {
        self.by_type
            .lock()
            .expect("frame stats")
            .get(&kind)
            .copied()
            .unwrap_or(0)
    }

    /// Packet type of the latest frame that started reaching the codec.
    pub fn last_type(&self) -> Option<u8> {
        *self.last_type.lock().expect("frame stats")
    }

    pub fn by_type(&self) -> BTreeMap<u8, u64> {
        self.by_type.lock().expect("frame stats").clone()
    }

    /// True once the transport itself ended: EOF or an I/O error on either pipe. A codec that
    /// dies while this is false rejected what it received (protocol/format failure).
    pub fn transport_ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    fn end(&self) {
        self.ended.store(true, Ordering::SeqCst);
    }

    fn frame(&self, kind: u8) {
        *self
            .by_type
            .lock()
            .expect("frame stats")
            .entry(kind)
            .or_default() += 1;
        *self.last_type.lock().expect("frame stats") = Some(kind);
    }

    fn reject(&self, len: u32) {
        *self.rejected.lock().expect("frame stats") = Some(len);
    }
}

/// Ends a guarded stream from our side and wakes the task reading it.
#[derive(Debug, Default)]
pub struct KillSwitch {
    killed: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl KillSwitch {
    pub fn kill(&self) {
        self.killed.store(true, Ordering::SeqCst);
        if let Some(waker) = self.waker.lock().expect("kill switch").take() {
            waker.wake();
        }
    }

    pub fn is_killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    fn check(&self, cx: &Context<'_>) -> bool {
        *self.waker.lock().expect("kill switch") = Some(cx.waker().clone());
        self.is_killed()
    }
}

/// `AsyncRead` adapter enforcing the inbound frame limits (see module docs).
pub struct FrameGuard<R> {
    inner: R,
    stats: Arc<FrameStats>,
    kill: Option<Arc<KillSwitch>>,
    header: [u8; HEADER],
    header_have: usize,
    header_sent: usize,
    body_left: u32,
    type_pending: bool,
    failed: bool,
    chunk: Box<[u8]>,
}

impl<R> FrameGuard<R> {
    pub fn new(inner: R, stats: Arc<FrameStats>) -> Self {
        Self {
            inner,
            stats,
            kill: None,
            header: [0; HEADER],
            header_have: 0,
            header_sent: HEADER,
            body_left: 0,
            type_pending: false,
            failed: false,
            chunk: vec![0u8; CHUNK].into_boxed_slice(),
        }
    }

    pub fn with_kill(inner: R, stats: Arc<FrameStats>, kill: Arc<KillSwitch>) -> Self {
        let mut guard = Self::new(inner, stats);
        guard.kill = Some(kill);
        guard
    }

    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for FrameGuard<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.kill.as_ref().is_some_and(|kill| kill.check(cx)) {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "sftp channel closed locally",
            )));
        }
        if this.failed {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "sftp frame rejected",
            )));
        }
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            if this.body_left == 0 && this.header_sent == HEADER {
                let mut header = ReadBuf::new(&mut this.header[this.header_have..]);
                match Pin::new(&mut this.inner).poll_read(cx, &mut header) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Err(error)) => {
                        this.stats.end();
                        return Poll::Ready(Err(error));
                    }
                    Poll::Ready(Ok(())) => {}
                }
                let n = header.filled().len();
                if n == 0 {
                    // End of stream; a partial header surfaces as EOF in the codec.
                    this.stats.end();
                    return Poll::Ready(Ok(()));
                }
                this.header_have += n;
                if this.header_have < HEADER {
                    continue;
                }
                let len = u32::from_be_bytes(this.header);
                if !(MIN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&len) {
                    this.failed = true;
                    this.stats.reject(len);
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "sftp frame length outside the accepted range",
                    )));
                }
                this.header_have = 0;
                this.header_sent = 0;
                this.body_left = len;
                this.type_pending = true;
            }
            if this.header_sent < HEADER {
                let n = (HEADER - this.header_sent).min(buf.remaining());
                buf.put_slice(&this.header[this.header_sent..this.header_sent + n]);
                this.header_sent += n;
                return Poll::Ready(Ok(()));
            }
            let want = (this.body_left as usize)
                .min(buf.remaining())
                .min(this.chunk.len());
            let mut body = ReadBuf::new(&mut this.chunk[..want]);
            match Pin::new(&mut this.inner).poll_read(cx, &mut body) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => {
                    this.stats.end();
                    return Poll::Ready(Err(error));
                }
                Poll::Ready(Ok(())) => {}
            }
            let n = body.filled().len();
            if n == 0 {
                this.stats.end();
                return Poll::Ready(Ok(()));
            }
            if this.type_pending {
                this.type_pending = false;
                this.stats.frame(this.chunk[0]);
            }
            buf.put_slice(&this.chunk[..n]);
            this.body_left -= n as u32;
            return Poll::Ready(Ok(()));
        }
    }
}

/// Outbound side of a channel: records a failed write (broken pipe) as the end of the transport.
pub struct WriteGuard<W> {
    inner: W,
    stats: Arc<FrameStats>,
}

impl<W> WriteGuard<W> {
    pub fn new(inner: W, stats: Arc<FrameStats>) -> Self {
        Self { inner, stats }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for WriteGuard<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_write(cx, buf);
        if matches!(result, Poll::Ready(Err(_))) {
            this.stats.end();
        }
        result
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let result = Pin::new(&mut this.inner).poll_flush(cx);
        if matches!(result, Poll::Ready(Err(_))) {
            this.stats.end();
        }
        result
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}
