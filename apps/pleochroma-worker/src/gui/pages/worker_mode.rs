//! The Help another coordinator page: this computer lends its render power to another
//! coordinator.
//!
//! The owner pastes the worker link made by "Add a render computer" on the other coordinator
//! (`PairingLink::parse`, worker role only), confirms with Connect (`claim_any` on the link's
//! addresses, then the certificate goes into the per-user `worker-cert` folder or a chosen one),
//! and the join starts (`crate::join::start`). A once-a-second timer shows what the join is doing.
//!
//! The last connection is saved in the settings (`run = "worker"` and the `worker_*` fields), so a
//! program started with that setting rejoins by itself: `gui::app` calls [`resume_if_saved`] after
//! the pages are wired, window shown or not (an auto-start with `--minimized` has none).
//!
//! A pairing link is a secret: it is held in a wiping string only until it is claimed, it is never
//! logged, and no type here derives `Debug`.

use crate::{
    cli::{ComputeMode, JoinArgs},
    gui::{
        app::{ActiveBackend, AppWindow, WorkerModeState},
        local,
        pages::{
            overview::uptime_text,
            picker::{self, Kind, Request},
        },
    },
    join::{self, JoinHandle, JoinPhase, JoinStatus, SlotPhase},
    settings::{self, CoordinatorSettings, RenderChoice, RunMode},
};
use pleochroma_net::{
    enroll,
    messages::{PeerRole, adaptive::PayloadChoice},
    pairing::PairingLink,
};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell,
    net::SocketAddr,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

/// How often the page reads the join's status.
const TICK: Duration = Duration::from_millis(500);

/// The most slots the stepper offers.
const MAX_SLOTS: i32 = 16;

/// What the page says about a link made for a phone or a viewer.
const VIEWER_LINK: &str =
    "This code is for a phone or a viewer. On the coordinator, choose Add a render computer.";

/// What the page says about a monitor or administrator link.
const MANAGER_LINK: &str = "This code is for managing a coordinator, not for lending render power. On the coordinator, choose Add a render computer.";

thread_local! {
    /// The page's driver, set by [`wire`]; the helper threads reach it through the event loop.
    static CTX: RefCell<Option<Rc<Ctx>>> = const { RefCell::new(None) };
    /// The tick timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// What the page remembers between ticks.
struct Inner {
    /// The running join, if any.
    handle: Option<JoinHandle>,
    /// The pasted link when it is a valid worker link (a secret, wiped when dropped).
    link: Option<PairingLink>,
}

/// The page's driver.
struct Ctx {
    window: slint::Weak<AppWindow>,
    backend: ActiveBackend,
    inner: RefCell<Inner>,
}

/// What the pasted text is.
enum LinkCheck {
    /// Nothing pasted.
    Empty,
    /// A worker link.
    Worker(Box<PairingLink>),
    /// Not usable; the reason in plain words.
    Problem(String),
}

/// What a successful claim hands back.
struct Claimed {
    /// The coordinator's render-computer addresses (`ip:port`), the one that answered first.
    addresses: Vec<String>,
    /// The coordinator's name, or empty.
    name: String,
}

/// Connects the Help another coordinator page of `window` to `backend`: shows the saved choices,
/// validates the pasted link, connects, starts and stops the join, and shows its status.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let ctx = Rc::new(Ctx {
        window: window.as_weak(),
        backend: backend.clone(),
        inner: RefCell::new(Inner {
            handle: None,
            link: None,
        }),
    });
    let max_threads = std::thread::available_parallelism()
        .map_or(16, std::num::NonZeroUsize::get)
        .clamp(1, 1024);
    let state = window.global::<WorkerModeState>();
    state.set_max_threads(i32::try_from(max_threads).unwrap_or(16));
    ctx.load_choices();

    state.on_link_edited({
        let ctx = Rc::clone(&ctx);
        move |text| ctx.link_edited(text.as_str())
    });
    state.on_paste({
        let ctx = Rc::clone(&ctx);
        move || ctx.paste()
    });
    state.on_connect({
        let ctx = Rc::clone(&ctx);
        move || ctx.connect()
    });
    state.on_stop({
        let ctx = Rc::clone(&ctx);
        move || ctx.stop()
    });
    state.on_start_saved({
        let ctx = Rc::clone(&ctx);
        move || ctx.start_saved()
    });
    state.on_choose_folder({
        let ctx = Rc::clone(&ctx);
        move || ctx.choose_folder()
    });
    state.on_reset_folder({
        let ctx = Rc::clone(&ctx);
        move || ctx.change_folder(None)
    });
    state.on_set_slots({
        let ctx = Rc::clone(&ctx);
        move |slots| ctx.set_slots(slots)
    });
    state.on_set_mode({
        let ctx = Rc::clone(&ctx);
        move |index| ctx.set_mode(index)
    });
    state.on_set_threads({
        let ctx = Rc::clone(&ctx);
        move |threads| ctx.set_threads(threads)
    });

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, {
        let ctx = Rc::clone(&ctx);
        move || ctx.tick()
    });
    TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
    ctx.tick();
    state.set_loaded(true);
    CTX.with(|slot| *slot.borrow_mut() = Some(ctx));
}

