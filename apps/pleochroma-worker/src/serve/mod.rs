//! The `serve` subcommand -- the coordinator (renders itself only with `--render`;
//! viewers and joining workers on separate ports).
//!
//! Accepts `pleochroma_net` viewer connections over TCP and replies, and (on a `worker`
//! build) accepts joining render workers on a separate worker port.
//!
//! # What `serve` does
//!
//! - **Always** serves the read-only design-library protocol ([`library`], backed by
//!   [`pleochroma_vault`]) to viewers on `--bind` (default 7878).
//! - **With `--render`** (implied by `--only-gpu`/`--only-cpu`; `worker` builds only)
//!   renders viewers' `RenderRequest`s with its own CPU/GPU lane. Without it no GPU is
//!   acquired and no sample is traced here: **`serve` does not render by default.**
//! - **On `worker` builds** opens the worker port (`--worker-bind`, default 7880) where
//!   `pleochroma-worker join` workers dial in and register in the coordinator's
//!   [`crate::coordinator::Registry`]; a viewer's request is then executed over them
//!   (and the own lane) by [`crate::coordinator::serve_render`] and its siblings.
//!
//! `WELCOME.render` follows [`crate::coordinator::viewer_render_capability`]: `None`
//! without `--render` and without joined workers (a library-only remote to the GUI),
//! the own lane's plain `Cpu`/`Gpu` backend with `--render` alone, and
//! `Backend::Coordinator{workers, threads, gpus}` as soon as a worker has joined.
//!
//! # Ports and trust
//!
//! Viewers use 7878 plus the viewer enrollment listener 7879; workers use 7880 plus the
//! worker enrollment listener 7881 ([`crate::enroll`]). Each TLS listener checks its own
//! client-certificate role and its own allowlist (`allowlist-viewers.txt`,
//! `allowlist-workers.txt`; see `crate::pki::role` and [`tls`]); the `HELLO` gate
//! ([`handshake`]) refuses a mismatched role with `ROLE_REFUSED`.
//!
//! **Remote administration** (off unless `--allow-remote-admin`) shares the viewer port: a
//! monitor or admin certificate, listed in `allowlist-monitors.txt` or `allowlist-admins.txt`,
//! whose `HELLO` role matches its certificate, gets an administration session
//! ([`crate::admin`]) instead of the viewer handler. Off, such a `HELLO` is refused as before.
//!
//! **The library protocol sits behind the exact same mutual-TLS authentication as the
//! render protocol -- there is no separate check.** Both are message variants read off
//! the same already-authenticated stream.
//!
//! # TLS
//!
//! Connection handlers are generic over `Read + Write` and never name `TcpStream`
//! directly; only [`tls::accept_tls`] wraps an accepted socket in
//! `rustls::StreamOwned<ServerConnection, TcpStream>`. Mutual TLS answers "may this peer
//! talk to me at all", a different question from
//! `pleochroma_net::handshake::verify_compatible`'s physics-compatibility check. TLS
//! validates the certificate chain only; the role and allowlist check in
//! [`tls::accept_tls`] is the actual authorization decision (no CRL/OCSP, just a
//! fingerprint list). `--trust-any-client-cert` skips the allowlists (never the role
//! check) and must be an explicit flag. `--insecure-no-tls` skips TLS entirely, is
//! refused on a non-loopback `--bind`, disables the worker port and enrollment, and
//! every such connection logs a warning.
//!
//! # Robustness
//!
//! - Every `RenderRequest`/`LibraryRequest` is validated defensively; failures get an
//!   `Error`/`NotFound` reply, not a dropped connection.
//! - Tracing runs inside `catch_unwind` (worker builds); the accept loop wraps the
//!   whole per-connection handler in a second `catch_unwind` as backstop.
//! - Every listener binds to loopback only unless `--allow-remote` is passed,
//!   independent of TLS.
//! - An authenticated client can still ask for an absurd render or search:
//!   `crate::validate`'s caps bound the former, and `SEARCH_RESULT_CAP` (1000 rows)
//!   bounds the latter.
//! - Every accepted connection gets `TCP_NODELAY` and TCP keepalive best-effort (see
//!   [`tune_accepted_socket`]), and [`HANDSHAKE_TIMEOUT`] until its first `HELLO`
//!   (slowloris), cleared by the connection handler once `HELLO` arrives.
//! - `--max-connections` (default 64, see [`ConnectionLimiter`]) bounds AUTHENTICATED
//!   connections, counted separately for viewers and joined workers. A connection over
//!   the cap is still accepted and given a definitive `<- ERROR` refusal
//!   ([`connection::refuse_for_capacity`]). For TLS the real slot is acquired only AFTER
//!   [`tls::accept_tls`] succeeds; a second, wider limiter (see
//!   [`limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER`]) bounds bare, not-yet-authenticated
//!   connections in flight, refused silently. A per-source-address cap on those
//!   (`--max-preauth-per-ip`, default 8, checked first so one address cannot take the
//!   wider cap's slots) closes an over-cap connection at once with a logged reason.
//!   Both pre-authentication counts are released as soon as [`tls::accept_tls`]
//!   succeeds, so an authenticated connection counts only against `--max-connections`.

use std::{
    net::{SocketAddr, TcpListener},
    sync::Arc,
    thread,
    time::Duration,
};

