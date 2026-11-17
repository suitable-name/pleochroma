//! The Render computers page: fills `RenderState` from the coordinator status once a second
//! and answers its callbacks.
//!
//! The page has this computer's own render lane (the "Render on this computer" switch, the
//! GPU and CPU mode and the CPU threads), the render computers joined now (with Disconnect),
//! the render computers that are allowed to join, and the total capacity. The own lane's
//! controls build a [`SettingsPatch`] (`render_enabled`, `render_mode`, `render_threads`) and
//! Disconnect calls `disconnect_worker`. Both can take a moment, so a helper thread runs them
//! one after the other and reports back to the window; only `status()` is read on the UI
//! thread. Times are relative ("Connected for 12 min"), so no time zone is needed.
//!
//! "Add a render computer" makes a one-time worker code (`issue_token(PeerRole::Worker, ..)`) and
//! shows its link and QR code in a dialog. The link is a secret: it is held in a wiping string,
//! shown with its code masked, copied only with the clipboard-history exclusion (and cleared from
//! the clipboard when the code runs out, is cancelled or is used), and the code is cancelled when
//! the dialog closes. The dialog follows the coordinator's events to say when a computer used it.

use crate::{
    control::{
        backend::{
            CoordinatorEvent, CoordinatorStatus, IssuedToken, PeerRole, SettingsPatch, TokenId,
            WorkerId,
        },
        status::unix_now,
    },
    gui::{
        app::{ActiveBackend, AppWindow, RcAllowedRow, RcConnectedRow, RenderState},
        pages::overview::{ago, kind_label, render_device, uptime_text, uses_this_computer},
        qr,
        util::{clock_text, copy_secret, mask_link},
    },
};
use pleochroma_net::admin::status::{
    CapacitySummary, RenderComputerStatus, RenderHardware, RenderMode,
};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc::{Receiver, Sender, channel},
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroizing;

/// How often the page re-reads the status.
const REFRESH: Duration = Duration::from_secs(1);

thread_local! {
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static REFRESH_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
    /// How many changes of the own lane are still being applied. While it is not zero the
    /// refresh leaves the own lane's controls alone, so a click does not flicker back.
    static IN_FLIGHT: Cell<u32> = const { Cell::new(0) };
    /// The "Add a render computer" dialog's state.
    static ADD: RefCell<AddState> = const { RefCell::new(AddState::new()) };
}

/// The phases of `RenderState.add-phase`.
mod phase {
    /// A code is being made.
    pub const MAKING: i32 = 0;
    /// A code is on screen and waiting for the computer.
    pub const WAITING: i32 = 1;
    /// A computer used the code.
    pub const CONNECTED: i32 = 2;
    /// The code ran out.
    pub const EXPIRED: i32 = 3;
    /// No code could be made.
    pub const PROBLEM: i32 = 4;
}

/// The live worker code of the dialog.
struct Live {
    id: TokenId,
    /// The pairing link, a secret; wiped when dropped.
    link: Zeroizing<String>,
    expires: Instant,
    total: Duration,
}

/// What the "Add a render computer" dialog remembers between ticks.
struct AddState {
    open: bool,
    /// The number of the newest code request; older answers are cancelled on arrival.
    generation: u64,
    live: Option<Live>,
    events: Option<Receiver<CoordinatorEvent>>,
    /// The link last copied, kept only to recognize it on the clipboard for clearing.
    copied_link: Option<Zeroizing<String>>,
    clipboard: Option<arboard::Clipboard>,
}

impl AddState {
    const fn new() -> Self {
        Self {
            open: false,
            generation: 0,
            live: None,
            events: None,
            copied_link: None,
            clipboard: None,
        }
    }
}

/// Something the helper thread does for the page.
enum Job {
    /// Apply a change of the own lane.
    Apply(SettingsPatch),
    /// Disconnect a joined render computer.
    Disconnect(WorkerId),
}

/// The row models of the page.
struct Models {
    /// Joined render computers.
    connected: Rc<VecModel<RcConnectedRow>>,
    /// Allowed render computers.
    allowed: Rc<VecModel<RcAllowedRow>>,
}

