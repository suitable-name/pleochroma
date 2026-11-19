//! The Developer page and the log viewer: tracer state, Last Start, GPU probe and the log list.
//!
//! The functions here only write properties. The selection (a set of file names) lives in a
//! thread-local, because the Slint rows carry it only for display.

use std::{cell::RefCell, collections::HashSet};

use pleochroma_mobile_core::{
    app_state::AppState,
    diagnostics::{
        Session,
        exit::LastStart,
        probe::{self, GroupState, ProbeGroup},
        session::TracerState,
        store::{LogEntry, LogKind},
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, Developer, LogRow, ProbeGroupRow, RenderModel};

/// The viewer shows at most this many bytes from the end of a log.
pub const VIEWER_TAIL_BYTES: u64 = 256 * 1024;

/// Characters per line of the viewer on a phone (about 44 at 12 sp on a 390 dp window).
const PHONE_COLUMNS: usize = 44;

/// What the page needs to remember between pushes.
#[derive(Default)]
pub struct DevState {
    /// The ticked file names.
    pub selected: HashSet<String>,
    /// File names in list order: the text logs, then the crash snapshots (the Slint row numbers).
    pub order: Vec<String>,
    /// The names being written right now (never deleted).
    pub locked: Vec<String>,
    /// What a delete confirmation will remove.
    pub pending_delete: Vec<String>,
    /// The log of the Last Start finding.
    pub last_start_log: Option<String>,
    /// The log shown in the viewer, and its text as read (unwrapped).
    pub viewer_name: String,
    pub viewer_raw: String,
    /// Whether this process started the probe that runs (false after a relaunch).
    pub probe_started_here: bool,
    /// Whether the last tick saw the probe running.
    pub probe_was_running: bool,
}

thread_local! {
    static STATE: RefCell<DevState> = RefCell::new(DevState::default());
}

/// Runs `f` on the page's remembered state.
pub fn with_state<R>(f: impl FnOnce(&mut DevState) -> R) -> R {
    STATE.with(|state| f(&mut state.borrow_mut()))
}

/// Pushes everything the Developer page shows. Cheap; call after any change.
pub fn push(window: &AppWindow, state: &AppState) {
    let session = &state.diagnostics;
    let developer = window.global::<Developer>();
    developer.set_unlocked(session.unlock().unlocked());
    developer.set_debug_build(cfg!(debug_assertions));
    push_tracer(window, session);
    push_last_start(window, session);
    push_probe(window, session);
    push_logs(window, session);
}

/// The tracer switch and its note line.
pub fn push_tracer(window: &AppWindow, session: &Session) {
    let developer = window.global::<Developer>();
    developer.set_tracer_on(session.tracer_on());
    let (note, warning) = tracer_note(&session.tracer_state());
    developer.set_tracer_note(note.into());
    developer.set_tracer_warning(warning);
}

/// The note under the tracer row and whether it is a warning.
#[must_use]
pub fn tracer_note(state: &TracerState) -> (String, bool) {
    match state {
        TracerState::Off => (String::new(), false),
        TracerState::On { file, bytes } => {
            (format!("Now writing: {file} · {}", fmt_size(*bytes)), false)
        }
        TracerState::Stopped { file, bytes } => {
            (format!("Stopped: {file} · {}", fmt_size(*bytes)), false)
        }
        TracerState::StoppedStorageFull => {
            ("Stopped: the device storage is full.".to_owned(), true)
        }
        TracerState::StoppedAtLimit => ("Stopped at the 8 MB limit for this log.".to_owned(), true),
    }
}

/// The two texts of the Last Start finding: the sentence and the detail line.
#[must_use]
pub fn last_start_texts(last: &LastStart) -> Option<(String, String, bool)> {
    match last {
        LastStart::Clean => None,
        LastStart::EngineCrash {
            engine, step, when, ..
        } => {
            let name = engine_name(*engine);
            Some((
                format!("The app closed while {step} ({when})."),
                format!("{name} uses the CPU for now."),
                true,
            ))
        }
        LastStart::CacheCrash { step, when, .. } => Some((
            format!("The app closed while {step} ({when})."),
            "They are built again.".to_owned(),
            true,
        )),
        LastStart::Interrupted { step, when, .. } => Some((
            format!("The app was closed while {step} ({when})."),
            "Nothing was changed.".to_owned(),
            false,
        )),
    }
}

/// The start-up sheet's body for a crash, or `None` for a start that needs no sheet.
#[must_use]
pub fn last_start_sheet_body(last: &LastStart) -> Option<String> {
    match last {
        LastStart::EngineCrash { engine, .. } => Some(format!(
            "The app closed unexpectedly while preparing the graphics card. {} uses the CPU on this device for now.",
            engine_name(*engine)
        )),
        LastStart::CacheCrash { .. } => Some(
            "The app closed unexpectedly while loading saved shaders. They are built again."
                .to_owned(),
        ),
        LastStart::Clean | LastStart::Interrupted { .. } => None,
    }
}

/// The log a Last Start finding names.
#[must_use]
pub fn last_start_log(last: &LastStart) -> Option<String> {
    match last {
        LastStart::Clean => None,
        LastStart::EngineCrash { log, .. }
        | LastStart::CacheCrash { log, .. }
        | LastStart::Interrupted { log, .. } => Some(log.clone()),
    }
}

/// "Ray-Traced" or "Path-Traced".
#[must_use]
pub const fn engine_name(engine: pleochroma::diag::Engine) -> &'static str {
    match engine {
        pleochroma::diag::Engine::PathTraced => "Path-Traced",
        pleochroma::diag::Engine::RayTraced => "Ray-Traced",
    }
}

