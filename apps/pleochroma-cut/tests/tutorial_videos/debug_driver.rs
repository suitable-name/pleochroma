//! The input driver of a debug run: the recorder's calls without a recorder.
//!
//! Debug mode takes its screenshots itself (`debug_log`, `debug_images`), so it needs neither
//! frames nor ffmpeg. [`PlainDriver`] gives the app the same events and the same mock time that
//! uiprobe's `Recorder` does (copied from uiprobe 0.18.8, `recording/mod.rs` and `input.rs`):
//! holds pump mock time in frame-sized steps, a glide ends in one hover move, a click is a move,
//! a rest, a pointer press and release, and the ripple's time. Nothing is captured, drawn or
//! encoded; captions, chapters and key badges are no-ops.

use slint::{
    ComponentHandle, LogicalPosition, SharedString,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;
use uiprobe::{
    backend::{drain_events, pump},
    keys::parse_chord,
    recording::{
        RecorderOptions,
        pacing::{glide_duration, wheel_steps},
    },
};

/// The longest step of mock time pumped at once (one frame at 30 fps).
const STEP_MS: u64 = 33;
/// The rest between the cursor arriving on a control and the click (the recorder's value).
const BEFORE_CLICK: Duration = Duration::from_millis(150);
/// The hold after each wheel tick (the recorder's value).
const WHEEL_HOLD: Duration = Duration::from_millis(40);

/// Keeps a strong handle to the attached window's component without naming its type.
trait Held {
    /// The window of the held component.
    fn window(&self) -> &slint::Window;
}

/// A held component.
struct Component<C: ComponentHandle>(C);

impl<C: ComponentHandle> Held for Component<C> {
    fn window(&self) -> &slint::Window {
        self.0.window()
    }
}

/// The attached window and where it sits in the (undrawn) frame.
struct Attached {
    held: Box<dyn Held>,
    /// The window's top-left corner in logical pixels of the main window.
    at: (f32, f32),
}

impl Attached {
    /// Whether the logical point `p` (main window pixels) is inside the attached window.
    fn contains(&self, p: (f32, f32)) -> bool {
        let window = self.held.window();
        let (w, h) = logical_size(window);
        window.is_visible()
            && p.0 >= self.at.0
            && p.1 >= self.at.1
            && p.0 < self.at.0 + w
            && p.1 < self.at.1 + h
    }
}

/// Plays a tutorial's input into the app on mock time, with no recording.
pub struct PlainDriver<'w> {
    window: &'w slint::Window,
    /// Dispatch one pointer move at the end of every glide (the recorder's `hover_events`).
    hover_events: bool,
    /// How long a click's ripple would last (the recorder's `cursor.ripple_ms`).
    ripple_ms: u32,
    /// The cursor tip in logical pixels.
    cursor: (f32, f32),
    /// Mock time pumped so far, in milliseconds.
    pumped_ms: u64,
    attached: Option<Attached>,
}

impl<'w> PlainDriver<'w> {
    /// A driver for `window`, with the option values of the recorder it stands in for. The
    /// cursor starts at the window centre.
    #[must_use]
    pub fn new(window: &'w slint::Window, options: &RecorderOptions) -> Self {
        let (w, h) = logical_size(window);
        Self {
            window,
            hover_events: options.hover_events,
            ripple_ms: options.cursor.ripple_ms,
            cursor: (w / 2.0, h / 2.0),
            pumped_ms: 0,
            attached: None,
        }
    }

    /// The mock time pumped so far.
    #[must_use]
    pub const fn video_time(&self) -> Duration {
        Duration::from_millis(self.pumped_ms)
    }

