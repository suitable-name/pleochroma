//! Token-based enrollment.
//!
//! A one-time, time-limited token the operator reads out (or pastes) to the person enrolling
//! a new viewer, or on the coordinator's WORKER enrollment listener a new joining worker.
//!
//! # Two listeners, one per role
//!
//! `serve` runs one enrollment listener per client role: the VIEWER one next to the viewer
//! port (default 7879) and the WORKER one next to the worker port (default 7881). Each has
//! its own [`EnrollRegistry`], so a token only claims on the listener that issued it. Each
//! mints certificates of its own role only (a worker's Common Name is `worker:<name>`, see
//! `crate::pki::role`) and appends a claimed fingerprint to its own role's allowlist.
//! `cert issue-token --role worker` sends its `Issue` name as `worker:<name>`; each listener
//! refuses an `Issue` for the other role rather than minting the wrong kind.
//!
//! # Roles per token, cancelling, claim events, stopping
//!
//! A registry's role is only the default. [`EnrollRegistry::issue_for_role`] gives each
//! pending token its own role, and a claim of a role other than the registry's appends to
//! the path given to [`EnrollRegistry::register_allowlist`] (no path: the claim is refused).
//! [`EnrollRegistry::remove`] cancels a pending token by its [`TokenId`], and
//! [`EnrollRegistry::subscribe`] delivers a [`ClaimEvent`] (role, certificate fingerprint,
//! claiming address) for every successful claim. The accept loop ends when the
//! [`ListenerStop`] flag is set and an accept returns (see [`spawn_enroll_listener`]).
//! Monitor and admin certificates enroll only on the VIEWER listener and only while remote
//! administration is on: `serve` then registers the two allowlists with the registry
//! ([`EnrollRegistry::register_allowlist`]), and `cert issue-token --role monitor|admin` (name
//! prefix `monitor:` or `admin:`) or the control API ([`EnrollRegistry::issue_for_role`]) mints
//! them. Without the registration they are refused. A claimed certificate's Common Name carries
//! the role prefix.
//!
//! # Why this needs its own listener, not the render port
//!
//! The render listener (`crate::serve`) requires mutual TLS: every connection must already
//! present a certificate signed by this worker's CA. An enrolling client has no such
//! certificate yet. Enrollment therefore has its own listener and TLS config that never
//! requires a client certificate. Its connection handler ([`handle_enroll_connection`])
//! decodes only `EnrollRequest` messages and has no `RenderRequest` handling, so a claim
//! connection cannot reach rendering even if the certificate check were skipped.
//!
//! # The token
//!
//! See `pleochroma_net::token`'s doc comment for the wire encoding. Three security
//! properties, all enforced here:
//!
//! 1. **The secret is a fresh 256-bit CSPRNG value, never the certificate's
//!    fingerprint** -- the fingerprint is public (it crosses the wire on every TLS
//!    handshake and is what `allowlist.txt` stores), so a bearer secret must never
//!    double as it.
//! 2. **The server stores only [`sha2::Sha256`] of the secret**
//!    ([`PendingEnrollment::secret_hash`]), compared in [`EnrollRegistry::claim`] via
//!    [`subtle::ConstantTimeEq`], not `==`.
//! 3. **The token commits to the server's identity**: it carries the worker's CA
//!    fingerprint alongside the secret, and `pleochroma_net::enroll::claim` verifies the
//!    enrollment listener's certificate chains to that exact CA *before* sending the
//!    secret (`PinnedCaVerifier`) -- otherwise a MITM could impersonate the worker and
//!    walk away with a validly issued client certificate.
//!
//! # Lifecycle
//!
//! - Single use: [`EnrollRegistry::claim`] removes the entry before returning it.
//! - [`TOKEN_TTL_SECS`] (180s), checked server-side against [`Instant::now`] on every
//!   claim -- never a client-side courtesy.
//! - The issued bundle lives only in [`EnrollRegistry`]'s in-memory `Vec`
//!   ([`crate::pki::issue_client_in_memory`]), never written to disk by this process; it
//!   reaches disk only on the claiming machine via `crate::enroll_client::claim`.
//! - The claimed fingerprint is appended to `allowlist.txt` only inside
//!   [`handle_enroll_connection`]'s `Claim` arm, strictly after
//!   [`EnrollRegistry::claim`] returns a match, never at issue time.
//! - On claim, expiry, or registry drop, pending key material is explicitly zeroized:
//!   [`PendingEnrollment`]/[`EnrollBundle`] hold sensitive fields as
//!   [`zeroize::Zeroizing`], so `Drop` overwrites the bytes before the allocator frees
//!   them. Does not protect against the process being killed abruptly (`Drop` never
//!   runs).
//! - A `serve` restart starts with an empty [`EnrollRegistry`]: pending tokens are lost
//!   (fail-closed), and the operator issues a new one.
//!
//! # Where issuing happens
//!
//! Issuing and claiming must happen inside the *same* running `serve` process, since the
//! bundle lives only in memory from the moment it's minted. `cert issue-token`
//! (`crate::enroll_client::run_issue_token`) is a small TLS client that connects to this
//! listener using the operator's own `ca.pem` for normal server verification.
//!
//! Issuing is restricted twice. The peer must be on loopback, checked against the actual
//! accepted `TcpStream::peer_addr()` (the same listener also accepts non-loopback claim
//! connections under `--allow-remote`). And the request must carry the operator secret,
//! `<pki_dir>/issue.secret`: 32 random bytes, hex, created with owner-only permissions the
//! first time `serve` starts an enrollment listener and compared in constant time. A
//! loopback source alone is not authority -- any local user, and any port forward onto the
//! loopback listener, appears as loopback -- whereas reading `issue.secret` needs the same
//! access as reading `ca.key`, which already lets an operator mint certificates directly.
//! A registry built without a secret refuses every `Issue`.

