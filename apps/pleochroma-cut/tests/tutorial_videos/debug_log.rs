//! Debug mode (`PLEOCHROMA_VIDEOS_DEBUG`): instead of a video, a run leaves a step log and
//! before/after screenshots of every step in `<out>/debug/<video-id>/`, for an agent to read.
//!
//! The log lives in a thread-local sink (the UI thread is the only one that films). The sink is a
//! no-op while debug mode is off: every entry point returns at once when no sink is installed, so
//! a normal run behaves exactly as before. Pointer, key and text input are recorded where the
//! director calls the recorder or the plain driver (`debug_rec.rs`), lookups where the target tables are queried
//! (`targets.rs`), readbacks by [`debug_check`] at the places that compare what the app shows.
//! Every text goes through the privacy mask (`Config::private_hit`) before it is stored.

use crate::{
    config::{Config, LOGICAL_SIZE, SCALE, first_private},
    debug_images::{self, Mark, Shape},
    debug_model::{self, Event, GuideState, Header, Record},
    debug_overlays::{self, Pending},
    director::snap,
    failure_capture,
    support::WindowView,
    targets::Spot,
};
use pleochroma_cut::{GuideModel, MainWindow, harness_support};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    fmt::Debug,
    fs,
    path::PathBuf,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uiprobe::screenshot::Shot;

mod extra_shot;
pub use extra_shot::page_shot;

/// What replaces a text that shows private fragments.
const MASKED: &str = "[private text masked]";
/// The longest the reading hold of one caption cue lasts in debug mode.
const DEBUG_READ_HOLD: Duration = Duration::from_millis(150);
/// The shortest hold after which a dialog that is up is photographed again.
const WATCH_HOLD: Duration = Duration::from_millis(100);
/// How many "during" pictures sets one step keeps (the rest are counted in a note).
const MAX_DURING: u32 = 4;
/// How many lookups one record keeps.
const LOOKUP_CAP: usize = 300;
/// The recorder line of the log header.
const RECORDER_NOTE: &str = "no video is recorded: a plain input driver gives the app the same events and mock time as \
     the recorder (no frame capture, no ffmpeg); reading holds are shortened to 150 ms, every \
     other hold and every wait is as in a video run";

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
}

struct Sink {
    dir: PathBuf,
    words: Vec<String>,
    ui: slint::Weak<MainWindow>,
    started: Instant,
    header: Header,
    done: Vec<Record>,
    cur: Record,
    /// Click, move and wheel numbers of the current record.
    counters: [u32; 3],
    /// The control the route says its next pointer action is for.
    hint: Option<String>,
    /// The latest screenshots of a dialog or window that is up, kept until it goes away.
    pending: Option<Pending>,
    /// How many "during" picture sets the current record has had.
    during: u32,
}

/// Whether debug mode is recording a run on this thread.
#[must_use]
pub fn is_on() -> bool {
    ACTIVE.with(Cell::get)
}

fn with_sink<R>(f: impl FnOnce(&mut Sink) -> R) -> Option<R> {
    if !is_on() {
        return None;
    }
    SINK.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        guard.as_mut().map(f)
    })
}

/// Installs the sink for video `id` and clears its folder. Does nothing unless debug mode is on.
///
/// # Errors
///
/// The folder could not be cleared or created, or `id` is not a plain identifier.
pub fn start(cfg: &Config, id: &str, ui: Option<&MainWindow>) -> Result<(), String> {
    if !cfg.debug {
        return Ok(());
    }
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("debug folder id must contain only ASCII letters, digits, and hyphens".into());
    }
    let dir = cfg.debug_dir(id);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|_| "could not clear the debug folder".to_owned())?;
    }
    fs::create_dir_all(&dir).map_err(|_| "could not create the debug folder".to_owned())?;
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let header = Header {
        video: id.to_owned(),
        window: LOGICAL_SIZE,
        scale: SCALE,
        library: cfg.library_kind().to_owned(),
        recorder: RECORDER_NOTE.to_owned(),
        started: debug_model::utc_text(seconds),
        ..Header::default()
    };
    let sink = Sink {
        dir,
        words: cfg.private_words.clone(),
        ui: ui.map(ComponentHandle::as_weak).unwrap_or_default(),
        started: Instant::now(),
        header,
        done: Vec::new(),
        cur: Record::new(None, "setup", "Setup before the first step".to_owned(), 0),
        counters: [0; 3],
        hint: None,
        pending: None,
        during: 0,
    };
    SINK.with(|cell| *cell.borrow_mut() = Some(sink));
    ACTIVE.with(|active| active.set(true));
    Ok(())
}