/// Connects the Render computers page of `window` to `backend`: shows the current state at
/// once, refreshes it every second, and wires the own lane's controls and Disconnect.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<RenderState>();
    let models = Models {
        connected: Rc::new(VecModel::default()),
        allowed: Rc::new(VecModel::default()),
    };
    state.set_connected(ModelRc::from(Rc::clone(&models.connected)));
    state.set_allowed(ModelRc::from(Rc::clone(&models.allowed)));
    let max_threads = std::thread::available_parallelism()
        .map_or(16, std::num::NonZeroUsize::get)
        .clamp(1, 1024);
    state.set_max_threads(i32::try_from(max_threads).unwrap_or(16));

    wire_own_lane(window, &spawn_helper(window, backend));
    wire_add_dialog(window, backend);

    refresh(window, backend, &models);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, REFRESH, {
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                refresh(&window, &backend, &models);
                add_tick(&window, &backend);
            }
        }
    });
    REFRESH_TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

/// Wires this computer's own render lane (the switch, the mode, the threads) and Disconnect to
/// the helper thread `jobs`.
fn wire_own_lane(window: &AppWindow, jobs: &Sender<Job>) {
    let state = window.global::<RenderState>();
    state.on_set_enabled({
        let jobs = jobs.clone();
        let weak = window.as_weak();
        move |on| {
            submit(
                &weak,
                &jobs,
                Job::Apply(SettingsPatch {
                    render_enabled: Some(on),
                    ..SettingsPatch::default()
                }),
            );
        }
    });
    state.on_set_mode({
        let jobs = jobs.clone();
        let weak = window.as_weak();
        move |index| {
            submit(
                &weak,
                &jobs,
                Job::Apply(SettingsPatch {
                    render_mode: Some(mode_from_index(index)),
                    ..SettingsPatch::default()
                }),
            );
        }
    });
    state.on_set_threads({
        let jobs = jobs.clone();
        let weak = window.as_weak();
        move |threads| {
            submit(
                &weak,
                &jobs,
                Job::Apply(SettingsPatch {
                    render_threads: Some(u32::try_from(threads).unwrap_or(0)),
                    ..SettingsPatch::default()
                }),
            );
        }
    });
    state.on_disconnect({
        let weak = window.as_weak();
        let jobs = jobs.clone();
        move |id| {
            // Id 0 is this computer's own lane, which is never disconnected.
            let Some(id) = u32::try_from(id).ok().filter(|id| *id != 0) else {
                if let Some(window) = weak.upgrade() {
                    window
                        .global::<RenderState>()
                        .set_error("That render computer is not valid.".into());
                }
                return;
            };
            submit(&weak, &jobs, Job::Disconnect(WorkerId(id)));
        }
    });
}

/// Wires the "Add a render computer" dialog.
fn wire_add_dialog(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<RenderState>();
    state.set_add_open(false);
    state.on_add_requested({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                add_open(&window, &backend);
            }
        }
    });
    state.on_add_close({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                add_close(&window, &backend);
            }
        }
    });
    state.on_add_new_code({
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                add_new_code(&window, &backend);
            }
        }
    });
    state.on_add_copy({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                add_copy(&window);
            }
        }
    });
}

