//! A small process-wide pool of idle, already-handshaken connections for the one-shot
//! lifecycle ([`super::one_shot`]).
//!
//! The export dispatches one `RenderRequest` per chunk. Opening a connection for each chunk
//! would repeat DNS, TCP, TLS and `HELLO`/`WELCOME` every time. A request that ends in a clean
//! `DONE` leaves the socket ready for another request, as the persistent live connection
//! relies on, so [`give_back`] parks it and the next chunk's [`take`] reuses it. A pooled
//! connection that fails is dropped by the caller, which connects fresh. Nothing here
//! repairs a stream.

use super::super::types::RemoteStream;
use crate::settings::WorkerSettings;
use pleochroma_net::messages::Welcome;
use std::{
    sync::{Mutex, PoisonError},
    time::{Duration, Instant},
};

/// How long a parked connection stays eligible. An export's chunks follow each other
/// within milliseconds, so this only bounds how long a connection the worker (or a NAT)
/// may have silently dropped can be handed out; a stale one is also caught by the
/// caller's retry on a fresh connection, but a half-open socket would otherwise cost the
/// 30 s first-event wait before that retry.
const IDLE_TTL: Duration = Duration::from_secs(10);

/// The most idle connections kept per worker: one per concurrently dispatching lane
/// (the export's remote lane plus a batch preview) with a little slack.
const MAX_IDLE_PER_WORKER: usize = 2;

/// One parked connection and what [`take`] needs to hand it out again.
struct Idle {
    /// [`pool_key`] of the worker it belongs to.
    key: String,
    stream: RemoteStream,
    welcome: Welcome,
    /// When it was parked, for [`IDLE_TTL`].
    since: Instant,
}

static POOL: Mutex<Vec<Idle>> = Mutex::new(Vec::new());

/// What identifies "the same connection target": the address plus the certificate
/// directory, since two settings entries may point one address at different identities.
fn pool_key(worker: &WorkerSettings) -> String {
    format!("{}|{}", worker.address, worker.cert_dir)
}

/// Removes and returns the most recently parked live connection for `worker`, dropping
/// any that outlived [`IDLE_TTL`] on the way. `None` means the caller connects fresh.
pub(super) fn take(worker: &WorkerSettings) -> Option<(RemoteStream, Welcome)> {
    let key = pool_key(worker);
    let idle = {
        let mut pool = POOL.lock().unwrap_or_else(PoisonError::into_inner);
        pool.retain(|idle| idle.since.elapsed() <= IDLE_TTL);
        let position = pool.iter().rposition(|idle| idle.key == key)?;
        pool.remove(position)
    };
    Some((idle.stream, idle.welcome))
}

/// Closes every parked connection (each one says goodbye as it drops, see
/// [`RemoteStream`]). For a process about to exit: the pool is a `static`, which is never
/// dropped, so a connection left parked would be cut off by the operating system instead.
pub fn close_idle() {
    let idle = std::mem::take(&mut *POOL.lock().unwrap_or_else(PoisonError::into_inner));
    drop(idle);
}

/// Parks `stream` (whose last request just ended in a clean `DONE`) for the next
/// [`take`] on the same worker, evicting the oldest parked connection of that worker
/// when [`MAX_IDLE_PER_WORKER`] would be exceeded.
pub(super) fn give_back(worker: &WorkerSettings, stream: RemoteStream, welcome: Welcome) {
    let key = pool_key(worker);
    let mut pool = POOL.lock().unwrap_or_else(PoisonError::into_inner);
    pool.retain(|idle| idle.since.elapsed() <= IDLE_TTL);
    while pool.iter().filter(|idle| idle.key == key).count() >= MAX_IDLE_PER_WORKER {
        match pool.iter().position(|idle| idle.key == key) {
            Some(oldest) => {
                pool.remove(oldest);
            }
            None => break,
        }
    }
    pool.push(Idle {
        key,
        stream,
        welcome,
        since: Instant::now(),
    });
}
