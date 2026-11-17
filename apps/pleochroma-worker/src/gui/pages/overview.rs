//! The Overview page: fills `OverviewState` from the coordinator status and the event stream
//! once a second.
//!
//! The page shows whether the coordinator runs and how to reach it (state, address, ports,
//! uptime), three summary cards (devices, render computers, jobs), the renders running now,
//! the recent activity and the warnings. The activity feed is built from
//! [`CoordinatorEvent::summary`] and [`CoordinatorEvent::kind`] of the events that arrive while
//! the window is open; each line carries a relative time ("5 min ago"), so no time zone is
//! needed. The two Connect buttons call `PairState.open-requested` from the Slint side.

use crate::{
    control::{
        backend::{CoordinatorBackend, CoordinatorState, CoordinatorStatus, PeerRole},
        status::unix_now,
    },
    gui::app::{ActiveBackend, AppWindow, OvActivityRow, OvLiveRow, OverviewState},
};
use pleochroma_net::admin::status::{
    ActiveRender, CoordinatorEvent, EventKind, JobState, RenderKind,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::{Arc, mpsc::Receiver},
    time::Duration,
};

/// How often the page re-reads the status and the events.
const REFRESH: Duration = Duration::from_secs(1);

/// How many events the feed remembers.
const FEED_KEPT: usize = 50;

/// How many events the page shows.
const FEED_SHOWN: usize = 8;

thread_local! {
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static REFRESH_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// The events seen since the window opened, newest first, and the stream they come from.
struct Feed {
    /// The backend the stream belongs to, to notice a switch of coordinator.
    source: Arc<dyn CoordinatorBackend>,
    /// The event stream of `source`.
    events: Receiver<CoordinatorEvent>,
    /// When it arrived (unix seconds), its tone and its text.
    entries: VecDeque<(u64, EventKind, String)>,
}

impl Feed {
    /// Subscribes to `backend`.
    fn new(backend: Arc<dyn CoordinatorBackend>) -> Self {
        Self {
            events: backend.events(),
            source: backend,
            entries: VecDeque::new(),
        }
    }

    /// Takes in what arrived since the last call. A different coordinator starts a new feed.
    fn collect(&mut self, backend: &Arc<dyn CoordinatorBackend>, now: u64) {
        if !Arc::ptr_eq(&self.source, backend) {
            *self = Self::new(Arc::clone(backend));
        }
        while let Ok(event) = self.events.try_recv() {
            // Log lines belong to the Log page.
            if matches!(event, CoordinatorEvent::Log(_)) {
                continue;
            }
            self.entries
                .push_front((now, event.kind(), event.summary()));
            self.entries.truncate(FEED_KEPT);
        }
    }
}

/// Connects the Overview page of `window` to `backend`: shows the current state at once and
/// refreshes it every second.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let state = window.global::<OverviewState>();
    let live = Rc::new(VecModel::<OvLiveRow>::default());
    let activity = Rc::new(VecModel::<OvActivityRow>::default());
    let warnings = Rc::new(VecModel::<SharedString>::default());
    state.set_live(ModelRc::from(Rc::clone(&live)));
    state.set_activity(ModelRc::from(Rc::clone(&activity)));
    state.set_warnings(ModelRc::from(Rc::clone(&warnings)));

    let models = Models {
        live,
        activity,
        warnings,
        feed: RefCell::new(Feed::new(backend.get())),
    };
    refresh(window, backend, &models);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, REFRESH, {
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                refresh(&window, &backend, &models);
            }
        }
    });
    REFRESH_TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

/// The row models of the page and the event feed behind the activity rows.
struct Models {
    /// "Rendering now".
    live: Rc<VecModel<OvLiveRow>>,
    /// "Recent activity".
    activity: Rc<VecModel<OvActivityRow>>,
    /// The warning cards.
    warnings: Rc<VecModel<SharedString>>,
    /// The events behind `activity`.
    feed: RefCell<Feed>,
}