fn push_last_start(window: &AppWindow, session: &Session) {
    let developer = window.global::<Developer>();
    let last = &session.start_report().last_start;
    with_state(|state| state.last_start_log = last_start_log(last));
    if let Some((text, detail, crash)) = last_start_texts(last) {
        developer.set_last_start_visible(true);
        developer.set_last_start_text(text.into());
        developer.set_last_start_detail(detail.into());
        developer.set_last_start_crash(crash);
    } else {
        developer.set_last_start_visible(false);
    }
}

/// The probe card, the button state and the "last run" text.
pub fn push_probe(window: &AppWindow, session: &Session) {
    let developer = window.global::<Developer>();
    let snapshot = probe::snapshot();
    let running = probe::is_running();
    let exporting = window.global::<RenderModel>().get_export_running();
    let started_here = with_state(|state| state.probe_started_here);

    let rows: Vec<ProbeGroupRow> = snapshot.groups.iter().map(group_row).collect();
    let done: u32 = snapshot.groups.iter().map(|group| group.done).sum();
    let units: u32 = snapshot.groups.iter().map(|group| group.total).sum();
    let elapsed: u64 = snapshot.groups.iter().map(|group| group.ms).sum();
    let failed = snapshot
        .groups
        .iter()
        .filter(|group| group.state == GroupState::Failed)
        .count();
    let has_result = !running && !snapshot.groups.is_empty();

    developer.set_probe_running(running);
    developer.set_probe_has_result(has_result);
    developer.set_probe_can_run(!running && !exporting);
    developer.set_probe_groups(ModelRc::new(VecModel::from(rows)));
    #[expect(
        clippy::cast_precision_loss,
        reason = "a progress fraction of a short list"
    )]
    let fraction = if units == 0 {
        0.0
    } else {
        done as f32 / units as f32
    };
    developer.set_probe_progress(fraction);
    if running {
        developer.set_probe_title(format!("Running · {done} of {units}").into());
        developer.set_probe_detail("The live render is stopped while the probe runs.".into());
    } else {
        developer.set_probe_title(probe_result_text(snapshot.ok, snapshot.total, failed).into());
        let saved = snapshot
            .file
            .as_deref()
            .map_or_else(String::new, |file| format!("Saved as {file} · "));
        developer.set_probe_detail(format!("{saved}{}", fmt_ms(elapsed)).into());
    }
    developer.set_probe_last(
        probe::last_result(session.store())
            .map_or_else(
                || "Not run yet".to_owned(),
                |(ok, total)| format!("Last run: {ok} of {total} ok"),
            )
            .into(),
    );
    let line = if running && !started_here {
        "Started before the app was reopened; it finishes on its own."
    } else if exporting && !running {
        "Not while an export runs."
    } else {
        ""
    };
    developer.set_probe_line(line.into());
}

/// "24 of 25 ok", with " · 1 rejected" when a group failed.
#[must_use]
pub fn probe_result_text(ok: u32, total: u32, failed: usize) -> String {
    if failed == 0 {
        format!("{ok} of {total} ok")
    } else {
        format!("{ok} of {total} ok · {failed} rejected")
    }
}