use crate::{certs::SwappableCert, control::shutdown::ListenerStop, pki};
use pleochroma_net::{
    enroll::{
        OPERATOR_SECRET_LEN, operator_secret_path, read_operator_secret, write_operator_secret,
    },
    messages::PeerRole,
};
use std::{
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    thread,
};
use zeroize::Zeroizing;

mod connection;
mod registry;
#[cfg(test)]
mod tests;

/// The per-listener pending-enrollment table ([`EnrollRegistry`]) and why issuing one
/// failed ([`EnrollIssueError`]).
pub use registry::{ClaimEvent, EnrollIssueError, EnrollRegistry, IssuedEnrollment, TokenId};

/// How long an issued token remains claimable. Fixed, not operator-configurable -- the
/// TTL is enforced server-side, never a courtesy either side can override.
pub const TOKEN_TTL_SECS: u64 = 180;

/// Caps how many enrollments can be pending at once. This is a sanity bound on memory against
/// a misbehaving local caller: `Issue` already needs loopback and the operator secret.
const MAX_PENDING: usize = 64;

/// This listener's own cap on concurrent authenticated-phase (post-TLS-handshake) connections.
///
/// Kept separate from the viewer/worker `--max-connections` limiter. Enrollment connections
/// need no client certificate, so sharing that limiter would let anonymous enrollment
/// traffic exhaust the real viewer and worker slots on the other port. A claim or issue
/// exchange is brief and one-shot, so a small cap is enough.
pub const ENROLL_MAX_CONNECTIONS: usize = 8;

/// Bare, not-yet-TLS-handshaked enrollment connections allowed in flight, as a multiple of
/// [`ENROLL_MAX_CONNECTIONS`]. Checked in the accept loop before a thread is spawned. Kept
/// local rather than reusing the `serve` constant, because this module builds in every
/// configuration.
const ENROLL_HANDSHAKE_MULTIPLIER: usize = 4;

/// Builds the enrollment listener's TLS server config: TLS 1.3 only, presenting
/// `[server_cert, ca_cert]` as the chain -- deliberately including the CA certificate
/// so a claiming client (which has no CA file of its own) receives the bytes it needs
/// to hash and compare against its token's fingerprint. See
/// `crate::enroll_client::PinnedCaVerifier`.
///
/// Requires no client certificate: an enrolling client has none yet, and `cert
/// issue-token` authenticates by connecting from loopback with the operator secret
/// instead -- a weaker TLS posture than the render listener's, hence a separate config on
/// a separate port.
///
/// # Errors
///
/// A human-readable message if the server certificate/key/CA can't be loaded from
/// `cert_path`/`key_path`/`ca_path`, or don't form a valid TLS server configuration.
fn build_enroll_server_config(
    ca_path: &Path,
    cert_path: &Path,
    key_path: &Path,
) -> Result<Arc<rustls::ServerConfig>, String> {
    let mut chain = pleochroma_net::tls::load_certs(cert_path)
        .map_err(|e| format!("--cert {}: {e}", cert_path.display()))?;
    let ca_certs = pleochroma_net::tls::load_certs(ca_path)
        .map_err(|e| format!("--ca {}: {e}", ca_path.display()))?;
    chain.extend(ca_certs);
    let key = pleochroma_net::tls::load_private_key(key_path)
        .map_err(|e| format!("--key {}: {e}", key_path.display()))?;

    let config = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|e| format!("failed to build the enrollment TLS config: {e}"))?;
    Ok(Arc::new(config))
}

