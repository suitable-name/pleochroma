//! The worker port (default 7880, separate from the viewer port): where `pleochroma-worker join`
//! workers dial in.
//!
//! Mutual TLS with WORKER client certificates only (`crate::serve::Auth` with role `Worker`,
//! `allowlist-workers.txt`), a `HELLO` that must say `PeerRole::Worker` and carry the worker's
//! `RenderCapability`, the full build-compatibility gate, then `WELCOME.registration =
//! Some(worker_id)` and the connection moves into the [`Registry`].

use super::registry::{Registry, WorkerInfo};
use crate::{
    control::{
        shutdown::{ConnectionSet, Latch, ListenerStop},
        table::ConnectionTable,
    },
    serve::{self, Auth, ConnectionLimiter, ConnectionSlot, HelloCheck, TlsStream, accept_tls},
};
use pleochroma_net::{
    handshake,
    messages::{NetError, PeerRole, Welcome, WorkerRegistration, negotiate},
};
use std::{
    net::{SocketAddr, TcpListener},
    sync::Arc,
    thread,
    time::Duration,
};

/// How often a joined computer's table entry checks that the computer is still registered.
const TABLE_GUARD_INTERVAL: Duration = Duration::from_millis(500);

/// Everything the worker port needs, gathered by `crate::serve::start`.
pub struct WorkerListenerConfig {
    /// Where to listen (default: the viewer host, port 7880).
    pub bind_addr: SocketAddr,
    /// The same server certificate and CA as the viewer port.
    pub tls_config: Arc<rustls::ServerConfig>,
    /// Role `Worker` and the worker allowlist.
    pub auth: Auth,
    /// The worker port's own `--max-connections` cap (separate from the viewers').
    pub limiter: ConnectionLimiter,
    /// `--max-connections`, for the refusal message.
    pub max_connections: usize,
    /// Where joined workers are registered.
    pub registry: Arc<Registry>,
    /// The coordinator's table of live connections: every worker that joins is listed in it
    /// (with a kill switch) until it leaves the registry, so the status shows it online and
    /// the window can disconnect it.
    pub table: ConnectionTable,
}

/// A running worker port: where it listens and what stops it.
pub struct WorkerListener {
    /// The bound address (the real port when the configured one was `0`).
    pub addr: SocketAddr,
    /// Stops the accept loop; see [`ListenerStop`].
    pub stop: ListenerStop,
    /// Raised when the accept loop thread has ended.
    pub exited: Latch,
    /// The worker sockets still in their TLS / `HELLO` handshake. A worker that finished
    /// joining is no longer here: it lives in the [`Registry`], which closes it.
    pub connections: ConnectionSet,
}

/// Binds the worker port and accepts joining workers on a background thread, one short-lived thread
/// per connection (TLS, `HELLO`, `WELCOME`, then hand-over to the registry).
///
/// Returns the bound address with what stops the listener.
///
/// The accept loop ends when [`WorkerListener::stop`] is raised, which also frees the port.
///
/// # Errors
///
/// A human-readable message if the bind fails.
pub fn spawn_worker_listener(config: WorkerListenerConfig) -> Result<WorkerListener, String> {
    let listener = TcpListener::bind(config.bind_addr)
        .map_err(|e| format!("failed to bind the worker port {}: {e}", config.bind_addr))?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("worker port bound but local_addr() failed: {e}"))?;
    tracing::info!(
        "pleochroma-worker serve: worker port on {bound} (`pleochroma-worker join {bound}`; worker certificates only)"
    );
    let config = Arc::new(config);
    // Caps unauthenticated connections: the slot is taken at accept and held until
    // `accept_worker` returns, independent of the post-TLS `config.limiter`.
    let handshake_limiter = ConnectionLimiter::new(
        config
            .max_connections
            .saturating_mul(serve::PRE_AUTH_HANDSHAKE_MULTIPLIER),
    );
    let stop = ListenerStop::new(bound);
    let loop_stop = stop.clone();
    let exited = Latch::new();
    let exit_guard = exited.exit_guard();
    let connections = ConnectionSet::new();
    let loop_connections = connections.clone();
    thread::spawn(move || {
        let _exit = exit_guard;
        for incoming in listener.incoming() {
            // Right after the accept, before the stream is looked at: the wake-up
            // connection of `ListenerStop::stop` is dropped here.
            if loop_stop.is_stopped() {
                return;
            }
            match incoming {
                Ok(stream) => {
                    let peer = stream.peer_addr().ok();
                    serve::tune_accepted_socket(&stream, peer);
                    serve::apply_handshake_timeout(&stream, peer);
                    let Ok(handshake_slot) = handshake_limiter.try_acquire() else {
                        tracing::warn!(
                            "worker connection {peer:?}: refusing -- too many unauthenticated connections"
                        );
                        continue;
                    };
                    let tracked = loop_connections.register(&stream);
                    let config = Arc::clone(&config);
                    thread::spawn(move || {
                        let _handshake_slot = handshake_slot;
                        let _tracked = tracked;
                        accept_worker(stream, peer, &config);
                    });
                }
                Err(e) => tracing::warn!("worker port accept error: {e}"),
            }
        }
    });
    Ok(WorkerListener {
        addr: bound,
        stop,
        exited,
        connections,
    })
}

