//! Bounded gateway event queue shared by every transport (local socket, SSH bridge).
//!
//! Producers (reader, writer and health threads) never block on it:
//! - presentation frames are bounded by bytes ([`PRESENTATION_QUEUE_BUDGET`]) and by count
//!   ([`PRESENTATION_QUEUE_FRAMES`]); a frame that does not fit is dropped and one
//!   [`GatewayEvent::QueueOverflow`] notice is arranged at once, so a burst that ends still
//!   triggers recovery without waiting for a later frame;
//! - control events are never dropped: when the channel is full they are held in order (frames
//!   never overtake them) and moved in as the consumer takes events. A producer asks
//!   [`EventSender::backlogged`] to stop reading its transport while the held backlog is full;
//! - the budget of an event is returned only once the consumer has taken it, including the
//!   rendezvous hand-off of [`EventStream::into_receiver`], so a paused consumer bounds the queue.
//!
//! The stream ends once every producer is gone and the held events were delivered.

use std::collections::VecDeque;
use std::sync::mpsc::{
    sync_channel, Receiver, RecvError, RecvTimeoutError, SyncSender, TryRecvError, TrySendError,
};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use crate::contracts::GatewayEvent;

/// Presentation queue byte budget per endpoint (PRD: 8 MiB).
pub const PRESENTATION_QUEUE_BUDGET: usize = 8 * 1024 * 1024;
/// Presentation frames queued at most (bytes are bounded separately by the budget above).
pub const PRESENTATION_QUEUE_FRAMES: usize = 512;
/// Queue slots reserved for control events, and the control events held when those are taken
/// too. Beyond it a producer stops reading its transport (server backpressure).
pub const CONTROL_BACKLOG: usize = 64;

struct Slot {
    /// Encoded bytes of a presentation frame; `None` for control events and notices.
    frame_bytes: Option<usize>,
    event: GatewayEvent,
}

enum Held {
    /// Overflow notice; the dropped count is filled in when the consumer takes it.
    Notice,
    Control(GatewayEvent),
}

struct State {
    /// Dropped once producers are gone and nothing is held, so the stream can end.
    tx: Option<SyncSender<Slot>>,
    held: VecDeque<Held>,
    held_controls: usize,
    queued_bytes: usize,
    /// Frames not yet taken by the consumer (including an adapter hand-off).
    queued_frames: usize,
    dropped_frames: usize,
    /// Notices in the channel or hand-off, not yet received by the consumer.
    notices_queued: usize,
    producers: usize,
    frames: usize,
    budget: usize,
}

impl State {
    /// Moves held events into free slots, in order. Returns false when the consumer is gone.
    fn flush(&mut self) -> bool {
        let Some(tx) = self.tx.clone() else {
            return false;
        };
        while let Some(front) = self.held.pop_front() {
            let (event, notice) = match front {
                Held::Notice => (GatewayEvent::QueueOverflow { dropped_frames: 0 }, true),
                Held::Control(event) => (event, false),
            };
            match tx.try_send(Slot {
                frame_bytes: None,
                event,
            }) {
                Ok(()) if notice => self.notices_queued += 1,
                Ok(()) => self.held_controls -= 1,
                Err(TrySendError::Full(slot)) => {
                    self.held.push_front(if notice {
                        Held::Notice
                    } else {
                        Held::Control(slot.event)
                    });
                    return true;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.held.clear();
                    self.held_controls = 0;
                    self.tx = None;
                    return false;
                }
            }
        }
        if self.producers == 0 {
            self.tx = None;
        }
        true
    }

    /// Counts one dropped frame and arranges one notice for it right away. Drops are covered
    /// by a notice still held last, or by one queued-but-unreceived when nothing is held.
    fn dropped_frame(&mut self) {
        self.dropped_frames += 1;
        let covered = matches!(self.held.back(), Some(Held::Notice))
            || (self.held.is_empty() && self.notices_queued > 0);
        if !covered {
            self.held.push_back(Held::Notice);
        }
    }
}

