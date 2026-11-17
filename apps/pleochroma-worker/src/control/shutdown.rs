//! The pieces `serve` uses to stop what it started.
//!
//! One stop mechanism for every accept loop ([`ListenerStop`]), a registry of live sockets that can
//! all be closed at once ([`ConnectionSet`]) and a one-shot signal a thread raises when it ends
//! ([`Latch`]).
//!
//! # How an accept loop is stopped
//!
//! A loop blocked in `TcpListener::accept` cannot be told anything. [`ListenerStop::stop`]
//! therefore does two things in order: it raises the shared flag, then it connects to the
//! listener's own address once. That connection makes `accept` return, the loop looks at
//! [`ListenerStop::is_stopped`] right after every accept (before it touches the new
//! stream), sees the flag and returns, which drops the listener and frees the port. Every
//! accept loop in the worker (viewer, worker port, enrollment) stops this way.
//!
//! # Closing connections
//!
//! Connection threads are not asked to stop through a flag: a thread blocked reading a
//! socket never looks at one. [`ConnectionSet`] keeps a second handle to every accepted
//! socket and [`ConnectionSet::close_all`] shuts all of them down at the operating-system
//! level, which makes every blocked read or write return an error, so the thread unwinds
//! and its guard removes the entry.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpStream},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// How long the wake-up connection of [`ListenerStop::stop`] may take to connect.
const WAKE_TIMEOUT: Duration = Duration::from_millis(500);

/// Stops one accept loop: a shared flag the loop checks after every accept, and the address
/// [`Self::stop`] connects to so a blocked `accept` returns.
///
/// Clones share the flag. The loop owner keeps one clone and reads
/// [`Self::is_stopped`]; whoever stops the listener keeps another and calls [`Self::stop`].
#[derive(Debug, Clone)]
pub struct ListenerStop {
    /// Raised by [`Self::stop`]; the accept loop returns when it sees it set.
    pub flag: Arc<AtomicBool>,
    /// The listener's own bound address. An unspecified address (`0.0.0.0`, `::`) is
    /// reached through loopback.
    pub wake_addr: SocketAddr,
}

impl ListenerStop {
    /// A new, not yet raised stop for the listener bound at `wake_addr`.
    #[must_use]
    pub fn new(wake_addr: SocketAddr) -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            wake_addr,
        }
    }

    /// Whether [`Self::stop`] has been called on this stop or any clone of it.
    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// Raises the flag, then connects once to [`Self::wake_addr`] (short timeout, every
    /// error ignored) so a blocking `accept` returns and the loop sees the flag.
    ///
    /// Safe to call more than once and when the listener is already gone: the connect
    /// then simply fails.
    pub fn stop(&self) {
        self.flag.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&reachable(self.wake_addr), WAKE_TIMEOUT);
    }
}

/// `addr`, with an unspecified host replaced by the loopback address of the same family.
const fn reachable(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), addr.port())
        }
        _ => addr,
    }
}

/// What a [`ConnectionSet`] knows about its live sockets.
#[derive(Debug, Default)]
struct SetState {
    /// The key of the next registration.
    next: u64,
    /// Set by [`ConnectionSet::close_all`]; later registrations are closed on arrival.
    closed: bool,
    /// One extra handle per live connection.
    streams: HashMap<u64, TcpStream>,
}

/// The shared half of a [`ConnectionSet`] and its guards.
#[derive(Debug, Default)]
struct SetShared {
    state: Mutex<SetState>,
    /// Notified when the last entry leaves.
    emptied: Condvar,
}

impl SetShared {
    fn lock(&self) -> MutexGuard<'_, SetState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The live sockets of one listener, so all of them can be closed together.
///
/// Clones share the set. See the module doc comment for why sockets are closed rather than
/// asked to stop.
#[derive(Debug, Clone, Default)]
pub struct ConnectionSet {
    shared: Arc<SetShared>,
}

/// Keeps one connection in its [`ConnectionSet`] while it is alive; dropping it removes the
/// entry (and the extra socket handle with it).
#[derive(Debug)]
#[must_use = "dropping the guard removes the connection from its set at once"]
pub struct ConnectionGuard {
    shared: Arc<SetShared>,
    id: Option<u64>,
}

impl ConnectionSet {
    /// An empty, open set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `stream` and returns the guard that removes it again.
    ///
    /// A stream whose handle cannot be duplicated is simply not tracked. A stream
    /// registered after [`Self::close_all`] is shut down at once.
    pub fn register(&self, stream: &TcpStream) -> ConnectionGuard {
        let id = stream.try_clone().ok().and_then(|extra| {
            let mut state = self.shared.lock();
            if state.closed {
                let _ = extra.shutdown(Shutdown::Both);
                return None;
            }
            let id = state.next;
            state.next += 1;
            state.streams.insert(id, extra);
            drop(state);
            Some(id)
        });
        ConnectionGuard {
            shared: Arc::clone(&self.shared),
            id,
        }
    }

    /// How many connections are live.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared.lock().streams.len()
    }