/// The reading hold of one caption cue: shortened in debug mode, where nobody reads.
#[must_use]
pub fn reading_hold(cfg: &Config, hold: Duration) -> Duration {
    if cfg.debug {
        hold.min(DEBUG_READ_HOLD)
    } else {
        hold
    }
}

impl Sink {
    fn mask(&self, text: &str) -> String {
        if first_private(&self.words, text).is_some() {
            MASKED.to_owned()
        } else {
            text.to_owned()
        }
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn state(&self, ui: &MainWindow) -> GuideState {
        let shown = snap(ui);
        let waiting = ui.global::<GuideModel>().get_current().waiting.to_string();
        GuideState {
            open: shown.open,
            step: if shown.open { shown.index + 1 } else { 0 },
            total: shown.total,
            done: shown.done,
            title: self.mask(&shown.title),
            waiting: self.mask(&waiting),
            target: self.mask(&shown.target),
        }
    }

    /// Files the setup record away when the first step begins.
    fn close_setup(&mut self) {
        if self.cur.index.is_some() {
            return;
        }
        let now = self.elapsed_ms();
        let tail = Record::new(None, "tail", "After the last step".to_owned(), now);
        let mut setup = std::mem::replace(&mut self.cur, tail);
        if !setup.is_empty() {
            setup.end_ms = now;
            setup.finished = true;
            self.done.push(setup);
        }
    }

    /// Captures every open window and writes `NN-<phase>-<window>.png`, or notes why not.
    fn shoot(&mut self, ui: &MainWindow, cfg: &Config, phase: &str, marks: &[Mark]) {
        match failure_capture::capture_windows(ui, cfg) {
            Ok(shots) => self.write_shots(phase, shots, marks),
            Err(reason) => self.cur.notes.push(skip_note(phase, &reason)),
        }
    }

    /// Writes `shots` as `NN-<phase>-<window>.png` into the folder and lists them in the record.
    fn write_shots(&mut self, phase: &str, shots: Vec<(String, Shot)>, marks: &[Mark]) {
        let number = self.cur.index.unwrap_or(0);
        for (window, shot) in shots {
            let name = format!("{number:02}-{phase}-{window}.png");
            let own: Vec<Mark> = marks
                .iter()
                .filter(|mark| mark.window == window)
                .cloned()
                .collect();
            match debug_images::write(&shot, &own, &self.dir.join(&name)) {
                Ok(()) => self.cur.images.push(name),
                Err(reason) => self
                    .cur
                    .notes
                    .push(format!("screenshot {name} not written: {reason}")),
            }
        }
    }

    /// Looks at what is up (dialogs, overlays, secondary windows). A set of pictures kept for
    /// something that is no longer up in the same way is written out as a "during" set. With
    /// `capture`, whatever is up now is photographed and kept in place of the older pictures, so
    /// the one written later is the last look before it went away.
    ///
    /// Only the setup and the steps take part: the stretch between two steps has no number to
    /// name its pictures with.
    fn watch(&mut self, capture: bool) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let up = debug_overlays::up_now(&ui);
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.up != up)
        {
            self.flush_during();
        }
        let numbered = self.cur.index.is_some() || self.done.is_empty();
        if !capture || up.is_empty() || !numbered {
            return;
        }
        match failure_capture::capture_windows_for(&ui, &self.words) {
            Ok(shots) => self.pending = Some(Pending { up, shots }),
            Err(reason) => {
                let note = skip_note("during", &reason);
                if self.cur.notes.last() != Some(&note) {
                    self.cur.notes.push(note);
                }
            }
        }
    }

    /// Writes the kept pictures as `NN-during-<window>.png` (`during2`, ... for a later set of
    /// the same step), with the pointer marks the step has made so far.
    fn flush_during(&mut self) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        self.during += 1;
        if self.during > MAX_DURING {
            if self.during == MAX_DURING + 1 {
                self.cur.notes.push(format!(
                    "during screenshots: only the first {MAX_DURING} sets of this step are kept"
                ));
            }
            return;
        }
        let phase = if self.during == 1 {
            "during".to_owned()
        } else {
            format!("during{}", self.during)
        };
        let marks = marks_of(&self.cur);
        self.write_shots(&phase, pending.shots, &marks);
        self.cur.notes.push(format!(
            "{phase} screenshots: taken while up: {}",
            pending.up.join(", ")
        ));
    }

    fn write_logs(&mut self) {
        self.header.total_ms = self.elapsed_ms();
        let mut records = self.done.clone();
        if !self.cur.is_empty() {
            records.push(self.cur.clone());
        }
        let markdown = debug_model::markdown(&self.header, &records);
        let json = debug_model::json_text(&self.header, &records);
        for (name, text) in [("steps.md", markdown), ("steps.json", json)] {
            if fs::write(self.dir.join(name), text).is_err() {
                eprintln!("tutorial-videos: could not write the debug log {name}");
            }
        }
    }

    /// The window a pointer action went to: `main`, `help`, `rough-planner`, `rough-color`,
    /// `compare`, `locate`, `rig`, `color-pattern`.
    fn window_name(&self, window: &slint::Window, origin: (f32, f32)) -> &'static str {
        if self
            .ui
            .upgrade()
            .is_some_and(|ui| std::ptr::eq(window, ui.window()))
        {
            return "main";
        }
        let help = harness_support::help_window();
        let rough = harness_support::rough_planner_window();
        let wizard = harness_support::rough_color_window();
        let compare = harness_support::compare_window();
        let locate = harness_support::locate_window();
        let rig = harness_support::rig_window();
        let pattern = harness_support::color_pattern_window();
        let is = |other: Option<&slint::Window>| other.is_some_and(|w| std::ptr::eq(window, w));
        for (name, other) in [
            ("help", help.as_ref().map(ComponentHandle::window)),
            ("rough-planner", rough.as_ref().map(ComponentHandle::window)),
            ("rough-color", wizard.as_ref().map(ComponentHandle::window)),
            ("compare", compare.as_ref().map(ComponentHandle::window)),
            ("locate", locate.as_ref().map(ComponentHandle::window)),
            ("rig", rig.as_ref().map(ComponentHandle::window)),
            (
                "color-pattern",
                pattern.as_ref().map(ComponentHandle::window),
            ),
        ] {
            if is(other) {
                return name;
            }
        }
        if origin == (0.0, 0.0) {
            return "main";
        }
        // The window is not one of those by identity (an attached window's own handle): the only
        // secondary window that is open is the one meant.
        let open = [
            ("help", help.is_some()),
            ("rough-planner", rough.is_some()),
            ("rough-color", wizard.is_some()),
            ("compare", compare.is_some()),
            ("locate", locate.is_some()),
            ("rig", rig.is_some()),
            ("color-pattern", pattern.is_some()),
        ];
        let mut shown = open.iter().filter(|(_, is_open)| *is_open);
        match (shown.next(), shown.next()) {
            (Some((name, _)), None) => name,
            _ => "secondary",
        }
    }

    /// The smallest elements that hold `at` in `window`, with their bounds.
    fn describe(&self, window: &str, at: (f32, f32)) -> String {
        let view = match window {
            "main" => self
                .ui
                .upgrade()
                .map(|ui| WindowView::of("MainWindow", &ui)),
            "help" => harness_support::help_window().map(|w| WindowView::of("HelpWindow", &w)),
            "rough-planner" => harness_support::rough_planner_window()
                .map(|w| WindowView::of("RoughPlannerWindow", &w)),
            "rough-color" => harness_support::rough_color_window()
                .map(|w| WindowView::of("RoughColourWindow", &w)),
            "compare" => {
                harness_support::compare_window().map(|w| WindowView::of("CompareWindow", &w))
            }
            "locate" => {
                harness_support::locate_window().map(|w| WindowView::of("LocateWindow", &w))
            }
            "rig" => harness_support::rig_window().map(|w| WindowView::of("RigWindow", &w)),
            "color-pattern" => harness_support::color_pattern_window()
                .map(|w| WindowView::of("ColorPatternWindow", &w)),
            _ => None,
        };
        let Some(view) = view else {
            return String::new();
        };
        let mut hits: Vec<(f32, String)> = view
            .elements()
            .into_iter()
            .filter_map(|element| {
                let spot = Spot::of(&element);
                (spot.w > 0.0
                    && spot.h > 0.0
                    && at.0 >= spot.x
                    && at.0 <= spot.x + spot.w
                    && at.1 >= spot.y
                    && at.1 <= spot.y + spot.h)
                    .then(|| {
                        (
                            spot.w * spot.h,
                            format!(
                                "{:?}/{:?}/{:?} [{:.0}, {:.0}, {:.0} x {:.0}]",
                                element.type_name(),
                                element.accessible_role(),
                                element.accessible_label(),
                                spot.x,
                                spot.y,
                                spot.w,
                                spot.h
                            ),
                        )
                    })
            })
            .collect();
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        let text = hits
            .into_iter()
            .take(4)
            .map(|(_, text)| text)
            .collect::<Vec<_>>()
            .join(" | ");
        self.mask(&text)
    }

    fn pointer(
        &mut self,
        kind: &'static str,
        window: &slint::Window,
        at: (f32, f32),
        origin: (f32, f32),
        detail: &str,
    ) {
        let name = self.window_name(window, origin);
        let (slot, prefix) = match kind {
            "click" => (0, "C"),
            "move" => (1, "H"),
            _ => (2, "W"),
        };
        self.counters[slot] += 1;
        let label = format!("{prefix}{}", self.counters[slot]);
        let under = self.describe(name, at);
        let target = self.hint.take();
        self.cur.events.push(Event::Pointer {
            label,
            kind,
            window: name.to_owned(),
            at,
            detail: detail.to_owned(),
            target,
            under,
        });
    }
}