/// Starts the helper thread that runs the page's jobs one after the other, and returns the
/// way to hand it work. Each finished job clears or sets the page's error text.
fn spawn_helper(window: &AppWindow, backend: &ActiveBackend) -> Sender<Job> {
    let (sender, receiver) = channel::<Job>();
    let weak = window.as_weak();
    let backend = backend.clone();
    let spawned = std::thread::Builder::new()
        .name("render-page".to_string())
        .spawn(move || {
            for job in receiver {
                let applies = matches!(job, Job::Apply(_));
                let result = match job {
                    Job::Apply(patch) => backend.get().apply_settings(&patch),
                    Job::Disconnect(id) => backend.get().disconnect_worker(id),
                };
                let weak = weak.clone();
                // The window may be gone already; then there is nothing to report.
                let _ = slint::invoke_from_event_loop(move || {
                    if applies {
                        IN_FLIGHT.with(|count| count.set(count.get().saturating_sub(1)));
                    }
                    if let Some(window) = weak.upgrade() {
                        window
                            .global::<RenderState>()
                            .set_error(result.err().unwrap_or_default().into());
                    }
                });
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("the render computers page could not start its helper: {e}");
    }
    sender
}

/// Hands `job` to the helper thread; if the helper is gone, says so on the page.
fn submit(weak: &slint::Weak<AppWindow>, jobs: &Sender<Job>, job: Job) {
    let applies = matches!(job, Job::Apply(_));
    if applies {
        IN_FLIGHT.with(|count| count.set(count.get() + 1));
    }
    if jobs.send(job).is_err() {
        if applies {
            IN_FLIGHT.with(|count| count.set(count.get().saturating_sub(1)));
        }
        if let Some(window) = weak.upgrade() {
            window
                .global::<RenderState>()
                .set_error("The change could not be sent. Close and reopen the window.".into());
        }
    }
}

/// Reads the status and permissions and updates the page state and rows.
fn refresh(window: &AppWindow, backend: &ActiveBackend, models: &Models) {
    let backend = backend.get();
    let status = backend.status();
    let caps = backend.capabilities();
    let now = unix_now();
    let state = window.global::<RenderState>();

    // The own lane's controls keep the value the owner just chose until it is applied.
    if IN_FLIGHT.with(Cell::get) == 0 {
        let own = &status.own_render;
        state.set_own_enabled(own.enabled);
        state.set_own_mode(mode_index(own.mode));
        state.set_own_threads(i32::try_from(own.threads.unwrap_or(0)).unwrap_or(0));
    }
    let (level, text) = own_state(&status);
    state.set_own_level(level);
    state.set_own_state(text.into());
    state.set_own_hardware(status.own_render.hardware.as_str().into());
    state.set_capacity(capacity_text(&status.capacity).into());
    state.set_can_change(caps.apply_settings);
    state.set_can_disconnect(caps.disconnect_workers);
    state.set_can_add(caps.issue_tokens);
    state.set_denied_reason(if caps.apply_settings && caps.disconnect_workers {
        SharedString::new()
    } else {
        caps.denied_reason().into()
    });

    sync_rows(&models.connected, connected_rows(&status, now));
    sync_rows(&models.allowed, allowed_rows(&status, now));
    state.set_loaded(true);
}

// ---------------------------------------------------------------- "Add a render computer"

/// Opens the dialog and asks for a worker code.
fn add_open(window: &AppWindow, backend: &ActiveBackend) {
    add_release(backend);
    ADD.with(|cell| {
        let mut add = cell.borrow_mut();
        add.open = true;
        add.events = Some(backend.get().events());
    });
    let state = window.global::<RenderState>();
    state.set_add_notice(SharedString::new());
    state.set_add_copied(false);
    state.set_add_open(true);
    add_request(window, backend);
}

/// Closes the dialog: the code is cancelled and its link leaves the screen and the clipboard.
fn add_close(window: &AppWindow, backend: &ActiveBackend) {
    add_release(backend);
    ADD.with(|cell| {
        let mut add = cell.borrow_mut();
        add.open = false;
        add.events = None;
        add.generation += 1;
    });
    let state = window.global::<RenderState>();
    state.set_add_open(false);
    clear_code_view(window);
}

/// "Show a new code", "Try again" and "Add another": a fresh code.
fn add_new_code(window: &AppWindow, backend: &ActiveBackend) {
    add_release(backend);
    window
        .global::<RenderState>()
        .set_add_notice(SharedString::new());
    add_request(window, backend);
}

/// The name a new render computer gets until the owner renames it: "Render computer 2".
fn default_computer_name(status: &CoordinatorStatus) -> String {
    let count = status
        .devices
        .iter()
        .filter(|device| device.role == PeerRole::Worker)
        .count()
        + 1;
    format!("Render computer {count}")
}

/// Asks the coordinator for a worker code on a helper thread and shows "Making a code…".
fn add_request(window: &AppWindow, backend: &ActiveBackend) {
    let generation = ADD.with(|cell| {
        let mut add = cell.borrow_mut();
        add.generation += 1;
        add.generation
    });
    let state = window.global::<RenderState>();
    state.set_add_phase(phase::MAKING);
    clear_code_view(window);
    let name = default_computer_name(&backend.get().status());
    let backend = backend.clone();
    let weak = window.as_weak();
    std::thread::spawn(move || {
        let result = backend.get().issue_token(PeerRole::Worker, &name);
        let _ = weak.upgrade_in_event_loop(move |window| {
            add_issued(&window, &backend, generation, result);
        });
    });
}

/// A code was made (or not) for the request numbered `generation`.
fn add_issued(
    window: &AppWindow,
    backend: &ActiveBackend,
    generation: u64,
    result: Result<IssuedToken, String>,
) {
    let current = ADD.with(|cell| {
        let add = cell.borrow();
        add.open && add.generation == generation
    });
    match (current, result) {
        (true, Ok(issued)) => add_show(window, backend, issued),
        (true, Err(error)) => add_problem(window, backend, &error),
        // A code nobody asked for any more: it must not stay valid.
        (false, Ok(issued)) => cancel_in_background(backend, issued.id),
        (false, Err(_)) => {}
    }
}

/// Shows a code that was just made.
fn add_show(window: &AppWindow, backend: &ActiveBackend, issued: IssuedToken) {
    let IssuedToken {
        id,
        link,
        expires_at,
        ..
    } = issued;
    let remaining = expires_at
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO);
    let Some(image) = qr::image(&link) else {
        cancel_in_background(backend, id);
        add_problem(
            window,
            backend,
            "The pairing code is too long to draw. Try again.",
        );
        return;
    };
    let state = window.global::<RenderState>();
    state.set_add_qr(image);
    state.set_add_link_shown(SharedString::from(mask_link(&link)));
    state.set_add_fraction(1.0);
    state.set_add_remaining(SharedString::from(clock_text(remaining)));
    state.set_add_copied(false);
    state.set_add_notice(SharedString::new());
    state.set_add_phase(phase::WAITING);
    ADD.with(|cell| {
        cell.borrow_mut().live = Some(Live {
            id,
            link: Zeroizing::new(link),
            expires: Instant::now() + remaining,
            total: remaining.max(Duration::from_secs(1)),
        });
    });
}

/// Shows why no code could be made.
fn add_problem(window: &AppWindow, backend: &ActiveBackend, error: &str) {
    let running = backend.get().status().state.is_running();
    let error = error.trim();
    let text = if !running {
        "The coordinator is not running. Start it, then try again."
    } else if error.is_empty() {
        "Something went wrong. Try again."
    } else {
        error
    };
    let state = window.global::<RenderState>();
    clear_code_view(window);
    state.set_add_notice(SharedString::from(text));
    state.set_add_phase(phase::PROBLEM);
}

/// Takes the code off the screen.
fn clear_code_view(window: &AppWindow) {
    let state = window.global::<RenderState>();
    state.set_add_qr(Image::default());
    state.set_add_link_shown(SharedString::new());
    state.set_add_copied(false);
}

/// Cancels the live code, if any, and clears the clipboard if it still holds its link.
fn add_release(backend: &ActiveBackend) {
    let live = ADD.with(|cell| cell.borrow_mut().live.take());
    if let Some(live) = live {
        cancel_in_background(backend, live.id);
    }
    clear_clipboard();
}

/// Cancels a code without making the window wait for it.
fn cancel_in_background(backend: &ActiveBackend, id: TokenId) {
    let backend = backend.clone();
    std::thread::spawn(move || backend.get().cancel_token(id));
}

/// The clock and the coordinator's events, once a second while the dialog is open.
fn add_tick(window: &AppWindow, backend: &ActiveBackend) {
    if !ADD.with(|cell| cell.borrow().open) {
        return;
    }
    let events: Vec<CoordinatorEvent> = ADD.with(|cell| {
        cell.borrow()
            .events
            .as_ref()
            .map(|events| events.try_iter().collect())
            .unwrap_or_default()
    });
    for event in events {
        match event {
            CoordinatorEvent::TokenClaimed {
                role: PeerRole::Worker,
                peer,
                ..
            } => {
                add_claimed(window, &peer.ip().to_string());
            }
            CoordinatorEvent::TokenExpired { id } => {
                let ours = ADD.with(|cell| cell.borrow().live.as_ref().is_some_and(|l| l.id == id));
                if ours {
                    add_expire(window, backend);
                }
            }
            _ => {}
        }
    }
    let state = window.global::<RenderState>();
    if state.get_add_phase() != phase::WAITING {
        return;
    }
    let left = ADD.with(|cell| {
        cell.borrow().live.as_ref().map(|live| {
            (
                live.expires.saturating_duration_since(Instant::now()),
                live.total,
            )
        })
    });
    match left {
        Some((remaining, total)) if !remaining.is_zero() => {
            state.set_add_fraction(remaining.as_secs_f32() / total.as_secs_f32());
            state.set_add_remaining(SharedString::from(clock_text(remaining)));
        }
        _ => add_expire(window, backend),
    }
}

/// The live code was used from `address`.
fn add_claimed(window: &AppWindow, address: &str) {
    let live = ADD.with(|cell| cell.borrow_mut().live.take());
    if live.is_none() {
        // A claim with no live code (a code of an earlier dialog): not ours to show.
        return;
    }
    clear_clipboard();
    let state = window.global::<RenderState>();
    clear_code_view(window);
    state.set_add_from(SharedString::from(address));
    state.set_add_phase(phase::CONNECTED);
}

/// The live code ran out.
fn add_expire(window: &AppWindow, backend: &ActiveBackend) {
    add_release(backend);
    let state = window.global::<RenderState>();
    clear_code_view(window);
    state.set_add_phase(phase::EXPIRED);
}

/// Copies the link of the live code with the platform's clipboard-history exclusion.
fn add_copy(window: &AppWindow) {
    let state = window.global::<RenderState>();
    ADD.with(|cell| {
        let mut add = cell.borrow_mut();
        let Some(link) = add.live.as_ref().map(|live| live.link.clone()) else {
            return;
        };
        if add.clipboard.is_none() {
            let Ok(clipboard) = arboard::Clipboard::new() else {
                state.set_add_notice(SharedString::from("The clipboard is not available."));
                return;
            };
            add.clipboard = Some(clipboard);
        }
        let copied = add
            .clipboard
            .as_mut()
            .is_some_and(|clipboard| copy_secret(clipboard, &link).is_ok());
        if copied {
            add.copied_link = Some(link);
            state.set_add_copied(true);
            state.set_add_notice(SharedString::new());
        } else {
            state.set_add_notice(SharedString::from("The link could not be copied."));
        }
    });
}

/// Empties the clipboard if it still holds the link this dialog copied.
fn clear_clipboard() {
    ADD.with(|cell| {
        let mut add = cell.borrow_mut();
        let Some(link) = add.copied_link.take() else {
            return;
        };
        if let Some(clipboard) = add.clipboard.as_mut() {
            let holds_link = clipboard
                .get_text()
                .is_ok_and(|text| text.as_str() == link.as_str());
            if holds_link {
                let _ = clipboard.clear();
            }
        }
    });
}

/// Brings `model` to `rows` without resetting it when only row contents changed.
fn sync_rows<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    if model.row_count() == rows.len() {
        for (i, row) in rows.into_iter().enumerate() {
            if model.row_data(i).as_ref() != Some(&row) {
                model.set_row_data(i, row);
            }
        }
    } else {
        model.set_vec(rows);
    }
}

