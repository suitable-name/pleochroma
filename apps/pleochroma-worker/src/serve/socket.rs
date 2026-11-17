//! Per-socket plumbing shared by every listener (viewer, worker, enrollment) and by `join`'s
//! outbound connections.
//!
//! TCP tuning, the pre-protocol handshake deadline, the read-only library database opener, and the
//! payload-encoding preference.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::{Duration, Instant},
};

use pleochroma_vault::db::sqlite::Database;

use crate::cli::ServeArgs;

/// A TOTAL deadline across however many logical reads a caller performs through this wrapper.
///
/// The per-read idle timeout of [`apply_handshake_timeout`] resets on every read, so it never fires
/// against a peer that keeps a connection alive by trickling data (see [`HANDSHAKE_TIMEOUT`]). The
/// deadline is checked before each underlying `read`, so such a peer is cut off once the whole
/// budget has elapsed.
///
/// Generic over any `Read`/`Write`, so it also works on in-memory streams. For a concrete
/// socket that can re-arm its own read timeout, see [`DeadlineSocket`]. A single underlying
/// read that blocks past the deadline is still bounded only by the timeout `inner` carries.
pub struct DeadlineIo<'a, S> {
    inner: &'a mut S,
    deadline: Instant,
}

impl<'a, S> DeadlineIo<'a, S> {
    pub(crate) const fn new(inner: &'a mut S, deadline: Instant) -> Self {
        Self { inner, deadline }
    }

    /// `Err(TimedOut)` once `deadline` has passed, without touching `inner` at all.
    fn check_deadline(&self) -> std::io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "pre-HELLO handshake deadline exceeded",
            ));
        }
        Ok(())
    }
}

impl<S: Read> Read for DeadlineIo<'_, S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.check_deadline()?;
        self.inner.read(buf)
    }
}

impl<S: Write> Write for DeadlineIo<'_, S> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The complement to [`DeadlineIo`] for a concrete socket.
///
/// It re-arms `sock`'s read timeout to
/// the time left before `deadline` before every read, so a peer that goes fully silent is cut
/// off near the deadline rather than after another whole idle timeout. It is used for the TLS
/// handshakes of [`super::tls::accept_tls`] and `crate::enroll::connection::accept_enroll_tls`,
/// whose reads run inside rustls (`complete_io`), so the deadline must act on the socket.
pub struct DeadlineSocket<'a> {
    sock: &'a TcpStream,
    deadline: Instant,
}

impl<'a> DeadlineSocket<'a> {
    pub(crate) const fn new(sock: &'a TcpStream, deadline: Instant) -> Self {
        Self { sock, deadline }
    }

    /// Re-arms `sock`'s read timeout to the time remaining, or fails outright without
    /// touching `sock` at all once `deadline` has already passed.
    fn rearm(&self) -> std::io::Result<()> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "pre-HELLO handshake deadline exceeded",
            ));
        }
        self.sock.set_read_timeout(Some(remaining))
    }
}

impl Read for DeadlineSocket<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.rearm()?;
        let mut sock = self.sock;
        sock.read(buf)
    }
}

impl Write for DeadlineSocket<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut sock = self.sock;
        sock.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut sock = self.sock;
        sock.flush()
    }
}

/// Opens the design-library database this `serve` instance serves: `args.db` if given,
/// else the default `facet_diagrams.sqlite` relative to the process's working directory.
///
/// Always opened READ-ONLY -- this phase never writes to the catalogue, so `serve`
/// fails fast with a clear error if the database doesn't already exist rather than
/// silently creating an empty one.
///
/// Called once at start-up to validate the path, and then by each connection's
/// [`super::library::LibraryHandle`] on its first library request (see
/// [`resolve_library_db_path`]), not opened once and shared: `rusqlite::Connection` is
/// `Send` but not `Sync`, and SQLite's concurrency model is many independent reader
/// connections.
///
/// # Errors
///
/// A human-readable message if the database can't be opened read-only (missing file,
/// permissions, not a valid SQLite database).
pub(super) fn open_library_database(path: &std::path::Path) -> Result<Database, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("--db {}: path is not valid Unicode", path.display()))?;
    Database::open_read_only(path_str).map_err(|e| {
        format!(
            "failed to open the design-library database read-only at {}: {e:#}",
            path.display()
        )
    })
}