/// Configuration [`spawn_enroll_listener`] needs, gathered once at `serve` startup from
/// the same `--ca`/`--cert`/`--key` args the listeners validated.
pub struct EnrollConfig {
    /// Where to listen (already derived and loopback-checked by `crate::serve`).
    pub bind_addr: SocketAddr,
    /// The CA's directory (`ca.pem`, `ca.key`).
    pub pki_dir: PathBuf,
    /// `None` when `--trust-any-client-cert` is set: no allowlist to append to (any
    /// CA-signed client of the right role is already trusted), so a successful claim
    /// skips that step.
    pub allowlist_path: Option<PathBuf>,
    /// TLS with the server certificate and CA chain, no client auth.
    pub tls_config: Arc<rustls::ServerConfig>,
    /// This listener's own authenticated-connection cap; see [`ENROLL_MAX_CONNECTIONS`].
    pub limiter: crate::serve::ConnectionLimiter,
    /// The pre-TLS cap on bare, not-yet-handshaked connections, checked in the accept loop
    /// before a thread is spawned.
    pub handshake_limiter: crate::serve::ConnectionLimiter,
    /// Which client role this listener enrolls by default (see the module doc comment).
    pub role: PeerRole,
    /// The listener's pending-token table, built by [`Self::build`] with the operator
    /// secret from (or created in) `<pki_dir>/issue.secret`. Clone this `Arc` before
    /// spawning to issue tokens per role ([`EnrollRegistry::issue_for_role`]), cancel them
    /// ([`EnrollRegistry::remove`]) and watch claims ([`EnrollRegistry::subscribe`]).
    pub registry: Arc<EnrollRegistry>,
}

impl EnrollConfig {
    /// Builds an [`EnrollConfig`] for `role` from the same `serve --ca/--cert/--key` paths and the
    /// role's resolved allowlist path.
    ///
    /// Builds its own pair of limiters rather than taking them from the caller; see
    /// [`ENROLL_MAX_CONNECTIONS`].
    ///
    /// # Errors
    ///
    /// A human-readable message if the TLS config can't be built (see
    /// [`build_enroll_server_config`]) or the operator secret can't be loaded or created
    /// (see [`load_or_create_operator_secret`]).
    pub fn build(
        bind_addr: SocketAddr,
        ca_path: &Path,
        cert_path: &Path,
        key_path: &Path,
        allowlist_path: Option<PathBuf>,
        role: PeerRole,
    ) -> Result<Self, String> {
        let tls_config = build_enroll_server_config(ca_path, cert_path, key_path)?;
        let pki_dir = pki::role::pki_dir_of(ca_path);
        let operator_secret = load_or_create_operator_secret(&pki_dir)?;
        Ok(Self {
            bind_addr,
            pki_dir,
            allowlist_path,
            tls_config,
            limiter: crate::serve::ConnectionLimiter::new(ENROLL_MAX_CONNECTIONS),
            handshake_limiter: crate::serve::ConnectionLimiter::new(
                ENROLL_MAX_CONNECTIONS.saturating_mul(ENROLL_HANDSHAKE_MULTIPLIER),
            ),
            role,
            registry: Arc::new(
                EnrollRegistry::for_role(role).with_operator_secret(operator_secret),
            ),
        })
    }
}

/// The enrollment listener's certificate: whatever the coordinator's swappable server
/// certificate is at the moment of the handshake, with the CA certificate appended (see
/// [`build_enroll_server_config`] for why the CA is in the chain).
struct ServerChainWithCa {
    server: Arc<SwappableCert>,
    ca: Vec<rustls::pki_types::CertificateDer<'static>>,
}

