//! The `join` subcommand (`worker` builds).
//!
//! A render worker that dials OUT to a coordinator's worker port and serves its
//! requests -- no inbound port needed on this machine, so NAT and cloud VMs work.
//!
//! The worker is the TLS/TCP CLIENT but the PROTOCOL SERVER: after TLS and a worker
//! `HELLO` (carrying this machine's `RenderCapability`), the coordinator sends
//! `ClientMessage`s and this side answers `StreamEvent`s through the very same request
//! loop a viewer connection uses (`crate::serve::serve_requests`), `PING` → `PONG`
//! included. `--slots K` opens K such connections (one request stream each); every slot
//! reconnects forever with jittered exponential backoff (1 s doubling to 60 s, reset
//! after every successful registration).
//!
//! `--token GW1-...` first claims a WORKER enrollment token over the coordinator's
//! worker enrollment listener (default: worker port + 1, 7881) and stores the bundle in
//! `--cert-dir`.
//!
//! `JoinArgs::coordinator` may list up to three addresses separated by commas (a coordinator
//! with several network adapters): each slot starts at a different one and moves to the next
//! after a failed attempt.
//!
//! Two ways in: [`run`] blocks until Ctrl-C or SIGTERM (the headless command), and [`start`]
//! returns a [`JoinHandle`] the window starts and stops.

mod handle;
mod session;

pub use handle::{JoinHandle, JoinPhase, JoinStatus, SlotPhase};
pub use session::{
    IdleDeadlineStream, JOIN_IDLE_TIMEOUT, JoinError, JoinTarget, Session, WorkerSetup,
    bundle_role, join_once,
};

use crate::cli::{ComputeMode, JoinArgs};
use handle::Shared;
use pleochroma::renderer::gpu_backend::GpuBackend;
use pleochroma_net::messages::PeerRole;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// The first reconnect delay (before jitter).
pub const BACKOFF_MIN: Duration = Duration::from_secs(1);
/// The longest reconnect delay (before jitter).
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// How long a stop waits for the slot threads before giving up on them (a render in flight
/// ends when its socket write fails; the process may exit with the thread still running).
const STOP_GRACE: Duration = Duration::from_secs(10);

/// What a join needs once its certificate and engine are ready.
struct Prepared {
    targets: Vec<Arc<JoinTarget>>,
    setup: Arc<WorkerSetup>,
}

/// Runs the `join` subcommand.
///
/// Optional token claim, bundle and role check, then `args.slots` reconnecting slots until Ctrl-C
/// or SIGTERM, which end it cleanly (the process then exits with status 0).
///
/// If the signal handler cannot be installed, a warning is logged and the join runs until
/// the process is killed, as before.
///
/// # Errors
///
/// A human-readable message for anything that can't get better by retrying: a failed
/// token claim, an unreadable bundle, or a bundle that is not a WORKER certificate.
pub fn run(args: &JoinArgs) -> Result<(), String> {
    let shared = Arc::new(Shared::new(args.slots));
    let for_signal = Arc::clone(&shared);
    if let Err(e) = ctrlc::set_handler(move || {
        tracing::info!("pleochroma-worker join: stopping");
        for_signal.request_stop();
    }) {
        tracing::warn!("could not install the Ctrl-C / SIGTERM handler ({e}); stop with kill");
    }
    let result = run_shared(args, &shared);
    shared.finished.store(true, Ordering::Relaxed);
    result
}

/// Starts a join in the background and returns the handle to watch and stop it.
///
/// The token claim, the certificate check and the engine start-up happen on the join's own
/// thread, so the call returns at once; a problem shows up in [`JoinHandle::status`] (phase
/// [`JoinPhase::Stopped`] with the error text). No signal handler is installed.
#[must_use]
pub fn start(args: &JoinArgs) -> JoinHandle {
    let shared = Arc::new(Shared::new(args.slots));
    let (args, for_thread) = (args.clone(), Arc::clone(&shared));
    let spawned = thread::Builder::new()
        .name("join".to_owned())
        .spawn(move || {
            if let Err(e) = run_shared(&args, &for_thread) {
                tracing::warn!("pleochroma-worker join: {e}");
                for_thread.set_error(e);
            }
            for_thread.finished.store(true, Ordering::Relaxed);
        });
    if let Err(e) = spawned {
        tracing::warn!("pleochroma-worker join: could not start its thread: {e}");
        shared.set_error(
            "This computer could not start the helper. Close other programs and try again.",
        );
        shared.finished.store(true, Ordering::Relaxed);
    }
    JoinHandle { shared }
}