fn group_row(group: &ProbeGroup) -> ProbeGroupRow {
    let detail = match group.state {
        GroupState::Waiting => String::new(),
        _ if group.total == 0 => fmt_ms(group.ms),
        _ => format!(
            "{} of {} built · {}",
            group.done,
            group.total,
            fmt_ms(group.ms)
        ),
    };
    ProbeGroupRow {
        title: group.title.clone().into(),
        detail: detail.into(),
        note: group.note.clone().into(),
        state: match group.state {
            GroupState::Waiting => 0,
            GroupState::Running => 1,
            GroupState::Ok => 2,
            GroupState::Failed => 3,
        },
    }
}

/// The log list, the selection and the buttons that depend on it.
pub fn push_logs(window: &AppWindow, session: &Session) {
    let developer = window.global::<Developer>();
    let entries = session.store().list();
    let locked = locked_names(session);
    let (selected, texts, snapshots, order) = with_state(|state| {
        let names: HashSet<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        state.selected.retain(|name| names.contains(name.as_str()));
        let (texts, snapshots): (Vec<&LogEntry>, Vec<&LogEntry>) = entries
            .iter()
            .partition(|entry| entry.kind != LogKind::CrashSnapshot);
        let order: Vec<String> = texts
            .iter()
            .chain(snapshots.iter())
            .map(|entry| entry.name.clone())
            .collect();
        state.order.clone_from(&order);
        state.locked.clone_from(&locked);
        let rows = |list: &[&LogEntry]| -> Vec<LogRow> {
            list.iter()
                .map(|entry| {
                    log_row(
                        entry,
                        locked.contains(&entry.name),
                        state.selected.contains(&entry.name),
                    )
                })
                .collect()
        };
        (
            state.selected.clone(),
            rows(&texts),
            rows(&snapshots),
            order,
        )
    });

    let text_names: Vec<String> =
        pleochroma_mobile_core::diagnostics::bundle::select_all_names(&entries);
    let all_selected =
        !text_names.is_empty() && text_names.iter().all(|name| selected.contains(name));
    let deletable = order
        .iter()
        .filter(|name| selected.contains(*name) && !locked.contains(*name))
        .count();
    let only_locked = !selected.is_empty() && deletable == 0;

    developer.set_logs(ModelRc::new(VecModel::from(texts)));
    developer.set_snapshots(ModelRc::new(VecModel::from(snapshots)));
    developer.set_all_selected(all_selected);
    developer.set_can_select_all(!text_names.is_empty());
    developer.set_can_email(!selected.is_empty());
    developer.set_can_delete(deletable > 0);
    developer.set_can_delete_all(!order.is_empty());
    developer.set_delete_note(
        if only_locked {
            "The tracer log being written can't be deleted; switch it off first."
        } else {
            ""
        }
        .into(),
    );
}

/// The files being written: the tracer file while it is on, the probe file while it runs.
#[must_use]
pub fn locked_names(session: &Session) -> Vec<String> {
    let mut names = Vec::new();
    if let TracerState::On { file, .. } = session.tracer_state() {
        names.push(file);
    }
    if probe::is_running()
        && let Some(file) = probe::snapshot().file
    {
        names.push(file);
    }
    names
}

fn log_row(entry: &LogEntry, locked: bool, selected: bool) -> LogRow {
    LogRow {
        name: entry.name.clone().into(),
        detail: row_detail(entry, locked).into(),
        binary: entry.kind == LogKind::CrashSnapshot,
        locked,
        selected,
    }
}

/// "3 KB · Nov 17, 15:31", "14 KB · … · being written" or the snapshot line.
#[must_use]
pub fn row_detail(entry: &LogEntry, locked: bool) -> String {
    let size = fmt_size(entry.bytes);
    if entry.kind == LogKind::CrashSnapshot {
        return format!("Crash snapshot · {size} · binary, e-mail only");
    }
    let mut text = size;
    if let Some(stamp) = stamp_label(&entry.name) {
        text.push_str(" · ");
        text.push_str(&stamp);
    }
    if locked {
        text.push_str(" · being written");
    }
    text
}