impl std::fmt::Debug for ServerChainWithCa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerChainWithCa").finish_non_exhaustive()
    }
}

impl rustls::server::ResolvesServerCert for ServerChainWithCa {
    fn resolve(
        &self,
        _client_hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let current = self.server.current();
        let mut chain = current.cert.clone();
        chain.extend(self.ca.iter().cloned());
        Some(Arc::new(rustls::sign::CertifiedKey::new(
            chain,
            Arc::clone(&current.key),
        )))
    }
}

impl EnrollConfig {
    /// Makes the listener present the coordinator's swappable server certificate.
    ///
    /// It is used instead of the one read from disk at start-up, so a certificate swap (new network address) is
    /// seen by the next pairing without a restart.
    ///
    /// # Errors
    ///
    /// A human-readable message if the CA at `ca_path` cannot be read.
    pub fn with_swappable_certificate(
        mut self,
        ca_path: &Path,
        server: Arc<SwappableCert>,
    ) -> Result<Self, String> {
        let ca = pleochroma_net::tls::load_certs(ca_path)
            .map_err(|e| format!("--ca {}: {e}", ca_path.display()))?;
        let config =
            rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
                .with_no_client_auth()
                .with_cert_resolver(Arc::new(ServerChainWithCa { server, ca }));
        self.tls_config = Arc::new(config);
        Ok(self)
    }
}

/// Loads `<pki_dir>/issue.secret`, creating it (32 fresh CSPRNG bytes, owner-only
/// permissions) when it does not exist yet.
///
/// # Errors
///
/// A human-readable message if an existing file is unreadable or malformed (never
/// silently replaced), or a new one can't be created.
fn load_or_create_operator_secret(
    pki_dir: &Path,
) -> Result<Zeroizing<[u8; OPERATOR_SECRET_LEN]>, String> {
    if operator_secret_path(pki_dir).exists() {
        return read_operator_secret(pki_dir);
    }
    let mut secret = Zeroizing::new([0u8; OPERATOR_SECRET_LEN]);
    registry::random_bytes(secret.as_mut_slice()).map_err(|e| e.to_string())?;
    write_operator_secret(pki_dir, &secret)?;
    tracing::info!(
        "pleochroma-worker serve: created the enrollment operator secret {} (`cert issue-token` reads it)",
        operator_secret_path(pki_dir).display()
    );
    Ok(secret)
}

/// Binds `config.bind_addr` and spawns a background thread accepting enrollment
/// connections until `stop` says otherwise.
///
/// Mirrors `crate::serve::run`'s accept-loop shape (one `thread::spawn` per connection,
/// `catch_unwind`-wrapped). Returns as soon as the bind succeeds, so a bad
/// `--enroll-bind` is reported synchronously at `serve` startup.
///
/// The loop checks `stop.flag` after every accept returns, whether it returned a
/// connection or an error, and exits when it is set. Stopping is therefore "set the flag,
/// then connect to `stop.wake_addr`" (see [`ListenerStop`]); that connection is dropped
/// unanswered. When `config.bind_addr`'s port is `0` the real port is not known before
/// the bind: use [`bind_enroll_listener`], read [`BoundEnrollListener::local_addr`], build
/// the `ListenerStop`, then [`BoundEnrollListener::spawn`].
///
/// Returns the listener's actual bound address.
///
/// # Errors
///
/// A human-readable message if the bind itself fails (e.g. the port is already in use) or
/// the role cannot enroll yet.
pub fn spawn_enroll_listener(
    config: EnrollConfig,
    stop: ListenerStop,
) -> Result<SocketAddr, String> {
    let bound = bind_enroll_listener(config)?;
    let addr = bound.local_addr();
    bound.spawn(stop);
    Ok(addr)
}

/// An enrollment listener that is bound but not accepting yet. See [`bind_enroll_listener`].
pub struct BoundEnrollListener {
    listener: TcpListener,
    addr: SocketAddr,
    config: EnrollConfig,
}

impl BoundEnrollListener {
    /// The address the listener really bound (the OS-picked port when `0` was asked).
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Starts the accept thread. It runs until `stop.flag` is set and an accept returns.
    pub fn spawn(self, stop: ListenerStop) {
        run_accept_loop(self.listener, self.config, stop);
    }
}