/// Rejoins the saved coordinator when the settings say this computer helps another one
/// (`run = "worker"` with a saved connection); does nothing otherwise.
///
/// Call it once on the UI thread after [`wire`] (`gui::app::run_with_instance`, after
/// `wire_pages`), whether the window is shown or the program started hidden in the tray. A
/// problem (the certificate folder is gone) shows on the page instead of failing the start.
pub fn resume_if_saved(_window: &AppWindow) {
    if !settings::current().resumes_worker() {
        return;
    }
    with_ctx(|ctx| {
        tracing::info!("this computer is set to help another coordinator: rejoining it");
        ctx.begin_join();
    });
}

/// Stops the join, if one runs (the window is closing for good). Does not change the settings,
/// so the next start rejoins.
pub fn shutdown() {
    with_ctx(|ctx| {
        if let Some(handle) = ctx.inner.borrow_mut().handle.take() {
            handle.stop();
        }
    });
}

/// Runs `f` with the page's driver, if the page is wired.
fn with_ctx(f: impl FnOnce(&Rc<Ctx>)) {
    let ctx = CTX.with(|slot| slot.borrow().clone());
    if let Some(ctx) = ctx {
        f(&ctx);
    }
}

impl Ctx {
    // ---------------------------------------------------------------- the choices

    /// Shows the saved choices: slots, mode, threads, the certificate folder and the saved
    /// connection.
    fn load_choices(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let saved = settings::current();
        let state = window.global::<WorkerModeState>();
        state.set_slots(
            i32::try_from(saved.worker_slots)
                .unwrap_or(2)
                .clamp(1, MAX_SLOTS),
        );
        state.set_mode(mode_index(saved.worker_mode));
        state.set_threads(i32::try_from(saved.worker_threads).unwrap_or(0));
        let folder = cert_dir_for(&saved);
        state.set_cert_folder(folder.display().to_string().into());
        state.set_cert_custom(saved.worker_cert_dir.is_some());
        state.set_saved_text(saved_text(&saved, &folder).into());
    }

    fn set_slots(&self, slots: i32) {
        let slots = slots.clamp(1, MAX_SLOTS);
        let wanted = u32::try_from(slots).unwrap_or(2);
        self.save(|s| s.worker_slots = wanted);
        if let Some(window) = self.window.upgrade() {
            window.global::<WorkerModeState>().set_slots(slots);
        }
    }

    fn set_mode(&self, index: i32) {
        let choice = mode_from_index(index);
        self.save(|s| s.worker_mode = choice);
        if let Some(window) = self.window.upgrade() {
            window
                .global::<WorkerModeState>()
                .set_mode(mode_index(choice));
        }
    }