#[cfg(feature = "worker")]
use crate::{cli::ComputeMode, coordinator};
use crate::{
    cli::ServeArgs,
    control::{
        api::{ControlParts, ControlState},
        events::EventHub,
        shutdown::{ConnectionSet, Latch, ListenerStop},
        status::ServeInfo,
        table::ConnectionTable,
    },
    enroll,
};
use pleochroma_net::{admin::status::CoordinatorEvent, messages::PeerRole};

mod accept;
mod connection;
pub(crate) mod handshake;
pub mod library;
mod lifecycle;
mod limiter;
mod socket;
#[cfg(feature = "worker")]
mod tilt;
mod tls;
// The GPU shader warm-up of a render computer (see its own doc comment).
#[cfg(feature = "worker")]
pub(crate) mod warmup;
// Zones received over the wire (see its own doc comment).
#[cfg(feature = "worker")]
pub(crate) mod zoning;
// These tests drive a real render round trip and need the `worker` feature.
// `library`'s own tests cover the library protocol unconditionally.
// `pub(crate)` so other test modules (the coordinator's) share its temp-dir helper.
#[cfg(all(test, feature = "worker"))]
pub(crate) mod tests;

pub use lifecycle::{
    DEFAULT_SHUTDOWN_DEADLINE, ShutdownHandle, ShutdownReport, StartingServe, run_until,
    start_background,
};

#[cfg(feature = "worker")]
pub(crate) use connection::refuse_for_capacity;
#[cfg(feature = "worker")]
pub(crate) use limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER;
pub use limiter::{ConnectionLimiter, ConnectionSlot};
pub(crate) use socket::HANDSHAKE_TIMEOUT;
// Ungated (unlike the `worker`-only re-exports below): `crate::enroll`'s listener runs
// in every build, and needs these two to apply the same pre-HELLO deadline to
// its own TLS-handshake-then-one-message exchange.
pub(crate) use socket::{DeadlineIo, DeadlineSocket};
#[cfg(feature = "worker")]
pub(crate) use socket::{apply_handshake_timeout, tune_accepted_socket};
pub(crate) use tls::Auth;
// For `certs`' tests, which run real handshakes through this config.
#[cfg(test)]
pub(crate) use tls::server_config_with_resolver;
#[cfg(feature = "worker")]
pub(crate) use tls::{TlsStream, accept_tls};

#[cfg(not(feature = "worker"))]
pub use connection::handle_connection;
#[cfg(feature = "worker")]
pub(crate) use connection::{
    LinkSettings, RequestContext, is_loopback_peer, local_render_capability,
    refuse_incompatible_handshake, serve_requests,
};
#[cfg(feature = "worker")]
pub use connection::{handle_connection, handle_connection_with_gpu};
#[cfg(feature = "worker")]
pub(crate) use handshake::{HelloCheck, read_and_check_hello, send_refusal};
#[cfg(feature = "worker")]
pub(crate) use socket::payload_preference_for;
#[cfg(feature = "worker")]
pub(crate) use tilt::{CancelPoll, handle_tilt_curves_request, poll_for_cancel};

/// Everything a running `serve` offers its control layer: the listening addresses, the joined
/// workers and jobs, the pairing and device API (see [`crate::control::api`]) and the status.
///
/// It is shared (`Arc`) between the [`ServeHandle`] and the viewer accept loop, so a remote
/// administration session ([`crate::admin`]) acts through exactly the methods the coordinator
/// window uses. A `ServeHandle` derefs to it, so `handle.viewer_addr` and `handle.revoke(..)`
/// work as before.
pub struct ServeCore {
    /// The viewer listener (`--bind`).
    pub viewer_addr: SocketAddr,
    /// The viewer enrollment listener, if started.
    pub viewer_enroll_addr: Option<SocketAddr>,
    /// The worker port, if opened (`worker` builds with TLS and without `--no-workers`).
    pub worker_addr: Option<SocketAddr>,
    /// The worker enrollment listener, if started.
    pub worker_enroll_addr: Option<SocketAddr>,
    /// The joined-worker registry, present exactly when [`Self::worker_addr`] is.
    #[cfg(feature = "worker")]
    pub registry: Option<Arc<coordinator::Registry>>,
    /// The coordinator's job execution state (always present on `worker` builds).
    #[cfg(feature = "worker")]
    pub coordinator: Option<Arc<coordinator::Coordinator>>,
    /// Pairing codes, device removal and renaming, and the event stream (see
    /// [`crate::control::api`]).
    pub(crate) control: ControlState,
    /// What this `serve` was started with that the status reports (see
    /// [`crate::control::status`]).
    pub(crate) info: ServeInfo,
    /// Every owner's jobs, for the coordinator window; present when the job store is.
    #[cfg(feature = "worker")]
    pub(crate) job_admin: Option<crate::control::jobs::JobAdmin>,
}

/// What [`start`] brought up.
///
/// Every listener's actual bound address (useful with port `0`) and, on `worker`
/// builds, the coordinator's joined-worker registry (both through [`ServeCore`], which this
/// derefs to). The listeners run on background threads; [`Self::wait`] blocks on the viewer
/// accept loop.
pub struct ServeHandle {
    /// What the control layer and the remote administration sessions share.
    core: Arc<ServeCore>,
    /// Raised when the viewer accept loop thread has ended.
    viewer_exit: Latch,
    lifecycle: Arc<lifecycle::Lifecycle>,
}

impl std::ops::Deref for ServeHandle {
    type Target = ServeCore;

    fn deref(&self) -> &ServeCore {
        &self.core
    }
}