struct Inner {
    state: Mutex<State>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Creates a queue with the PRD limits.
pub fn event_queue() -> (EventSender, EventStream) {
    event_queue_with(PRESENTATION_QUEUE_BUDGET, PRESENTATION_QUEUE_FRAMES)
}

fn event_queue_with(budget: usize, frames: usize) -> (EventSender, EventStream) {
    let (tx, rx) = sync_channel(frames + CONTROL_BACKLOG);
    let inner = Arc::new(Inner {
        state: Mutex::new(State {
            tx: Some(tx),
            held: VecDeque::new(),
            held_controls: 0,
            queued_bytes: 0,
            queued_frames: 0,
            dropped_frames: 0,
            notices_queued: 0,
            producers: 1,
            frames,
            budget,
        }),
    });
    (
        EventSender {
            inner: inner.clone(),
        },
        EventStream { rx, inner },
    )
}

/// Producer handle; cloneable across the threads of one connection.
pub struct EventSender {
    inner: Arc<Inner>,
}

impl Clone for EventSender {
    fn clone(&self) -> Self {
        self.inner.state().producers += 1;
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl Drop for EventSender {
    fn drop(&mut self) {
        let mut state = self.inner.state();
        state.producers -= 1;
        if state.producers == 0 && state.held.is_empty() {
            state.tx = None;
        }
    }
}

impl EventSender {
    /// One presentation frame of `bytes` encoded bytes: queued, or dropped with a notice.
    /// Returns false when the consumer is gone.
    pub fn frame(&self, bytes: usize, event: GatewayEvent) -> bool {
        let mut state = self.inner.state();
        let Some(tx) = state.tx.clone() else {
            return false;
        };
        if !state.held.is_empty()
            || state.queued_bytes.saturating_add(bytes) > state.budget
            || state.queued_frames >= state.frames
        {
            state.dropped_frame();
        } else {
            match tx.try_send(Slot {
                frame_bytes: Some(bytes),
                event,
            }) {
                Ok(()) => {
                    state.queued_bytes += bytes;
                    state.queued_frames += 1;
                }
                Err(TrySendError::Full(_)) => state.dropped_frame(),
                Err(TrySendError::Disconnected(_)) => {
                    state.tx = None;
                    return false;
                }
            }
        }
        state.flush()
    }

    /// One control event, never dropped; held in order while the channel is full.
    pub fn control(&self, event: GatewayEvent) -> bool {
        let mut state = self.inner.state();
        state.held_controls += 1;
        state.held.push_back(Held::Control(event));
        state.flush()
    }

    /// Informational event delivered only if it fits now without overtaking held events.
    pub fn offer(&self, event: GatewayEvent) -> bool {
        let mut state = self.inner.state();
        let Some(tx) = state.tx.clone() else {
            return false;
        };
        if !state.held.is_empty() {
            return true;
        }
        match tx.try_send(Slot {
            frame_bytes: None,
            event,
        }) {
            Ok(()) | Err(TrySendError::Full(_)) => true,
            Err(TrySendError::Disconnected(_)) => {
                state.tx = None;
                false
            }
        }
    }

    /// True while the held control backlog is full: stop reading the transport.
    pub fn backlogged(&self) -> bool {
        self.inner.state().held_controls >= CONTROL_BACKLOG
    }

    /// Retries held events. Returns false when the consumer is gone.
    pub fn flush(&self) -> bool {
        self.inner.state().flush()
    }
}

/// Consumer side: returns the queue budget as events are taken.
pub struct EventStream {
    rx: Receiver<Slot>,
    inner: Arc<Inner>,
}

impl EventStream {
    /// Fills an overflow notice with every frame dropped in its episode so far.
    fn taken(&self, slot: Slot) -> (Option<usize>, bool, GatewayEvent) {
        match slot.event {
            GatewayEvent::QueueOverflow { .. } => {
                let dropped_frames = std::mem::take(&mut self.inner.state().dropped_frames);
                (None, true, GatewayEvent::QueueOverflow { dropped_frames })
            }
            event => (slot.frame_bytes, false, event),
        }
    }

    /// The consumer received the event: its budget is returned and held events move in.
    fn received(inner: &Inner, frame_bytes: Option<usize>, notice: bool) {
        let mut state = inner.state();
        if let Some(bytes) = frame_bytes {
            state.queued_bytes -= bytes;
            state.queued_frames -= 1;
        }
        if notice {
            state.notices_queued -= 1;
        }
        let _ = state.flush();
    }

    fn deliver(&self, slot: Slot) -> GatewayEvent {
        let (frame_bytes, notice, event) = self.taken(slot);
        Self::received(&self.inner, frame_bytes, notice);
        event
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<GatewayEvent, RecvTimeoutError> {
        let slot = self.rx.recv_timeout(timeout)?;
        Ok(self.deliver(slot))
    }

    pub fn recv(&self) -> Result<GatewayEvent, RecvError> {
        let slot = self.rx.recv()?;
        Ok(self.deliver(slot))
    }

    pub fn try_recv(&self) -> Result<GatewayEvent, TryRecvError> {
        let slot = self.rx.try_recv()?;
        Ok(self.deliver(slot))
    }

    /// Adapts to a plain receiver through a rendezvous hand-off: the budget of an event is
    /// returned only once the consumer has taken it, so a paused consumer still bounds the queue.
    pub fn into_receiver(self) -> Option<Receiver<GatewayEvent>> {
        let (tx, rx) = sync_channel(0);
        std::thread::Builder::new()
            .name("herdr-desktop-event-adapter".into())
            .spawn(move || {
                while let Ok(slot) = self.rx.recv() {
                    let (frame_bytes, notice, event) = self.taken(slot);
                    let sent = tx.send(event);
                    Self::received(&self.inner, frame_bytes, notice);
                    if sent.is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(n: usize) -> GatewayEvent {
        GatewayEvent::ShellError(format!("controle-{n}"))
    }

    /// Would catch: a notice that waits for a later frame, a byte budget not enforced, or held
    /// controls stranded after the consumer drains the channel.
    #[test]
    fn notice_is_queued_at_the_drop_and_held_controls_follow_in_order() {
        let (sender, stream) = event_queue_with(100, 2);
        assert!(sender.frame(60, error(0)));
        assert!(sender.frame(60, error(1)), "second frame exceeds 100 bytes");
        match stream.try_recv().unwrap() {
            GatewayEvent::ShellError(m) => assert_eq!(m, "controle-0"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            stream.try_recv().unwrap(),
            GatewayEvent::QueueOverflow { dropped_frames: 1 }
        ));
        for n in 0..(2 + CONTROL_BACKLOG + 3) {
            assert!(sender.control(error(100 + n)));
        }
        assert!(!sender.backlogged(), "3 held, below the backlog");
        drop(sender);
        let mut got = Vec::new();
        while let Ok(event) = stream.recv_timeout(Duration::from_millis(50)) {
            got.push(event);
        }
        assert_eq!(got.len(), 2 + CONTROL_BACKLOG + 3);
        assert!(
            matches!(&got[CONTROL_BACKLOG + 4], GatewayEvent::ShellError(m) if m == "controle-168")
        );
        assert!(matches!(
            stream.recv_timeout(Duration::from_millis(50)),
            Err(RecvTimeoutError::Disconnected)
        ));
    }
}