    fn set_threads(&self, threads: i32) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<WorkerModeState>();
        let threads = threads.clamp(0, state.get_max_threads());
        let wanted = u32::try_from(threads).unwrap_or(0);
        self.save(|s| s.worker_threads = wanted);
        state.set_threads(threads);
    }

    /// Saves a change of the choices; a refusal shows as the page's problem.
    fn save(&self, change: impl FnOnce(&mut CoordinatorSettings)) {
        let result = settings::update(change);
        if let (Err(e), Some(window)) = (result, self.window.upgrade()) {
            window.global::<WorkerModeState>().set_error(e.into());
        }
    }

    /// "Choose folder…": the owner picks where the certificate lives.
    fn choose_folder(&self) {
        // The dialog runs on a helper thread; the choice comes back to the UI thread.
        let request = Request::new(Kind::Folder, "Choose the certificate folder");
        let started = picker::pick(&self.window, &request, |_window, path| {
            with_ctx(|ctx| ctx.change_folder(Some(path)));
        });
        if let (Err(reason), Some(window)) = (started, self.window.upgrade()) {
            window.global::<WorkerModeState>().set_error(reason.into());
        }
    }

    /// Uses `folder` (or the per-user default for `None`) for the certificate.
    fn change_folder(&self, folder: Option<PathBuf>) {
        self.save(|s| s.worker_cert_dir = folder);
        self.load_choices();
    }

    // ---------------------------------------------------------------- the link

    /// The Paste button: reads the clipboard into the field.
    fn paste(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let text = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
        match text {
            Ok(text) if !text.trim().is_empty() => {
                let text = text.trim().to_owned();
                window
                    .global::<WorkerModeState>()
                    .set_link_text(SharedString::from(text.as_str()));
                self.link_edited(&text);
            }
            _ => Self::show_link(
                &window,
                &LinkCheck::Problem(
                    "The clipboard has no text. Copy the link on the other coordinator first."
                        .to_owned(),
                ),
            ),
        }
    }

    /// The field changed: validates the text and shows who the link is for.
    fn link_edited(&self, text: &str) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let check = check_link(text);
        Self::show_link(&window, &check);
        self.inner.borrow_mut().link = match check {
            LinkCheck::Worker(link) => Some(*link),
            LinkCheck::Empty | LinkCheck::Problem(_) => None,
        };
    }

    fn show_link(window: &AppWindow, check: &LinkCheck) {
        let state = window.global::<WorkerModeState>();
        match check {
            LinkCheck::Empty => {
                state.set_link_state(0);
                state.set_link_message(SharedString::new());
                state.set_link_coordinator(SharedString::new());
            }
            LinkCheck::Worker(link) => {
                state.set_link_state(1);
                state.set_link_message("link valid".into());
                state.set_link_coordinator(coordinator_label(link).into());
            }
            LinkCheck::Problem(text) => {
                state.set_link_state(2);
                state.set_link_message(text.as_str().into());
                state.set_link_coordinator(SharedString::new());
            }
        }
    }

    // ---------------------------------------------------------------- connect, start, stop

    /// Connect: claims the link on a helper thread, then starts the join.
    fn connect(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<WorkerModeState>();
        let link = {
            let inner = self.inner.borrow();
            if inner.handle.is_some() || state.get_busy() {
                return;
            }
            inner.link.clone()
        };
        let Some(link) = link else {
            return;
        };
        let folder = cert_dir_for(&settings::current());
        if let Some(problem) = folder_conflict(&folder) {
            state.set_error(problem.into());
            return;
        }
        state.set_error(SharedString::new());
        state.set_busy(true);
        state.set_status_text("Connecting to the coordinator…".into());
        let weak = self.window.clone();
        let spawned = std::thread::Builder::new()
            .name("worker-claim".to_owned())
            .spawn(move || {
                let result = claim(&link, &folder);
                let _ = weak.upgrade_in_event_loop(move |_window| {
                    with_ctx(|ctx| ctx.claimed(result));
                });
            });
        if spawned.is_err() {
            state.set_busy(false);
            state.set_error("The connection could not be started. Try again.".into());
        }
    }

    /// The claim finished: save the connection and start, or show why not.
    fn claimed(&self, result: Result<Claimed, String>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<WorkerModeState>();
        state.set_busy(false);
        match result {
            Err(problem) => state.set_error(problem.into()),
            Ok(claimed) => {
                // The code is spent: forget it.
                self.inner.borrow_mut().link = None;
                state.set_link_text(SharedString::new());
                state.set_link_state(0);
                state.set_link_message(SharedString::new());
                state.set_link_coordinator(SharedString::new());
                let Claimed { addresses, name } = claimed;
                self.save(|s| {
                    s.worker_coordinator = addresses;
                    s.worker_coordinator_name = name;
                });
                self.load_choices();
                self.begin_join();
            }
        }
    }

    /// "Start" on the saved connection.
    fn start_saved(&self) {
        self.begin_join();
    }

    /// Starts the join with the saved connection and the saved choices, and remembers that this
    /// computer helps (so the next start rejoins).
    fn begin_join(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<WorkerModeState>();
        if self.inner.borrow().handle.is_some() {
            return;
        }
        let saved = settings::current();
        if saved.worker_coordinator.is_empty() {
            state.set_error(
                "No coordinator is saved yet. Paste a link from \"Add a render computer\" first."
                    .into(),
            );
            return;
        }
        let folder = cert_dir_for(&saved);
        if let Some(problem) = join::bundle_problem(&folder) {
            state.set_error(problem.into());
            return;
        }
        state.set_error(SharedString::new());
        let handle = join::start(&join_args(&saved, folder));
        self.inner.borrow_mut().handle = Some(handle);
        self.save(|s| s.run = RunMode::Worker);
        self.tick();
    }

    /// Stop: ends the join and goes back to running a coordinator at the next start. The saved
    /// connection stays, so Start rejoins without a new link.
    fn stop(&self) {
        let handle = self.inner.borrow_mut().handle.take();
        if let Some(handle) = handle {
            handle.stop();
        }
        self.save(|s| {
            if s.run == RunMode::Worker {
                s.run = RunMode::Here;
            }
        });
        self.load_choices();
        self.tick();
    }

    // ---------------------------------------------------------------- the clock

    /// Shows what the join is doing and keeps the hardware line current.
    fn tick(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<WorkerModeState>();
        let hardware = self.backend.get().status().own_render.hardware;
        state.set_hardware(hardware_text(&hardware, state.get_max_threads()).into());

        let status = self.inner.borrow().handle.as_ref().map(JoinHandle::status);
        let Some(status) = status else {
            state.set_phase(0);
            if !state.get_busy() {
                state.set_status_text(SharedString::new());
            }
            state.set_status_right(SharedString::new());
            state.set_slot_chips(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            return;
        };
        if status.phase == JoinPhase::Stopped {
            // It ended by itself (the certificate could not be used): show why and let go.
            state.set_error(status.error.as_str().into());
            self.inner.borrow_mut().handle = None;
            state.set_phase(0);
            state.set_status_text(SharedString::new());
            state.set_slot_chips(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            return;
        }
        let name = coordinator_name(&settings::current());
        state.set_phase(phase_code(status.phase));
        state.set_status_text(status_text(status.phase, &name).into());
        state.set_status_right(status_right(&status).into());
        state.set_error(status.error.as_str().into());
        state.set_slot_chips(ModelRc::new(VecModel::from(
            status
                .slots
                .iter()
                .enumerate()
                .map(|(i, phase)| SharedString::from(chip_text(i, *phase)))
                .collect::<Vec<_>>(),
        )));
    }
}

// -------------------------------------------------------------------- the claim

/// Redeems the link's code against its coordinator and writes the certificate into `folder`.
fn claim(link: &PairingLink, folder: &Path) -> Result<Claimed, String> {
    let enroll_addrs: Vec<SocketAddr> = link
        .addrs
        .iter()
        .map(|addr| SocketAddr::new(addr.ip(), link.enroll_port))
        .collect();
    let outcome = enroll::claim_any(link.token(), &enroll_addrs).map_err(|e| e.to_string())?;
    crate::enroll_client::write_bundle(folder, &outcome.bundle)
        .map_err(|e| format!("The certificate could not be saved: {e}"))?;
    // The address that answered goes first; the others stay as spares.
    let answered = outcome.addr.ip();
    let mut addrs = link.addrs.clone();
    addrs.sort_by_key(|addr| addr.ip() != answered);
    Ok(Claimed {
        addresses: addrs.iter().map(ToString::to_string).collect(),
        name: link.name.clone(),
    })
}

/// Refuses a folder that already holds a certificate for another purpose, so claiming never
/// overwrites a viewer certificate. A folder with no certificate, or a worker one, is fine.
fn folder_conflict(folder: &Path) -> Option<String> {
    match join::bundle_role(folder) {
        Ok(PeerRole::Viewer | PeerRole::Monitor | PeerRole::Admin) => Some(
            "That folder already holds a certificate for another purpose. Choose another folder."
                .to_owned(),
        ),
        Ok(PeerRole::Worker) | Err(_) => None,
    }
}

// -------------------------------------------------------------------- pure helpers

/// Reads the pasted text.
fn check_link(text: &str) -> LinkCheck {
    if text.trim().is_empty() {
        return LinkCheck::Empty;
    }
    match PairingLink::parse(text) {
        Ok(link) => match link.role {
            PeerRole::Worker => LinkCheck::Worker(Box::new(link)),
            PeerRole::Viewer => LinkCheck::Problem(VIEWER_LINK.to_owned()),
            PeerRole::Monitor | PeerRole::Admin => LinkCheck::Problem(MANAGER_LINK.to_owned()),
        },
        Err(e) => LinkCheck::Problem(e.to_string()),
    }
}

/// "Garage coordinator · 192.168.178.40:7880": who the link is for and where.
fn coordinator_label(link: &PairingLink) -> String {
    let name = link.name.trim();
    let name = if name.is_empty() {
        "Another coordinator"
    } else {
        name
    };
    link.addrs
        .first()
        .map_or_else(|| name.to_owned(), |addr| format!("{name} \u{b7} {addr}"))
}

/// The name the status lines use.
fn coordinator_name(settings: &CoordinatorSettings) -> String {
    let name = settings.worker_coordinator_name.trim();
    if name.is_empty() {
        "the coordinator".to_owned()
    } else {
        name.to_owned()
    }
}

/// The saved connection as a line, or empty when none is saved or its certificate is gone.
fn saved_text(settings: &CoordinatorSettings, folder: &Path) -> String {
    let Some(first) = settings.worker_coordinator.first() else {
        return String::new();
    };
    if join::bundle_problem(folder).is_some() {
        return String::new();
    }
    let name = settings.worker_coordinator_name.trim();
    if name.is_empty() {
        first.clone()
    } else {
        format!("{name} \u{b7} {first}")
    }
}

/// The certificate folder: the chosen one, or `worker-cert` in the per-user data folder.
fn cert_dir_for(settings: &CoordinatorSettings) -> PathBuf {
    if let Some(chosen) = &settings.worker_cert_dir {
        return chosen.clone();
    }
    settings::default_data_dir()
        .or_else(|| local::folder().map(|f| f.data_dir().to_path_buf()))
        .unwrap_or_default()
        .join("worker-cert")
}

/// The `join` arguments the saved choices mean.
fn join_args(settings: &CoordinatorSettings, cert_dir: PathBuf) -> JoinArgs {
    JoinArgs {
        coordinator: settings.worker_coordinator.join(","),
        cert_dir,
        slots: usize::try_from(settings.worker_slots).unwrap_or(2).max(1),
        threads: usize::try_from(settings.worker_threads).unwrap_or(0),
        compute_mode: compute_mode(settings.worker_mode),
        token: None,
        enroll_addr: None,
        payload_encoding: PayloadChoice::Auto,
    }
}

/// "GPU only" needs a build with the GPU renderer; without it the work stays hybrid.
const fn compute_mode(choice: RenderChoice) -> ComputeMode {
    match choice {
        RenderChoice::Gpu if cfg!(feature = "gpu") => ComputeMode::OnlyGpu,
        RenderChoice::Cpu => ComputeMode::OnlyCpu,
        RenderChoice::GpuAndCpu | RenderChoice::Gpu => ComputeMode::Hybrid,
    }
}

/// 0 GPU + CPU, 1 GPU only, 2 CPU only: the order of the page's segments.
const fn mode_index(choice: RenderChoice) -> i32 {
    match choice {
        RenderChoice::GpuAndCpu => 0,
        RenderChoice::Gpu => 1,
        RenderChoice::Cpu => 2,
    }
}

/// The inverse of [`mode_index`]; anything unknown means GPU + CPU.
const fn mode_from_index(index: i32) -> RenderChoice {
    match index {
        1 => RenderChoice::Gpu,
        2 => RenderChoice::Cpu,
        _ => RenderChoice::GpuAndCpu,
    }
}

/// "Radeon 780M · 16 threads" (or just the threads when the graphics card is unknown).
fn hardware_text(hardware: &str, threads: i32) -> String {
    let threads = format!("{threads} threads");
    if hardware.trim().is_empty() {
        format!("{threads} on this computer")
    } else {
        format!("{} \u{b7} {threads}", hardware.trim())
    }
}

/// The page's phase code: 1 connecting, 2 joined and waiting, 3 rendering, 4 reconnecting.
const fn phase_code(phase: JoinPhase) -> i32 {
    match phase {
        JoinPhase::Connecting => 1,
        JoinPhase::Joined => 2,
        JoinPhase::Rendering => 3,
        JoinPhase::Reconnecting => 4,
        JoinPhase::Stopped => 0,
    }
}

/// The headline of the status card.
fn status_text(phase: JoinPhase, name: &str) -> String {
    match phase {
        JoinPhase::Connecting => format!("Connecting to {name}\u{2026}"),
        JoinPhase::Joined => format!("Connected \u{b7} waiting for work from {name}"),
        JoinPhase::Rendering => format!("Connected \u{b7} rendering for {name}"),
        JoinPhase::Reconnecting => format!("Waiting for {name} \u{b7} trying again"),
        JoinPhase::Stopped => String::new(),
    }
}

/// "for 18 min" while joined, otherwise empty.
fn status_right(status: &JoinStatus) -> String {
    status
        .joined_for
        .map(|joined| format!("for {}", uptime_text(joined.as_secs())))
        .unwrap_or_default()
}

/// One slot's chip: "slot 1: rendering".
fn chip_text(index: usize, phase: SlotPhase) -> String {
    let word = match phase {
        SlotPhase::Connecting => "connecting",
        SlotPhase::Idle => "waiting",
        SlotPhase::Rendering => "rendering",
        SlotPhase::Reconnecting => "reconnecting",
    };
    format!("slot {}: {word}", index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link_text(role: PeerRole) -> String {
        PairingLink::new(
            role,
            "Garage coordinator",
            vec!["192.168.178.40:7880".parse().expect("address")],
            7881,
            "GW1-TESTTOKEN123",
        )
        .build()
        .expect("link")
    }

    #[test]
    fn a_worker_link_is_accepted_and_names_its_coordinator() {
        let LinkCheck::Worker(link) = check_link(&link_text(PeerRole::Worker)) else {
            panic!("a worker link must be accepted");
        };
        assert_eq!(
            coordinator_label(&link),
            "Garage coordinator \u{b7} 192.168.178.40:7880"
        );
    }

    #[test]
    fn a_viewer_link_says_where_to_get_the_right_one() {
        let LinkCheck::Problem(text) = check_link(&link_text(PeerRole::Viewer)) else {
            panic!("a viewer link is not for lending render power");
        };
        assert_eq!(
            text,
            "This code is for a phone or a viewer. On the coordinator, choose Add a render computer."
        );
        assert!(matches!(
            check_link(&link_text(PeerRole::Admin)),
            LinkCheck::Problem(_)
        ));
    }

    #[test]
    fn nothing_and_nonsense_are_told_apart_and_leak_nothing() {
        assert!(matches!(check_link("   "), LinkCheck::Empty));
        let LinkCheck::Problem(text) = check_link("https://example.com/GW1-SECRET") else {
            panic!("not a pairing link");
        };
        assert!(!text.contains("SECRET"), "{text}");
    }

    #[test]
    fn the_modes_round_trip_and_the_gpu_choice_needs_the_gpu_build() {
        for choice in [
            RenderChoice::GpuAndCpu,
            RenderChoice::Gpu,
            RenderChoice::Cpu,
        ] {
            assert_eq!(mode_from_index(mode_index(choice)), choice);
        }
        assert_eq!(mode_from_index(9), RenderChoice::GpuAndCpu);
        assert_eq!(compute_mode(RenderChoice::Cpu), ComputeMode::OnlyCpu);
        assert_eq!(compute_mode(RenderChoice::GpuAndCpu), ComputeMode::Hybrid);
        let gpu = compute_mode(RenderChoice::Gpu);
        assert_eq!(
            gpu,
            if cfg!(feature = "gpu") {
                ComputeMode::OnlyGpu
            } else {
                ComputeMode::Hybrid
            }
        );
    }

    #[test]
    fn the_saved_choices_become_the_join_arguments() {
        let settings = CoordinatorSettings {
            worker_coordinator: vec!["10.0.0.5:7880".to_owned(), "192.168.1.5:7880".to_owned()],
            worker_slots: 3,
            worker_threads: 6,
            worker_mode: RenderChoice::Cpu,
            ..CoordinatorSettings::default()
        };
        let args = join_args(&settings, PathBuf::from("certs"));
        assert_eq!(args.coordinator, "10.0.0.5:7880,192.168.1.5:7880");
        assert_eq!((args.slots, args.threads), (3, 6));
        assert_eq!(args.compute_mode, ComputeMode::OnlyCpu);
        assert!(args.token.is_none() && args.enroll_addr.is_none());
        assert_eq!(args.cert_dir, PathBuf::from("certs"));
    }

    #[test]
    fn the_status_reads_in_plain_words() {
        assert_eq!(
            status_text(JoinPhase::Rendering, "Garage coordinator"),
            "Connected \u{b7} rendering for Garage coordinator"
        );
        assert_eq!(
            status_text(JoinPhase::Joined, "the coordinator"),
            "Connected \u{b7} waiting for work from the coordinator"
        );
        assert_eq!(chip_text(0, SlotPhase::Rendering), "slot 1: rendering");
        assert_eq!(chip_text(1, SlotPhase::Idle), "slot 2: waiting");
        assert_eq!(
            hardware_text("Radeon 780M", 16),
            "Radeon 780M \u{b7} 16 threads"
        );
        assert_eq!(hardware_text("", 8), "8 threads on this computer");
        assert_eq!(phase_code(JoinPhase::Reconnecting), 4);
    }

    #[test]
    fn a_missing_certificate_hides_the_saved_connection() {
        let settings = CoordinatorSettings {
            worker_coordinator: vec!["10.0.0.5:7880".to_owned()],
            worker_coordinator_name: "Garage coordinator".to_owned(),
            ..CoordinatorSettings::default()
        };
        let missing = std::env::temp_dir().join("pleochroma-worker-mode-no-such-folder");
        assert_eq!(saved_text(&settings, &missing), "");
        assert_eq!(coordinator_name(&settings), "Garage coordinator".to_owned());
        assert_eq!(
            coordinator_name(&CoordinatorSettings::default()),
            "the coordinator"
        );
    }
}
