//! The renders viewers asked for that this coordinator serves now.
//!
//! The durable job queue ([`super::jobs`]) knows stored jobs only. A viewer's live stream, a
//! final picture, the tilt curves, a live pass, beam tiles and a batch of previews are answered
//! on the viewer's connection and leave no record there, so the Overview could not show them.
//! This module is that record: a process-wide registry of [`ActiveRender`]s.
//!
//! - **[`begin`]** adds an entry and returns a [`RenderGuard`]; dropping the guard removes it,
//!   whatever ended the request (done, cancelled, the viewer left, an error, a panic).
//! - **The emitter** reports progress through [`note_progress`], on the connection thread that
//!   owns the guard, so no guard has to be threaded through the render code. [`note_route`] does
//!   the same for the route once the coordinator has planned the request.
//! - **[`snapshot`]** copies the entries for the status. The registry is one short [`Mutex`]
//!   that no one holds across work, so the status never waits for a render or for a GPU
//!   renderer.

use super::status::unix_now;
use pleochroma_net::admin::status::{ActiveRender, DeviceStatus, RenderKind, fingerprint_short};
use std::{
    cell::Cell,
    collections::HashMap,
    hash::BuildHasher,
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// The route text of a render this computer's own lane traces.
pub const THIS_COMPUTER: &str = "This computer";

/// The activity a device shows while one of its renders runs.
pub const RENDERING: &str = "Rendering";

/// The engine text of the path tracer.
pub const PATH_TRACED: &str = "Path-Traced";

/// The engine text of the ray tracer.
pub const RAY_TRACED: &str = "Ray-Traced";

/// A request for this many samples per pixel or more is an open-ended live view (it has no
/// target to show a bar against).
pub const OPEN_ENDED_SAMPLES: u32 = 100_000;

/// How long the rate is measured over before it is refreshed.
const RATE_WINDOW: Duration = Duration::from_secs(2);

/// Everything static about a render when it begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRenderStart {
    /// The asking device's name; empty to show its short fingerprint (the status replaces it by
    /// the name on the Devices page).
    pub device: String,
    /// The asking device's fingerprint (hex); empty for a viewer with no certificate.
    pub fingerprint: String,
    /// What kind of request it is.
    pub kind: RenderKind,
    /// [`PATH_TRACED`] or [`RAY_TRACED`]; empty when unknown.
    pub engine: String,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// The target samples per pixel; `None` for an open-ended live view.
    pub target: Option<u32>,
    /// Who traces it (see [`route_text`]).
    pub route: String,
}

/// One registered render and the window its rate is measured over.
struct Entry {
    render: ActiveRender,
    /// When and at how many samples the rate was last refreshed.
    anchor: (Instant, u32),
}

impl Entry {
    fn new(id: u64, info: ActiveRenderStart, now: Instant) -> Self {
        let device = if info.device.is_empty() {
            fingerprint_short(&info.fingerprint)
        } else {
            info.device
        };
        Self {
            render: ActiveRender {
                id,
                device,
                fingerprint: info.fingerprint,
                kind: info.kind,
                engine: info.engine,
                width: info.width,
                height: info.height,
                samples_done: 0,
                samples_target: info.target,
                started_unix: unix_now(),
                route: info.route,
                rate_mps: None,
                preparing: None,
            },
            anchor: (now, 0),
        }
    }

    /// Records the samples per pixel traced so far and refreshes the rate once a window has
    /// passed.
    fn record(&mut self, samples: u32, now: Instant) {
        self.render.samples_done = samples;
        let (since, base) = self.anchor;
        if samples < base {
            self.anchor = (now, samples);
            return;
        }
        let elapsed = now.saturating_duration_since(since);
        if elapsed >= RATE_WINDOW {
            let pixels = f64::from(self.render.width) * f64::from(self.render.height);
            let work = f64::from(samples - base) * pixels;
            self.render.rate_mps = Some(work / elapsed.as_secs_f64() / 1.0e6);
            self.anchor = (now, samples);
        }
    }
}