impl ServeHandle {
    /// The shared part of this `serve`, for threads that outlive a borrow of the handle.
    #[must_use]
    pub const fn core(&self) -> &Arc<ServeCore> {
        &self.core
    }

    /// Blocks until the viewer accept loop ends: forever, until [`Self::shutdown`] (or a
    /// [`ShutdownHandle`]) stops it.
    pub fn wait(self) {
        self.viewer_exit.wait();
    }

    /// A cloneable handle that can shut this `serve` down from another thread.
    #[must_use]
    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle::new(Arc::clone(&self.lifecycle))
    }

    /// Shuts `serve` down: stops every listener (so the ports are free again), closes the
    /// connections, stops the job service and waits up to `deadline` for the threads.
    ///
    /// See the `lifecycle` module doc comment for the order and for what survives a timeout.
    #[must_use]
    pub fn shutdown(&self, deadline: Duration) -> ShutdownReport {
        self.shutdown_handle().shutdown(deadline)
    }
}

/// Runs the `serve` subcommand: starts it with [`start_background`], then serves until the accept
/// loop ends.
///
/// Without a stop signal that is forever; the command line program uses [`run_until`] to add Ctrl-C
/// and SIGTERM.
///
/// # Errors
///
/// Whatever [`start`] returns.
pub fn run(args: &ServeArgs) -> Result<(), String> {
    start_background(args.clone()).wait()?.wait();
    Ok(())
}

/// Brings up every listener `args` asks for and returns without blocking.
///
/// Refuses a non-loopback bind address (viewer, worker or enrollment) unless
/// `args.allow_remote` is set, independent of TLS. Builds the mutual-TLS server config
/// from `args.ca`/`args.cert`/`args.key` (required unless `args.insecure_no_tls`),
/// pre-loading the viewer allowlist to fail fast on a bad path (the per-connection check
/// re-reads it every time). Preflights the library database once; one that cannot be
/// opened is a warning, not an error: the coordinator then runs without a library and
/// answers library requests with a plain "no library" error. `worker` builds
/// acquire this process's `GpuBackend` only for `--render` without `--only-cpu`; open
/// the worker port with its own limiter, registry and liveness thread; and start the
/// worker enrollment listener.
///
/// # Errors
///
/// A human-readable message if an address doesn't parse or is non-loopback without
/// `--allow-remote`, if `--insecure-no-tls` is combined with a non-loopback bind, if TLS
/// is enabled but `--ca`/`--cert`/`--key` are missing or unloadable, if an allowlist
/// can't be loaded, or if a bind fails. A failed start stops whatever it had already
/// started.
pub fn start(args: &ServeArgs) -> Result<ServeHandle, String> {
    start_inner(
        args,
        #[cfg(feature = "worker")]
        coordinator::LivenessConfig::default(),
    )
}

/// [`start`] with a custom joined-worker liveness schedule -- for tests that need a
/// silent worker dropped in milliseconds rather than 30 s.
///
/// # Errors
///
/// See [`start`].
#[cfg(all(test, feature = "worker"))]
pub(crate) fn start_with_liveness(
    args: &ServeArgs,
    liveness: coordinator::LivenessConfig,
) -> Result<ServeHandle, String> {
    start_inner(args, liveness)
}

fn start_inner(
    args: &ServeArgs,
    #[cfg(feature = "worker")] liveness: coordinator::LivenessConfig,
) -> Result<ServeHandle, String> {
    let mut lifecycle = lifecycle::Lifecycle::new();
    let started = start_parts(
        args,
        #[cfg(feature = "worker")]
        liveness,
        &mut lifecycle,
    );
    match started {
        Ok(started) => Ok(ServeHandle {
            core: started.core,
            viewer_exit: started.viewer_exit,
            lifecycle: Arc::new(lifecycle),
        }),
        Err(message) => {
            // Whatever was already listening is stopped, so a retry finds the ports free.
            let _ = lifecycle.shutdown(Duration::from_secs(2));
            Err(message)
        }
    }
}

/// What [`start_parts`] brought up, before it becomes a [`ServeHandle`].
struct Started {
    core: Arc<ServeCore>,
    viewer_exit: Latch,
}

/// What the address watch needs, held back until the viewer port is bound (the discovery
/// announcement names that port).
struct WatchParts {
    certificate: Arc<crate::certs::SwappableCert>,
    /// The `--cert` and `--key` files (or the defaults next to `--ca`) the watch reissues.
    files: crate::certs::ServerFiles,
}

/// The server-certificate part of start-up: reissues a certificate that lacks the fixed
/// name and builds the transport. With TLS it also returns the pieces of the address watch
/// that reissues and swaps the certificate when the LAN addresses change;
/// [`start_address_watch`] starts it once the viewer port is bound.
fn prepare_transport(
    args: &ServeArgs,
    bind_addr: SocketAddr,
) -> Result<(tls::Transport, Option<WatchParts>), String> {
    // The files the coordinator really runs with: `--cert` and `--key`, not a guess next to
    // `--ca`.
    let files = crate::certs::ServerFiles::from_args(args);
    if !args.insecure_no_tls
        && let Some(files) = &files
        && let Err(error) = crate::certs::migrate_server_certificate(
            files,
            &crate::control::status::extra_certificate_names(),
            &pleochroma_net::netinfo::lan_ipv4(),
        )
    {
        tracing::warn!(
            "pleochroma-worker serve: could not reissue the server certificate with the fixed name ({error})"
        );
    }
    let (transport, certificate) = tls::build_transport_with_cert(args, bind_addr)?;
    let watch = certificate
        .zip(files)
        .map(|(certificate, files)| WatchParts { certificate, files });
    Ok((transport, watch))
}

