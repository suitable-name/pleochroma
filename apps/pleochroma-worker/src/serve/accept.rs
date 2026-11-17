//! The VIEWER listener's accept loop.
//!
//! Per-socket tuning, the connection caps (a per-source-address cap and a wider global cap on
//! connections that have not finished authenticating, then the real `--max-connections` cap), the
//! transport (mutual TLS or `--insecure-no-tls`), and one thread per connection running the viewer
//! handler. See `crate::serve`'s "Robustness" section for why the caps are acquired in this order.

use super::{
    ConnectionLimiter, ConnectionSlot,
    connection::{self, report_connection_result},
    library::LibraryHandle,
    socket::{apply_handshake_timeout, tune_accepted_socket},
    tls::{Transport, accept_tls},
};
use crate::control::{
    shutdown::{ConnectionSet, ListenerStop},
    table::ConnectionTable,
};
use pleochroma_net::messages::PeerRole;
use std::{
    collections::{HashMap, hash_map::Entry},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    thread,
};

/// Caps how many connections one source IP address may have in flight that have not
/// finished authenticating (`--max-preauth-per-ip`).
///
/// Counts per address only, never pins a thread or a socket itself: the accept loop
/// asks [`Self::try_admit`] before spawning anything, and the returned [`AddressSlot`]
/// is dropped the moment the connection authenticates (or ends). Addresses are
/// canonicalised (an IPv4-mapped IPv6 peer counts as its IPv4 address); a peer whose
/// address is unknown shares the unspecified address's count.
#[derive(Debug, Clone)]
pub struct PreAuthAddresses {
    held: Arc<Mutex<HashMap<IpAddr, usize>>>,
    max: usize,
}

/// RAII handle for one [`PreAuthAddresses`] admission; releases it when dropped.
#[derive(Debug)]
pub struct AddressSlot {
    held: Arc<Mutex<HashMap<IpAddr, usize>>>,
    address: IpAddr,
}

impl PreAuthAddresses {
    /// A counter admitting at most `max` unauthenticated connections per address.
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            held: Arc::new(Mutex::new(HashMap::new())),
            max,
        }
    }

    /// Admits one more unauthenticated connection from `peer`.
    ///
    /// # Errors
    ///
    /// `Err(held)` with the number of unauthenticated connections `peer`'s address
    /// already has when that is the cap; nothing is counted then.
    pub fn try_admit(&self, peer: Option<SocketAddr>) -> Result<AddressSlot, usize> {
        let address = peer.map_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED), |p| p.ip().to_canonical());
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.get(&address).copied().unwrap_or(0);
        if count >= self.max {
            return Err(count);
        }
        held.insert(address, count + 1);
        drop(held);
        Ok(AddressSlot {
            held: Arc::clone(&self.held),
            address,
        })
    }
}

impl Drop for AddressSlot {
    fn drop(&mut self) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Entry::Occupied(mut entry) = held.entry(self.address) {
            if *entry.get() <= 1 {
                entry.remove();
            } else {
                *entry.get_mut() -= 1;
            }
        }
    }
}

/// The two counts a connection holds until it has authenticated: its source address's
/// share of the per-address cap and one slot of the global pre-authentication cap.
/// Dropping it releases both.
struct PreAuthSlots {
    _address: AddressSlot,
    _handshake: ConnectionSlot,
}

/// Everything the viewer listener's connection threads share, built once by [`super::start`] and
/// handed to every thread behind one `Arc`.
///
/// One argument instead of a positional parameter per setting.
pub struct ViewerListener {
    /// Mutual TLS (with the viewer [`super::tls::Auth`]) or plaintext.
    pub transport: Transport,
    /// The real `--max-connections` cap for authenticated viewer connections.
    pub limiter: ConnectionLimiter,
    /// The wider cap on bare, not-yet-authenticated connections (see
    /// [`super::limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER`]).
    pub handshake_limiter: ConnectionLimiter,
    /// The per-source-address cap on connections that have not finished authenticating
    /// (`--max-preauth-per-ip`).
    pub pre_auth_addresses: PreAuthAddresses,
    /// The library database every connection opens its own read-only handle from, when
    /// it serves its first library request (see [`LibraryHandle`]). Still the anchor of the
    /// asset cache and the job directory when [`Self::library_present`] is false.
    pub db_path: PathBuf,
    /// Whether the library database could be opened at start-up. When it could not, the
    /// coordinator runs without a library: every library request is answered with a plain
    /// "no library" error ([`LibraryHandle::absent`]) and everything else works.
    pub library_present: bool,
    /// `--max-connections`, for the refusal message.
    pub max_connections: usize,
    /// Every accepted viewer socket, so shutdown can close them all at once.
    pub connections: ConnectionSet,
    /// The identified viewer connections (fingerprint, name, role) with a kill switch each,
    /// for the control API's device list and `revoke`.
    pub table: ConnectionTable,
    /// What a remote administration session acts through: the same control API the coordinator
    /// window uses (see [`crate::admin`]).
    pub core: Arc<super::ServeCore>,
    /// The render side of a `worker` build.
    #[cfg(feature = "worker")]
    pub render: RenderSetup,
}

