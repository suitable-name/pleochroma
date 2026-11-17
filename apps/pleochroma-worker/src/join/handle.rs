//! A join that can be started and stopped from the window: [`JoinHandle`], its live
//! [`JoinStatus`], and the shared state the slot threads write into.
//!
//! The headless `join` command uses the same state through [`super::run`], so a Ctrl-C or
//! SIGTERM stops both the same way: the stop flag is set and every live socket is shut down,
//! which ends each slot's blocked read at once.

use std::{
    net::{Shutdown, TcpStream},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// A write of at least this many bytes counts as "rendering" (pongs and requests are smaller).
const BIG_WRITE_BYTES: usize = 1024;

/// How long after a big write a slot still counts as rendering.
const RENDERING_HOLD: Duration = Duration::from_secs(3);

/// A slot that is dialling or registering.
const SLOT_CONNECTING: u8 = 0;
/// A slot that is registered with the coordinator.
const SLOT_JOINED: u8 = 1;
/// A slot that is waiting to dial again (or has stopped).
const SLOT_WAITING: u8 = 2;

/// Where a join is, as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinPhase {
    /// Starting up or dialling for the first time; nothing joined yet.
    Connecting,
    /// Joined and waiting for work.
    Joined,
    /// Joined and sending rendered pictures to the coordinator.
    Rendering,
    /// No slot is joined: the connection was lost or could not be made; it tries again.
    Reconnecting,
    /// Stopped (asked to, or ended with a problem).
    Stopped,
}

/// What one slot is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotPhase {
    /// Dialling or registering.
    Connecting,
    /// Registered, waiting for a request.
    Idle,
    /// Registered and sending pictures.
    Rendering,
    /// Waiting to dial again.
    Reconnecting,
}

/// A snapshot of a join for the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinStatus {
    /// Where the join is as a whole.
    pub phase: JoinPhase,
    /// The last problem in plain words; empty when nothing is wrong (or a slot is joined).
    pub error: String,
    /// What each slot is doing, in slot order.
    pub slots: Vec<SlotPhase>,
    /// The coordinator address a slot is joined to (the first joined slot's), or empty.
    pub joined_address: String,
    /// How long the longest-joined slot has been joined.
    pub joined_for: Option<Duration>,
}

/// Locks `mutex`, ignoring poison (a panicking slot must not take the status down with it).
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One slot's live state.
pub(super) struct Slot {
    epoch: Instant,
    state: AtomicU8,
    /// Milliseconds after `epoch` of the last big write, plus one; `0` is never.
    big_write_at: AtomicU64,
    joined_since: Mutex<Option<Instant>>,
    joined_address: Mutex<String>,
    socket: Mutex<Option<TcpStream>>,
    stopped: AtomicBool,
}

impl Slot {
    fn new() -> Self {
        Self {
            epoch: Instant::now(),
            state: AtomicU8::new(SLOT_CONNECTING),
            big_write_at: AtomicU64::new(0),
            joined_since: Mutex::new(None),
            joined_address: Mutex::new(String::new()),
            socket: Mutex::new(None),
            stopped: AtomicBool::new(false),
        }
    }

    /// The slot starts a connection attempt.
    pub(super) fn connecting(&self) {
        self.state.store(SLOT_CONNECTING, Ordering::Relaxed);
    }

    /// The slot registered with the coordinator at `address`.
    pub(super) fn joined(&self, address: &str) {
        *locked(&self.joined_since) = Some(Instant::now());
        address.clone_into(&mut locked(&self.joined_address));
        self.state.store(SLOT_JOINED, Ordering::Relaxed);
    }

    /// The slot is between connections.
    pub(super) fn waiting(&self) {
        self.state.store(SLOT_WAITING, Ordering::Relaxed);
        *locked(&self.joined_since) = None;
        *locked(&self.socket) = None;
    }

    /// Notes that `bytes` were written to the coordinator.
    pub(super) fn note_write(&self, bytes: usize) {
        if bytes >= BIG_WRITE_BYTES {
            let ms = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX - 1);
            self.big_write_at
                .store(ms.saturating_add(1), Ordering::Relaxed);
        }
    }

    /// Keeps a handle on the connection's socket so a stop can shut it down. If the join was
    /// stopped meanwhile, the socket is shut down at once.
    pub(super) fn attach_socket(&self, tcp: &TcpStream) {
        *locked(&self.socket) = tcp.try_clone().ok();
        if self.stopped.load(Ordering::Relaxed) {
            self.shutdown();
        }
    }

    /// Marks the slot stopped and shuts its socket down.
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        self.shutdown();
    }

    fn shutdown(&self) {
        let _ = locked(&self.socket)
            .as_ref()
            .map(|tcp| tcp.shutdown(Shutdown::Both));
    }

    fn rendering(&self) -> bool {
        let at = self.big_write_at.load(Ordering::Relaxed);
        if at == 0 {
            return false;
        }
        let now = u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
        let held = u64::try_from(RENDERING_HOLD.as_millis()).unwrap_or(u64::MAX);
        now.saturating_sub(at - 1) < held
    }

    fn phase(&self) -> SlotPhase {
        match self.state.load(Ordering::Relaxed) {
            SLOT_JOINED if self.rendering() => SlotPhase::Rendering,
            SLOT_JOINED => SlotPhase::Idle,
            SLOT_WAITING => SlotPhase::Reconnecting,
            _ => SlotPhase::Connecting,
        }
    }
}