/// Announces this coordinator on the network (mDNS) for the CA in `args.ca`, on the viewer
/// `port`, under this computer's display name. `None` (with a logged reason) when the CA
/// cannot be read or the announcement cannot start: discovery is a convenience, never a
/// reason to fail the start.
fn start_announcement(args: &ServeArgs, port: u16) -> Option<pleochroma_net::discovery::Announcer> {
    let ca = args.ca.as_deref()?;
    let certificates = match pleochroma_net::tls::load_certs(ca) {
        Ok(certificates) => certificates,
        Err(error) => {
            tracing::warn!(
                "pleochroma-worker serve: not announcing on the network, the CA could not be read ({error})"
            );
            return None;
        }
    };
    let fingerprint = pleochroma_net::tls::fingerprint(certificates.first()?);
    let name = crate::control::status::configured_display_name();
    match pleochroma_net::discovery::announce(&name, port, &fingerprint) {
        Ok(announcer) => {
            tracing::info!(
                "pleochroma-worker serve: announcing {name:?} on the network (port {port})"
            );
            Some(announcer)
        }
        Err(error) => {
            tracing::warn!("pleochroma-worker serve: could not announce on the network: {error}");
            None
        }
    }
}

/// Starts the certificate address watch and, when the discovery setting is on, the network
/// announcement. Returns the watch and whether the announcement runs.
///
/// The announcement belongs to the watch: its swap hook is wrapped with
/// [`crate::certs::announcing_hook`], which holds the announcer, so the announcement ends
/// (with its goodbye) when shutdown stops the watch. Address changes are also published as
/// events.
///
/// The discovery setting comes from [`crate::control::status::discovery_setting`], not from
/// `ServeArgs`: `ServeArgs` mirrors the command line, which has no discovery flag. It is off
/// without a loaded settings file and for a loopback `viewer` address.
fn start_address_watch(
    args: &ServeArgs,
    parts: WatchParts,
    viewer: SocketAddr,
    hub: &EventHub,
) -> (crate::certs::AddressWatch, bool) {
    // A coordinator that listens on loopback only has nobody on the network to announce to.
    let announcer = if crate::control::status::discovery_setting() && !viewer.ip().is_loopback() {
        start_announcement(args, viewer.port())
    } else {
        None
    };
    let discoverable = announcer.is_some();
    let events = hub.clone();
    let base: crate::certs::SwapHook = Arc::new(move |swap| {
        tracing::info!(
            "pleochroma-worker serve: server certificate swapped, now listing {} address(es)",
            swap.addresses.len()
        );
        let addresses: Vec<String> = swap.addresses.iter().map(ToString::to_string).collect();
        events.publish(&CoordinatorEvent::CertificateSwapped {
            addresses: addresses.clone(),
        });
        events.publish(&CoordinatorEvent::AddressesChanged { addresses });
    });
    let hook = if let Some(announcer) = announcer {
        crate::certs::announcing_hook(base, Arc::new(announcer))
    } else {
        base
    };
    let watch = crate::certs::AddressWatch::start(crate::certs::CertificateRefresh::new(
        parts.files,
        crate::control::status::extra_certificate_names(),
        parts.certificate,
        hook,
    ));
    (watch, discoverable)
}

/// Makes the worker registry tell the activity feed when a render computer joins or leaves,
/// whoever caused it: the computer itself, a liveness timeout or an administrator.
#[cfg(feature = "worker")]
fn publish_worker_events(registry: &coordinator::Registry, hub: &EventHub) {
    use crate::{control::status::worker_display_name, coordinator::WorkerEvent};
    use pleochroma_net::admin::status::WorkerId;

    let hub = hub.clone();
    registry.set_observer(move |event| {
        let published = match event {
            WorkerEvent::Joined(info) => CoordinatorEvent::WorkerJoined {
                id: WorkerId(info.worker_id),
                name: worker_display_name(info),
            },
            WorkerEvent::Left(info) => CoordinatorEvent::WorkerLeft {
                id: WorkerId(info.worker_id),
                name: worker_display_name(info),
            },
        };
        hub.publish(&published);
    });
}

/// Makes the job service tell the activity feed when a job is submitted, starts, or ends, so
/// the feed shows what the coordinator does on its own and not only what an administrator
/// does.
#[cfg(feature = "worker")]
fn publish_job_events(jobs: &crate::jobs::JobService, control: &ControlState) {
    use crate::{
        control::jobs::{owner_display, state_of},
        jobs::JobChange,
    };
    use pleochroma_net::admin::status::JobId as StatusJobId;

    let control = control.clone();
    jobs.set_observer(move |owner, info, change| {
        let (owner_name, _) = owner_display(owner, &control.device_names());
        let id = StatusJobId(info.job_id);
        let event = match change {
            JobChange::Submitted => CoordinatorEvent::JobSubmitted {
                id,
                owner: owner_name,
                label: info.label.clone(),
            },
            JobChange::Changed => CoordinatorEvent::JobStateChanged {
                id,
                state: state_of(info.phase),
                owner: owner_name,
                label: info.label.clone(),
            },
        };
        control.hub().publish(&event);
    });
}