/// The render side of the viewer listener (`worker` builds): the own lane (`--render`) and the
/// coordinator's joined-worker registry.
#[cfg(feature = "worker")]
pub struct RenderSetup {
    /// CPU tracer threads for the own lane.
    pub threads: usize,
    /// The process's GPU backend -- [`pleochroma::renderer::gpu_backend::GpuBackend::disabled`]
    /// unless `--render` without `--only-cpu`.
    pub gpu: Arc<pleochroma::renderer::gpu_backend::GpuBackend>,
    /// `--only-gpu`/`--only-cpu`/hybrid.
    pub compute_mode: crate::cli::ComputeMode,
    /// `serve --render`.
    pub own_lane: bool,
    /// The joined-worker registry (`None` with `--no-workers` or `--insecure-no-tls`).
    pub registry: Option<Arc<crate::coordinator::Registry>>,
    /// Request execution over the own lane and joined workers.
    pub coordinator: Arc<crate::coordinator::Coordinator>,
    /// The HDR asset cache: opened with `--render` or a worker port (the coordinator holds
    /// HDR jobs' maps through it), `None` otherwise (and when it cannot be opened), in which
    /// case HDR scenes are refused.
    pub assets: Option<Arc<crate::assets::AssetCache>>,
    /// The durable job service: opened with `--render` (jobs render on the own lane),
    /// `None` otherwise (and when its directory cannot be opened), in which case job requests
    /// are refused.
    pub jobs: Option<Arc<crate::jobs::JobService>>,
    /// `--payload-encoding`: how frames sent to a viewer are compressed.
    pub payload: pleochroma_net::messages::adaptive::PayloadChoice,
}

/// What TLS established about a viewer: its certificate's role and the certificate
/// itself (both `None` without TLS).
type PeerAuth = (
    Option<PeerRole>,
    Option<rustls::pki_types::CertificateDer<'static>>,
);

/// The identity a viewer's jobs queue under (one active job per viewer certificate):
/// the client certificate's SHA-256 fingerprint, or -- without TLS -- the peer's IP
/// address.
#[cfg(feature = "worker")]
fn viewer_key(
    certificate: Option<&rustls::pki_types::CertificateDer<'_>>,
    peer: Option<SocketAddr>,
) -> String {
    certificate.map_or_else(
        || {
            format!(
                "address:{}",
                peer.map_or_else(|| "unknown".to_string(), |p| p.ip().to_string())
            )
        },
        |cert| {
            format!(
                "certificate:{}",
                pleochroma_net::tls::fingerprint_to_hex(&pleochroma_net::tls::fingerprint(cert))
            )
        },
    )
}

/// The library handle of one connection: lazy over the database, or the "no library" handle
/// when the coordinator started without one.
fn library_for(shared: &ViewerListener, peer: Option<SocketAddr>) -> LibraryHandle {
    if shared.library_present {
        LibraryHandle::lazy(shared.db_path.clone(), peer)
    } else {
        LibraryHandle::absent(peer)
    }
}

