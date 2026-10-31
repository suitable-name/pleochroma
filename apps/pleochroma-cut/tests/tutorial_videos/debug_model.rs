//! The data of a debug run and how it is written: `steps.md` for reading, `steps.json` with the
//! same content for tools. Every text in here has been through the privacy mask already.

use serde_json::{Value, json};
use std::fmt::Write as _;

/// What the guide panel shows.
#[derive(Clone, Debug, Default)]
pub struct GuideState {
    pub open: bool,
    /// The step the panel shows, 1-based (0 when closed).
    pub step: i32,
    pub total: i32,
    pub done: bool,
    pub title: String,
    pub waiting: String,
    pub target: String,
}

impl GuideState {
    fn line(&self) -> String {
        format!(
            "open={}, step={}/{}, done={}, title={:?}, waiting={:?}, highlight={:?}",
            self.open, self.step, self.total, self.done, self.title, self.waiting, self.target
        )
    }

    fn json(&self) -> Value {
        json!({
            "open": self.open,
            "step": self.step,
            "total": self.total,
            "done": self.done,
            "title": self.title,
            "waiting": self.waiting,
            "highlight_target": self.target,
        })
    }
}

/// One thing that happened during a step, in order.
#[derive(Clone, Debug)]
pub enum Event {
    /// A pointer action: a click, the end of a cursor move, a wheel turn.
    Pointer {
        label: String,
        kind: &'static str,
        window: String,
        at: (f32, f32),
        detail: String,
        /// The control the route looked for (label and bounds), when the route said so.
        target: Option<String>,
        /// The smallest elements that hold the point.
        under: String,
    },
    /// A key chord.
    Keys(String),
    /// Typed text (private text masked).
    Text(String),
    /// A lookup of a control on screen; `found` is its bounds `[x, y, w, h]`.
    Lookup {
        kind: &'static str,
        wanted: String,
        found: Option<[f32; 4]>,
        repeats: u32,
    },
    /// A readback the route checked.
    Check {
        label: String,
        expected: String,
        observed: String,
        ok: bool,
    },
    /// The route that was taken.
    Route(String),
}

impl Event {
    fn line(&self) -> String {
        match self {
            Self::Pointer {
                label,
                kind,
                window,
                at,
                detail,
                target,
                under,
            } => {
                let mut text = format!("{label} {kind} in {window} at ({:.0}, {:.0})", at.0, at.1);
                if !detail.is_empty() {
                    let _ = write!(text, " [{detail}]");
                }
                if let Some(target) = target {
                    let _ = write!(text, "; target {target}");
                }
                if !under.is_empty() {
                    let _ = write!(text, "; under the point: {under}");
                }
                text
            }
            Self::Keys(chord) => format!("keys {chord}"),
            Self::Text(text) => format!("typed {text:?}"),
            Self::Lookup {
                kind,
                wanted,
                found,
                repeats,
            } => {
                let place = found.map_or_else(
                    || "NOT FOUND".to_owned(),
                    |b| {
                        format!(
                            "found at [{:.0}, {:.0}, {:.0} x {:.0}]",
                            b[0], b[1], b[2], b[3]
                        )
                    },
                );
                let times = if *repeats > 1 {
                    format!(" (x{repeats})")
                } else {
                    String::new()
                };
                format!("lookup {kind} {wanted}: {place}{times}")
            }
            Self::Check {
                label,
                expected,
                observed,
                ok,
            } => format!(
                "readback {label}: expected {expected}, observed {observed} -> {}",
                if *ok { "ok" } else { "MISMATCH" }
            ),
            Self::Route(name) => format!("route: {name}"),
        }
    }

    fn json(&self) -> Value {
        match self {
            Self::Pointer {
                label,
                kind,
                window,
                at,
                detail,
                target,
                under,
            } => json!({
                "type": "pointer", "label": label, "kind": kind, "window": window,
                "x": at.0, "y": at.1, "detail": detail, "target": target, "under": under,
            }),
            Self::Keys(chord) => json!({ "type": "keys", "chord": chord }),
            Self::Text(text) => json!({ "type": "text", "text": text }),
            Self::Lookup {
                kind,
                wanted,
                found,
                repeats,
            } => json!({
                "type": "lookup", "kind": kind, "wanted": wanted, "found": found,
                "repeats": repeats,
            }),
            Self::Check {
                label,
                expected,
                observed,
                ok,
            } => json!({
                "type": "check", "label": label, "expected": expected, "observed": observed,
                "ok": ok,
            }),
            Self::Route(name) => json!({ "type": "route", "name": name }),
        }
    }
}

/// One step of the tutorial, or the setup before the first one, or what came after the last.
#[derive(Clone, Debug)]
pub struct Record {
    /// The step number, 1-based; `None` for the setup and the tail.
    pub index: Option<usize>,
    pub kind: &'static str,
    pub title: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub before: Option<GuideState>,
    pub after: Option<GuideState>,
    pub events: Vec<Event>,
    /// Image file names written for this record.
    pub images: Vec<String>,
    pub notes: Vec<String>,
    /// Lookups left out because the record had too many.
    pub dropped_lookups: u32,
    pub finished: bool,
    pub error: Option<String>,
}