fn start_parts(
    args: &ServeArgs,
    #[cfg(feature = "worker")] liveness: coordinator::LivenessConfig,
    lifecycle: &mut lifecycle::Lifecycle,
) -> Result<Started, String> {
    let bind_addr = checked_addr(&args.bind, "--bind", args.allow_remote)?;
    let (transport, watch_parts) = prepare_transport(args, bind_addr)?;
    let db_path = socket::resolve_library_db_path(args);
    let library_present = library_available(args, &db_path);
    let limiter = ConnectionLimiter::new(args.max_connections);
    let handshake_limiter = ConnectionLimiter::new(
        args.max_connections
            .saturating_mul(limiter::PRE_AUTH_HANDSHAKE_MULTIPLIER),
    );

    let listener =
        TcpListener::bind(bind_addr).map_err(|e| format!("failed to bind {bind_addr}: {e}"))?;
    let viewer_addr = listener
        .local_addr()
        .map_err(|e| format!("viewer listener bound but local_addr() failed: {e}"))?;
    tracing::info!(
        "pleochroma-worker serve: protocol v{}, viewers on {viewer_addr} ({}; {})",
        pleochroma_net::messages::PROTOCOL_VERSION,
        transport_summary(&transport),
        capability_summary(args),
    );

    // One hub and one table from here on, so the worker port registers its connections in
    // the same table as the viewer port and everything publishes into the same stream.
    let hub = EventHub::new();
    let table = ConnectionTable::new(hub.clone());
    // Without TLS there is no address watch, and so no announcement.
    let discovery = watch_parts.is_some_and(|parts| {
        let (watch, announced) = start_address_watch(args, parts, viewer_addr, &hub);
        lifecycle.set_watch(watch);
        announced
    });

    let viewer_auth = match &transport {
        tls::Transport::Tls { auth, .. } => Some(auth.clone()),
        tls::Transport::Insecure => None,
    };
    let (viewer_enroll_addr, viewer_registry) =
        start_viewer_enrollment(args, bind_addr, &transport, lifecycle)?;

    #[cfg(feature = "worker")]
    let (worker, render) = start_worker_side(
        args, bind_addr, &transport, liveness, &db_path, lifecycle, &table,
    )?;
    #[cfg(not(feature = "worker"))]
    let worker = WorkerSide::default();
    #[cfg(feature = "worker")]
    {
        lifecycle.set_registry(worker.registry.clone());
        lifecycle.set_jobs(render.jobs.clone());
        if let Some(registry) = &worker.registry {
            publish_worker_events(registry, &hub);
        }
    }

    let connections = ConnectionSet::new();
    #[cfg(feature = "worker")]
    let job_admin = render.jobs.clone().map(crate::control::jobs::JobAdmin::new);
    let control = ControlState::with_table(
        ControlParts::from_args(
            args,
            viewer_auth.as_ref(),
            viewer_registry,
            worker.enroll_registry.clone(),
        ),
        hub,
        table,
    );
    #[cfg(feature = "worker")]
    if let Some(jobs) = &render.jobs {
        publish_job_events(jobs, &control);
    }
    let info = ServeInfo::new(args, db_path.clone(), library_present, discovery);
    // What the control API and the remote administration sessions share with the handle.
    let core = Arc::new(ServeCore {
        viewer_addr,
        viewer_enroll_addr,
        worker_addr: worker.addr,
        worker_enroll_addr: worker.enroll_addr,
        #[cfg(feature = "worker")]
        registry: worker.registry.clone(),
        #[cfg(feature = "worker")]
        coordinator: worker.coordinator,
        control: control.clone(),
        info,
        #[cfg(feature = "worker")]
        job_admin,
    });
    let shared = Arc::new(accept::ViewerListener {
        transport,
        limiter,
        handshake_limiter,
        pre_auth_addresses: accept::PreAuthAddresses::new(args.max_preauth_per_ip),
        db_path,
        library_present,
        max_connections: args.max_connections,
        connections: connections.clone(),
        table: control.table(),
        core: Arc::clone(&core),
        #[cfg(feature = "worker")]
        render,
    });
    let viewer_exit = spawn_viewer_accept(listener, viewer_addr, shared, lifecycle)?;
    lifecycle.add_connections("viewer connections", connections);

    Ok(Started { core, viewer_exit })
}

/// Whether the design library can be served: `false` for "rendering only"
/// (`args.no_library`), and `false` (with a logged reason) when the file cannot be opened.
///
/// A library that cannot be opened is not a failed start: the coordinator runs without one and
/// answers library requests with a plain "no library" error. `db_path` still anchors the asset
/// cache and the job directory. A library that opens now but breaks later is handled per
/// request by `LibraryHandle`.
fn library_available(args: &ServeArgs, db_path: &std::path::Path) -> bool {
    if args.no_library {
        tracing::info!("pleochroma-worker serve: no design library (rendering only)");
        return false;
    }
    match socket::open_library_database(db_path) {
        Ok(database) => {
            drop(database);
            true
        }
        Err(message) => {
            tracing::warn!(
                "pleochroma-worker serve: starting without a design library ({message})"
            );
            false
        }
    }
}

/// Starts the viewer enrollment listener (unless pairing is off or there is no TLS) and hands
/// its stop to `lifecycle`. Returns its address and its pairing-code registry.
fn start_viewer_enrollment(
    args: &ServeArgs,
    bind_addr: SocketAddr,
    transport: &tls::Transport,
    lifecycle: &mut lifecycle::Lifecycle,
) -> Result<(Option<SocketAddr>, Option<Arc<enroll::EnrollRegistry>>), String> {
    let (auth, certificate) = match transport {
        tls::Transport::Tls {
            auth, certificate, ..
        } => (Some(auth), Some(certificate)),
        tls::Transport::Insecure => (None, None),
    };
    let started = start_enrollment(
        args,
        args.enroll_bind.as_deref(),
        bind_addr,
        1,
        auth,
        certificate,
    )?;
    Ok(started.map_or((None, None), |(addr, stop, registry)| {
        lifecycle.add_listener(stop);
        (Some(addr), Some(registry))
    }))
}