/// What is wrong with the certificate folder `cert_dir`, in plain words for the window, or
/// `None` when it holds a worker certificate.
#[must_use]
pub fn bundle_problem(cert_dir: &Path) -> Option<String> {
    match bundle_role(cert_dir) {
        Ok(PeerRole::Worker) => None,
        Ok(PeerRole::Viewer | PeerRole::Monitor | PeerRole::Admin) => Some(
            "The certificate in this folder is not for a render computer. Connect again with a new link from \"Add a render computer\" on the coordinator.".to_owned(),
        ),
        Err(_) => Some(
            "No certificate for this computer was found. Connect again with a new link from \"Add a render computer\" on the coordinator.".to_owned(),
        ),
    }
}

/// The body of [`run`] and of [`start`]'s thread: prepare, run the slots, wait for them.
fn run_shared(args: &JoinArgs, shared: &Arc<Shared>) -> Result<(), String> {
    let prepared = prepare(args)?;
    if shared.stopping() {
        return Ok(());
    }
    // Builds the kernels in the background (log lines only), so the first chunk does not wait for
    // a compile; its `PREPARING` tells the coordinator in the meantime.
    let warm = if args.compute_mode == ComputeMode::OnlyCpu {
        None
    } else {
        crate::serve::warmup::start(Arc::clone(&prepared.setup.gpu), "join")
    };
    let threads: Vec<_> = (0..shared.slots.len())
        .map(|index| {
            let targets = prepared.targets.clone();
            let setup = Arc::clone(&prepared.setup);
            let shared = Arc::clone(shared);
            thread::spawn(move || run_slot(index, &targets, &setup, &shared))
        })
        .collect();
    wait_for_slots(threads, shared);
    if let Some(warm) = warm {
        warm.cancel();
    }
    Ok(())
}