/// The registered renders, oldest first.
static REGISTRY: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// The next render id (0 is never used: it means "none").
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// The render the current thread serves (the one it began last), 0 for none.
    static CURRENT: Cell<u64> = const { Cell::new(0) };
}

/// The registry, whoever panicked while holding it.
fn entries() -> MutexGuard<'static, Vec<Entry>> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `apply` on the entry `id`, if it is still registered.
fn update(id: u64, apply: impl FnOnce(&mut Entry)) {
    let mut list = entries();
    if let Some(entry) = list.iter_mut().find(|entry| entry.render.id == id) {
        apply(entry);
    }
}

/// A registered render. Dropping it removes the render from the registry.
///
/// It is `Send` and `Sync` (it holds only the id), so it may be dropped on any thread.
#[derive(Debug)]
pub struct RenderGuard {
    id: u64,
}

/// Registers a render and makes it the current one of this thread (see [`note_progress`]).
#[must_use = "the render is listed only while the guard lives"]
pub fn begin(info: ActiveRenderStart) -> RenderGuard {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let entry = Entry::new(id, info, Instant::now());
    entries().push(entry);
    let _ = CURRENT.try_with(|current| current.set(id));
    RenderGuard { id }
}

impl RenderGuard {
    /// The render's id, as [`ActiveRender::id`] shows it.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Records the samples per pixel traced so far. Cheap: one short lock.
    pub fn progress(&self, samples_done: u32) {
        update(self.id, |entry| entry.record(samples_done, Instant::now()));
    }

    /// Replaces the route text once it is known.
    pub fn route(&self, route: &str) {
        update(self.id, |entry| route.clone_into(&mut entry.render.route));
    }

    /// Sets the measured rate, in millions of samples times pixels per second.
    pub fn rate(&self, rate_mps: f64) {
        update(self.id, |entry| entry.render.rate_mps = Some(rate_mps));
    }
}

impl Drop for RenderGuard {
    fn drop(&mut self) {
        entries().retain(|entry| entry.render.id != self.id);
        let _ = CURRENT.try_with(|current| {
            if current.get() == self.id {
                current.set(0);
            }
        });
    }
}

/// Records progress for the render this thread serves, as [`RenderGuard::progress`] does.
///
/// The stream emitter calls it with the value it sends to the viewer; it does nothing on a thread
/// that serves no registered render (a joined worker's connection).
pub fn note_progress(samples_done: u32) {
    let id = CURRENT.try_with(Cell::get).unwrap_or(0);
    if id != 0 {
        update(id, |entry| entry.record(samples_done, Instant::now()));
    }
}

/// Sets the route of the render this thread serves, as [`RenderGuard::route`] does.
pub fn note_route(route: &str) {
    let id = CURRENT.try_with(Cell::get).unwrap_or(0);
    if id != 0 {
        update(id, |entry| route.clone_into(&mut entry.render.route));
    }
}

/// The registered renders, newest first. `preparing` is filled from the shader warm-up for a
/// render that has traced nothing yet. Never waits for a render.
#[must_use]
pub fn snapshot() -> Vec<ActiveRender> {
    snapshot_with(crate::serve::warmup::preparing())
}

/// [`snapshot`] with the warm-up counts given.
fn snapshot_with(preparing: Option<(u32, u32)>) -> Vec<ActiveRender> {
    let mut list: Vec<ActiveRender> = entries().iter().map(|entry| entry.render.clone()).collect();
    list.reverse();
    for render in &mut list {
        render.preparing = if render.samples_done == 0 {
            preparing
        } else {
            None
        };
    }
    list
}

/// The device and fingerprint a viewer connection's identity key stands for.
///
/// `certificate:<hex>` gives the fingerprint (the device is its short form, until [`apply_device_names`] replaces
/// it), `address:<ip>` the address, and anything else a plain "Viewer".
#[must_use]
pub fn viewer_identity(key: &str) -> (String, String) {
    if let Some(hex) = key.strip_prefix("certificate:") {
        return (fingerprint_short(hex), hex.to_owned());
    }
    if let Some(address) = key.strip_prefix("address:") {
        return (address.to_owned(), String::new());
    }
    ("Viewer".to_owned(), String::new())
}

