//! The connection caps: [`ConnectionLimiter`] (`--max-connections`, one instance per
//! listener role -- viewers and joined workers are counted separately) and its RAII
//! [`ConnectionSlot`]. See `crate::serve`'s "Robustness" section.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Caps how many connections this worker handles at once (`--max-connections`, default 64).
///
/// One instance per listener ROLE, created by [`super::start`]: the viewer listener's is
/// shared by every accepted viewer connection regardless of transport (TLS or
/// `--insecure-no-tls`) or build mode (library-only or `worker`); the coordinator's
/// worker port gets a second, separate instance (separate pools for viewers and
/// workers), held for as long as a joined worker stays registered.
///
/// `Clone` (cheap: `active` is already an `Arc`) so a listener's own accept loop and its
/// spawned connection threads can share one counter. The enrollment listener (`crate::enroll`)
/// builds its own pair instead of sharing the viewer or worker one: an unauthenticated flood
/// of bare TCP connects to the enrollment port must not exhaust slots meant for
/// authenticated viewers and workers on other ports.
#[derive(Debug, Clone)]
pub struct ConnectionLimiter {
    active: Arc<AtomicUsize>,
    max: usize,
}

impl ConnectionLimiter {
    /// `pub(crate)` so other listeners' tests (such as `crate::enroll`'s) can build one
    /// without a full `serve` startup.
    #[must_use]
    pub(crate) fn new(max: usize) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            max,
        }
    }

    /// Attempts to reserve one connection slot for a just-accepted socket.
    ///
    /// `Ok` hands back a [`ConnectionSlot`] that releases the slot when dropped --
    /// including when the connection thread's `catch_unwind`-wrapped handler panics:
    /// `Drop::drop` still runs while a panic unwinds (just not through an `abort`), and
    /// nothing in the connection-handling path aborts. `Err` carries the active count seen
    /// when the attempt was refused, which [`super::connection::refuse_for_capacity`] reports.
    /// The slot this call reserved to observe that count is released at once, so a refused
    /// connection never counts against the cap.
    ///
    /// # Errors
    ///
    /// `Err(active)` when this cap is already reached, carrying the active count
    /// observed at the moment of refusal -- not a "hard" error, just this call's own
    /// verdict for the caller to act on (typically [`super::connection::refuse_for_capacity`]).
    pub fn try_acquire(&self) -> Result<ConnectionSlot, usize> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        if active > self.max {
            self.active.fetch_sub(1, Ordering::SeqCst);
            Err(active - 1)
        } else {
            Ok(ConnectionSlot {
                active: Arc::clone(&self.active),
            })
        }
    }
}

/// How many bare, not-yet-authenticated connections the accept loop may have in flight, as a
/// multiple of `--max-connections`.
///
/// [`super::start`] builds a second, separate [`ConnectionLimiter`] with this larger cap.
///
/// The real slot is reserved only after `accept_tls` succeeds, not in the accept loop before
/// it: reserving earlier would let bare TCP connects that never send a `ClientHello` hold every
/// slot for up to [`super::HANDSHAKE_TIMEOUT`], locking out authenticated viewers. Such a peer
/// can still pin a thread until that timeout, so the wider cap bounds those threads. Four times
/// the real cap leaves room for a burst of viewers reconnecting at once.
pub const PRE_AUTH_HANDSHAKE_MULTIPLIER: usize = 4;

/// RAII handle for one reserved [`ConnectionLimiter`] slot.
///
/// Moved into the connection thread with everything else that connection needs, and held
/// for the connection handler's lifetime. [`Drop::drop`] releases the slot however that call
/// returns: normally, via `?`, or via a caught panic.
#[derive(Debug)]
pub struct ConnectionSlot {
    active: Arc<AtomicUsize>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Unit tests for [`ConnectionLimiter`]/[`ConnectionSlot`] and
/// [`super::connection::refuse_for_capacity`]. Not gated on `feature = "worker"`, since the
/// connection cap applies to a library-only build too. No real sockets or threads:
/// [`ConnectionLimiter`] is tested as counter logic and `refuse_for_capacity` against an
/// in-memory buffer. The real-socket, real-TLS tests are in `serve::tests` and
/// `serve::tests::mtls`, both `worker`-only.
#[cfg(test)]
mod limiter_tests {
    use super::ConnectionLimiter;
    use crate::serve::connection;
    use std::sync::atomic::Ordering;

    #[test]
    fn acquires_up_to_max_and_refuses_the_next() {
        let limiter = ConnectionLimiter::new(2);
        let a = limiter
            .try_acquire()
            .expect("1st connection is under the cap");
        let b = limiter
            .try_acquire()
            .expect("2nd connection is exactly at the cap");
        let refused = limiter
            .try_acquire()
            .expect_err("3rd connection is over the cap");
        assert_eq!(
            refused, 2,
            "the refusal should report the active count that caused it"
        );

        drop(a);
        let c = limiter
            .try_acquire()
            .expect("a slot freed by drop can be reacquired");
        drop(b);
        drop(c);
        assert_eq!(limiter.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_refused_attempt_never_permanently_eats_into_the_cap() {
        let limiter = ConnectionLimiter::new(1);
        let _held = limiter.try_acquire().unwrap();
        for _ in 0..5 {
            limiter.try_acquire().unwrap_err();
        }
        // Each refusal above released the slot it provisionally reserved to observe the
        // count -- the active count must still read exactly 1 (`_held`), not 6.
        assert_eq!(limiter.active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_slot_releases_its_count_even_when_the_holder_panics() {
        let limiter = ConnectionLimiter::new(1);
        let slot = limiter.try_acquire().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slot = slot; // moved in; dropped while unwinding out of this closure
            panic!("simulated connection-thread panic while holding a ConnectionSlot");
        }));
        assert!(result.is_err());
        // `Drop::drop` runs while a panic unwinds (this is what `serve::run`'s real
        // connection threads rely on -- see `ConnectionLimiter::try_acquire`'s doc
        // comment) -- the slot must be released exactly as if the closure had returned
        // normally instead of panicking.
        assert_eq!(limiter.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn refuse_for_capacity_writes_a_decodable_error_naming_the_capacity_code() {
        let mut buf: Vec<u8> = Vec::new();
        connection::refuse_for_capacity(&mut buf, None, 64, 64);

        let mut cursor = std::io::Cursor::new(buf);
        let err: pleochroma_net::messages::ErrorMsg =
            pleochroma_net::messages::read_message(&mut cursor).unwrap();
        assert_eq!(err.code, connection::CONNECTION_LIMIT_REACHED_CODE);
        assert!(err.message.contains("64"), "{}", err.message);

        // The shape `pleochroma_net::client::handshake` falls back to: an `ErrorMsg` with
        // no `Welcome` before it (see `refuse_for_capacity`).
    }
}