impl Record {
    #[must_use]
    pub const fn new(
        index: Option<usize>,
        kind: &'static str,
        title: String,
        start_ms: u64,
    ) -> Self {
        Self {
            index,
            kind,
            title,
            start_ms,
            end_ms: start_ms,
            before: None,
            after: None,
            events: Vec::new(),
            images: Vec::new(),
            notes: Vec::new(),
            dropped_lookups: 0,
            finished: false,
            error: None,
        }
    }

    /// Whether anything was recorded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.events.is_empty() && self.images.is_empty() && self.notes.is_empty()
    }

    fn heading(&self) -> String {
        let result = match (&self.error, self.finished) {
            (Some(_), _) => "FAILED",
            (None, true) => "ok",
            (None, false) => "not finished",
        };
        self.index.map_or_else(
            || format!("{} ({result})", self.title),
            |n| format!("Step {n}: {} ({result})", self.title),
        )
    }

    fn markdown(&self, out: &mut String) {
        let _ = write!(out, "## {}\n\n", self.heading());
        let _ = writeln!(
            out,
            "- time: {:.1} s to {:.1} s",
            ms_seconds(self.start_ms),
            ms_seconds(self.end_ms)
        );
        if let Some(state) = &self.before {
            let _ = writeln!(out, "- guide before: {}", state.line());
        }
        if let Some(state) = &self.after {
            let _ = writeln!(out, "- guide after: {}", state.line());
        }
        if !self.images.is_empty() {
            let _ = writeln!(out, "- images: {}", self.images.join(", "));
        }
        for note in &self.notes {
            let _ = writeln!(out, "- {note}");
        }
        if self.dropped_lookups > 0 {
            let _ = writeln!(
                out,
                "- {} further lookups were left out of this log",
                self.dropped_lookups
            );
        }
        out.push_str("- actions, lookups and readbacks in order:\n");
        for (at, event) in self.events.iter().enumerate() {
            let _ = writeln!(out, "  {}. {}", at + 1, event.line());
        }
        match &self.error {
            Some(error) => {
                let _ = writeln!(out, "- result: FAILED: {error}");
            }
            None if self.finished => out.push_str("- result: ok\n"),
            None => {}
        }
        out.push('\n');
    }

    fn json(&self) -> Value {
        json!({
            "index": self.index,
            "kind": self.kind,
            "title": self.title,
            "start_ms": self.start_ms,
            "end_ms": self.end_ms,
            "guide_before": self.before.as_ref().map(GuideState::json),
            "guide_after": self.after.as_ref().map(GuideState::json),
            "images": self.images,
            "notes": self.notes,
            "dropped_lookups": self.dropped_lookups,
            "events": self.events.iter().map(Event::json).collect::<Vec<_>>(),
            "result": match (&self.error, self.finished) {
                (Some(_), _) => "failed",
                (None, true) => "ok",
                (None, false) => "not finished",
            },
            "error": self.error,
        })
    }
}

/// What the log says about the whole run.
#[derive(Clone, Debug, Default)]
pub struct Header {
    pub video: String,
    pub window: (u32, u32),
    pub scale: f32,
    pub library: String,
    pub recorder: String,
    pub started: String,
    pub total_ms: u64,
    pub finished: bool,
    /// The finding text, when the run failed.
    pub finding: Option<String>,
    pub failure_images: Vec<String>,
    pub failure_note: Option<String>,
}

fn ms_seconds(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

/// `steps.md`.
#[must_use]
pub fn markdown(header: &Header, records: &[Record]) -> String {
    let mut out = format!("# Debug run of {}\n\n", header.video);
    let _ = writeln!(
        out,
        "- window: {} x {} logical pixels, scale {}; images are at most 1920 pixels wide",
        header.window.0, header.window.1, header.scale
    );
    let _ = writeln!(out, "- library: {}", header.library);
    let _ = writeln!(out, "- recorder: {}", header.recorder);
    let _ = writeln!(out, "- started: {}", header.started);
    let _ = writeln!(
        out,
        "- total time: {:.1} s{}",
        ms_seconds(header.total_ms),
        if header.finished { "" } else { " (so far)" }
    );
    out.push_str("- markers: C = click (ring), H = end of a cursor move (cross), W = wheel (square), S = cursor before the step (dot)\n");
    match &header.finding {
        Some(finding) => {
            let _ = write!(out, "- result: FAILED\n- finding: {finding}\n");
        }
        None if header.finished => out.push_str("- result: ok\n"),
        None => out.push_str("- result: not finished\n"),
    }
    if !header.failure_images.is_empty() {
        let _ = writeln!(
            out,
            "- failure screenshots: {}",
            header.failure_images.join(", ")
        );
    }
    if let Some(note) = &header.failure_note {
        let _ = writeln!(out, "- {note}");
    }
    out.push('\n');
    for record in records {
        record.markdown(&mut out);
    }
    out
}

/// `steps.json`.
#[must_use]
pub fn json_text(header: &Header, records: &[Record]) -> String {
    let value = json!({
        "video": header.video,
        "window": [header.window.0, header.window.1],
        "scale": header.scale,
        "library": header.library,
        "recorder": header.recorder,
        "started": header.started,
        "total_ms": header.total_ms,
        "finished": header.finished,
        "finding": header.finding,
        "failure_images": header.failure_images,
        "failure_note": header.failure_note,
        "steps": records.iter().map(Record::json).collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// The UTC date and time of a Unix time in seconds.
#[must_use]
pub fn utc_text(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(0) + 719_468;
    let rest = seconds % 86_400;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = if month_part < 10 {
        month_part + 3
    } else {
        month_part - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}