/// "This computer", "2 render computers", "This computer + 1 render computer": who traces a
/// render. A render nothing can be said for falls back to this computer.
#[must_use]
pub fn route_text(own: bool, workers: usize) -> String {
    let computers = match workers {
        0 => String::new(),
        1 => "1 render computer".to_owned(),
        n => format!("{n} render computers"),
    };
    match (own, computers.is_empty()) {
        (_, true) => THIS_COMPUTER.to_owned(),
        (true, false) => format!("{THIS_COMPUTER} + {computers}"),
        (false, false) => computers,
    }
}

/// The engine text for a scene.
#[must_use]
pub const fn engine_text(is_ray: bool) -> &'static str {
    if is_ray { RAY_TRACED } else { PATH_TRACED }
}

/// Whether the render's route includes this computer's own lane.
#[must_use]
pub fn uses_this_computer(render: &ActiveRender) -> bool {
    render.route.starts_with(THIS_COMPUTER)
}

/// Whether the own render lane is busy: a job runs, or a render's route includes this computer.
#[must_use]
pub fn own_lane_busy(job_running: bool, renders: &[ActiveRender]) -> bool {
    job_running || renders.iter().any(uses_this_computer)
}

/// Shows each render under the name its device has on the Devices page. `names` maps a
/// fingerprint (hex) to a name; a render with no entry keeps what it has.
pub fn apply_device_names<S: BuildHasher>(
    renders: &mut [ActiveRender],
    names: &HashMap<String, String, S>,
) {
    for render in renders {
        if let Some(name) = names.get(&render.fingerprint)
            && !name.is_empty()
        {
            name.clone_into(&mut render.device);
        }
    }
}