/// Accepts viewer connections on `listener` until `stop` is raised, one thread per
/// connection.
///
/// The flag is read right after every accept, before the new stream is looked at, so the
/// wake-up connection of [`ListenerStop::stop`] is dropped unserved and the loop returns,
/// which drops `listener` and frees the port.
pub fn run_accept_loop(listener: &TcpListener, shared: &Arc<ViewerListener>, stop: &ListenerStop) {
    for incoming in listener.incoming() {
        if stop.is_stopped() {
            return;
        }
        match incoming {
            Ok(stream) => {
                let peer = stream.peer_addr().ok();
                tune_accepted_socket(&stream, peer);
                apply_handshake_timeout(&stream, peer);

                // Bounds bare, not-yet-authenticated connections regardless of
                // transport -- checked before spawning a thread at all. The source
                // address comes first, so one address can never take more than its share
                // of the global pre-authentication slots.
                let address_slot = match shared.pre_auth_addresses.try_admit(peer) {
                    Ok(slot) => slot,
                    Err(held) => {
                        tracing::warn!(
                            "connection {peer:?}: refusing -- this address already has {held} connection(s) \
                             that have not finished authenticating (--max-preauth-per-ip)"
                        );
                        continue;
                    }
                };
                let Ok(handshake_slot) = shared.handshake_limiter.try_acquire() else {
                    tracing::warn!(
                        "connection {peer:?}: refusing -- too many not-yet-authenticated connections in \
                         flight (see --max-connections)"
                    );
                    continue;
                };
                let pre_auth = PreAuthSlots {
                    _address: address_slot,
                    _handshake: handshake_slot,
                };

                // `Transport::Insecure` (loopback only) has no authentication step to wait
                // for, so the REAL slot is reserved right here; for `Transport::Tls` it is
                // acquired inside the spawned thread, only after `accept_tls` succeeds.
                let pre_acquired_slot = matches!(shared.transport, Transport::Insecure)
                    .then(|| shared.limiter.try_acquire());

                spawn_connection_handler(stream, peer, pre_auth, pre_acquired_slot, shared);
            }
            Err(e) => tracing::warn!("accept error: {e}"),
        }
    }
}

/// Dispatches one just-accepted `stream` on the listener's transport and spawns its own
/// thread running the viewer handler.
///
/// `pre_auth` is released once the peer has authenticated, so an authenticated connection
/// counts only against `--max-connections`. `pre_acquired_slot` holds the real cap's verdict
/// for `Transport::Insecure` (`Some`). For TLS it is `None`: the real slot is acquired inside
/// the thread, after [`accept_tls`] succeeds. Reserving it earlier would let bare TCP connects
/// hold every real slot for up to `HANDSHAKE_TIMEOUT`, locking out certificate-holding
/// viewers. `Err(active)` goes to [`connection::refuse_for_capacity`].
///
/// Authentication is complete when [`accept_tls`] returns; the `HELLO` that follows runs on a
/// connection that already holds its real slot.
fn spawn_connection_handler(
    stream: TcpStream,
    peer: Option<SocketAddr>,
    pre_auth: PreAuthSlots,
    pre_acquired_slot: Option<Result<ConnectionSlot, usize>>,
    shared_ref: &Arc<ViewerListener>,
) {
    let shared = Arc::clone(shared_ref);
    // Held by the connection's thread for its whole life; shutdown closes the socket
    // through the set, and dropping the guard takes it out again.
    let tracked = shared.connections.register(&stream);
    match &shared_ref.transport {
        Transport::Insecure => {
            tracing::warn!(
                "--insecure-no-tls: accepted PLAINTEXT connection from {peer:?} -- no TLS, no \
                 authentication"
            );
            let slot = pre_acquired_slot.expect(
                "the accept loop always pre-acquires the real slot for Transport::Insecure",
            );
            thread::spawn(move || {
                let _tracked = tracked;
                let mut stream = stream;
                match slot {
                    // `_slot` releases this connection's counted slot when the arm ends.
                    Ok(_slot) => {
                        drop(pre_auth);
                        serve_accepted(stream, peer, (None, None), &shared, &|| {});
                    }
                    Err(active) => {
                        connection::refuse_for_capacity(
                            &mut stream,
                            peer,
                            active,
                            shared.max_connections,
                        );
                        drop(pre_auth);
                    }
                }
            });
        }
        Transport::Tls { config, auth, .. } => {
            let config = Arc::clone(config);
            let auth = auth.clone();
            // A second handle to the socket: the table's kill switch for this connection.
            let kill_switch = stream.try_clone().ok();
            thread::spawn(move || {
                let _tracked = tracked;
                serve_tls_connection(
                    stream,
                    kill_switch,
                    (&config, &auth),
                    peer,
                    pre_auth,
                    &shared,
                );
            });
        }
    }
}

