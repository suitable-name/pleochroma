//! Film the actual Compare pop-out as a second captured window.

use crate::{
    debug_log,
    director::Film,
    pacing, routes,
    support::{WindowView, app::pump},
    targets::{self, Spot},
};
use pleochroma_cut::{CompareModel, CompareWindow, RetargetModel, harness_support};
use slint::{
    ComponentHandle, Image, LogicalPosition, Model,
    platform::{PointerEventButton, WindowEvent},
};
use std::time::Duration;
use uiprobe::screenshot;

#[derive(Clone, PartialEq, Eq)]
struct ComparePixels {
    before_size: (u32, u32),
    after_size: (u32, u32),
    before: Vec<u8>,
    after: Vec<u8>,
}

impl ComparePixels {
    fn both_differ_from(&self, other: &Self) -> bool {
        (self.before_size != other.before_size || self.before != other.before)
            && (self.after_size != other.after_size || self.after != other.after)
    }
}

impl Film<'_> {
    /// The recorder supplies a real pointer click and its visible cursor/ripple.
    fn click_compare(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        spot: &Spot,
    ) -> Result<(), String> {
        let (x, y) = spot.center();
        self.rec.click_at(window, (x, y), origin)?;
        self.cursor = (origin.0 + x, origin.1 + y);
        pump(60);
        Ok(())
    }

    pub(crate) fn film_compare_table(&mut self, visual: bool) -> Result<(), String> {
        let button = targets::resolve(&self.view(), "compare_button");
        let targets::Resolved::Found(button) = button else {
            return Err("Compare to Baseline button is missing".into());
        };
        self.click_and_invoke(&button)?;
        let table = self.ui.global::<RetargetModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            table.get_compare_open() && table.get_compare_rows().row_count() > 0
        }) {
            return Err("Compare to Baseline did not show its populated tier table".into());
        }
        self.rec.hold(pacing::RESULT_HOLD)?;
        if !visual {
            let close = targets::run_label(&self.view(), "Close")
                .ok_or("snapshot comparison table Close button is missing")?;
            self.click_and_invoke(&close)?;
            if table.get_compare_open() {
                return Err("snapshot table Close click did not close it".into());
            }
            return Ok(());
        }
        let visual = targets::run_label_start(&self.view(), "Compare Visually")
            .ok_or("Compare Visually button is missing from the snapshot table")?;
        self.click_and_invoke(&visual)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::compare_window()
                .is_some_and(|window| window.global::<CompareModel>().get_is_open())
        }) {
            return Err("Compare click did not open a live comparison window".into());
        }
        let compare = harness_support::compare_window().ok_or("Compare window disappeared")?;
        screenshot::set_window_size(compare.window(), (1100, 780), crate::config::SCALE);
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let model = compare.global::<CompareModel>();
            !model.get_before_label().is_empty()
                && !model.get_after_label().is_empty()
                && model.get_before_image().size().width > 0
                && model.get_after_image().size().width > 0
        }) {
            return Err("Compare window did not render both labelled images".into());
        }
        let origin = self.rec.place_window(compare.window());
        self.rec.attach_window(&compare, origin);
        self.scan_compare_text(&compare)?;
        self.rec.chapter("Compare to Baseline");
        self.rec
            .caption("Compare the two designs in the separate window.");
        self.rec.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    pub(crate) fn film_compare_read_close(&mut self) -> Result<(), String> {
        let compare = harness_support::compare_window()
            .ok_or("Compare window is missing on its reading step")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("Compare window is not attached to the recording")?;
        if !compare.global::<CompareModel>().get_is_open() {
            return Err("Compare window has no live comparison".into());
        }
        self.compare_pick_controls(&compare, origin)?;
        self.compare_drag_divider(&compare, origin)?;
        self.compare_orbit_zoom_reset(&compare, origin)?;
        self.compare_close(&compare, origin)
    }

    /// Clicks each mode and each renderer control of the Compare window and reads the choice back.
    fn compare_pick_controls(
        &mut self,
        compare: &CompareWindow,
        origin: (f32, f32),
    ) -> Result<(), String> {
        let model = compare.global::<CompareModel>();
        for (label, index) in [("Overlay", 2), ("Split Slider", 1), ("Side by Side", 0)] {
            let view = WindowView::of("CompareWindow", compare);
            let spot = targets::run_label(&view, label)
                .ok_or_else(|| format!("Compare window control {label:?} is missing"))?;
            self.click_compare(compare.window(), origin, &spot)?;
            if model.get_mode_index() != index {
                return Err(format!(
                    "Compare {label:?} click did not select mode {index}"
                ));
            }
            self.rec.hold(Duration::from_millis(450))?;
            self.scan_compare_text(compare)?;
        }
        for (label, index) in [("Path Traced", 1), ("Ray Traced", 2), ("Solid", 0)] {
            let spot = targets::run_label(&WindowView::of("CompareWindow", compare), label)
                .ok_or_else(|| format!("Compare renderer control {label:?} is missing"))?;
            self.click_compare(compare.window(), origin, &spot)?;
            if model.get_renderer_index() != index {
                return Err(format!("Compare renderer {label:?} did not select {index}"));
            }
            self.rec.hold(Duration::from_millis(600))?;
            self.scan_compare_text(compare)?;
        }
        Ok(())
    }

    /// Presses at `start`, glides the cursor to `end` and releases there (both are window-local
    /// positions in the Compare window; `origin` is where that window sits in the video). The
    /// release is sent even when the glide fails, and the glide's error is returned after it.
    fn compare_drag(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        start: LogicalPosition,
        end: LogicalPosition,
    ) -> Result<(), String> {
        let frame_start = (origin.0 + start.x, origin.1 + start.y);
        if self.cursor != frame_start {
            self.glide_to(frame_start)?;
        }
        window.dispatch_event(WindowEvent::PointerPressed {
            position: start,
            button: PointerEventButton::Left,
        });
        pump(30);
        let mut last = start;
        let moved = (|| {
            for segment in 1..=10 {
                let fraction = segment as f32 / 10.0;
                let next = LogicalPosition::new(
                    (end.x - start.x).mul_add(fraction, start.x),
                    (end.y - start.y).mul_add(fraction, start.y),
                );
                self.glide_to((origin.0 + next.x, origin.1 + next.y))?;
                last = next;
            }
            Ok(())
        })();
        window.dispatch_event(WindowEvent::PointerReleased {
            position: last,
            button: PointerEventButton::Left,
        });
        pump(60);
        moved
    }

    /// Switches to the split view and drags its divider sideways.
    fn compare_drag_divider(
        &mut self,
        compare: &CompareWindow,
        origin: (f32, f32),
    ) -> Result<(), String> {
        let model = compare.global::<CompareModel>();
        let split = targets::run_label(&WindowView::of("CompareWindow", compare), "Split Slider")
            .ok_or("Compare Split Slider control is missing")?;
        self.click_compare(compare.window(), origin, &split)?;
        let divider =
            targets::run_label(&WindowView::of("CompareWindow", compare), "Split Divider")
                .ok_or("Compare Split Divider is missing")?;
        let (x, y) = divider.center();
        let before = model.get_split_fraction();
        self.compare_drag(
            compare.window(),
            origin,
            LogicalPosition::new(x, y),
            LogicalPosition::new(x + 70.0, y),
        )?;
        pump(60);
        if (model.get_split_fraction() - before).abs() < f32::EPSILON {
            return Err("Compare split divider drag did not change its position".into());
        }
        self.rec.hold(Duration::from_millis(450))
    }

    /// Captures the solid baseline, then demonstrates zoom, orbit and the real double-click reset.
    fn compare_orbit_zoom_reset(
        &mut self,
        compare: &CompareWindow,
        origin: (f32, f32),
    ) -> Result<(), String> {
        let picture = targets::run_label(
            &WindowView::of("CompareWindow", compare),
            "Comparison of the Two Stones",
        )
        .ok_or("Compare image area is missing")?;
        let model = compare.global::<CompareModel>();
        if model.get_renderer_index() != 0 {
            return Err(format!(
                "Compare must use Solid before pose readback, renderer={}",
                model.get_renderer_index()
            ));
        }
        let (x, y) = (
            picture.w.mul_add(0.25, picture.x),
            picture.h.mul_add(0.5, picture.y),
        );
        let (before_reset, after_reset) = self.double_click_compare(compare, origin, (x, y))?;
        let baseline = Self::wait_compare_generation_pixels(compare, &before_reset, &after_reset)?;
        self.rec
            .scroll_at_in(compare.window(), (x, y), origin, -120.0)?;
        let zoomed = Self::wait_compare_pixels(compare, Some(&baseline))?;
        if !zoomed.both_differ_from(&baseline) {
            return Err("Compare wheel zoom did not change both Solid images".into());
        }
        self.rec.hold(Duration::from_millis(450))?;

        self.compare_drag(
            compare.window(),
            origin,
            LogicalPosition::new(x, y),
            LogicalPosition::new(x + 52.0, y + 25.0),
        )?;
        let orbited = Self::wait_compare_pixels(compare, Some(&zoomed))?;
        if !orbited.both_differ_from(&zoomed) {
            return Err("Compare orbit drag did not change both Solid images".into());
        }
        self.rec.hold(Duration::from_millis(450))?;
        self.double_click_compare(compare, origin, (x, y))?;
        let reset = Self::wait_compare_pixels_equal(compare, &baseline)?;
        if reset != baseline {
            return Err("Compare double-click did not restore the original Solid images".into());
        }
        self.rec.hold(Duration::from_millis(450))?;
        self.scan_compare_text(compare)
    }

    /// Sends two pointer press/release pairs at one visible point, with no cursor movement
    /// between them; the click ripple appears after the second release.
    fn double_click_compare(
        &mut self,
        compare: &CompareWindow,
        origin: (f32, f32),
        point: (f32, f32),
    ) -> Result<(Image, Image), String> {
        let window = compare.window();
        self.glide_to((origin.0 + point.0, origin.1 + point.1))?;
        self.rec.hold(pacing::BEFORE_CLICK)?;
        let position = LogicalPosition::new(point.0, point.1);
        debug_log::pointer("click", window, point, origin, "double click");
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        pump(20);
        self.rec.hold(Duration::from_millis(80))?;
        // Capture image handles after the first click's delay, immediately before the second
        // click. A new pair afterwards proves the reset gesture generated a new frame generation.
        let model = compare.global::<CompareModel>();
        let before = model.get_before_image();
        let after = model.get_after_image();
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
        pump(20);
        self.rec.click()?;
        pump(60);
        Ok((before, after))
    }

    fn wait_compare_pixels(
        compare: &CompareWindow,
        different_from: Option<&ComparePixels>,
    ) -> Result<ComparePixels, String> {
        let mut previous: Option<ComparePixels> = None;
        let mut stable_samples = 0;
        let mut settled = None;
        let ready = pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let Ok(current) = Self::compare_pixels(compare) else {
                previous = None;
                stable_samples = 0;
                return false;
            };
            if different_from.is_some_and(|baseline| !current.both_differ_from(baseline)) {
                previous = Some(current);
                stable_samples = 0;
                return false;
            }
            if previous.as_ref() == Some(&current) {
                stable_samples += 1;
            } else {
                stable_samples = 0;
            }
            previous = Some(current.clone());
            if stable_samples >= 3 {
                settled = Some(current);
                true
            } else {
                false
            }
        });
        if !ready {
            return Err("Compare Solid images did not reach a stable pixel readback".into());
        }
        settled.ok_or_else(|| "Compare Solid pixel readback disappeared".to_owned())
    }

    fn wait_compare_generation_pixels(
        compare: &CompareWindow,
        previous_before: &Image,
        previous_after: &Image,
    ) -> Result<ComparePixels, String> {
        let model = compare.global::<CompareModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            model.get_before_image() != *previous_before
                && model.get_after_image() != *previous_after
        }) {
            return Err(
                "Compare double-click did not produce a new paired image generation".into(),
            );
        }
        Self::wait_compare_pixels(compare, None)
    }

    fn wait_compare_pixels_equal(
        compare: &CompareWindow,
        expected: &ComparePixels,
    ) -> Result<ComparePixels, String> {
        let mut stable_samples = 0;
        let mut settled = None;
        let ready = pacing::wait_real(pacing::STEP_TIMEOUT, || {
            let Ok(current) = Self::compare_pixels(compare) else {
                stable_samples = 0;
                return false;
            };
            if current == *expected {
                stable_samples += 1;
                if stable_samples >= 3 {
                    settled = Some(current);
                    return true;
                }
            } else {
                stable_samples = 0;
            }
            false
        });
        if !ready {
            return Err("Compare double-click did not restore both baseline images".into());
        }
        settled.ok_or_else(|| "Compare baseline pixel readback disappeared".to_owned())
    }

    fn compare_pixels(compare: &CompareWindow) -> Result<ComparePixels, String> {
        let model = compare.global::<CompareModel>();
        let before = model
            .get_before_image()
            .to_rgba8()
            .ok_or("Compare before image has no readable RGBA pixels")?;
        let after = model
            .get_after_image()
            .to_rgba8()
            .ok_or("Compare after image has no readable RGBA pixels")?;
        if before.width() == 0 || before.height() == 0 || after.width() == 0 || after.height() == 0
        {
            return Err("Compare image pixel buffers are empty".into());
        }
        Ok(ComparePixels {
            before_size: (before.width(), before.height()),
            after_size: (after.width(), after.height()),
            before: before.as_bytes().to_vec(),
            after: after.as_bytes().to_vec(),
        })
    }

    /// Closes the Compare window, then the snapshot table behind it when that is still open.
    fn compare_close(&mut self, compare: &CompareWindow, origin: (f32, f32)) -> Result<(), String> {
        let close = targets::run_label(&WindowView::of("CompareWindow", compare), "Close")
            .ok_or("Compare window Close button is missing")?;
        self.click_compare(compare.window(), origin, &close)?;
        if harness_support::compare_window().is_some()
            || compare.global::<CompareModel>().get_is_open()
        {
            return Err("Compare Close click left the window open".into());
        }
        self.rec.detach_window();
        let table = self.ui.global::<RetargetModel>();
        if table.get_compare_open() {
            let close = targets::run_label(&self.view(), "Close")
                .ok_or("snapshot comparison table Close button is missing")?;
            self.click_and_invoke(&close)?;
            if table.get_compare_open() {
                return Err("snapshot table Close click did not close it".into());
            }
        }
        self.rec.caption("");
        Ok(())
    }

    fn scan_compare_text(&self, compare: &CompareWindow) -> Result<(), String> {
        for text in routes::visible_texts(&WindowView::of("CompareWindow", compare)) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in Compare window ({word:?}): {text:?}"
                ));
            }
        }
        Ok(())
    }
}