/// 0 GPU + CPU, 1 GPU only, 2 CPU only: the order of the page's segments.
const fn mode_index(mode: RenderMode) -> i32 {
    match mode {
        RenderMode::GpuAndCpu => 0,
        RenderMode::GpuOnly => 1,
        RenderMode::CpuOnly => 2,
    }
}

/// The inverse of [`mode_index`]; anything unknown means GPU + CPU.
const fn mode_from_index(index: i32) -> RenderMode {
    match index {
        1 => RenderMode::GpuOnly,
        2 => RenderMode::CpuOnly,
        _ => RenderMode::GpuAndCpu,
    }
}

/// The own lane's state: the dot level (0 idle, 1 busy, 3 off) and its words. While a viewer's
/// render uses this computer it names the device and the kind ("Rendering · Phone 1 ·
/// Live view", plus "and 1 more" for further renders); while the graphics card builds its shader
/// cache it says so.
fn own_state(status: &CoordinatorStatus) -> (i32, String) {
    let own = &status.own_render;
    let mut rendering = status
        .renders
        .iter()
        .filter(|render| uses_this_computer(render));
    if !own.enabled {
        (3, "Off".to_string())
    } else if !status.state.is_running() {
        (3, "Stopped".to_string())
    } else if let Some(first) = rendering.next() {
        let more = match rendering.count() {
            0 => String::new(),
            n => format!(" and {n} more"),
        };
        (
            1,
            format!(
                "Rendering \u{b7} {} \u{b7} {}{more}",
                render_device(first),
                kind_label(first.kind)
            ),
        )
    } else if crate::gui::shader_cache::building(status) {
        (1, "Building shader cache".to_string())
    } else if own.busy {
        (1, "Busy".to_string())
    } else {
        (0, "Idle".to_string())
    }
}