fn skip_note(phase: &str, reason: &str) -> String {
    if reason.contains("private text") {
        format!("{phase} screenshots: screenshot skipped: private text visible")
    } else {
        format!("{phase} screenshots: screenshot skipped: the capture failed")
    }
}

/// The marks of a record's pointer actions.
fn marks_of(record: &Record) -> Vec<Mark> {
    record
        .events
        .iter()
        .filter_map(|event| match event {
            Event::Pointer {
                label,
                kind,
                window,
                at,
                ..
            } => Some(Mark {
                label: label.clone(),
                window: window.clone(),
                shape: match *kind {
                    "click" => Shape::Ring,
                    "move" => Shape::Cross,
                    _ => Shape::Square,
                },
                x: at.0,
                y: at.1,
            }),
            _ => None,
        })
        .collect()
}

/// Step `n` (0-based) is about to act: files the setup away and takes the "before" screenshots.
/// `cursor` is where the drawn cursor stands.
pub fn begin_step(ui: &MainWindow, cfg: &Config, n: usize, cursor: (f32, f32)) {
    with_sink(|sink| {
        // A dialog the setup left up is not photographed again; one that went away is written out.
        sink.watch(false);
        sink.pending = None;
        sink.during = 0;
        sink.close_setup();
        let state = sink.state(ui);
        let mut record = Record::new(Some(n + 1), "step", state.title.clone(), sink.elapsed_ms());
        record.before = Some(state);
        sink.cur = record;
        sink.counters = [0; 3];
        sink.hint = None;
        let start = Mark {
            label: "S".to_owned(),
            window: "main".to_owned(),
            shape: Shape::Dot,
            x: cursor.0,
            y: cursor.1,
        };
        sink.shoot(ui, cfg, "before", &[start]);
    });
}