/// Resolves `--db` (or the default `facet_diagrams.sqlite`) to the path every
/// connection opens its own [`open_library_database`] handle from, when it first serves
/// a library request.
///
/// [`super::start`] hands out a path, not an open [`Database`]: `rusqlite::Connection` is not
/// `Sync`, so it cannot be shared between connections. [`super::start`] still opens and drops
/// one at startup, to fail fast on a bad `--db` before binding a socket.
///
/// With `args.no_library` ("rendering only") no library file is meant: the returned path is a
/// name that is never opened, inside the data folder, so the jobs and the asset cache sit there
/// and not next to a `facet_diagrams.sqlite` of the working directory.
#[must_use]
pub(super) fn resolve_library_db_path(args: &ServeArgs) -> std::path::PathBuf {
    if args.no_library {
        return no_library_anchor(args);
    }
    args.db
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from(pleochroma_vault::db::sqlite::DEFAULT_DB_FILE))
}

/// The anchor of the jobs and the asset cache when there is no library: a file name in the data
/// folder (the folder of the loaded settings, else the one above `--ca`'s folder, else the
/// working directory). Nothing is ever created or opened at this path.
fn no_library_anchor(args: &ServeArgs) -> std::path::PathBuf {
    let folder = crate::settings::data_dir()
        .or_else(|| {
            args.ca
                .as_deref()
                .map(crate::pki::role::pki_dir_of)
                .and_then(|pki| pki.parent().map(std::path::Path::to_path_buf))
                .filter(|dir| !dir.as_os_str().is_empty())
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    folder.join("no-library.sqlite")
}

/// How long an accepted connection may sit idle before the OS probes it with a TCP
/// keepalive packet. Catches a peer that vanished without a clean FIN/RST; not meant to
/// second-guess a busy connection.
const TCP_KEEPALIVE_IDLE: Duration = Duration::from_secs(30);

/// How often a keepalive probe repeats once idle. Short relative to
/// `TCP_KEEPALIVE_IDLE` so a dead peer is detected within a few seconds, rather than
/// waiting on the OS's own default probe count/interval.
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// Best-effort tuning for one just-accepted `stream`.
///
/// Disables Nagle's algorithm (`TCP_NODELAY`, since this protocol writes many small
/// latency-sensitive messages) and enables TCP keepalive so a peer that vanishes without a clean
/// FIN/RST is eventually detected instead of leaking the connection's thread forever. Never aborts
/// the accept loop on failure; logged at `debug` since failure is expected to be harmless on
/// deployed platforms.
pub fn tune_accepted_socket(stream: &TcpStream, peer: Option<SocketAddr>) {
    if let Err(e) = stream.set_nodelay(true) {
        tracing::debug!("connection {peer:?}: failed to set TCP_NODELAY: {e}");
    }

    let sock_ref = socket2::SockRef::from(stream);
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(TCP_KEEPALIVE_IDLE)
        .with_interval(TCP_KEEPALIVE_INTERVAL);
    if let Err(e) = sock_ref.set_tcp_keepalive(&keepalive) {
        tracing::debug!("connection {peer:?}: failed to set TCP keepalive: {e}");
    }
}

/// The time budget for each phase of a just-accepted connection's pre-protocol handshake.
///
/// The phases are the TLS handshake (see [`super::tls::accept_tls`]) and then the first `HELLO`
/// frame (under `--insecure-no-tls`, the `HELLO` alone). A connection that exceeds it is dropped.
/// This bounds a slowloris client that connects and never speaks, and one that trickles bytes
/// forever so that no single read goes idle.
///
/// Two mechanisms enforce it, each with its own budget:
/// - [`apply_handshake_timeout`] sets a per-read idle timeout on the raw `TcpStream` at
///   accept time. It catches a peer that goes silent, not one that keeps sending slowly.
/// - [`accept_tls`](super::tls::accept_tls) and [`super::handshake::read_and_check_hello`]
///   each wrap their reads in [`DeadlineSocket`]/[`DeadlineIo`], with a deadline of
///   `Instant::now() + HANDSHAKE_TIMEOUT` taken at the start of that phase and checked before
///   every read. Because the phases are bounded independently, the worst case is about twice
///   this constant.
///
/// Cleared once `HELLO` arrives, so it never bounds the connection loop that follows, which
/// has its own timeouts (see `stream_emit::TimeoutCache`).
///
/// `pub(crate)` so `crate::enroll` applies the same bound to its TLS-then-one-message
/// exchange. That listener has no mutual-TLS authentication step, so it needs a deadline at
/// least as much.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Applies [`HANDSHAKE_TIMEOUT`] to `stream`'s read and write timeouts.
///
/// Best-effort, like [`tune_accepted_socket`]: a failure is logged at `debug` and never aborts the
/// accept loop. Worst case, this one connection stays unbounded.
pub fn apply_handshake_timeout(stream: &TcpStream, peer: Option<SocketAddr>) {
    if let Err(e) = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)) {
        tracing::debug!("connection {peer:?}: failed to set handshake read timeout: {e}");
    }
    if let Err(e) = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)) {
        tracing::debug!("connection {peer:?}: failed to set handshake write timeout: {e}");
    }
}