/// "Nov 17, 15:31" from the `YYYYMMDD-HHMMSS` stamp in a log's name.
#[must_use]
pub fn stamp_label(name: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let bytes = name.as_bytes();
    let start = (0..bytes.len().saturating_sub(14)).find(|&at| {
        bytes[at..at + 8].iter().all(u8::is_ascii_digit)
            && bytes[at + 8] == b'-'
            && bytes[at + 9..at + 15].iter().all(u8::is_ascii_digit)
    })?;
    let stamp = &name[start..start + 15];
    let month: usize = stamp[4..6].parse().ok()?;
    let day: u32 = stamp[6..8].parse().ok()?;
    let month_name = MONTHS.get(month.checked_sub(1)?)?;
    Some(format!(
        "{month_name} {day}, {}:{}",
        &stamp[9..11],
        &stamp[11..13]
    ))
}

/// "412 B", "14 KB" or "3.1 MB".
#[must_use]
pub fn fmt_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes < KB {
        format!("{bytes} B")
    } else if bytes < MB {
        format!("{} KB", ((bytes + KB / 2) / KB).max(1))
    } else {
        #[expect(clippy::cast_precision_loss, reason = "a size shown with one decimal")]
        let mb = bytes as f64 / MB as f64;
        format!("{mb:.1} MB")
    }
}