/// Step `n` (0-based) is over, with `result`: takes the "after" screenshots and writes the log.
pub fn end_step(ui: &MainWindow, cfg: &Config, n: usize, result: &Result<(), String>) {
    with_sink(|sink| {
        // What the step opened and closed again is written out as "during"; what is still up is
        // in the "after" pictures.
        sink.watch(false);
        sink.pending = None;
        if sink.cur.index != Some(n + 1) {
            // The step failed before it began (it went by before it could be shown).
            sink.close_setup();
            let state = sink.state(ui);
            sink.cur = Record::new(Some(n + 1), "step", state.title, sink.elapsed_ms());
            sink.counters = [0; 3];
        }
        let state = sink.state(ui);
        sink.cur.after = Some(state);
        sink.cur.end_ms = sink.elapsed_ms();
        sink.cur.error = result.as_ref().err().map(|reason| sink.mask(reason));
        let marks = marks_of(&sink.cur);
        sink.shoot(ui, cfg, "after", &marks);
        sink.cur.finished = true;
        let now = sink.elapsed_ms();
        let tail = Record::new(None, "tail", "After the last step".to_owned(), now);
        let finished = std::mem::replace(&mut sink.cur, tail);
        sink.done.push(finished);
        sink.write_logs();
    });
}

/// Closes the run: the finding, the failure screenshots, the final log; removes the sink. Call it
/// once per video after the director returned, whether or not it failed.
pub fn finish(ui: Option<&MainWindow>, cfg: &Config, id: &str, outcome: &Result<(), String>) {
    if !cfg.debug {
        return;
    }
    if !is_on() && start(cfg, id, ui).is_err() {
        return;
    }
    with_sink(|sink| {
        let now = sink.elapsed_ms();
        if !sink.cur.is_empty() {
            sink.cur.end_ms = now;
            sink.cur.finished = outcome.is_ok();
            let tail = Record::new(None, "tail", "After the last step".to_owned(), now);
            let last = std::mem::replace(&mut sink.cur, tail);
            sink.done.push(last);
        }
        sink.header.finished = outcome.is_ok();
        sink.header.finding = outcome.as_ref().err().map(|reason| sink.mask(reason));
        if let (Err(_), Some(ui)) = (outcome, ui) {
            let marks = sink
                .done
                .iter()
                .rev()
                .find(|record| record.error.is_some())
                .map(marks_of)
                .unwrap_or_default();
            match failure_capture::capture_windows(ui, cfg) {
                Ok(shots) => {
                    for (window, shot) in shots {
                        let name = format!("failure-{window}.png");
                        let own: Vec<Mark> = marks
                            .iter()
                            .filter(|mark| mark.window == window)
                            .cloned()
                            .collect();
                        match debug_images::write(&shot, &own, &sink.dir.join(&name)) {
                            Ok(()) => sink.header.failure_images.push(name),
                            Err(reason) => {
                                sink.header.failure_note = Some(format!(
                                    "failure screenshot {name} not written: {reason}"
                                ));
                            }
                        }
                    }
                }
                Err(reason) => {
                    sink.header.failure_note = Some(skip_note("failure", &reason));
                }
            }
        }
        sink.write_logs();
    });
    ACTIVE.with(|active| active.set(false));
    SINK.with(|cell| cell.borrow_mut().take());
}