/// Starts the viewer accept loop on its own thread and registers it with `lifecycle`. Returns
/// the latch raised when the loop has ended.
fn spawn_viewer_accept(
    listener: TcpListener,
    viewer_addr: SocketAddr,
    shared: Arc<accept::ViewerListener>,
    lifecycle: &mut lifecycle::Lifecycle,
) -> Result<Latch, String> {
    let viewer_stop = ListenerStop::new(viewer_addr);
    let viewer_exit = Latch::new();
    let exit_guard = viewer_exit.exit_guard();
    let loop_stop = viewer_stop.clone();
    thread::Builder::new()
        .name("serve-viewer-accept".to_owned())
        .spawn(move || {
            let _exit = exit_guard;
            accept::run_accept_loop(&listener, &shared, &loop_stop);
        })
        .map_err(|e| format!("could not start the viewer accept thread: {e}"))?;
    lifecycle.add_loop("viewer listener", viewer_stop, viewer_exit.clone());
    Ok(viewer_exit)
}

/// The worker port's part of [`ServeHandle`].
#[derive(Default)]
struct WorkerSide {
    addr: Option<SocketAddr>,
    enroll_addr: Option<SocketAddr>,
    /// The worker enrollment listener's registry (pairing codes for render computers).
    enroll_registry: Option<Arc<enroll::EnrollRegistry>>,
    #[cfg(feature = "worker")]
    registry: Option<Arc<coordinator::Registry>>,
    #[cfg(feature = "worker")]
    coordinator: Option<Arc<coordinator::Coordinator>>,
}

/// `worker` builds: acquires the own lane's GPU (only for `--render`), and -- unless
/// `--no-workers` or `--insecure-no-tls` -- opens the worker port with its own limiter,
/// the registry and its liveness thread, plus the worker enrollment listener.
#[cfg(feature = "worker")]
fn start_worker_side(
    args: &ServeArgs,
    bind_addr: SocketAddr,
    transport: &tls::Transport,
    liveness: coordinator::LivenessConfig,
    db_path: &std::path::Path,
    lifecycle: &mut lifecycle::Lifecycle,
    table: &ConnectionTable,
) -> Result<(WorkerSide, accept::RenderSetup), String> {
    use pleochroma::renderer::gpu_backend::GpuBackend;
    // Acquired once. A device lost later is re-acquired by the backend itself (cool-down
    // and hourly attempt budget in `pleochroma::renderer::gpu_backend`), and each
    // connection's `WELCOME` reads the backend's state when it is built
    // (`local_render_capability`), so nothing here re-acquires.
    let gpu = Arc::new(
        if args.render && args.compute_mode != ComputeMode::OnlyCpu {
            GpuBackend::acquire()
        } else {
            GpuBackend::disabled()
        },
    );
    // Makes the hybrid CPU/GPU split decision once, before the first real request,
    // rather than paying its 3-sample probe on whichever request happens to arrive
    // first. Only meaningful with a real adapter and the hybrid path (`OnlyGpu`/
    // `OnlyCpu` never calibrate a split at all -- see `render_core::hybrid::job_key`'s
    // doc comment).
    if args.render && args.compute_mode == ComputeMode::Hybrid && gpu.adapter_label().is_some() {
        crate::render_core::hybrid::calibrate_now(
            &gpu,
            &probe_scene(),
            args.threads,
            args.compute_mode,
        );
    }

    // Builds every kernel the own lane can need in the background, on the lane's own backend
    // (each step is one FIFO turn, so a real request waits for at most one step). Its progress
    // shows in the status (`shader_cache`) and in the viewers' `PREPARING`. Stopped at shutdown.
    if args.render
        && args.compute_mode != ComputeMode::OnlyCpu
        && let Some(warm) = warmup::start(Arc::clone(&gpu), "serve")
    {
        lifecycle.set_warmup(warm);
    }

    let mut side = WorkerSide::default();
    match transport {
        _ if args.no_workers => {
            tracing::info!("pleochroma-worker serve: --no-workers -- no worker port");
        }
        tls::Transport::Insecure => tracing::warn!(
            "pleochroma-worker serve: --insecure-no-tls -- no worker port (joining workers always need mutual TLS \
             and a worker certificate)"
        ),
        tls::Transport::Tls { .. } => {
            open_worker_port(
                args, bind_addr, transport, liveness, &mut side, lifecycle, table,
            )?;
        }
    }
    let own = args.render.then(|| coordinator::OwnLaneSetup {
        gpu: Arc::clone(&gpu),
        threads: args.threads,
        compute_mode: args.compute_mode,
    });
    // HDR maps: the own lane renders HDR scenes from this cache, and a coordinator with a
    // worker port holds every HDR job's map through it to forward it to joined workers
    // -- see `crate::assets` for its location and cap.
    let assets = if args.render || side.registry.is_some() {
        crate::assets::open_configured(db_path)
    } else {
        None
    };
    let coordinator = coordinator::Coordinator::new(
        side.registry.clone(),
        own,
        args.interactive_workers,
        u64::from(args.max_job_memory_mib) * 1024 * 1024,
    )
    .with_interactive_pin(args.pin_interactive_worker.clone())
    .with_small_pictures(
        args.whole_image_secs,
        args.whole_image_pixel_samples,
        args.jobs_per_viewer,
    )
    .with_assets(assets.clone());
    let jobs = start_job_service(args, db_path, assets.as_ref(), &gpu);
    if let Some(pin) = coordinator.interactive_pin() {
        tracing::info!(
            "pleochroma-worker serve: live-view requests that take joined workers use worker {:?} \
             first while it is available (--pin-interactive-worker)",
            pin.label()
        );
    }
    let coordinator = Arc::new(coordinator);
    side.coordinator = Some(Arc::clone(&coordinator));
    let render = accept::RenderSetup {
        threads: args.threads,
        gpu,
        compute_mode: args.compute_mode,
        own_lane: args.render,
        registry: side.registry.clone(),
        coordinator,
        assets,
        jobs,
        payload: args.payload_encoding,
    };
    Ok((side, render))
}