/// "412 ms", "4.1 s" or "2 min 31 s".
#[must_use]
pub fn fmt_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a duration shown with one decimal"
        )]
        let seconds = ms as f64 / 1000.0;
        format!("{seconds:.1} s")
    } else {
        format!("{} min {} s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// The title and body of a delete confirmation (blueprint 4.5 "Delete").
///
/// `kept` names the files that stay because they are being written: (tracer, probe).
#[must_use]
pub fn delete_texts(logs: usize, snapshots: usize, kept: (bool, bool)) -> (String, String) {
    let log_part = |count: usize| format!("{count} log{}", if count == 1 { "" } else { "s" });
    let snapshot_part = |count: usize| {
        format!(
            "{count} crash snapshot{}",
            if count == 1 { "" } else { "s" }
        )
    };
    let title = match (logs, snapshots) {
        (0, snapshots) => format!("Delete {}?", snapshot_part(snapshots)),
        (logs, 0) => format!("Delete {}?", log_part(logs)),
        (logs, snapshots) => format!(
            "Delete {} and {}?",
            log_part(logs),
            snapshot_part(snapshots)
        ),
    };
    let kept_sentence = match kept {
        (true, true) => "The logs being written are kept. ",
        (true, false) => "The tracer log being written is kept. ",
        (false, true) => "The probe log being written is kept. ",
        (false, false) => "",
    };
    (title, format!("{kept_sentence}This cannot be undone."))
}

/// Wraps `text` for a phone: each line at `columns` characters, a continuation indented under
/// the message when the line starts with a time column ("+12.345 s ").
#[must_use]
pub fn wrap_hanging(text: &str, columns: usize) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for line in text.lines() {
        wrap_line(line, columns, &mut out);
    }
    out
}

fn wrap_line(line: &str, columns: usize, out: &mut String) {
    let chars: Vec<char> = line.chars().collect();
    let indent = time_column_width(line).min(columns / 2);
    let mut start = 0;
    let mut first = true;
    loop {
        let width = if first { columns } else { columns - indent };
        if chars.len() - start <= width {
            if !first {
                out.extend(std::iter::repeat_n(' ', indent));
            }
            out.extend(&chars[start..]);
            out.push('\n');
            return;
        }
        let mut end = start + width;
        if let Some(space) = chars[start..end].iter().rposition(|&c| c == ' ')
            && space > 0
        {
            end = start + space;
        }
        if !first {
            out.extend(std::iter::repeat_n(' ', indent));
        }
        out.extend(&chars[start..end]);
        out.push('\n');
        start = end;
        while start < chars.len() && chars[start] == ' ' {
            start += 1;
        }
        first = false;
    }
}

/// The length of a leading "+12.345 s " time column, or 0.
fn time_column_width(line: &str) -> usize {
    if !line.starts_with('+') {
        return 0;
    }
    line.find(" s ").map_or(0, |at| at + 3)
}

/// Shows `text` in the viewer and remembers the raw text for Copy all.
pub fn push_viewer(window: &AppWindow, name: &str, raw: &str, whole_bytes: u64, locked: bool) {
    let developer = window.global::<Developer>();
    let phone = window.global::<crate::Shell>().get_window_class() == 0;
    developer.set_viewer_name(name.into());
    developer.set_viewer_info(viewer_info(whole_bytes, locked).into());
    let shown = if phone {
        wrap_hanging(raw, PHONE_COLUMNS)
    } else {
        raw.to_owned()
    };
    developer.set_viewer_text(SharedString::from(shown));
    with_state(|state| {
        name.clone_into(&mut state.viewer_name);
        raw.clone_into(&mut state.viewer_raw);
    });
}

/// The line under the viewer's title.
#[must_use]
pub fn viewer_info(whole_bytes: u64, locked: bool) -> String {
    if whole_bytes > VIEWER_TAIL_BYTES {
        format!("Last 256 KB of {}", fmt_size(whole_bytes))
    } else if locked {
        format!(
            "{} · the whole file · still being written",
            fmt_size(whole_bytes)
        )
    } else {
        format!("{} · the whole file", fmt_size(whole_bytes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: LogKind, bytes: u64) -> LogEntry {
        LogEntry {
            name: name.to_owned(),
            kind,
            bytes,
            modified_ms: 0,
        }
    }

    #[test]
    fn sizes_and_durations_read_plainly() {
        assert_eq!(fmt_size(412), "412 B");
        assert_eq!(fmt_size(14 * 1024), "14 KB");
        assert_eq!(fmt_size(3 * 1024 * 1024 + 100 * 1024), "3.1 MB");
        assert_eq!(fmt_ms(412), "412 ms");
        assert_eq!(fmt_ms(4100), "4.1 s");
        assert_eq!(fmt_ms(151_000), "2 min 31 s");
    }

    #[test]
    fn the_stamp_in_a_name_becomes_a_date() {
        assert_eq!(
            stamp_label("crash-20261117-153100.log").as_deref(),
            Some("Nov 17, 15:31")
        );
        assert_eq!(
            stamp_label("crash-snapshot-20261117-153100.pb").as_deref(),
            Some("Nov 17, 15:31")
        );
        assert_eq!(stamp_label("notes.log"), None);
    }

    #[test]
    fn rows_name_the_snapshot_and_the_open_file() {
        let snapshot = entry(
            "crash-snapshot-20261117-153100.pb",
            LogKind::CrashSnapshot,
            410 * 1024,
        );
        assert_eq!(
            row_detail(&snapshot, false),
            "Crash snapshot · 410 KB · binary, e-mail only"
        );
        let tracer = entry("tracer-20261117-153200.log", LogKind::Tracer, 14 * 1024);
        assert_eq!(
            row_detail(&tracer, true),
            "14 KB · Nov 17, 15:32 · being written"
        );
    }

    #[test]
    fn delete_texts_follow_the_blueprint() {
        assert_eq!(
            delete_texts(2, 0, (false, false)),
            (
                "Delete 2 logs?".to_owned(),
                "This cannot be undone.".to_owned()
            )
        );
        assert_eq!(
            delete_texts(2, 1, (false, false)).0,
            "Delete 2 logs and 1 crash snapshot?"
        );
        assert_eq!(
            delete_texts(1, 1, (true, false)),
            (
                "Delete 1 log and 1 crash snapshot?".to_owned(),
                "The tracer log being written is kept. This cannot be undone.".to_owned()
            )
        );
    }

    #[test]
    fn the_tracer_note_covers_every_state() {
        assert_eq!(tracer_note(&TracerState::Off), (String::new(), false));
        let on = TracerState::On {
            file: "tracer-1.log".into(),
            bytes: 14 * 1024,
        };
        assert_eq!(tracer_note(&on).0, "Now writing: tracer-1.log · 14 KB");
        assert!(tracer_note(&TracerState::StoppedStorageFull).1);
        assert_eq!(
            tracer_note(&TracerState::StoppedAtLimit).0,
            "Stopped at the 8 MB limit for this log."
        );
    }

    #[test]
    fn a_long_line_wraps_under_its_message() {
        let wrapped = wrap_hanging(
            "+12.345 s compile done ew_arm_generic (isotropic) 4187 ms",
            30,
        );
        let lines: Vec<&str> = wrapped.lines().collect();
        assert!(lines.len() >= 2);
        assert!(lines[0].starts_with("+12.345 s "));
        assert!(lines[1].starts_with("          "));
        assert!(lines.iter().all(|line| line.chars().count() <= 30));
        assert_eq!(wrap_hanging("short", 30), "short\n");
    }

    #[test]
    fn the_viewer_info_says_what_is_shown() {
        assert_eq!(
            viewer_info(14 * 1024, true),
            "14 KB · the whole file · still being written"
        );
        assert_eq!(viewer_info(3 * 1024 * 1024, false), "Last 256 KB of 3.0 MB");
    }
}