/// Records a pointer action: a `click`, the end of a cursor `move`, or a `wheel` turn. `at` is
/// local to `window`; `origin` is the window's place in the frame.
pub fn pointer(
    kind: &'static str,
    window: &slint::Window,
    at: (f32, f32),
    origin: (f32, f32),
    detail: &str,
) {
    with_sink(|sink| {
        // Before the action acts: a dialog it closes is photographed as it stood.
        sink.watch(true);
        sink.pointer(kind, window, at, origin, detail);
    });
}

/// A hold of `d` just ended: a dialog or window that is up is photographed again (its state has
/// settled), so the "during" pictures of a step show it after its last change.
pub fn after_hold(d: Duration) {
    if d >= WATCH_HOLD {
        with_sink(|sink| sink.watch(true));
    }
}

/// Records the end of a cursor move over the main window.
pub fn cursor_move(to: (f32, f32)) {
    with_sink(|sink| {
        let label = {
            sink.counters[1] += 1;
            format!("H{}", sink.counters[1])
        };
        let under = sink.describe("main", to);
        let target = sink.hint.take();
        sink.cur.events.push(Event::Pointer {
            label,
            kind: "move",
            window: "main".to_owned(),
            at: to,
            detail: String::new(),
            target,
            under,
        });
    });
}