/// Waits for the slot threads. After a stop it gives them [`STOP_GRACE`] and then lets go.
fn wait_for_slots(threads: Vec<thread::JoinHandle<()>>, shared: &Shared) {
    let mut stop_seen: Option<Instant> = None;
    while !threads.iter().all(thread::JoinHandle::is_finished) {
        if shared.stopping() {
            let since = *stop_seen.get_or_insert_with(Instant::now);
            if since.elapsed() > STOP_GRACE {
                tracing::warn!("pleochroma-worker join: a slot did not end in time; leaving it");
                return;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    for slot in threads {
        if slot.join().is_err() {
            tracing::error!("pleochroma-worker join: a slot thread panicked");
        }
    }
}

/// The token claim, the certificate check and the engine start-up.
fn prepare(args: &JoinArgs) -> Result<Prepared, String> {
    let addresses: Vec<&str> = args
        .coordinator
        .split(',')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .collect();
    let Some(first) = addresses.first() else {
        return Err(format!(
            "coordinator address {:?} must be host:port",
            args.coordinator
        ));
    };
    if let Some(token) = &args.token {
        let enroll_addr = args
            .enroll_addr
            .as_ref()
            .map_or_else(|| default_enroll_addr(first), |addr| Ok(addr.clone()))?;
        let bundle = pleochroma_net::enroll::claim(token, &enroll_addr)
            .map_err(|e| format!("claiming the worker token at {enroll_addr}: {e}"))?;
        crate::enroll_client::write_bundle(&args.cert_dir, &bundle)?;
        tracing::info!(
            "pleochroma-worker join: claimed a worker certificate into {}",
            args.cert_dir.display()
        );
    }
    check_worker_bundle(&args.cert_dir)?;
    let targets = addresses
        .iter()
        .map(|address| JoinTarget::from_bundle(address, &args.cert_dir).map(Arc::new))
        .collect::<Result<Vec<_>, _>>()?;

    // Acquired once; a device lost later is re-acquired by the backend itself (cool-down
    // and hourly attempt budget in `pleochroma::renderer::gpu_backend`), and every
    // reconnect advertises the backend's state at that moment
    // (`WorkerSetup::current_capability`).
    let gpu = Arc::new(if args.compute_mode == ComputeMode::OnlyCpu {
        GpuBackend::disabled()
    } else {
        GpuBackend::acquire()
    });
    // Decides the hybrid CPU/GPU split once, before the coordinator's first chunk, instead of
    // paying the 3-sample probe on whichever chunk arrives first. Only the hybrid mode with a
    // real adapter splits work; the other modes never calibrate.
    if args.compute_mode == ComputeMode::Hybrid && gpu.adapter_label().is_some() {
        crate::render_core::hybrid::calibrate_now(
            &gpu,
            &probe_scene(),
            args.threads,
            args.compute_mode,
        );
    }
    // HDR maps are cached next to the certificate bundle (or where
    // PLEOCHROMA_ASSET_CACHE_DIR says); without a cache this worker takes no HDR jobs.
    let assets = crate::assets::open_configured(&args.cert_dir);
    // The start-up baseline: the HELLO recomputes its backend from `gpu` per connection.
    let capability = pleochroma_net::messages::RenderCapability {
        hdr: assets.is_some(),
        ..crate::serve::local_render_capability(&gpu, args.threads)
    };
    tracing::info!(
        "pleochroma-worker join: protocol v{}, joining {} with {} slot(s) as {:?} (HDR environments: {})",
        pleochroma_net::messages::PROTOCOL_VERSION,
        args.coordinator,
        args.slots,
        capability.backend,
        if capability.hdr { "yes" } else { "no" }
    );
    let setup = Arc::new(WorkerSetup {
        gpu,
        threads: args.threads,
        compute_mode: args.compute_mode,
        capability,
        assets,
        payload: args.payload_encoding,
    });
    Ok(Prepared { targets, setup })
}

/// The scene for [`crate::render_core::hybrid::calibrate_now`]'s start-up probe. It is a real
/// traceable scene, so the probe measures genuine per-dispatch overhead. Its resolution and
/// estimator (`Estimator::APP_DEFAULT`) select the calibration key the apps' scenes hit; the
/// material, geometry and lighting are arbitrary.
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

/// Refuses a bundle whose certificate is not a WORKER certificate, with the fix.
fn check_worker_bundle(cert_dir: &Path) -> Result<(), String> {
    match bundle_role(cert_dir).map_err(|e| format!("--cert-dir {}: {e}", cert_dir.display()))? {
        PeerRole::Worker => Ok(()),
        PeerRole::Viewer => Err(format!(
            "--cert-dir {}: this is a VIEWER certificate -- `join` needs a worker certificate (`cert issue-client \
             --role worker` on the coordinator, or `join --token` with a `cert issue-token --role worker` token)",
            cert_dir.display()
        )),
        PeerRole::Monitor | PeerRole::Admin => Err(format!(
            "--cert-dir {}: this is a MONITOR or ADMIN certificate -- `join` needs a worker certificate (`cert \
             issue-client --role worker` on the coordinator, or `join --token` with a `cert issue-token --role \
             worker` token)",
            cert_dir.display()
        )),
    }
}

/// The worker enrollment listener's default address: the coordinator host, one port
/// above its worker port (7881 for 7880).
///
/// # Errors
///
/// A human-readable message if `coordinator` isn't `host:port`.
pub fn default_enroll_addr(coordinator: &str) -> Result<String, String> {
    let (host, port) = coordinator
        .rsplit_once(':')
        .ok_or_else(|| format!("coordinator address {coordinator:?} must be host:port"))?;
    let port: u16 = port
        .parse()
        .map_err(|_| format!("coordinator address {coordinator:?}: bad port"))?;
    let enroll_port = port
        .checked_add(1)
        .ok_or_else(|| "pass --enroll-addr explicitly (the worker port is 65535)".to_string())?;
    Ok(format!("{host}:{enroll_port}"))
}

/// One slot: join, serve until the connection ends, back off, repeat -- until a stop. Slot
/// `index` starts at address `index` of `targets` and moves to the next after a failed attempt.
fn run_slot(index: usize, targets: &[Arc<JoinTarget>], setup: &WorkerSetup, shared: &Shared) {
    let slot_state = &shared.slots[index];
    let mut which = index % targets.len();
    let mut backoff = Backoff::new();
    while !shared.stopping() {
        let target = &targets[which];
        slot_state.connecting();
        match session::join_once_observed(target, setup, Some(slot_state)) {
            Ok(session) => {
                backoff.reset();
                match session.ended {
                    Ok(()) => {
                        tracing::info!(
                            "pleochroma-worker join: slot {index}: the coordinator closed worker #{}'s connection",
                            session.worker_id
                        );
                        shared.set_error("The coordinator closed the connection.");
                    }
                    Err(e) => {
                        tracing::warn!(
                            "pleochroma-worker join: slot {index}: worker #{}'s connection ended: {e}",
                            session.worker_id
                        );
                        shared.set_error("The connection to the coordinator was lost.");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "pleochroma-worker join: slot {index}: {} -- {e}",
                    target.addr
                );
                shared.set_error(e.plain(&target.addr));
                which = (which + 1) % targets.len();
            }
        }
        slot_state.waiting();
        if shared.stopping() {
            break;
        }
        let delay = backoff.next_delay();
        tracing::info!("pleochroma-worker join: slot {index}: reconnecting in {delay:.1?}");
        sleep_unless_stopped(delay, &shared.stop);
    }
}

/// Sleeps `delay` in short steps, returning early once `stop` is set.
fn sleep_unless_stopped(delay: Duration, stop: &AtomicBool) {
    let slice = Duration::from_millis(200);
    let mut left = delay;
    while !left.is_zero() && !stop.load(Ordering::Relaxed) {
        let nap = left.min(slice);
        thread::sleep(nap);
        left -= nap;
    }
}

/// Jittered exponential backoff: [`BACKOFF_MIN`] doubling to [`BACKOFF_MAX`].
///
/// Each delay is scaled by a random factor in `[0.5, 1.0)`, so workers reconnecting after a
/// coordinator restart do not arrive in lockstep.
#[derive(Debug)]
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    /// Starts at [`BACKOFF_MIN`].
    #[must_use]
    pub const fn new() -> Self {
        Self { next: BACKOFF_MIN }
    }

    /// Back to [`BACKOFF_MIN`] (after a successful registration).
    pub const fn reset(&mut self) {
        self.next = BACKOFF_MIN;
    }

    /// The next delay (jittered), doubling the base for the one after.
    pub fn next_delay(&mut self) -> Duration {
        let base = self.next;
        self.next = (self.next * 2).min(BACKOFF_MAX);
        base.mul_f64(random_unit().mul_add(0.5, 0.5))
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new()
    }
}

/// A random number in `[0, 1)` from the system CSPRNG (`ring`, already this crate's
/// randomness source); `0.5` if it fails.
fn random_unit() -> f64 {
    use ring::rand::SecureRandom;
    let mut bytes = [0_u8; 4];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        return 0.5;
    }
    f64::from(u32::from_le_bytes(bytes)) / (f64::from(u32::MAX) + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_to_the_cap_with_jitter_and_resets() {
        let mut backoff = Backoff::new();
        let mut bases = Vec::new();
        for _ in 0..10 {
            let base = backoff.next;
            let delay = backoff.next_delay();
            assert!(
                delay >= base / 2 && delay <= base,
                "{delay:?} vs base {base:?}"
            );
            bases.push(base.as_secs());
        }
        assert_eq!(bases, [1, 2, 4, 8, 16, 32, 60, 60, 60, 60]);
        backoff.reset();
        assert_eq!(backoff.next, BACKOFF_MIN);
    }

    #[test]
    fn a_folder_without_a_certificate_has_a_plain_problem() {
        let missing = std::env::temp_dir().join("pleochroma-join-no-such-folder");
        let problem = bundle_problem(&missing).expect("a problem");
        assert!(problem.contains("Add a render computer"), "{problem}");
        assert!(!problem.contains("--cert-dir"), "{problem}");
    }

    #[test]
    fn a_start_with_an_unreadable_folder_ends_with_the_problem_in_the_status() {
        let args = JoinArgs {
            coordinator: "127.0.0.1:7880".to_owned(),
            cert_dir: std::env::temp_dir().join("pleochroma-join-no-such-folder"),
            slots: 2,
            threads: 0,
            compute_mode: ComputeMode::OnlyCpu,
            token: None,
            enroll_addr: None,
            payload_encoding: pleochroma_net::messages::adaptive::PayloadChoice::Auto,
        };
        let handle = start(&args);
        for _ in 0..100 {
            if handle.is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
        let status = handle.status();
        assert!(handle.is_finished());
        assert_eq!(status.phase, JoinPhase::Stopped);
        assert!(!status.error.is_empty());
    }

    #[test]
    fn the_enroll_address_defaults_to_one_port_above_the_worker_port() {
        assert_eq!(
            default_enroll_addr("coord.lan:7880").unwrap(),
            "coord.lan:7881"
        );
        assert_eq!(default_enroll_addr("[::1]:7880").unwrap(), "[::1]:7881");
        assert!(default_enroll_addr("coord.lan").is_err());
        assert!(default_enroll_addr("coord.lan:65535").is_err());
    }
}