/// The preference list `choice` gives a connection from `peer`: the pinned encoding alone, or
/// [`payload_preference_for`] under `auto`.
///
/// It decides what `WELCOME` announces; each frame's own header names what it actually carries.
#[cfg(feature = "worker")]
#[must_use]
pub fn payload_preference_for_choice(
    choice: pleochroma_net::messages::adaptive::PayloadChoice,
    peer: Option<SocketAddr>,
) -> Vec<pleochroma_net::messages::PayloadEncoding> {
    match choice {
        pleochroma_net::messages::adaptive::PayloadChoice::Fixed(encoding) => vec![encoding],
        pleochroma_net::messages::adaptive::PayloadChoice::Auto => {
            payload_preference_for(peer).to_vec()
        }
    }
}

/// The payload-encoding preference this server uses for a connection from `peer`.
///
/// It is raw only for a loopback peer (memory bandwidth beats every codec), else zstd level 1,
/// then LZ4, then raw (coordinator guide section 11). The connection handler negotiates this list
/// against the peer's `HELLO.accept_encodings`.
#[cfg(feature = "worker")]
#[must_use]
pub fn payload_preference_for(
    peer: Option<SocketAddr>,
) -> &'static [pleochroma_net::messages::PayloadEncoding] {
    if peer.is_some_and(|p| p.ip().is_loopback()) {
        &pleochroma_net::messages::LOOPBACK_SERVER_PREFERENCE
    } else {
        &pleochroma_net::messages::DEFAULT_SERVER_PREFERENCE
    }
}

#[cfg(test)]
mod tests {
    use super::DeadlineIo;
    use std::{
        io::Read,
        time::{Duration, Instant},
    };

    /// A `Read` that hands back exactly one byte per call, sleeping `gap` first every
    /// time -- simulates a peer that keeps a connection alive by trickling data, always
    /// well inside any single idle read timeout, forever.
    struct Trickle {
        gap: Duration,
    }

    impl Read for Trickle {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.gap);
            buf[0] = 0;
            Ok(1)
        }
    }

    /// A peer sending one byte every 100 ms is still cut off once a 300 ms total budget
    /// elapses. `DeadlineIo` checks the deadline before every read, so this takes effect
    /// around the 3rd or 4th read, without any single read going idle.
    #[test]
    fn deadline_io_drops_a_trickling_peer_once_the_total_budget_elapses() {
        let mut trickle = Trickle {
            gap: Duration::from_millis(100),
        };
        let deadline = Instant::now() + Duration::from_millis(300);
        let mut guarded = DeadlineIo::new(&mut trickle, deadline);

        let start = Instant::now();
        let mut buf = [0u8; 16];
        let err = guarded.read_exact(&mut buf).expect_err(
            "a 16-byte read at one byte/100ms must never complete inside a 300ms budget",
        );
        let elapsed = start.elapsed();

        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        assert!(
            elapsed >= Duration::from_millis(250) && elapsed < Duration::from_millis(700),
            "expected to be dropped at ~300ms, was dropped at {elapsed:?}"
        );
    }

    /// The complement: a deadline that has already passed refuses the very first read,
    /// with no attempt to read anything at all (proven by a `Trickle`-shaped reader that
    /// would otherwise sleep -- if `DeadlineIo` called it, this test would take 100ms
    /// instead of returning immediately).
    #[test]
    fn deadline_io_refuses_immediately_once_the_deadline_has_already_passed() {
        let mut trickle = Trickle {
            gap: Duration::from_millis(100),
        };
        let deadline = Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("the process has been running for at least 1ms");
        let mut guarded = DeadlineIo::new(&mut trickle, deadline);

        let start = Instant::now();
        let mut buf = [0u8; 1];
        let err = guarded.read_exact(&mut buf).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut, "{err}");
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "must fail without ever calling the inner reader"
        );
    }
}