/// "2 GPUs · 40 threads · up to 7680 × 4320"; empty when there is nothing to total.
fn capacity_text(capacity: &CapacitySummary) -> String {
    let mut parts = Vec::new();
    if capacity.gpus > 0 {
        parts.push(format!(
            "{} {}",
            capacity.gpus,
            if capacity.gpus == 1 { "GPU" } else { "GPUs" }
        ));
    }
    if capacity.threads > 0 {
        parts.push(format!(
            "{} {}",
            capacity.threads,
            if capacity.threads == 1 {
                "thread"
            } else {
                "threads"
            }
        ));
    }
    if capacity.max_width > 0 && capacity.max_height > 0 {
        parts.push(format!(
            "up to {} \u{d7} {}",
            capacity.max_width, capacity.max_height
        ));
    }
    parts.join(" \u{b7} ")
}

/// The joined render computers (the own lane is the card above the table).
fn connected_rows(status: &CoordinatorStatus, now: u64) -> Vec<RcConnectedRow> {
    status
        .render_computers
        .iter()
        .filter(|computer| !computer.own_lane)
        .map(|computer| connected_row(computer, now))
        .collect()
}

/// One row of the joined table.
fn connected_row(computer: &RenderComputerStatus, now: u64) -> RcConnectedRow {
    let hardware = if computer.hardware_name.is_empty() {
        let word = match computer.hardware {
            RenderHardware::Gpu => "Graphics card",
            RenderHardware::Cpu => "Processor",
            RenderHardware::GpuAndCpu => "Graphics card and processor",
        };
        word.to_string()
    } else {
        computer.hardware_name.clone()
    };
    let busy = computer.busy > 0;
    let state_sub = if !computer.activity.is_empty() {
        computer.activity.clone()
    } else if computer.slots > 0 {
        format!(
            "{} of {} {} busy",
            computer.busy,
            computer.slots,
            plural(computer.slots, "slot", "slots")
        )
    } else {
        String::new()
    };
    RcConnectedRow {
        id: i32::try_from(computer.id.0).unwrap_or(i32::MAX),
        name: computer.name.as_str().into(),
        kind: if computer.platform.is_empty() {
            "Render computer".into()
        } else {
            computer.platform.as_str().into()
        },
        hardware: hardware.into(),
        hardware_sub: format!(
            "{} {} \u{b7} {} {}",
            computer.threads,
            plural(computer.threads, "thread", "threads"),
            computer.slots,
            plural(computer.slots, "slot", "slots")
        )
        .into(),
        busy,
        state: if busy { "Busy" } else { "Idle" }.into(),
        state_sub: state_sub.into(),
        since: since_text(computer.connected_since, now).into(),
        rate: match computer.rate {
            Some(rate) if rate > 0.0 => format!("{rate:.1} samples per second"),
            _ => String::new(),
        }
        .into(),
        addr: computer.address.clone().unwrap_or_default().into(),
    }
}