/// Binds `config.bind_addr` and logs it, without accepting yet; [`BoundEnrollListener::spawn`]
/// starts the accept thread.
///
/// Exists so a caller can learn the bound port before it builds the [`ListenerStop`] whose
/// `wake_addr` must reach that port.
///
/// # Errors
///
/// A human-readable message if the bind fails or the role cannot enroll yet.
pub fn bind_enroll_listener(config: EnrollConfig) -> Result<BoundEnrollListener, String> {
    let role_flag = match config.role {
        PeerRole::Viewer => "",
        PeerRole::Worker => " --role worker",
        PeerRole::Monitor | PeerRole::Admin => {
            return Err(format!(
                "an enrollment listener cannot be made for {} certificates: they enroll on the \
                 viewer listener",
                pki::role::role_name(config.role)
            ));
        }
    };
    let listener = TcpListener::bind(config.bind_addr).map_err(|e| {
        format!(
            "failed to bind enrollment listener {}: {e}",
            config.bind_addr
        )
    })?;
    let bound_addr = listener
        .local_addr()
        .map_err(|e| format!("enrollment listener bound but local_addr() failed: {e}"))?;
    tracing::info!(
        "pleochroma-worker serve: {} enrollment listener on {bound_addr} (token TTL {TOKEN_TTL_SECS}s; \
         `pleochroma-worker cert issue-token{role_flag} --admin-addr {bound_addr}` to mint one)",
        pki::role::role_name(config.role)
    );

    Ok(BoundEnrollListener {
        listener,
        addr: bound_addr,
        config,
    })
}

/// The accept thread of one enrollment listener; see [`spawn_enroll_listener`].
fn run_accept_loop(listener: TcpListener, config: EnrollConfig, stop: ListenerStop) {
    let registry = config.registry;
    let tls_config = config.tls_config;
    let pki_dir = config.pki_dir;
    let allowlist_path = config.allowlist_path;
    let limiter = config.limiter;
    let handshake_limiter = config.handshake_limiter;

    thread::spawn(move || {
        for incoming in listener.incoming() {
            // Checked after every accept, the wake connection included, which is dropped
            // unanswered. Connections still being handled keep their own threads.
            if stop.flag.load(Ordering::SeqCst) {
                break;
            }
            match incoming {
                Ok(stream) => {
                    let peer = stream.peer_addr().ok();
                    // Bounds bare, not-yet-handshaked connections; checked before a thread
                    // is spawned.
                    let Ok(handshake_slot) = handshake_limiter.try_acquire() else {
                        tracing::warn!(
                            "enrollment connection {peer:?}: refusing -- too many not-yet-handshaked enrollment \
                             connections in flight"
                        );
                        continue;
                    };
                    let tls_config = Arc::clone(&tls_config);
                    let registry = Arc::clone(&registry);
                    let pki_dir = pki_dir.clone();
                    let allowlist_path = allowlist_path.clone();
                    let limiter = limiter.clone();
                    thread::spawn(move || {
                        let _handshake_slot = handshake_slot; // held for this thread's whole lifetime
                        let Some(tls_stream) =
                            connection::accept_enroll_tls(stream, &tls_config, peer)
                        else {
                            return;
                        };
                        // Taken only after the TLS handshake succeeds, so a connection that
                        // never completes TLS holds no slot. `accept_enroll_tls` bounds that
                        // handshake with `HANDSHAKE_TIMEOUT`.
                        let Ok(_slot) = limiter.try_acquire() else {
                            tracing::warn!(
                                "enrollment connection {peer:?}: refusing -- already at this listener's own \
                                 enrollment connection cap"
                            );
                            return;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            connection::handle_enroll_connection(
                                tls_stream,
                                &registry,
                                &pki_dir,
                                allowlist_path.as_deref(),
                                peer,
                            )
                        }));
                        match result {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => {
                                tracing::warn!(
                                    "enrollment connection {peer:?} ended with an error: {e}"
                                );
                            }
                            Err(_) => {
                                tracing::warn!(
                                    "enrollment connection {peer:?} panicked and was dropped"
                                );
                            }
                        }
                    });
                }
                Err(e) => tracing::warn!("enrollment accept error: {e}"),
            }
        }
    });
}