/// Reads the status, permissions and events and updates the page state and rows.
fn refresh(window: &AppWindow, backend: &ActiveBackend, models: &Models) {
    let backend = backend.get();
    let status = backend.status();
    let caps = backend.capabilities();
    let now = unix_now();

    let mut feed = models.feed.borrow_mut();
    feed.collect(&backend, now);

    let state = window.global::<OverviewState>();
    let hero = hero(&status, now);
    state.set_hero_title(hero.title.into());
    state.set_hero_level(hero.level);
    state.set_hero_sub(hero.ports.into());
    state.set_address(hero.address.into());
    state.set_uptime(hero.uptime.into());

    let (devices_value, devices_sub) = devices_card(&status);
    state.set_devices_value(devices_value.into());
    state.set_devices_sub(devices_sub.into());
    let (computers_value, computers_sub) = computers_card(&status);
    state.set_computers_value(computers_value.into());
    state.set_computers_sub(computers_sub.into());
    state.set_jobs_value(format!("{} running", status.jobs_running()).into());
    state.set_jobs_sub(format!("{} queued", status.jobs_queued()).into());

    let live_rows = live_rows(&status, now);
    state.set_live_note(
        if live_rows.is_empty() {
            "nothing active".to_string()
        } else {
            format!("{} active", live_rows.len())
        }
        .into(),
    );
    sync_rows(&models.live, live_rows);
    sync_rows(&models.activity, activity_rows(&feed, now));
    sync_rows(
        &models.warnings,
        warning_texts(&status)
            .into_iter()
            .map(SharedString::from)
            .collect(),
    );
    state.set_can_pair(status.state.is_running() && caps.issue_tokens);
    state.set_loaded(true);
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

/// The top card's texts.
struct Hero {
    /// "Running · phones on your network can connect".
    title: String,
    /// 0 fine, 1 attention, 2 problem, 3 off.
    level: i32,
    /// "Ports 7878 · 7879 · 7880 · 7881", or empty.
    ports: String,
    /// The address other devices use, or empty.
    address: String,
    /// "3 h 12 min", or empty when the coordinator does not run.
    uptime: String,
}

/// True when the viewer port listens on this computer only.
fn local_only(status: &CoordinatorStatus) -> bool {
    status
        .addresses
        .viewer
        .is_some_and(|address| address.ip().is_loopback())
}

/// The hero card: the state in plain words, the address, the ports and the uptime.
fn hero(status: &CoordinatorStatus, now: u64) -> Hero {
    let running = status.state.is_running();
    let (title, level) = match &status.state {
        CoordinatorState::Starting => ("Starting\u{2026}".to_string(), 1),
        CoordinatorState::Stopping => ("Stopping\u{2026}".to_string(), 1),
        CoordinatorState::Stopped => ("Stopped".to_string(), 3),
        CoordinatorState::Error(reason) => (format!("Problem: {reason}"), 2),
        CoordinatorState::Running if local_only(status) => (
            "Running \u{b7} only this computer can connect".to_string(),
            1,
        ),
        CoordinatorState::Running => (
            "Running \u{b7} phones on your network can connect".to_string(),
            0,
        ),
    };
    let addresses = &status.addresses;
    let ports: Vec<String> = [
        addresses.viewer,
        addresses.viewer_enroll,
        addresses.worker,
        addresses.worker_enroll,
    ]
    .into_iter()
    .flatten()
    .map(|address| address.port().to_string())
    .collect();
    let address = if !running {
        String::new()
    } else if local_only(status) {
        addresses
            .viewer
            .map(|a| a.ip().to_string())
            .unwrap_or_default()
    } else {
        addresses.lan.first().cloned().unwrap_or_else(|| {
            addresses
                .viewer
                .filter(|a| !a.ip().is_unspecified())
                .map(|a| a.ip().to_string())
                .unwrap_or_default()
        })
    };
    let uptime = if let (true, Some(started)) = (running, status.started) {
        uptime_text(now.saturating_sub(started))
    } else {
        String::new()
    };
    Hero {
        title,
        level,
        ports: if ports.is_empty() || !running {
            String::new()
        } else {
            format!("Ports {}", ports.join(" \u{b7} "))
        },
        address,
        uptime,
    }
}

/// The warning cards: the coordinator's own warnings, and the local-only notice.
fn warning_texts(status: &CoordinatorStatus) -> Vec<String> {
    let mut out = Vec::new();
    if status.state.is_running() && local_only(status) {
        out.push("Phones can't connect: this computer only accepts local connections.".to_string());
    }
    out.extend(status.warnings.iter().cloned());
    out
}

/// "Devices online": the value ("2 of 3") and the line under it.
fn devices_card(status: &CoordinatorStatus) -> (String, String) {
    let devices: Vec<_> = status
        .devices
        .iter()
        .filter(|device| device.role != PeerRole::Worker)
        .collect();
    let online: Vec<&str> = devices
        .iter()
        .filter(|device| device.online)
        .map(|device| device.name.as_str())
        .collect();
    let value = format!("{} of {}", online.len(), devices.len());
    let sub = match (devices.is_empty(), online.as_slice()) {
        (true, _) => "No devices yet".to_string(),
        (false, []) => "Nobody is connected right now".to_string(),
        (false, [one]) => (*one).to_string(),
        (false, [first, second]) => format!("{first}, {second}"),
        (false, [first, second, rest @ ..]) => {
            format!("{first}, {second} and {} more", rest.len())
        }
    };
    (value, sub)
}

/// "Render computers": the value ("1 + this computer") and the line under it.
fn computers_card(status: &CoordinatorStatus) -> (String, String) {
    let joined: Vec<_> = status
        .render_computers
        .iter()
        .filter(|computer| !computer.own_lane)
        .collect();
    let own = status.own_render.enabled;
    let value = match (own, joined.len()) {
        (true, 0) => "This computer".to_string(),
        (true, n) => format!("{n} + this computer"),
        (false, 0) => "None".to_string(),
        (false, n) => n.to_string(),
    };
    let busy: Vec<&str> = joined
        .iter()
        .filter(|computer| computer.busy > 0)
        .map(|computer| computer.name.as_str())
        .collect();
    let sub = if joined.is_empty() {
        if !own {
            "Render on this computer is off".to_string()
        } else if let Some(render) = status
            .renders
            .iter()
            .find(|render| uses_this_computer(render))
        {
            format!("Rendering for {}", render_device(render))
        } else if status.own_render.busy {
            "This computer is busy".to_string()
        } else {
            "This computer is idle".to_string()
        }
    } else {
        match (busy.as_slice(), joined.as_slice()) {
            ([], [only]) => format!("{} is idle", only.name),
            ([], _) => "All are idle".to_string(),
            ([one], _) => format!("{one} is busy"),
            (many, _) => format!("{} are busy", many.len()),
        }
    };
    (value, sub)
}

/// The word for what a render is: "Live view", "Final picture" and so on.
pub(crate) const fn kind_label(kind: RenderKind) -> &'static str {
    match kind {
        RenderKind::Live => "Live view",
        RenderKind::FinalImage => "Final picture",
        RenderKind::TiltCurves => "Tilt curves",
        RenderKind::LivePass => "Live pass",
        RenderKind::BeamTiles => "Beam tiles",
        RenderKind::Batch => "Batch preview",
    }
}