/// Says "Rendering" on every device that has a render running, unless the job rule already
/// wrote something there.
pub fn mark_device_activity(devices: &mut [DeviceStatus], renders: &[ActiveRender]) {
    for device in devices {
        if device.activity.is_empty()
            && renders.iter().any(|render| {
                !render.fingerprint.is_empty() && render.fingerprint == device.fingerprint
            })
        {
            RENDERING.clone_into(&mut device.activity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::messages::PeerRole;
    use std::sync::{Arc, Barrier};

    fn start(device: &str) -> ActiveRenderStart {
        ActiveRenderStart {
            device: device.to_owned(),
            fingerprint: "ab".repeat(32),
            kind: RenderKind::Live,
            engine: PATH_TRACED.to_owned(),
            width: 640,
            height: 360,
            target: Some(512),
            route: THIS_COMPUTER.to_owned(),
        }
    }

    /// The registered renders of the devices whose name starts with `prefix`, newest first. The
    /// registry is process-wide and the tests run in parallel, so each test owns a prefix.
    fn mine(prefix: &str) -> Vec<ActiveRender> {
        snapshot_with(None)
            .into_iter()
            .filter(|render| render.device.starts_with(prefix))
            .collect()
    }

    #[test]
    fn begin_progress_and_drop_update_the_snapshot() {
        let guard = begin(start("t-life"));
        let listed = mine("t-life");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, guard.id());
        assert_eq!(listed[0].samples_done, 0);
        assert_eq!(listed[0].samples_target, Some(512));
        assert_eq!(listed[0].kind, RenderKind::Live);
        assert!(listed[0].started_unix > 1_600_000_000);

        guard.progress(128);
        guard.route("2 render computers");
        guard.rate(4.5);
        let listed = mine("t-life");
        assert_eq!(listed[0].samples_done, 128);
        assert_eq!(listed[0].route, "2 render computers");
        assert_eq!(listed[0].rate_mps, Some(4.5));

        drop(guard);
        assert!(mine("t-life").is_empty());
    }

    #[test]
    fn the_thread_helpers_follow_the_current_render() {
        note_progress(9);
        note_route("nowhere");
        assert!(mine("t-thread").is_empty(), "no render, nothing recorded");

        let guard = begin(start("t-thread"));
        note_progress(33);
        note_route("This computer + 1 render computer");
        let listed = mine("t-thread");
        assert_eq!(listed[0].samples_done, 33);
        assert_eq!(listed[0].route, "This computer + 1 render computer");

        drop(guard);
        note_progress(34);
        assert!(mine("t-thread").is_empty(), "a finished render stays gone");
    }

    #[test]
    fn the_newest_render_comes_first() {
        let first = begin(start("t-order-a"));
        let second = begin(start("t-order-b"));
        let third = begin(start("t-order-c"));
        let listed = mine("t-order");
        let names: Vec<&str> = listed.iter().map(|r| r.device.as_str()).collect();
        assert_eq!(names, ["t-order-c", "t-order-b", "t-order-a"]);
        assert!(listed[0].id > listed[1].id && listed[1].id > listed[2].id);
        drop(second);
        let names: Vec<String> = mine("t-order").into_iter().map(|r| r.device).collect();
        assert_eq!(names, ["t-order-c", "t-order-a"]);
        drop((first, third));
        assert!(mine("t-order").is_empty());
    }

    #[test]
    fn guards_on_several_threads_are_listed_together() {
        const THREADS: usize = 6;
        let ready = Arc::new(Barrier::new(THREADS + 1));
        let leave = Arc::new(Barrier::new(THREADS + 1));
        let workers: Vec<_> = (0..THREADS)
            .map(|n| {
                let (ready, leave) = (Arc::clone(&ready), Arc::clone(&leave));
                std::thread::spawn(move || {
                    let guard = begin(start(&format!("t-conc-{n}")));
                    guard.progress(u32::try_from(n).unwrap() + 1);
                    ready.wait();
                    leave.wait();
                })
            })
            .collect();
        ready.wait();
        let listed = mine("t-conc");
        assert_eq!(listed.len(), THREADS);
        for n in 0..THREADS {
            let render = listed
                .iter()
                .find(|r| r.device == format!("t-conc-{n}"))
                .expect("every thread's render is listed");
            assert_eq!(render.samples_done, u32::try_from(n).unwrap() + 1);
        }
        let mut ids: Vec<u64> = listed.iter().map(|r| r.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), THREADS, "ids are unique");
        leave.wait();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(mine("t-conc").is_empty());
    }

    #[test]
    fn a_panic_drops_the_entry() {
        let result = std::panic::catch_unwind(|| {
            let _guard = begin(start("t-panic"));
            assert_eq!(mine("t-panic").len(), 1);
            panic!("the request failed");
        });
        assert!(result.is_err());
        assert!(mine("t-panic").is_empty());
        // The registry still works after the unwind.
        let again = begin(start("t-panic"));
        assert_eq!(mine("t-panic").len(), 1);
        drop(again);
    }

    #[test]
    fn a_guard_may_be_dropped_on_another_thread() {
        let guard = begin(start("t-move"));
        std::thread::spawn(move || drop(guard)).join().unwrap();
        assert!(mine("t-move").is_empty());
    }

    #[test]
    fn preparing_shows_only_until_the_first_sample() {
        let guard = begin(start("t-prep"));
        let waiting = snapshot_with(Some((12, 40)))
            .into_iter()
            .find(|r| r.id == guard.id())
            .unwrap();
        assert_eq!(waiting.preparing, Some((12, 40)));
        guard.progress(1);
        let tracing = snapshot_with(Some((12, 40)))
            .into_iter()
            .find(|r| r.id == guard.id())
            .unwrap();
        assert_eq!(tracing.preparing, None);
    }

    #[test]
    fn the_rate_is_measured_over_a_window() {
        let now = Instant::now();
        let mut entry = Entry::new(1, start("t-rate"), now);
        entry.record(10, now + Duration::from_millis(500));
        assert_eq!(entry.render.rate_mps, None, "too early for a rate");
        entry.record(40, now + Duration::from_secs(2));
        // 40 samples of 640 x 360 pixels in 2 s.
        let rate = entry.render.rate_mps.expect("a window passed");
        assert!(
            (rate - 40.0 * 640.0 * 360.0 / 2.0 / 1.0e6).abs() < 1e-9,
            "{rate}"
        );
        // A restart (fewer samples than before) begins a new window.
        entry.record(5, now + Duration::from_secs(3));
        assert_eq!(entry.render.samples_done, 5);
        assert_eq!(entry.anchor.1, 5);
    }

    #[test]
    fn identities_routes_and_engines_read_as_plain_words() {
        let hex = "cd".repeat(32);
        assert_eq!(
            viewer_identity(&format!("certificate:{hex}")),
            ("cdcdcdcd".to_owned(), hex)
        );
        assert_eq!(
            viewer_identity("address:10.0.0.8"),
            ("10.0.0.8".to_owned(), String::new())
        );
        assert_eq!(viewer_identity(""), ("Viewer".to_owned(), String::new()));

        assert_eq!(route_text(true, 0), "This computer");
        assert_eq!(route_text(false, 0), "This computer");
        assert_eq!(route_text(false, 1), "1 render computer");
        assert_eq!(route_text(false, 2), "2 render computers");
        assert_eq!(route_text(true, 1), "This computer + 1 render computer");
        assert_eq!(route_text(true, 3), "This computer + 3 render computers");
        assert_eq!(engine_text(true), "Ray-Traced");
        assert_eq!(engine_text(false), "Path-Traced");
    }

    #[test]
    fn a_device_without_a_name_shows_its_short_fingerprint() {
        let guard = begin(ActiveRenderStart {
            device: String::new(),
            fingerprint: "12345678".repeat(8),
            ..start("")
        });
        let listed: Vec<ActiveRender> = snapshot_with(None)
            .into_iter()
            .filter(|r| r.id == guard.id())
            .collect();
        assert_eq!(listed[0].device, "12345678");
    }

    fn render_of(fingerprint: &str, route: &str) -> ActiveRender {
        let mut entry = Entry::new(
            1,
            ActiveRenderStart {
                fingerprint: fingerprint.to_owned(),
                route: route.to_owned(),
                ..start("short")
            },
            Instant::now(),
        );
        entry.record(1, Instant::now());
        entry.render
    }

    #[test]
    fn the_status_fill_names_devices_and_marks_their_activity() {
        let phone = "aa".repeat(32);
        let tablet = "bb".repeat(32);
        let mut renders = vec![
            render_of(&phone, THIS_COMPUTER),
            render_of("", THIS_COMPUTER),
        ];
        let names = HashMap::from([(phone.clone(), "Phone 1".to_owned())]);
        apply_device_names(&mut renders, &names);
        assert_eq!(renders[0].device, "Phone 1");
        assert_eq!(
            renders[1].device, "short",
            "no fingerprint, no name to look up"
        );

        let mut devices = vec![
            DeviceStatus::new("Phone 1", phone.clone(), PeerRole::Viewer),
            DeviceStatus::new("Tablet", tablet, PeerRole::Viewer),
        ];
        mark_device_activity(&mut devices, &renders);
        assert_eq!(devices[0].activity, "Rendering");
        assert!(devices[1].activity.is_empty());

        // A job's own activity text is kept.
        let mut busy = vec![DeviceStatus::new("Phone 1", phone, PeerRole::Viewer)];
        "Rendering Final render".clone_into(&mut busy[0].activity);
        mark_device_activity(&mut busy, &renders);
        assert_eq!(busy[0].activity, "Rendering Final render");
    }

    #[test]
    fn the_own_lane_is_busy_for_a_job_or_a_render_that_uses_it() {
        let own = render_of("", "This computer + 1 render computer");
        let remote = render_of("", "2 render computers");
        assert!(uses_this_computer(&own));
        assert!(!uses_this_computer(&remote));
        assert!(!own_lane_busy(false, &[]));
        assert!(own_lane_busy(true, &[]));
        assert!(own_lane_busy(false, std::slice::from_ref(&own)));
        assert!(!own_lane_busy(false, std::slice::from_ref(&remote)));
        assert!(own_lane_busy(false, &[remote, own]));
    }
}
