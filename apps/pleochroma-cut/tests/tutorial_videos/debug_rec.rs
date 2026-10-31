//! The recorder as the director uses it, with the debug log in front of its input calls.
//!
//! `DebugRec` has two modes. `Recording` is uiprobe's `Recorder`, called exactly as before (a
//! normal run). `Plain` is the [`PlainDriver`] of a debug run: the same events and mock time, no
//! frames, no ffmpeg. Every call the video code makes exists here with the recorder's own
//! signature; the five input calls that move the pointer or press keys record the action in the
//! debug log (a no-op while debug mode is off) before they forward. No route needs to know about
//! the log or the mode.

use crate::{debug_driver::PlainDriver, debug_log};
use std::time::Duration;
use uiprobe::recording::{Recorder, RecorderOptions, RecordingSummary};

/// A recorder, or the plain input driver of a debug run, with pointer and key input logged.
#[expect(
    clippy::large_enum_variant,
    reason = "one value lives for the whole run; boxing would only add indirection"
)]
pub enum DebugRec<'w> {
    /// A video run.
    Recording(Recorder<'w>),
    /// A debug run: no video.
    Plain(PlainDriver<'w>),
}

impl<'w> DebugRec<'w> {
    /// Wraps a started recorder.
    #[must_use]
    pub const fn new(inner: Recorder<'w>) -> Self {
        Self::Recording(inner)
    }

    /// The driver of a debug run for `window`, with the option values of the recorder it
    /// replaces (hover events, ripple time).
    #[must_use]
    pub fn plain(window: &'w slint::Window, options: &RecorderOptions) -> Self {
        Self::Plain(PlainDriver::new(window, options))
    }

    /// The video time so far (the mock time pumped, in a debug run).
    #[must_use]
    pub fn video_time(&self) -> Duration {
        match self {
            Self::Recording(rec) => rec.video_time(),
            Self::Plain(plain) => plain.video_time(),
        }
    }

    /// Whether this is a debug run's driver (nothing is captured).
    #[must_use]
    pub const fn is_plain(&self) -> bool {
        matches!(self, Self::Plain(_))
    }

    /// Ends the recording ([`Recorder::finish`]); a debug run has no summary.
    ///
    /// # Errors
    ///
    /// As [`Recorder::finish`].
    pub fn finish(self) -> Result<Option<RecordingSummary>, String> {
        match self {
            Self::Recording(rec) => rec.finish().map(Some),
            Self::Plain(_) => Ok(None),
        }
    }

    /// [`Recorder::hold`].
    ///
    /// # Errors
    ///
    /// As [`Recorder::hold`].
    pub fn hold(&mut self, d: Duration) -> Result<(), String> {
        match self {
            Self::Recording(rec) => rec.hold(d),
            Self::Plain(plain) => {
                plain.hold(d)?;
                // Debug mode photographs a dialog that is up once its state has settled.
                debug_log::after_hold(d);
                Ok(())
            }
        }
    }

    /// [`Recorder::move_to`], logged as the end point of a cursor move over the main window.
    ///
    /// # Errors
    ///
    /// As [`Recorder::move_to`].
    pub fn move_to(&mut self, to: (f32, f32), d: Duration) -> Result<(), String> {
        debug_log::cursor_move(to);
        match self {
            Self::Recording(rec) => rec.move_to(to, d),
            Self::Plain(plain) => plain.move_to(to, d),
        }
    }

    /// [`Recorder::click`]: the ripple only, nothing is pressed.
    ///
    /// # Errors
    ///
    /// As [`Recorder::click`].
    pub fn click(&mut self) -> Result<(), String> {
        match self {
            Self::Recording(rec) => rec.click(),
            Self::Plain(plain) => plain.click(),
        }
    }

    /// [`Recorder::click_at`], logged as a click.
    ///
    /// # Errors
    ///
    /// As [`Recorder::click_at`].
    pub fn click_at(
        &mut self,
        window: &slint::Window,
        at_logical: (f32, f32),
        origin: (f32, f32),
    ) -> Result<(), String> {
        debug_log::pointer("click", window, at_logical, origin, "");
        match self {
            Self::Recording(rec) => rec.click_at(window, at_logical, origin),
            Self::Plain(plain) => plain.click_at(window, at_logical, origin),
        }
    }

    /// [`Recorder::type_text`], logged as typed text.
    ///
    /// # Errors
    ///
    /// As [`Recorder::type_text`].
    pub fn type_text(
        &mut self,
        window: &slint::Window,
        text: &str,
        per_key: Duration,
    ) -> Result<(), String> {
        debug_log::text(text);
        match self {
            Self::Recording(rec) => rec.type_text(window, text, per_key),
            Self::Plain(plain) => plain.type_text(window, text, per_key),
        }
    }

    /// [`Recorder::scroll_at`], logged as a wheel turn.
    ///
    /// # Errors
    ///
    /// As [`Recorder::scroll_at`].
    pub fn scroll_at(
        &mut self,
        window: &slint::Window,
        at: (f32, f32),
        delta_y: f32,
    ) -> Result<(), String> {
        debug_log::pointer("wheel", window, at, (0.0, 0.0), &format!("delta {delta_y}"));
        match self {
            Self::Recording(rec) => rec.scroll_at(window, at, delta_y),
            Self::Plain(plain) => plain.scroll_at_in(window, at, (0.0, 0.0), delta_y),
        }
    }

    /// [`Recorder::scroll_at_in`], logged as a wheel turn.
    ///
    /// # Errors
    ///
    /// As [`Recorder::scroll_at_in`].
    pub fn scroll_at_in(
        &mut self,
        window: &slint::Window,
        at: (f32, f32),
        origin: (f32, f32),
        delta_y: f32,
    ) -> Result<(), String> {
        debug_log::pointer("wheel", window, at, origin, &format!("delta {delta_y}"));
        match self {
            Self::Recording(rec) => rec.scroll_at_in(window, at, origin, delta_y),
            Self::Plain(plain) => plain.scroll_at_in(window, at, origin, delta_y),
        }
    }

    /// [`Recorder::press_keys`], logged as a key chord.
    ///
    /// # Errors
    ///
    /// As [`Recorder::press_keys`].
    pub fn press_keys(&mut self, window: &slint::Window, chord: &str) -> Result<(), String> {
        debug_log::keys(chord);
        match self {
            Self::Recording(rec) => rec.press_keys(window, chord),
            Self::Plain(plain) => plain.press_keys(window, chord),
        }
    }

    /// [`Recorder::show_keys`]; a debug run draws no badge.
    pub fn show_keys(&mut self, label: &str) {
        if let Self::Recording(rec) = self {
            rec.show_keys(label);
        }
    }

    /// [`Recorder::caption`]; a debug run has no captions.
    pub fn caption(&mut self, text: &str) {
        if let Self::Recording(rec) = self {
            rec.caption(text);
        }
    }

    /// [`Recorder::chapter`]; a debug run has no chapters.
    pub fn chapter(&mut self, title: &str) {
        if let Self::Recording(rec) = self {
            rec.chapter(title);
        }
    }

    /// [`Recorder::refresh`]; a debug run captures nothing.
    ///
    /// # Errors
    ///
    /// As [`Recorder::refresh`].
    pub fn refresh(&mut self) -> Result<(), String> {
        match self {
            Self::Recording(rec) => rec.refresh(),
            Self::Plain(_) => Ok(()),
        }
    }

    /// [`Recorder::attach_window`].
    pub fn attach_window<C: slint::ComponentHandle + 'static>(
        &mut self,
        component: &C,
        at_logical: (f32, f32),
    ) {
        match self {
            Self::Recording(rec) => rec.attach_window(component, at_logical),
            Self::Plain(plain) => plain.attach_window(component, at_logical),
        }
    }

    /// [`Recorder::detach_window`].
    pub fn detach_window(&mut self) {
        match self {
            Self::Recording(rec) => rec.detach_window(),
            Self::Plain(plain) => plain.detach_window(),
        }
    }

    /// [`Recorder::attached_origin`].
    #[must_use]
    pub fn attached_origin(&self) -> Option<(f32, f32)> {
        match self {
            Self::Recording(rec) => rec.attached_origin(),
            Self::Plain(plain) => plain.attached_origin(),
        }
    }

    /// [`Recorder::place_window`].
    #[must_use]
    pub fn place_window(&self, window: &slint::Window) -> (f32, f32) {
        match self {
            Self::Recording(rec) => rec.place_window(window),
            Self::Plain(plain) => plain.place_window(window),
        }
    }
}