/// Who asked for `render`: the device name, or "This computer" for a viewer on this computer
/// that has no device record.
pub(crate) fn render_device(render: &ActiveRender) -> &str {
    let name: &str = &render.device;
    if name.is_empty() {
        "This computer"
    } else {
        name
    }
}

/// True when `render` is traced (also) by this computer's own render lane.
pub(crate) fn uses_this_computer(render: &ActiveRender) -> bool {
    render.route.contains("This computer")
}

/// "1,234": `value` with a comma between the thousands.
fn grouped(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// How long a render has run: "12 s", "2 min", "1 h 5 min".
pub(crate) fn elapsed_text(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} s")
    } else {
        uptime_text(seconds)
    }
}

/// The trace rate in plain words: "4.2 M samples/s", "42 M samples/s", "1.3 G samples/s"; empty
/// while it is unknown or too small to mean anything.
pub(crate) fn rate_text(rate_mps: Option<f64>) -> String {
    match rate_mps {
        Some(rate) if rate.is_finite() && rate >= 1000.0 => {
            format!("{:.1} G samples/s", rate / 1000.0)
        }
        Some(rate) if rate.is_finite() && rate >= 10.0 => format!("{rate:.0} M samples/s"),
        Some(rate) if rate.is_finite() && rate >= 0.05 => format!("{rate:.1} M samples/s"),
        _ => String::new(),
    }
}