/// Says which control the next pointer action is aimed at.
pub fn target(spot: &Spot) {
    with_sink(|sink| {
        let label = spot
            .handle
            .as_ref()
            .and_then(crate::support::ElementHandle::accessible_label)
            .map(|label| label.to_string())
            .unwrap_or_default();
        sink.hint = Some(format!(
            "{} at [{:.0}, {:.0}, {:.0} x {:.0}]",
            sink.mask(&format!("{label:?}")),
            spot.x,
            spot.y,
            spot.w,
            spot.h
        ));
    });
}

/// Records a key chord.
pub fn keys(chord: &str) {
    with_sink(|sink| {
        sink.watch(true);
        sink.cur.events.push(Event::Keys(sink.mask(chord)));
    });
}

/// Records typed text.
pub fn text(text: &str) {
    with_sink(|sink| {
        sink.watch(true);
        sink.cur.events.push(Event::Text(sink.mask(text)));
    });
}

/// Records a lookup of a control on screen and what it found.
pub fn lookup(kind: &'static str, wanted: &dyn Debug, found: Option<&Spot>) {
    with_sink(|sink| {
        let wanted = sink.mask(&format!("{wanted:?}"));
        let found = found.map(|spot| [spot.x, spot.y, spot.w, spot.h]);
        if let Some(Event::Lookup {
            kind: last_kind,
            wanted: last_wanted,
            found: last_found,
            repeats,
        }) = sink.cur.events.last_mut()
            && *last_kind == kind
            && *last_wanted == wanted
            && *last_found == found
        {
            *repeats += 1;
            return;
        }
        let lookups = sink
            .cur
            .events
            .iter()
            .filter(|event| matches!(event, Event::Lookup { .. }))
            .count();
        if lookups >= LOOKUP_CAP {
            sink.cur.dropped_lookups += 1;
            return;
        }
        sink.cur.events.push(Event::Lookup {
            kind,
            wanted,
            found,
            repeats: 1,
        });
    });
}

/// Records a readback the route compared: what it expected and what the app showed. The check is
/// equal when both print the same.
pub fn debug_check(label: &str, expected: &dyn Debug, observed: &dyn Debug) {
    if !is_on() {
        return;
    }
    let (expected, observed) = (format!("{expected:?}"), format!("{observed:?}"));
    let ok = expected == observed;
    push_check(label, &expected, &observed, ok);
}

/// [`debug_check`] for a comparison that is not plain equality (a tolerance, a range).
pub fn debug_check_with(label: &str, expected: &dyn Debug, observed: &dyn Debug, ok: bool) {
    if !is_on() {
        return;
    }
    push_check(
        label,
        &format!("{expected:?}"),
        &format!("{observed:?}"),
        ok,
    );
}

fn push_check(label: &str, expected: &str, observed: &str, ok: bool) {
    with_sink(|sink| {
        let event = Event::Check {
            label: sink.mask(label),
            expected: sink.mask(expected),
            observed: sink.mask(observed),
            ok,
        };
        sink.cur.events.push(event);
    });
}

/// Records the route a step takes (the same route twice in a row is one entry).
pub fn route(name: &str) {
    with_sink(|sink| {
        let name = sink.mask(name);
        if matches!(sink.cur.events.last(), Some(Event::Route(last)) if *last == name) {
            return;
        }
        sink.cur.events.push(Event::Route(name));
    });
}

/// The route of a step that waits for the UI event `name`.
pub fn route_event(name: &str) {
    if is_on() {
        route(&format!("drive the event {name}"));
    }
}

/// The route of a step that runs the command `id` from the command palette.
pub fn route_command(id: &str) {
    if is_on() {
        route(&format!("command {id}"));
    }
}

/// The route of a step whose recipe performs `perform`.
pub fn route_perform(perform: &dyn Debug) {
    if is_on() {
        route(&format!("perform {perform:?}"));
    }
}

/// Takes the last entry back if it is a route (a script that turned out not to exist).
pub fn drop_route() {
    with_sink(|sink| {
        if matches!(sink.cur.events.last(), Some(Event::Route(_))) {
            sink.cur.events.pop();
        }
    });
}

/// The route of the authored script of step `n` (0-based) of guide `id`.
pub fn route_script(id: &str, n: usize) {
    if is_on() {
        route(&format!("authored script of {id} step {}", n + 1));
    }
}