    /// Whether no connection is live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Shuts every live socket down in both directions and refuses later registrations.
    /// Threads blocked on them return an I/O error and end.
    pub fn close_all(&self) {
        let mut state = self.shared.lock();
        state.closed = true;
        for stream in state.streams.values() {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    /// Waits until the set is empty or `deadline` passes. Returns whether it is empty.
    #[must_use]
    pub fn wait_empty(&self, deadline: Instant) -> bool {
        let mut state = self.shared.lock();
        loop {
            if state.streams.is_empty() {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .shared
                .emptied
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let Some(id) = self.id else {
            return;
        };
        let now_empty = {
            let mut state = self.shared.lock();
            state.streams.remove(&id);
            state.streams.is_empty()
        };
        if now_empty {
            self.shared.emptied.notify_all();
        }
    }
}

/// What a [`Latch`] holds.
#[derive(Debug, Default)]
struct LatchState {
    set: bool,
}

/// A signal that is raised once and never lowered: a thread raises it when it ends, any
/// number of others wait for it. Clones share the signal.
#[derive(Debug, Clone, Default)]
pub struct Latch {
    inner: Arc<(Mutex<LatchState>, Condvar)>,
}

/// Raises its [`Latch`] when dropped, so the signal also fires when the thread panics.
#[derive(Debug)]
#[must_use = "the latch is raised when this guard is dropped"]
pub struct LatchGuard {
    latch: Latch,
}

impl Latch {
    /// A latch that is not raised yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, LatchState> {
        self.inner.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Raises the latch and wakes every waiter.
    pub fn set(&self) {
        self.lock().set = true;
        self.inner.1.notify_all();
    }

    /// Whether the latch has been raised.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.lock().set
    }

    /// Blocks until the latch is raised.
    pub fn wait(&self) {
        let mut state = self.lock();
        while !state.set {
            state = self
                .inner
                .1
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
    }

    /// Blocks until the latch is raised or `deadline` passes. Returns whether it is raised.
    #[must_use]
    pub fn wait_until(&self, deadline: Instant) -> bool {
        let mut state = self.lock();
        loop {
            if state.set {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            state = self
                .inner
                .1
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// A guard that raises this latch when it is dropped; hold it for a thread's whole
    /// body.
    pub fn exit_guard(&self) -> LatchGuard {
        LatchGuard {
            latch: self.clone(),
        }
    }
}

impl Drop for LatchGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            tracing::error!("a pleochroma-worker serve thread panicked");
        }
        self.latch.set();
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionSet, Latch, ListenerStop};
    use std::{
        io::Read,
        net::{TcpListener, TcpStream},
        thread,
        time::{Duration, Instant},
    };

    /// Runs an accept loop shaped like the real ones; ends when the stop is raised.
    fn spawn_loop(listener: TcpListener, stop: &ListenerStop) -> Latch {
        let ended = Latch::new();
        let guard = ended.exit_guard();
        let stop = stop.clone();
        thread::spawn(move || {
            let _guard = guard;
            for incoming in listener.incoming() {
                if stop.is_stopped() {
                    return;
                }
                drop(incoming);
            }
        });
        ended
    }

    #[test]
    fn stop_unblocks_a_blocking_accept_and_frees_the_port() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = ListenerStop::new(addr);
        let ended = spawn_loop(listener, &stop);
        assert!(!stop.is_stopped());
        assert!(!ended.is_set(), "the loop blocks in accept until stopped");

        stop.stop();
        assert!(stop.is_stopped());
        assert!(
            ended.wait_until(Instant::now() + Duration::from_secs(5)),
            "the accept loop must end within the deadline"
        );
        TcpListener::bind(addr).expect("the port is free once the loop has ended");
    }

    #[test]
    fn stop_reaches_a_listener_bound_to_the_unspecified_address() {
        let listener = TcpListener::bind("0.0.0.0:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = ListenerStop::new(addr);
        let ended = spawn_loop(listener, &stop);
        stop.stop();
        assert!(ended.wait_until(Instant::now() + Duration::from_secs(5)));
    }

    #[test]
    fn a_clone_shares_the_flag_and_a_second_stop_is_harmless() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let stop = ListenerStop::new(listener.local_addr().unwrap());
        let other = stop.clone();
        let ended = spawn_loop(listener, &stop);
        other.stop();
        assert!(stop.is_stopped());
        assert!(ended.wait_until(Instant::now() + Duration::from_secs(5)));
        // The listener is gone; the wake-up connect fails and is ignored.
        stop.stop();
    }

    #[test]
    fn close_all_ends_a_blocked_read_and_the_guard_empties_the_set() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();

        let set = ConnectionSet::new();
        let guard = set.register(&server);
        assert_eq!(set.len(), 1);

        let reader = thread::spawn(move || {
            let mut server = server;
            let mut byte = [0_u8; 1];
            // Blocks: the client never writes.
            let result = server.read(&mut byte);
            drop(guard);
            result
        });
        thread::sleep(Duration::from_millis(100));
        set.close_all();
        let outcome = reader.join().unwrap();
        assert!(
            matches!(outcome, Ok(0) | Err(_)),
            "a closed socket never delivers data: {outcome:?}"
        );
        assert!(set.wait_empty(Instant::now() + Duration::from_secs(5)));
        assert!(set.is_empty());
        drop(client);
    }

    #[test]
    fn a_connection_registered_after_close_all_is_closed_on_arrival() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();

        let set = ConnectionSet::new();
        set.close_all();
        let _guard = set.register(&server);
        assert!(set.is_empty(), "a late registration is not kept");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut byte = [0_u8; 1];
        assert!(matches!(client.read(&mut byte), Ok(0) | Err(_)));
    }

    #[test]
    fn wait_empty_times_out_while_a_connection_lives() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let set = ConnectionSet::new();
        let _guard = set.register(&server);
        assert!(!set.wait_empty(Instant::now() + Duration::from_millis(50)));
    }

    #[test]
    fn a_latch_guard_raises_the_latch_even_when_the_thread_panics() {
        let latch = Latch::new();
        let guard = latch.exit_guard();
        let worker = thread::spawn(move || {
            let _guard = guard;
            // Panics on purpose: the guard must still raise the latch.
            panic!("test panic inside a guarded thread");
        });
        assert!(worker.join().is_err());
        assert!(latch.is_set());
        latch.wait();
    }
}