/// The progress words of a render: "128 of 512 samples" with a target, "Live · 1,234
/// samples" without one, "Preparing graphics card… 12 of 40" while the card builds its
/// shaders.
pub(crate) fn samples_text(render: &ActiveRender) -> String {
    if let Some((done, total)) = render.preparing {
        return format!("Preparing graphics card\u{2026} {done} of {total}");
    }
    match render.samples_target {
        Some(target) if target > 0 => format!(
            "{} of {} samples",
            grouped(render.samples_done),
            grouped(target)
        ),
        _ => format!("Live \u{b7} {} samples", grouped(render.samples_done)),
    }
}

/// One row for an interactive render.
fn render_row(render: &ActiveRender, now: u64) -> OvLiveRow {
    // (fraction, determinate): the warm-up while preparing, else samples against the target.
    let (progress, determinate) = match (render.preparing, render.samples_target) {
        (Some((done, total)), _) if total > 0 => (done as f32 / total as f32, true),
        (Some(_), _) => (0.0, true),
        (None, Some(target)) if target > 0 => (render.samples_done as f32 / target as f32, true),
        (None, _) => (0.0, false),
    };
    let progress = progress.clamp(0.0, 1.0);
    let label = if render.engine.is_empty() {
        kind_label(render.kind).to_string()
    } else {
        format!("{} \u{b7} {}", kind_label(render.kind), render.engine)
    };
    OvLiveRow {
        owner: render_device(render).into(),
        label: label.into(),
        size: if render.width == 0 || render.height == 0 {
            SharedString::new()
        } else {
            format!("{} \u{d7} {}", render.width, render.height).into()
        },
        progress,
        percent: if determinate {
            format!("{} %", (progress * 100.0).round()).into()
        } else {
            SharedString::new()
        },
        route: render.route.as_str().into(),
        samples: samples_text(render).into(),
        rate: rate_text(render.rate_mps).into(),
        elapsed: elapsed_text(now.saturating_sub(render.started_unix)).into(),
        determinate,
        preparing: render.preparing.is_some(),
    }
}

/// The renders running now: the interactive renders first (newest first, as the status lists
/// them), then the running jobs.
fn live_rows(status: &CoordinatorStatus, now: u64) -> Vec<OvLiveRow> {
    let renders = status.renders.iter().map(|render| render_row(render, now));
    let jobs = status
        .jobs
        .iter()
        .filter(|job| job.state == JobState::Running)
        .map(|job| {
            let progress = job.progress.clamp(0.0, 1.0);
            OvLiveRow {
                owner: job.owner.as_str().into(),
                label: if job.label.is_empty() {
                    job.design_name.as_str().into()
                } else {
                    job.label.as_str().into()
                },
                size: format!("{} \u{d7} {}", job.width, job.height).into(),
                progress,
                percent: format!("{} %", (progress * 100.0).round()).into(),
                route: job.route.as_str().into(),
                samples: SharedString::new(),
                rate: SharedString::new(),
                elapsed: SharedString::new(),
                determinate: true,
                preparing: false,
            }
        });
    renders.chain(jobs).collect()
}