/// The thread body of one TLS viewer connection: [`accept_tls`], the role gate, the real
/// connection slot, then the administration session or the viewer handler.
///
/// `kill_switch` is a second handle to the socket for the connection table, so revoking the
/// device can shut it down.
fn serve_tls_connection(
    stream: TcpStream,
    kill_switch: Option<TcpStream>,
    (config, auth): (&Arc<rustls::ServerConfig>, &super::tls::Auth),
    peer: Option<SocketAddr>,
    pre_auth: PreAuthSlots,
    shared: &ViewerListener,
) {
    let Some((mut tls_stream, cert_role)) = accept_tls(stream, config, auth, peer) else {
        return; // accept_tls already logged why; `pre_auth` drops with the thread
    };
    // The real slot is acquired only now that accept_tls has succeeded.
    let certificate = tls_stream
        .conn
        .peer_certificates()
        .and_then(<[_]>::first)
        .map(|c| c.clone().into_owned());
    // Listed under its certificate's name and fingerprint until the thread ends;
    // revoking the device shuts the socket down through the kill switch. Only a
    // connection the role gate lets in is kept at all: a certificate of the port's
    // own role (its allowlist was checked by `accept_tls`), or a monitor or
    // administrator certificate that remote administration admits. A refused one
    // (a removed monitor, a worker certificate on the viewer port) never shows as
    // connected. Even then it is listed only once its `HELLO` was accepted (the
    // handler calls `list_now`), so a peer that connects and says nothing, or
    // whose `HELLO` is refused, is never shown.
    let admitted = cert_role == auth.role
        || (matches!(cert_role, PeerRole::Monitor | PeerRole::Admin)
            && shared.core.info.remote_admin
            && certificate.as_ref().is_some_and(|certificate| {
                crate::admin::session::is_listed(
                    &shared.core,
                    cert_role,
                    &pleochroma_net::tls::fingerprint(certificate),
                )
            }));
    let listed = certificate
        .as_ref()
        .filter(|_| admitted)
        .map(|certificate| {
            shared
                .table
                .register_certificate_pending(certificate, cert_role, peer, kill_switch)
        });
    let list_now = || {
        if let Some(tracked) = &listed {
            tracked.list_now();
        }
    };
    match shared.limiter.try_acquire() {
        Ok(_slot) => {
            // Authenticated and counted against `--max-connections`: no
            // longer a pre-authentication connection.
            drop(pre_auth);
            // A monitor or admin certificate gets an administration session
            // while remote administration is on; off, it goes to the viewer
            // handler, whose `HELLO` gate refuses it in plain words.
            if shared.core.info.remote_admin
                && matches!(cert_role, PeerRole::Monitor | PeerRole::Admin)
            {
                serve_admin(
                    tls_stream,
                    peer,
                    cert_role,
                    certificate.as_ref(),
                    shared,
                    &list_now,
                );
            } else {
                serve_accepted(
                    tls_stream,
                    peer,
                    (Some(cert_role), certificate),
                    shared,
                    &list_now,
                );
            }
        }
        Err(active) => {
            connection::refuse_for_capacity(&mut tls_stream, peer, active, shared.max_connections);
        }
    }
}

/// Runs one remote administration session inside `catch_unwind`: the `HELLO` gate with the
/// administration policy, then [`crate::admin::session::run`]. `certificate` is the peer's
/// authenticated client certificate.
fn serve_admin<S>(
    stream: S,
    peer: Option<SocketAddr>,
    role: PeerRole,
    certificate: Option<&rustls::pki_types::CertificateDer<'static>>,
    shared: &ViewerListener,
    admitted: &dyn Fn(),
) where
    S: crate::admin::SessionIo,
{
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::admin::serve_connection(stream, role, certificate, peer, &shared.core, admitted)
    }));
    report_connection_result(peer, result);
}