/// `worker` builds: opens the worker port with its own limiter, the registry with its
/// liveness thread, and the worker enrollment listener, recording them in `side`.
#[cfg(feature = "worker")]
fn open_worker_port(
    args: &ServeArgs,
    bind_addr: SocketAddr,
    transport: &tls::Transport,
    liveness: coordinator::LivenessConfig,
    side: &mut WorkerSide,
    lifecycle: &mut lifecycle::Lifecycle,
    table: &ConnectionTable,
) -> Result<(), String> {
    let tls::Transport::Tls {
        config,
        certificate,
        ..
    } = transport
    else {
        return Ok(()); // no worker port without TLS
    };
    let worker_bind = derived_addr(
        args.worker_bind.as_deref(),
        bind_addr,
        crate::cli::WORKER_PORT_OFFSET,
        "--worker-bind",
        args.allow_remote,
    )?;
    let auth = tls::worker_auth(args)?;
    let worker_limiter = ConnectionLimiter::new(args.max_connections);
    let registry = coordinator::Registry::new(liveness);
    coordinator::Registry::spawn_liveness(&registry);
    let listener = coordinator::spawn_worker_listener(coordinator::WorkerListenerConfig {
        bind_addr: worker_bind,
        tls_config: Arc::clone(config),
        auth: auth.clone(),
        limiter: worker_limiter,
        max_connections: args.max_connections,
        registry: Arc::clone(&registry),
        table: table.clone(),
    })?;
    let addr = listener.addr;
    lifecycle.add_loop("worker listener", listener.stop, listener.exited);
    lifecycle.add_connections("worker connections", listener.connections);
    // Derived from the CONFIGURED worker address (an ephemeral port 0 stays 0).
    let mut enroll_registry = None;
    side.enroll_addr = start_enrollment(
        args,
        args.worker_enroll_bind.as_deref(),
        worker_bind,
        1,
        Some(&auth),
        Some(certificate),
    )?
    .map(|(enroll_addr, stop, registry)| {
        lifecycle.add_listener(stop);
        enroll_registry = Some(registry);
        enroll_addr
    });
    side.enroll_registry = enroll_registry;
    side.addr = Some(addr);
    side.registry = Some(registry);
    Ok(())
}

/// `worker` builds: opens the durable job service and starts its executor on the own lane
/// (only with `--render`), one job at a time on its own thread.
#[cfg(feature = "worker")]
fn start_job_service(
    args: &ServeArgs,
    db_path: &std::path::Path,
    assets: Option<&Arc<crate::assets::AssetCache>>,
    gpu: &Arc<pleochroma::renderer::gpu_backend::GpuBackend>,
) -> Option<Arc<crate::jobs::JobService>> {
    if !args.render {
        return None;
    }
    crate::jobs::JobService::open_configured(db_path, assets.cloned()).inspect(|service| {
        if let Err(error) = service.start_executor(crate::jobs::RenderLane {
            prefer_gpu: args.compute_mode != ComputeMode::OnlyCpu,
            gpu: Arc::clone(gpu),
            threads: args.threads,
        }) {
            tracing::warn!("pleochroma-worker serve: the job executor did not start ({error})");
        }
    })
}

/// A representative scene for [`crate::render_core::hybrid::calibrate_now`]'s start-up
/// probe: a real, traceable scene (so the probe measures genuine per-dispatch overhead,
/// not just call overhead) at a typical interactive/live-view resolution. Its resolution
/// and its estimator select the calibration key (`render_core::hybrid::job_key` buckets
/// the resolution to the enclosing power of two), and it asks for
/// `Estimator::APP_DEFAULT`, so the start-up calibration seeds the key the apps' scenes
/// hit; the exact material/geometry/lighting below are arbitrary.
#[cfg(feature = "worker")]
fn probe_scene() -> pleochroma_net::SceneState {
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    pleochroma_net::SceneState {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 512,
        height: 512,
        yaw: 0.4,
        camera_roll: 0.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        estimator: pleochroma::optics::raytracer::Estimator::APP_DEFAULT,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: pleochroma_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        camera_pan: [0.0, 0.0],
        stone_pose: [0.0, 0.0, 0.0],
    }
}