/// The newest [`FEED_SHOWN`] events of `feed` as rows.
fn activity_rows(feed: &Feed, now: u64) -> Vec<OvActivityRow> {
    feed.entries
        .iter()
        .take(FEED_SHOWN)
        .map(|(when, kind, text)| OvActivityRow {
            when: ago(now.saturating_sub(*when)).into(),
            text: text.as_str().into(),
            tone: match kind {
                EventKind::Info => 0,
                EventKind::Good => 1,
                EventKind::Notice => 2,
                EventKind::Problem => 3,
            },
        })
        .collect()
}

/// How long ago, in plain words: "just now", "5 min ago", "3 h ago", "yesterday".
pub(crate) fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        86_400..=172_799 => "yesterday".to_string(),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

/// The uptime in plain words: "under a minute", "12 min", "3 h 12 min" or "2 days".
pub(crate) fn uptime_text(seconds: u64) -> String {
    let minutes = seconds / 60;
    if minutes == 0 {
        "under a minute".to_string()
    } else if minutes < 60 {
        format!("{minutes} min")
    } else if minutes < 24 * 60 {
        let (hours, rest) = (minutes / 60, minutes % 60);
        if rest == 0 {
            format!("{hours} h")
        } else {
            format!("{hours} h {rest} min")
        }
    } else {
        let days = minutes / (24 * 60);
        format!("{days} {}", if days == 1 { "day" } else { "days" })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::admin::status::{
        DeviceStatus, JobId, RenderComputerStatus, RenderHardware, WorkerId,
    };

    fn status(state: CoordinatorState) -> CoordinatorStatus {
        CoordinatorStatus::minimal("Studio PC", "0.12.0", 45, state)
    }

    fn device(name: &str, role: PeerRole, online: bool) -> DeviceStatus {
        let mut device = DeviceStatus::new(name, "ab".repeat(32), role);
        device.online = online;
        device
    }

    fn worker(name: &str, busy: u32) -> RenderComputerStatus {
        RenderComputerStatus {
            id: WorkerId(1),
            name: name.to_string(),
            fingerprint: String::new(),
            own_lane: false,
            hardware: RenderHardware::Gpu,
            hardware_name: String::new(),
            threads: 8,
            slots: 2,
            busy,
            rate: None,
            platform: String::new(),
            connected_since: None,
            activity: String::new(),
            address: None,
        }
    }

    #[test]
    fn relative_times_are_plain_words() {
        assert_eq!(ago(10), "just now");
        assert_eq!(ago(5 * 60), "5 min ago");
        assert_eq!(ago(3 * 3600), "3 h ago");
        assert_eq!(ago(30 * 3600), "yesterday");
        assert_eq!(ago(4 * 86_400), "4 days ago");
        assert_eq!(uptime_text(20), "under a minute");
        assert_eq!(uptime_text(3 * 3600 + 12 * 60), "3 h 12 min");
        assert_eq!(uptime_text(2 * 86_400), "2 days");
    }

    #[test]
    fn hero_says_who_can_connect() {
        let mut running = status(CoordinatorState::Running);
        running.started = Some(1_000);
        running.addresses.viewer = Some("0.0.0.0:7878".parse().unwrap());
        running.addresses.viewer_enroll = Some("0.0.0.0:7879".parse().unwrap());
        running.addresses.lan = vec!["192.168.178.20".to_string()];
        let hero = hero(&running, 1_000 + 3 * 3600);
        assert_eq!(hero.level, 0);
        assert_eq!(hero.address, "192.168.178.20");
        assert_eq!(hero.ports, "Ports 7878 \u{b7} 7879");
        assert_eq!(hero.uptime, "3 h");
        assert!(warning_texts(&running).is_empty());

        running.addresses.viewer = Some("127.0.0.1:7878".parse().unwrap());
        let hero = super::hero(&running, 1_000);
        assert_eq!(hero.level, 1);
        assert_eq!(hero.address, "127.0.0.1");
        assert_eq!(warning_texts(&running).len(), 1);

        let stopped = super::hero(&status(CoordinatorState::Stopped), 5);
        assert_eq!((stopped.level, stopped.address.as_str()), (3, ""));
        assert_eq!(stopped.uptime, "");
    }

    #[test]
    fn device_card_names_who_is_online() {
        let mut status = status(CoordinatorState::Running);
        assert_eq!(devices_card(&status).1, "No devices yet");
        status.devices.push(device("S23", PeerRole::Viewer, true));
        status
            .devices
            .push(device("Laptop", PeerRole::Viewer, false));
        status.devices.push(device("rig", PeerRole::Worker, true));
        assert_eq!(devices_card(&status), ("1 of 2".into(), "S23".into()));
        status.devices[1].online = true;
        status.devices.push(device("Tab", PeerRole::Viewer, true));
        assert_eq!(devices_card(&status).1, "S23, Laptop and 1 more");
    }

    #[test]
    fn render_card_counts_joined_computers_and_the_own_lane() {
        let mut status = status(CoordinatorState::Running);
        assert_eq!(computers_card(&status).0, "None");
        status.own_render.enabled = true;
        assert_eq!(
            computers_card(&status),
            ("This computer".into(), "This computer is idle".into())
        );
        status.render_computers.push(worker("garage-rig", 1));
        assert_eq!(
            computers_card(&status),
            ("1 + this computer".into(), "garage-rig is busy".into())
        );
        status.render_computers[0].busy = 0;
        assert_eq!(computers_card(&status).1, "garage-rig is idle");
    }

    fn render(kind: RenderKind) -> ActiveRender {
        ActiveRender {
            id: 1,
            device: "Phone 1".to_string(),
            fingerprint: "cd".repeat(32),
            kind,
            engine: "Path-Traced".to_string(),
            width: 1280,
            height: 720,
            samples_done: 128,
            samples_target: Some(512),
            started_unix: 1_000,
            route: "This computer".to_string(),
            rate_mps: Some(4.2),
            preparing: None,
        }
    }

    #[test]
    fn kind_labels_are_plain_words() {
        assert_eq!(kind_label(RenderKind::Live), "Live view");
        assert_eq!(kind_label(RenderKind::FinalImage), "Final picture");
        assert_eq!(kind_label(RenderKind::TiltCurves), "Tilt curves");
        assert_eq!(kind_label(RenderKind::LivePass), "Live pass");
        assert_eq!(kind_label(RenderKind::BeamTiles), "Beam tiles");
        assert_eq!(kind_label(RenderKind::Batch), "Batch preview");
    }

    #[test]
    fn samples_text_with_and_without_a_target() {
        let mut live = render(RenderKind::Live);
        assert_eq!(samples_text(&live), "128 of 512 samples");
        live.samples_done = 1_234;
        live.samples_target = Some(20_000);
        assert_eq!(samples_text(&live), "1,234 of 20,000 samples");
        live.samples_target = None;
        assert_eq!(samples_text(&live), "Live \u{b7} 1,234 samples");
        live.samples_target = Some(0);
        assert_eq!(samples_text(&live), "Live \u{b7} 1,234 samples");
        live.samples_done = 0;
        assert_eq!(samples_text(&live), "Live \u{b7} 0 samples");
        live.samples_done = 1_234_567;
        assert_eq!(samples_text(&live), "Live \u{b7} 1,234,567 samples");
    }

    #[test]
    fn preparing_says_so() {
        let mut live = render(RenderKind::Live);
        live.samples_done = 0;
        live.preparing = Some((12, 40));
        assert_eq!(
            samples_text(&live),
            "Preparing graphics card\u{2026} 12 of 40"
        );
        let row = render_row(&live, 1_060);
        assert!(row.preparing);
        assert!(row.determinate);
        assert!((row.progress - 0.3).abs() < 1e-6);
        assert_eq!(row.percent.as_str(), "30 %");
    }

    #[test]
    fn rate_and_elapsed_are_formatted_for_people() {
        assert_eq!(rate_text(Some(4.2)), "4.2 M samples/s");
        assert_eq!(rate_text(Some(42.4)), "42 M samples/s");
        assert_eq!(rate_text(Some(1_300.0)), "1.3 G samples/s");
        assert_eq!(rate_text(Some(0.0)), "");
        assert_eq!(rate_text(Some(f64::NAN)), "");
        assert_eq!(rate_text(None), "");
        assert_eq!(elapsed_text(12), "12 s");
        assert_eq!(elapsed_text(2 * 60 + 5), "2 min");
        assert_eq!(elapsed_text(3600 + 5 * 60), "1 h 5 min");
    }

    #[test]
    fn a_final_picture_row_has_a_bar_and_the_texts() {
        let mut final_picture = render(RenderKind::FinalImage);
        final_picture.samples_done = 205;
        final_picture.samples_target = Some(512);
        let row = render_row(&final_picture, 1_000 + 2 * 60);
        assert_eq!(row.owner.as_str(), "Phone 1");
        assert_eq!(row.label.as_str(), "Final picture \u{b7} Path-Traced");
        assert_eq!(row.size.as_str(), "1280 \u{d7} 720");
        assert!(row.determinate && !row.preparing);
        assert_eq!(row.percent.as_str(), "40 %");
        assert_eq!(row.samples.as_str(), "205 of 512 samples");
        assert_eq!(row.route.as_str(), "This computer");
        assert_eq!(row.rate.as_str(), "4.2 M samples/s");
        assert_eq!(row.elapsed.as_str(), "2 min");
    }

    #[test]
    fn a_live_view_row_pulses_and_a_loopback_viewer_is_this_computer() {
        let mut live = render(RenderKind::Live);
        live.samples_target = None;
        live.engine = String::new();
        live.device = String::new();
        live.rate_mps = None;
        let row = render_row(&live, 1_005);
        assert!(!row.determinate);
        assert_eq!(row.percent.as_str(), "");
        assert_eq!(row.owner.as_str(), "This computer");
        assert_eq!(row.label.as_str(), "Live view");
        assert_eq!(row.rate.as_str(), "");
        assert_eq!(row.elapsed.as_str(), "5 s");
    }

    #[test]
    fn renders_come_before_jobs() {
        use pleochroma_net::admin::status::JobStatus;
        let mut status = status(CoordinatorState::Running);
        assert!(live_rows(&status, 1_100).is_empty());
        status.jobs.push(JobStatus {
            id: JobId([1; 16]),
            owner: "Laptop".to_string(),
            owner_fingerprint: String::new(),
            state: JobState::Running,
            progress: 0.5,
            samples_done: 100,
            samples_total: 200,
            width: 640,
            height: 480,
            label: "Final render".to_string(),
            design_name: String::new(),
            route: String::new(),
            created: 900,
            finished: None,
            result_len: None,
            note: String::new(),
        });
        status.renders.push(render(RenderKind::Live));
        let rows = live_rows(&status, 1_100);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].owner.as_str(), "Phone 1");
        assert_eq!(rows[1].owner.as_str(), "Laptop");
        assert_eq!(rows[1].samples.as_str(), "");
        assert_eq!(rows[1].percent.as_str(), "50 %");
    }

    #[test]
    fn the_render_card_names_who_uses_this_computer() {
        let mut status = status(CoordinatorState::Running);
        status.own_render.enabled = true;
        status.renders.push(render(RenderKind::Live));
        assert_eq!(computers_card(&status).1, "Rendering for Phone 1");
        status.renders[0].route = "2 render computers".to_string();
        assert_eq!(computers_card(&status).1, "This computer is idle");
        assert!(!uses_this_computer(&status.renders[0]));
    }
}