    /// Lets the app run for `d` of mock time, pumped in steps of at most 33 ms.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "keeps the signature of `Recorder::hold`, which the routes call"
    )]
    pub fn hold(&mut self, d: Duration) -> Result<(), String> {
        let mut left = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        while left > 0 {
            let step = left.min(STEP_MS);
            pump(step);
            self.pumped_ms = self.pumped_ms.saturating_add(step);
            left -= step;
        }
        Ok(())
    }

    /// `Recorder::move_to`: holds `d`, then (with hover events) one pointer move at `to`, to the
    /// attached window when `to` lies inside it, else to the main window.
    pub fn move_to(&mut self, to: (f32, f32), d: Duration) -> Result<(), String> {
        self.hold(d)?;
        self.cursor = to;
        if self.hover_events {
            // A window that cannot take the event (hidden, closing) is not an error.
            match self.attached.as_ref().filter(|a| a.contains(to)) {
                Some(attached) => {
                    let local = LogicalPosition::new(to.0 - attached.at.0, to.1 - attached.at.1);
                    let _ = attached
                        .held
                        .window()
                        .dispatch_event_with_result(WindowEvent::PointerMoved { position: local });
                }
                None => {
                    let _ = self
                        .window
                        .dispatch_event_with_result(WindowEvent::PointerMoved {
                            position: LogicalPosition::new(to.0, to.1),
                        });
                }
            }
        }
        Ok(())
    }

    /// Glides to `to` for the time uiprobe gives for the distance.
    fn glide_to(&mut self, to: (f32, f32)) -> Result<(), String> {
        let distance = (to.0 - self.cursor.0).hypot(to.1 - self.cursor.1);
        self.move_to(to, glide_duration(distance))
    }

    /// `Recorder::click`: the ripple's time, nothing pressed.
    pub fn click(&mut self) -> Result<(), String> {
        self.hold(Duration::from_millis(u64::from(self.ripple_ms)))
    }

    /// `Recorder::click_at`: glide, rest, one real move, press and release on `window`, then the
    /// ripple's time.
    pub fn click_at(
        &mut self,
        window: &slint::Window,
        at_logical: (f32, f32),
        origin: (f32, f32),
    ) -> Result<(), String> {
        self.glide_to((origin.0 + at_logical.0, origin.1 + at_logical.1))?;
        self.hold(BEFORE_CLICK)?;
        let position = LogicalPosition::new(at_logical.0, at_logical.1);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        drain_events();
        self.click()
    }

    /// `Recorder::type_text`: per character a key press and release, then a hold of `per_key`.
    pub fn type_text(
        &mut self,
        window: &slint::Window,
        text: &str,
        per_key: Duration,
    ) -> Result<(), String> {
        for character in text.chars() {
            let key: SharedString = character.to_string().into();
            window.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
            window.dispatch_event(WindowEvent::KeyReleased { text: key });
            drain_events();
            self.hold(per_key)?;
        }
        Ok(())
    }

    /// `Recorder::scroll_at_in`: glide to `origin + at`, one move, then the wheel ticks.
    pub fn scroll_at_in(
        &mut self,
        window: &slint::Window,
        at: (f32, f32),
        origin: (f32, f32),
        delta_y: f32,
    ) -> Result<(), String> {
        self.glide_to((origin.0 + at.0, origin.1 + at.1))?;
        let position = LogicalPosition::new(at.0, at.1);
        window.dispatch_event(WindowEvent::PointerMoved { position });
        for step in wheel_steps(delta_y) {
            window.dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y: step,
            });
            drain_events();
            self.hold(WHEEL_HOLD)?;
        }
        Ok(())
    }

    /// `Recorder::press_keys` without the badge: the chord's events (modifiers first, released
    /// last), then the event queue is drained. uiprobe's `press_events` is crate-private, so the
    /// body of its `dispatch_chord` is repeated here.
    ///
    /// # Errors
    ///
    /// The chord does not parse; nothing is sent then.
    #[expect(
        clippy::unused_self,
        clippy::needless_pass_by_ref_mut,
        reason = "keeps the signature of `Recorder::press_keys`, which the routes call"
    )]
    pub fn press_keys(&mut self, window: &slint::Window, chord: &str) -> Result<(), String> {
        let parsed = parse_chord(chord)?;
        let modifiers = parsed.modifier_keys();
        for key in &modifiers {
            window.dispatch_event(WindowEvent::KeyPressed {
                text: (*key).into(),
            });
        }
        window.dispatch_event(WindowEvent::KeyPressed {
            text: parsed.text.clone(),
        });
        window.dispatch_event(WindowEvent::KeyReleased { text: parsed.text });
        for key in modifiers.iter().rev() {
            window.dispatch_event(WindowEvent::KeyReleased {
                text: (*key).into(),
            });
        }
        drain_events();
        Ok(())
    }

    /// `Recorder::attach_window`: remembers the window (a strong handle) and its place, so
    /// pointer moves that end over it go to it.
    pub fn attach_window<C: ComponentHandle + 'static>(&mut self, component: &C, at: (f32, f32)) {
        self.attached = Some(Attached {
            held: Box::new(Component(component.clone_strong())),
            at,
        });
    }

    /// `Recorder::detach_window`.
    pub fn detach_window(&mut self) {
        self.attached = None;
    }

    /// `Recorder::attached_origin`.
    #[must_use]
    pub fn attached_origin(&self) -> Option<(f32, f32)> {
        self.attached.as_ref().map(|a| a.at)
    }

    /// `Recorder::place_window`: the window's own reported position, else centred over the main
    /// window.
    #[must_use]
    pub fn place_window(&self, window: &slint::Window) -> (f32, f32) {
        let reported = window.position();
        if reported.x != 0 || reported.y != 0 {
            let scale = window.scale_factor().max(f32::EPSILON);
            // Window positions are far below 2^24, where f32 is exact.
            return (reported.x as f32 / scale, reported.y as f32 / scale);
        }
        let (width, height) = logical_size(window);
        let main = logical_size(self.window);
        (
            ((main.0 - width) / 2.0).max(0.0),
            ((main.1 - height) / 2.0).max(0.0),
        )
    }
}

/// The logical size of `window` (physical size over its scale factor; window dimensions are far
/// below 2^24, where f32 is exact).
fn logical_size(window: &slint::Window) -> (f32, f32) {
    let physical = window.size();
    let scale = window.scale_factor().max(f32::EPSILON);
    (
        physical.width as f32 / scale,
        physical.height as f32 / scale,
    )
}