/// Starts one token-enrollment listener (see [`crate::enroll`]) for `auth.role` at
/// `explicit`, else `base`'s host `offset` ports up. `None` (no listener) under
/// `--no-enroll` or `--insecure-no-tls` (no `auth`).
///
/// Builds its own pair of connection limiters rather than sharing the matching viewer or
/// worker listener's `--max-connections` limiter (see [`enroll::ENROLL_MAX_CONNECTIONS`]).
///
/// Returns the bound address with the [`ListenerStop`] that ends the listener and the
/// listener's [`enroll::EnrollRegistry`] (the control API issues pairing codes from it); the
/// caller hands the stop to the lifecycle. The stop is built after the bind, so its wake address is the
/// real port even when the configured one was `0`.
fn start_enrollment(
    args: &ServeArgs,
    explicit: Option<&str>,
    base: SocketAddr,
    offset: u16,
    auth: Option<&Auth>,
    certificate: Option<&Arc<crate::certs::SwappableCert>>,
) -> Result<Option<(SocketAddr, ListenerStop, Arc<enroll::EnrollRegistry>)>, String> {
    let Some(auth) = auth else {
        return Ok(None);
    };
    if args.no_enroll {
        return Ok(None);
    }
    let (Some(ca), Some(cert), Some(key)) = (&args.ca, &args.cert, &args.key) else {
        return Ok(None); // `tls::build_transport` already requires these.
    };
    let flag = match auth.role {
        PeerRole::Viewer => "--enroll-bind",
        PeerRole::Worker => "--worker-enroll-bind",
        // Remote administration shares the viewer port and the viewer enrollment listener.
        PeerRole::Monitor | PeerRole::Admin => return Ok(None),
    };
    let bind = derived_addr(explicit, base, offset, flag, args.allow_remote)?;
    let mut config =
        enroll::EnrollConfig::build(bind, ca, cert, key, auth.allowlist.clone(), auth.role)?;
    // The listener presents the coordinator's live certificate, so a swap after an address
    // change reaches the next pairing without a restart.
    if let Some(certificate) = certificate {
        config = config.with_swappable_certificate(ca, Arc::clone(certificate))?;
    }
    let registry = Arc::clone(&config.registry);
    // Remote administration on: the viewer listener also enrolls monitors and administrators,
    // listing their certificates in their own allowlists. Without this registration such a
    // code is never issued (see `enroll::EnrollRegistry::issue_for_role`).
    if args.allow_remote_admin && auth.role == PeerRole::Viewer {
        registry.register_allowlist(
            PeerRole::Monitor,
            crate::pki::monitor_allowlist_in(&config.pki_dir),
        );
        registry.register_allowlist(
            PeerRole::Admin,
            crate::pki::admin_allowlist_in(&config.pki_dir),
        );
    }
    let bound = enroll::bind_enroll_listener(config)?;
    let addr = bound.local_addr();
    let stop = ListenerStop::new(addr);
    bound.spawn(stop.clone());
    Ok(Some((addr, stop, registry)))
}

/// Parses `addr` for `flag` and refuses a non-loopback address without `--allow-remote`.
fn checked_addr(addr: &str, flag: &str, allow_remote: bool) -> Result<SocketAddr, String> {
    let parsed: SocketAddr = addr
        .parse()
        .map_err(|e| format!("invalid {flag} address {addr:?}: {e}"))?;
    if !parsed.ip().is_loopback() && !allow_remote {
        return Err(format!(
            "refusing to bind non-loopback address {parsed} ({flag}) without --allow-remote -- exposing this \
             server beyond localhost must be explicit (see --help)"
        ));
    }
    Ok(parsed)
}

/// `explicit` if given, else `base`'s host with the port `offset` higher (port `0` --
/// an ephemeral port -- stays `0`), then [`checked_addr`].
fn derived_addr(
    explicit: Option<&str>,
    base: SocketAddr,
    offset: u16,
    flag: &str,
    allow_remote: bool,
) -> Result<SocketAddr, String> {
    let addr = if let Some(explicit) = explicit {
        explicit.to_string()
    } else {
        let port = if base.port() == 0 {
            0
        } else {
            base.port().checked_add(offset).ok_or_else(|| {
                format!(
                    "{flag}: port {} + {offset} overflows; pass {flag} explicitly",
                    base.port()
                )
            })?
        };
        SocketAddr::new(base.ip(), port).to_string()
    };
    checked_addr(&addr, flag, allow_remote)
}

/// The transport half of the startup log line.
const fn transport_summary(transport: &tls::Transport) -> &'static str {
    match transport {
        tls::Transport::Tls {
            auth: Auth {
                allowlist: Some(_), ..
            },
            ..
        } => "mutual TLS, allowlist enforced",
        tls::Transport::Tls { .. } => "mutual TLS, ANY CA-signed client of the right role trusted",
        tls::Transport::Insecure => "PLAINTEXT, no TLS",
    }
}

/// One-line capability summary for the startup log line, matching what `WELCOME`
/// advertises before any worker joins.
#[cfg(feature = "worker")]
fn capability_summary(args: &ServeArgs) -> String {
    if !args.render {
        return "library + coordinator for joined workers; no own render lane (add --render to render here)"
            .to_string();
    }
    match args.compute_mode {
        ComputeMode::OnlyCpu => "library + render (CPU only, --only-cpu)".to_string(),
        ComputeMode::OnlyGpu => "library + render (GPU only, --only-gpu)".to_string(),
        ComputeMode::Hybrid => "library + render (--render)".to_string(),
    }
}

#[cfg(not(feature = "worker"))]
fn capability_summary(_args: &ServeArgs) -> String {
    "library only (no render capacity and no worker port -- build with --features worker to add them)"
        .to_string()
}