/// The render computers on the allowlist, joined or not.
fn allowed_rows(status: &CoordinatorStatus, now: u64) -> Vec<RcAllowedRow> {
    status
        .devices
        .iter()
        .filter(|device| device.role == PeerRole::Worker)
        .map(|device| RcAllowedRow {
            name: device.name.as_str().into(),
            kind: if device.platform.is_empty() {
                "Render computer".into()
            } else {
                device.platform.as_str().into()
            },
            added: device
                .added
                .map(|added| format!("Added {}", ago(now.saturating_sub(added))))
                .unwrap_or_default()
                .into(),
            online: device.online,
            state: if device.online { "Online" } else { "Offline" }.into(),
            state_sub: if device.online {
                "Allowed to take jobs"
            } else {
                "Will join when it starts"
            }
            .into(),
            seen: if device.online {
                "Now".to_string()
            } else {
                device.last_seen.map_or_else(
                    || "Never connected".to_string(),
                    |seen| format!("Last seen {}", ago(now.saturating_sub(seen))),
                )
            }
            .into(),
            addr: device.address.clone().unwrap_or_default().into(),
        })
        .collect()
}

/// "Connected for 12 min" or "Connected just now".
fn since_text(since: Option<u64>, now: u64) -> String {
    since.map_or_else(
        || "Connected".to_string(),
        |since| {
            let seconds = now.saturating_sub(since);
            if seconds < 60 {
                "Connected just now".to_string()
            } else {
                format!("Connected for {}", uptime_text(seconds))
            }
        },
    )
}