/// Runs the viewer handler inside `catch_unwind` (`worker` builds: the render-capable
/// handler with the coordinator's `WELCOME` advertisement).
///
/// The library database is not opened here: the connection's [`LibraryHandle`] opens it
/// when (and only if) the peer sends a library request.
#[cfg(feature = "worker")]
fn serve_accepted<S>(
    stream: S,
    peer: Option<SocketAddr>,
    (cert_role, certificate): PeerAuth,
    shared: &ViewerListener,
    admitted: &dyn Fn(),
) where
    S: Read + Write + crate::stream_emit::TimeoutRead + crate::stream_emit::TimeoutWrite,
{
    let library = library_for(shared, peer);
    let render = &shared.render;
    let viewer = viewer_key(certificate.as_ref(), peer);
    let preference = super::socket::payload_preference_for_choice(render.payload, peer);
    let ctx = connection::ViewerContext {
        threads: render.threads,
        gpu: &render.gpu,
        db: &library,
        compute_mode: render.compute_mode,
        encodings: &preference,
        link: Some(connection::LinkSettings {
            choice: render.payload,
            peer: viewer.clone(),
            loopback: connection::is_loopback_peer(peer),
        }),
        own_lane: render.own_lane,
        registry: render.registry.as_ref(),
        cert_role,
        coordinator: Some((&render.coordinator, viewer)),
        assets: render.assets.as_deref(),
        jobs: render.jobs.as_ref(),
        admitted: Some(admitted),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        connection::handle_viewer_connection(stream, &ctx)
    }));
    report_connection_result(peer, result);
}

/// Library-only counterpart of the `worker` build's `serve_accepted`.
#[cfg(not(feature = "worker"))]
fn serve_accepted<S: Read + Write + connection::ClearHandshakeTimeout>(
    stream: S,
    peer: Option<SocketAddr>,
    (cert_role, _certificate): PeerAuth,
    shared: &ViewerListener,
    admitted: &dyn Fn(),
) {
    // The library-only handler has no `HELLO` hook: the connection is listed as it starts.
    admitted();
    let library = library_for(shared, peer);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        connection::handle_connection(stream, &library, cert_role)
    }));
    report_connection_result(peer, result);
}

#[cfg(test)]
mod tests {
    use super::PreAuthAddresses;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    fn peer(last: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)), 40_000)
    }

    #[test]
    fn admits_up_to_the_cap_refuses_the_next_and_a_release_frees_one() {
        let counter = PreAuthAddresses::new(2);
        let first = counter
            .try_admit(Some(peer(1)))
            .expect("1st is under the cap");
        let _second = counter
            .try_admit(Some(peer(1)))
            .expect("2nd is exactly at the cap");
        assert_eq!(
            counter
                .try_admit(Some(peer(1)))
                .expect_err("3rd is over the cap"),
            2,
            "the refusal reports how many the address already holds"
        );

        drop(first);
        let _third = counter
            .try_admit(Some(peer(1)))
            .expect("a slot freed by drop can be taken again");
        counter
            .try_admit(Some(peer(1)))
            .expect_err("back at the cap after the re-admission");
    }

    #[test]
    fn addresses_are_counted_independently_and_the_map_empties_again() {
        let counter = PreAuthAddresses::new(1);
        let a = counter.try_admit(Some(peer(1))).unwrap();
        let b = counter
            .try_admit(Some(peer(2)))
            .expect("another address has its own count");
        counter.try_admit(Some(peer(1))).unwrap_err();
        drop((a, b));
        assert!(counter.held.lock().unwrap().is_empty());
    }

    #[test]
    fn an_ipv4_mapped_ipv6_peer_counts_as_its_ipv4_address() {
        let counter = PreAuthAddresses::new(1);
        let _v4 = counter.try_admit(Some(peer(7))).unwrap();
        let mapped = Ipv4Addr::new(10, 0, 0, 7).to_ipv6_mapped();
        let via_v6 = Some(SocketAddr::new(IpAddr::V6(mapped), 40_001));
        counter
            .try_admit(via_v6)
            .expect_err("the mapped form is the same address");
        let other = Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 40_002));
        counter
            .try_admit(other)
            .expect("a different address is admitted");
    }

    #[test]
    fn a_refused_attempt_counts_for_nothing() {
        let counter = PreAuthAddresses::new(1);
        let held = counter.try_admit(Some(peer(3))).unwrap();
        for _ in 0..5 {
            counter.try_admit(Some(peer(3))).unwrap_err();
        }
        drop(held);
        counter
            .try_admit(Some(peer(3)))
            .expect("refusals never leaked a count");
    }
}