/// The state a join shares between its slot threads, its supervisor and the window.
pub(super) struct Shared {
    /// Set once a stop is requested.
    pub(super) stop: AtomicBool,
    /// Set when the supervisor has ended (all slots finished, or the start failed).
    pub(super) finished: AtomicBool,
    pub(super) slots: Vec<Arc<Slot>>,
    error: Mutex<String>,
}

impl Shared {
    /// State for `slots` slots (at least one).
    pub(super) fn new(slots: usize) -> Self {
        Self {
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            slots: (0..slots.max(1)).map(|_| Arc::new(Slot::new())).collect(),
            error: Mutex::new(String::new()),
        }
    }

    /// Asks every slot to end: sets the flag and shuts the sockets down.
    pub(super) fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        for slot in &self.slots {
            slot.stop();
        }
    }

    /// Whether a stop was requested.
    pub(super) fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Records the problem the window shows (an empty text clears it).
    pub(super) fn set_error(&self, text: impl Into<String>) {
        *locked(&self.error) = text.into();
    }

    /// A snapshot for the window.
    pub(super) fn status(&self) -> JoinStatus {
        let slots: Vec<SlotPhase> = self.slots.iter().map(|slot| slot.phase()).collect();
        let joined = slots
            .iter()
            .filter(|p| matches!(p, SlotPhase::Idle | SlotPhase::Rendering))
            .count();
        let rendering = slots.iter().filter(|p| **p == SlotPhase::Rendering).count();
        let waiting = slots
            .iter()
            .filter(|p| **p == SlotPhase::Reconnecting)
            .count();
        let phase = if self.stopping() || self.finished.load(Ordering::Relaxed) {
            JoinPhase::Stopped
        } else if rendering > 0 {
            JoinPhase::Rendering
        } else if joined > 0 {
            JoinPhase::Joined
        } else if waiting > 0 {
            JoinPhase::Reconnecting
        } else {
            JoinPhase::Connecting
        };
        let (joined_for, joined_address) = self
            .slots
            .iter()
            .filter(|slot| slot.state.load(Ordering::Relaxed) == SLOT_JOINED)
            .filter_map(|slot| {
                let since = (*locked(&slot.joined_since))?;
                Some((since.elapsed(), locked(&slot.joined_address).clone()))
            })
            .max_by_key(|(elapsed, _)| *elapsed)
            .map_or((None, String::new()), |(elapsed, address)| {
                (Some(elapsed), address)
            });
        JoinStatus {
            phase,
            error: if joined > 0 {
                String::new()
            } else {
                locked(&self.error).clone()
            },
            slots,
            joined_address,
            joined_for,
        }
    }
}

/// A running (or finished) join, started with [`super::start`].
///
/// Dropping the handle stops the join, so closing the window never leaves a worker behind.
pub struct JoinHandle {
    pub(super) shared: Arc<Shared>,
}

impl JoinHandle {
    /// What the join is doing now. Cheap; callable every second.
    #[must_use]
    pub fn status(&self) -> JoinStatus {
        self.shared.status()
    }

    /// Asks the join to end. Returns at once; the slots end within moments (a picture that is
    /// half sent is dropped, the coordinator hands the tile to another computer).
    pub fn stop(&self) {
        self.shared.request_stop();
    }

    /// Whether every thread of the join has ended.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.shared.finished.load(Ordering::Relaxed)
    }
}

impl Drop for JoinHandle {
    fn drop(&mut self) {
        self.shared.request_stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_join_is_connecting_and_a_stop_ends_it() {
        let shared = Shared::new(2);
        let status = shared.status();
        assert_eq!(status.phase, JoinPhase::Connecting);
        assert_eq!(status.slots, vec![SlotPhase::Connecting; 2]);
        shared.request_stop();
        assert_eq!(shared.status().phase, JoinPhase::Stopped);
    }

    #[test]
    fn joined_slots_show_idle_then_rendering_after_a_big_write() {
        let shared = Shared::new(2);
        shared.slots[0].joined("10.0.0.5:7880");
        let status = shared.status();
        assert_eq!(status.phase, JoinPhase::Joined);
        assert_eq!(status.joined_address, "10.0.0.5:7880");
        assert_eq!(status.slots, vec![SlotPhase::Idle, SlotPhase::Connecting]);
        // A pong is small; a picture is not.
        shared.slots[0].note_write(40);
        assert_eq!(shared.status().phase, JoinPhase::Joined);
        shared.slots[0].note_write(64 * 1024);
        assert_eq!(shared.status().phase, JoinPhase::Rendering);
    }

    #[test]
    fn with_no_slot_joined_the_problem_shows_and_with_one_it_hides() {
        let shared = Shared::new(1);
        shared.slots[0].waiting();
        shared.set_error("Can't reach the coordinator.");
        let status = shared.status();
        assert_eq!(status.phase, JoinPhase::Reconnecting);
        assert_eq!(status.error, "Can't reach the coordinator.");
        shared.slots[0].joined("10.0.0.5:7880");
        assert_eq!(shared.status().error, "");
    }

    #[test]
    fn a_finished_join_reads_as_stopped() {
        let shared = Shared::new(1);
        shared.finished.store(true, Ordering::Relaxed);
        assert_eq!(shared.status().phase, JoinPhase::Stopped);
    }
}