/// `one` for 1, `many` otherwise.
const fn plural(count: u32, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::backend::CoordinatorState;
    use pleochroma_net::admin::status::DeviceStatus;

    fn status() -> CoordinatorStatus {
        CoordinatorStatus::minimal("Studio PC", "0.12.0", 45, CoordinatorState::Running)
    }

    fn rig(own_lane: bool) -> RenderComputerStatus {
        RenderComputerStatus {
            id: WorkerId(3),
            name: "garage-rig".to_string(),
            fingerprint: "cd".repeat(32),
            own_lane,
            hardware: RenderHardware::Gpu,
            hardware_name: "RTX 4070".to_string(),
            threads: 24,
            slots: 2,
            busy: 1,
            rate: Some(12.5),
            platform: "Worker \u{b7} Windows".to_string(),
            connected_since: Some(1_000),
            activity: String::new(),
            address: Some("192.168.178.40".to_string()),
        }
    }

    #[test]
    fn modes_round_trip_through_the_segment_index() {
        for mode in [
            RenderMode::GpuAndCpu,
            RenderMode::GpuOnly,
            RenderMode::CpuOnly,
        ] {
            assert_eq!(mode_from_index(mode_index(mode)), mode);
        }
        assert_eq!(mode_from_index(9), RenderMode::GpuAndCpu);
    }

    #[test]
    fn the_worker_code_clock_rounds_up_and_the_link_hides_its_code() {
        assert_eq!(clock_text(Duration::from_secs(180)), "3:00");
        assert_eq!(clock_text(Duration::from_millis(500)), "0:01");
        let link = "pleochroma://pair?v=1&r=worker&n=Garage&a=192.168.178.40:7880&e=7881&t=GW1-SECRETSECRET";
        let masked = mask_link(link);
        assert!(
            !masked.contains("SECRET") && !masked.contains("GW1-"),
            "{masked}"
        );
        assert!(masked.contains("a=192.168.178.40:7880"));
        assert_eq!(mask_link("not a link"), "pleochroma://pair?\u{2026}");
    }

    #[test]
    fn a_new_render_computer_is_named_after_the_ones_already_allowed() {
        let mut status = status();
        assert_eq!(default_computer_name(&status), "Render computer 1");
        status.devices.push(DeviceStatus::new(
            "Attic PC",
            "ef".repeat(32),
            PeerRole::Worker,
        ));
        status.devices.push(DeviceStatus::new(
            "Phone",
            "ab".repeat(32),
            PeerRole::Viewer,
        ));
        assert_eq!(default_computer_name(&status), "Render computer 2");
    }

    #[test]
    fn capacity_reads_in_plain_words() {
        assert_eq!(capacity_text(&CapacitySummary::default()), "");
        let capacity = CapacitySummary {
            gpus: 2,
            threads: 40,
            max_width: 7680,
            max_height: 4320,
        };
        assert_eq!(
            capacity_text(&capacity),
            "2 GPUs \u{b7} 40 threads \u{b7} up to 7680 \u{d7} 4320"
        );
        let one = CapacitySummary {
            gpus: 1,
            threads: 1,
            ..CapacitySummary::default()
        };
        assert_eq!(capacity_text(&one), "1 GPU \u{b7} 1 thread");
    }

    #[test]
    fn the_own_lane_is_the_card_not_a_row() {
        let mut status = status();
        status.render_computers.push(rig(true));
        status.render_computers.push(rig(false));
        assert_eq!(connected_rows(&status, 1_000).len(), 1);
    }

    #[test]
    fn a_joined_row_says_what_it_offers_and_does() {
        let row = connected_row(&rig(false), 1_000 + 12 * 60);
        assert_eq!(row.id, 3);
        assert_eq!(row.hardware.as_str(), "RTX 4070");
        assert_eq!(row.hardware_sub.as_str(), "24 threads \u{b7} 2 slots");
        assert!(row.busy);
        assert_eq!(row.state.as_str(), "Busy");
        assert_eq!(row.state_sub.as_str(), "1 of 2 slots busy");
        assert_eq!(row.since.as_str(), "Connected for 12 min");
        assert_eq!(row.rate.as_str(), "12.5 samples per second");
    }

    #[test]
    fn allowed_rows_are_the_worker_allowlist() {
        let mut status = status();
        let mut attic = DeviceStatus::new("Attic PC", "ef".repeat(32), PeerRole::Worker);
        let now = 1_000_000_u64;
        attic.last_seen = Some(now - 2 * 3600);
        attic.added = Some(now - 3 * 86_400);
        status.devices.push(attic);
        status.devices.push(DeviceStatus::new(
            "Phone",
            "ab".repeat(32),
            PeerRole::Viewer,
        ));
        let rows = allowed_rows(&status, now);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].seen.as_str(), "Last seen 2 h ago");
        assert_eq!(rows[0].added.as_str(), "Added 3 days ago");
        assert_eq!(rows[0].state.as_str(), "Offline");
    }

    #[test]
    fn own_lane_state_follows_switch_and_coordinator() {
        let mut status = status();
        assert_eq!(own_state(&status), (3, "Off".to_string()));
        status.own_render.enabled = true;
        assert_eq!(own_state(&status), (0, "Idle".to_string()));
        status.own_render.busy = true;
        assert_eq!(own_state(&status), (1, "Busy".to_string()));
        status.shader_cache.state = pleochroma_net::admin::status::ShaderCacheState::Running;
        status.shader_cache.fresh = true;
        assert_eq!(own_state(&status), (1, "Building shader cache".to_string()));
        status.state = CoordinatorState::Stopped;
        assert_eq!(own_state(&status), (3, "Stopped".to_string()));
    }

    #[test]
    fn own_lane_names_the_render_that_uses_it() {
        use pleochroma_net::admin::status::{ActiveRender, RenderKind};
        let render = |device: &str, kind: RenderKind, route: &str| ActiveRender {
            id: 1,
            device: device.to_string(),
            fingerprint: String::new(),
            kind,
            engine: String::new(),
            width: 640,
            height: 480,
            samples_done: 0,
            samples_target: None,
            started_unix: 1,
            route: route.to_string(),
            rate_mps: None,
            preparing: None,
        };
        let mut status = status();
        status.own_render.enabled = true;
        status
            .renders
            .push(render("Phone 2", RenderKind::Live, "1 render computer"));
        assert_eq!(own_state(&status), (0, "Idle".to_string()));
        status.renders.push(render(
            "Phone 1",
            RenderKind::FinalImage,
            "This computer + 1 render computer",
        ));
        assert_eq!(
            own_state(&status),
            (
                1,
                "Rendering \u{b7} Phone 1 \u{b7} Final picture".to_string()
            )
        );
        status
            .renders
            .push(render("Tablet", RenderKind::Live, "This computer"));
        assert_eq!(
            own_state(&status).1,
            "Rendering \u{b7} Phone 1 \u{b7} Final picture and 1 more"
        );
    }
}