/// TLS (role + allowlist), the connection cap, then [`register_worker`].
fn accept_worker(
    stream: std::net::TcpStream,
    peer: Option<SocketAddr>,
    config: &WorkerListenerConfig,
) {
    // A second handle to the socket: the table's kill switch for this connection.
    let kill_switch = stream.try_clone().ok();
    let Some((mut tls, cert_role)) = accept_tls(stream, &config.tls_config, &config.auth, peer)
    else {
        return; // accept_tls already logged why
    };
    let certificate = tls
        .conn
        .peer_certificates()
        .and_then(<[_]>::first)
        .map(|c| c.clone().into_owned());
    let slot = match config.limiter.try_acquire() {
        Ok(slot) => slot,
        Err(active) => {
            serve::refuse_for_capacity(&mut tls, peer, active, config.max_connections);
            return;
        }
    };
    match register_worker(tls, peer, cert_role, slot, &config.registry) {
        Ok(Some(worker_id)) => {
            let capacity = config.registry.capacity();
            tracing::info!(
                "coordinator: worker #{worker_id} joined from {peer:?} -- now {} worker connection(s), {} CPU \
                 thread(s), {} GPU(s)",
                capacity.workers,
                capacity.threads,
                capacity.gpus
            );
            // Listed in the table once it has joined (a refused connection never shows).
            if let Some(certificate) = certificate {
                let tracked =
                    config
                        .table
                        .register_certificate(&certificate, cert_role, peer, kill_switch);
                keep_listed_while_joined(tracked, Arc::clone(&config.registry), worker_id);
            }
        }
        Ok(None) => {} // refused; already logged
        Err(e) => tracing::warn!("worker connection {peer:?}: handshake failed: {e}"),
    }
}

/// Keeps `tracked` (the worker's entry in the connection table) alive for as long as
/// `worker_id` is registered, then drops it, which removes the entry and reports the device as
/// disconnected. The registry owns the connection, not this listener, so a small thread
/// watches for the registry entry to go (a dropped worker, a disconnect or the shutdown).
fn keep_listed_while_joined(
    tracked: crate::control::table::TrackedConnection,
    registry: Arc<Registry>,
    worker_id: u32,
) {
    let spawned = thread::Builder::new()
        .name("worker-table-guard".to_owned())
        .spawn(move || {
            let _tracked = tracked;
            while registry
                .workers()
                .iter()
                .any(|(info, _)| info.worker_id == worker_id)
            {
                thread::sleep(TABLE_GUARD_INTERVAL);
            }
        });
    if let Err(error) = spawned {
        // The closure (and with it the entry) is dropped: the worker still works, it is only
        // not listed.
        tracing::warn!("worker port: the connection list could not follow a worker ({error})");
    }
}

/// The worker port's `HELLO`/`WELCOME`: role gate (worker `HELLO` and worker
/// certificate), build compatibility, a fresh `worker_id` in `WELCOME.registration`,
/// then the connection goes into `registry`, idle. `Ok(None)` if refused (the refusal
/// was sent).
///
/// # Errors
///
/// A transport failure reading `HELLO` or writing `WELCOME`.
fn register_worker(
    mut tls: TlsStream,
    peer: Option<SocketAddr>,
    cert_role: PeerRole,
    connection_slot: ConnectionSlot,
    registry: &Registry,
) -> Result<Option<u32>, NetError> {
    let hello = match serve::read_and_check_hello(&mut tls, PeerRole::Worker, Some(cert_role))? {
        HelloCheck::Accepted(hello) => hello,
        HelloCheck::Refused(refusal) => {
            serve::send_refusal(&mut tls, &refusal);
            return Ok(None);
        }
    };
    let local_hello = handshake::local_hello();
    if let Err(incompatible) = handshake::verify_compatible(&local_hello, &hello) {
        serve::refuse_incompatible_handshake(&mut tls, &local_hello, &hello, incompatible);
        return Ok(None);
    }
    let Some(capability) = hello.capability.clone() else {
        // Unreachable: the role gate only accepts a worker HELLO carrying one.
        return Ok(None);
    };

    // The WORKER encodes FRAMEs on this link and the coordinator decodes them; one build
    // runs on both ends, so every encoding the worker can decode it can also encode.
    let payload_encoding = negotiate(serve::payload_preference_for(peer), &hello.accept_encodings);
    // The certificate's `worker:<label>` -- the stable identity `--pin-interactive-worker`
    // names (the TLS layer already checked the role and the allowlist).
    let label = tls
        .conn
        .peer_certificates()
        .and_then(<[_]>::first)
        .and_then(|der| crate::pki::worker_label_of_certificate(der.as_ref()));
    let worker_id = registry.allocate_id();
    // A worker connection is not a viewer: nothing to advertise to it (`Welcome::new` sets
    // the zoning bit false). The coordinator still sends zones ahead of a zoned chunk: the
    // worker runs the same protocol version and release, so it reads them.
    let welcome = Welcome::new(
        local_hello.build_hash,
        local_hello.source_hash,
        None,
        false,
        false,
        Some(WorkerRegistration { worker_id }),
        payload_encoding,
    );
    pleochroma_net::messages::write_message(&mut tls, &welcome)?;
    // The registry (liveness, request execution) sets its own deadlines from here on.
    let _ = tls.sock.set_read_timeout(None);
    let _ = tls.sock.set_write_timeout(None);

    registry.insert(
        WorkerInfo {
            worker_id,
            capability,
            peer,
            label,
            payload_encoding,
        },
        Box::new(tls),
        Some(connection_slot),
    );
    Ok(Some(worker_id))
}
