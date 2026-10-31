//! Visible routes of the Rough Color lessons: opening the wizard from a saved plan, and the two
//! planner controls that need a saved plan with a rough color (Best Color, Use Color).
//!
//! A rough color is made by the wizard from photographs of a rough on a camera rig, which no video
//! can film. Before the app launches for a `library-rough-color-*` lesson the harness therefore
//! writes a saved plan that already has one (`support::rough_color_seed`: a C-shaped mesh rough,
//! two layouts, a fixed two-zone fit and two synthetic photos, stored as the planner's Save and
//! the wizard's Accept store them). The routes open it from Saved Plans, as a learner would: the
//! wizard then restores the stored photos and fit, and the planner draws the stones in the color
//! and offers Stone Orientation and Use Color. A missing seed stops the video with a sentence.
//!
//! The wizard lesson's reading steps walk the pages they describe (`film_rough_color_pages`): the
//! restored plan opens only Compare and Accept, so the first of those steps also fills in the
//! rig, its alignment, two photo files and the calibration a capture session leaves behind
//! (`harness_support::prime_rough_color_wizard`), with small synthetic photos written for the
//! Photos page. The pages are then opened by clicking their pills in the step strip.

use crate::{
    director::Film,
    pacing, routes,
    scripts::pure_tables::output_path,
    support::{WindowView, app::pump, rough_color_seed},
    targets::{self, Spot},
};
use pleochroma_cut::{
    EditorModel, GuideModel, RoughPlanModel, RoughPlannerWindow, Zoning, harness_support,
};
use slint::{ComponentHandle, Model};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use uiprobe::{AccessibleRole, screenshot};

/// How many wheel turns a control may need to scroll into view in the planner's left column.
const SCROLL_TRIES: usize = 24;

/// The label of every pill of the wizard's step strip, in the order of the pages (the number, then
/// the step's title).
const WIZARD_PAGES: [&str; 8] = [
    "1 Checklist",
    "2 Photos",
    "3 Calibration",
    "4 Masks",
    "5 Surfaces",
    "6 Fit",
    "7 Compare",
    "8 Accept",
];

/// The caption shown while each page of the wizard is on screen.
const WIZARD_CAPTIONS: [&str; 8] = [
    "Checklist: pick the camera rig and see whether the mesh is aligned to it.",
    "Photos: a stone photo and a white frame for each view.",
    "Calibration: how the camera and the backlight are known.",
    "Masks: pixels that do not show the stone are left out of the fit.",
    "Surfaces: a polished or a frosted skin of the rough.",
    "Fit: one color for the whole stone, or one for each zone.",
    "Compare: the photo, the render and the difference.",
    "Accept: store the result with the saved plan.",
];

/// How many views get a synthetic stone photo and white frame for the Photos page.
const PHOTO_VIEWS: usize = 2;

/// The size of a synthetic photo, pixels.
const PHOTO_SIZE: (u32, u32) = (64, 48);

/// Writes a small synthetic backlit stone photo (one violet color with a soft vignette) and a
/// white frame (the same vignette in grey) for each of the first views into `folder`, and returns
/// their paths as `(stone, white)` pairs. The wizard only reads them if Prepare Photos is pressed.
fn write_synthetic_photos(folder: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    std::fs::create_dir_all(folder)
        .map_err(|error| format!("cannot create the folder of the synthetic photos: {error}"))?;
    (1..=PHOTO_VIEWS)
        .map(|view| {
            let stone = folder.join(format!("view-{view}-stone.png"));
            let white = folder.join(format!("view-{view}-white.png"));
            write_synthetic_photo(&stone, [0.42, 0.26, 0.55], view)?;
            write_synthetic_photo(&white, [0.92, 0.92, 0.92], view)?;
            Ok((stone, white))
        })
        .collect()
}

/// One synthetic photo: `colour` (linear RGB) with a vignette that is a little darker for every
/// later `view`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "each channel is clamped to 0..=255 before it becomes a byte"
)]
fn write_synthetic_photo(path: &Path, colour: [f32; 3], view: usize) -> Result<(), String> {
    let (width, height) = PHOTO_SIZE;
    let shade = (-0.04_f32).mul_add(view as f32, 1.04);
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        let dx = (x as f32 + 0.5) / width as f32 - 0.5;
        let dy = (y as f32 + 0.5) / height as f32 - 0.5;
        let falloff = (-0.6_f32).mul_add(dx.mul_add(dx, dy * dy), 1.0);
        image::Rgb(
            colour.map(|channel| (255.0 * channel * falloff * shade).clamp(0.0, 255.0) as u8),
        )
    });
    image
        .save(path)
        .map_err(|error| format!("cannot write a synthetic photo: {error}"))
}

impl Film<'_> {
    /// The Rough Planner window and its screen origin, opening and attaching it first when it is
    /// not open (the lessons before this step only read).
    fn rough_color_planner(&mut self) -> Result<(RoughPlannerWindow, (f32, f32)), String> {
        if harness_support::rough_planner_window().is_none() {
            self.film_rough_open()?;
        }
        let rough =
            harness_support::rough_planner_window().ok_or("Rough Planner window is missing")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("Rough Planner is not attached to the video")?;
        Ok((rough, origin))
    }

    /// A control of the planner's left column, scrolled fully into view.
    fn rough_color_control(
        &mut self,
        window: &slint::Window,
        origin: (f32, f32),
        label: &str,
        role: AccessibleRole,
    ) -> Result<Spot, String> {
        let planner = || harness_support::rough_planner_window().ok_or("Rough Planner disappeared");
        let left_scroll = targets::Query::Id("RoughPlannerWindow::left_scroll");
        let view = WindowView::of("RoughPlannerWindow", &planner()?);
        let viewport = targets::run(&view, &left_scroll)
            .ok_or("Rough Planner rough/shape scroll viewport is missing")?;
        self.rec
            .scroll_at_in(window, viewport.center(), origin, 10_000.0)?;
        self.hold(Duration::from_millis(140))?;
        for _ in 0..SCROLL_TRIES {
            let view = WindowView::of("RoughPlannerWindow", &planner()?);
            let viewport = targets::run(&view, &left_scroll)
                .ok_or("Rough Planner rough/shape scroll viewport is missing")?;
            let control = targets::role_label(&view, label, role, None);
            if let Some(clipped) = control
                .as_ref()
                .and_then(|spot| spot.clipped_to(&viewport, view.size))
            {
                return Ok(clipped);
            }
            let delta =
                control.as_ref().map_or(
                    -260.0,
                    |spot| if spot.y < viewport.y { 220.0 } else { -260.0 },
                );
            self.rec
                .scroll_at_in(window, viewport.center(), origin, delta)?;
            self.hold(Duration::from_millis(140))?;
        }
        Err(format!(
            "Rough Planner control {label:?} did not enter the left scroll viewport"
        ))
    }

    /// A visible, enabled title-bar or list button of the planner window, by its exact label.
    fn rough_color_button(rough: &RoughPlannerWindow, label: &str) -> Result<Spot, String> {
        let view = WindowView::of("RoughPlannerWindow", rough);
        targets::role_label(&view, label, AccessibleRole::Button, None)
            .ok_or_else(|| format!("the Rough Planner button {label:?} is not visible"))
    }

    /// Opens the seeded plan from Saved Plans, as a learner would, unless the planner already
    /// shows a plan with a rough color; returns the planner and its screen origin.
    ///
    /// # Errors
    ///
    /// The seed is not in the list (the harness writes it before the app launches for these
    /// lessons), a click did nothing, or the opened plan shows no rough color.
    fn open_seeded_plan(&mut self, what: &str) -> Result<(RoughPlannerWindow, (f32, f32)), String> {
        let (rough, origin) = self.rough_color_planner()?;
        if rough.global::<Zoning>().get_colour_available() {
            return Ok((rough, origin));
        }
        let model = rough.global::<RoughPlanModel>();
        let name = rough_color_seed::plan_name();
        // The list is read on a worker thread when the window is built.
        let first_row = || {
            model
                .get_saved_plans()
                .row_data(0)
                .map(|row| row.name.to_string())
        };
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || first_row().is_some()) {
            return Err(format!(
                "{what}: Saved Plans is empty; the plan {name:?} with a rough color is written \
                 into the library before the app launches for the library-rough-color lessons, \
                 and it is missing"
            ));
        }
        if first_row().as_deref() != Some(name) {
            return Err(format!(
                "{what}: the newest saved plan is {:?}, not the seeded {name:?}",
                first_row().unwrap_or_default()
            ));
        }
        let saved = Self::rough_color_button(&rough, "Saved Plans")?;
        self.click_secondary(rough.window(), origin, &saved)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || model.get_saved_open()) {
            return Err(format!("{what}: Saved Plans did not open the list"));
        }
        self.hold(pacing::RESULT_HOLD)?;
        let open = Self::rough_color_button(&rough, "Open")?;
        self.click_secondary(rough.window(), origin, &open)?;
        // The banner and the closing of the list wait for the rows of the opened plan.
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !model.get_loaded_banner().is_empty() && !model.get_saved_open()
        }) {
            return Err(format!(
                "{what}: the saved plan did not open: {:?}",
                model.get_error_text()
            ));
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            rough.global::<Zoning>().get_colour_available()
        }) {
            return Err(format!(
                "{what}: the opened plan {name:?} shows no rough color: {:?} {:?}",
                model.get_status_text(),
                model.get_error_text()
            ));
        }
        self.rec
            .caption("A saved plan that already has a rough color.");
        self.hold(pacing::RESULT_HOLD)?;
        Ok((rough, origin))
    }

    /// Step "Open the wizard": the seeded plan, whose rough is a mesh, is opened from Saved Plans
    /// and the Rough Color... button opens the wizard, which restores the plan's stored photos and
    /// fit and then replaces the planner in the video.
    pub(crate) fn film_rough_color_open(&mut self) -> Result<(), String> {
        let (rough, origin) = self.open_seeded_plan("rough_color_wizard_opened")?;
        let model = rough.global::<RoughPlanModel>();
        let window = rough.window();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            !model.get_mesh_busy() && model.get_locate_possible()
        }) {
            return Err(format!(
                "the mesh rough of the seeded plan did not load: busy={}, locate_possible={}, error={:?}",
                model.get_mesh_busy(),
                model.get_locate_possible(),
                model.get_model_error()
            ));
        }

        let button =
            self.rough_color_control(window, origin, "Rough Color...", AccessibleRole::Button)?;
        self.click_secondary(window, origin, &button)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::rough_color_window().is_some()
        }) {
            return Err("Rough Color... did not open the wizard window".into());
        }
        let wizard =
            harness_support::rough_color_window().ok_or("Rough Color window disappeared")?;
        screenshot::set_window_size(wizard.window(), (1100, 780), crate::config::SCALE);
        // The wizard loads the stored colour on a worker thread; let it settle before the picture.
        self.hold(pacing::RESULT_HOLD)?;
        self.rough_color_privacy_scan(&WindowView::of("RoughColourWindow", &wizard))?;
        // Only one window is in the picture at a time: the wizard takes the planner's place.
        self.rec.detach_window();
        self.attached_window = None;
        let origin = self.rec.place_window(wizard.window());
        self.rec.attach_window(&wizard, origin);
        self.attached_window = Some("rough color wizard");
        self.rec.chapter("Rough Color");
        self.rec
            .caption("Photos of the rough become the color of the stone.");
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    /// Fails when a text on screen in the wizard names something private (a user or a path).
    fn rough_color_privacy_scan(&self, view: &WindowView) -> Result<(), String> {
        for text in routes::visible_texts(view) {
            if let Some(word) = self.cfg.private_hit(&text) {
                return Err(format!(
                    "private text in the Rough Color wizard ({word:?}): {text:?}"
                ));
            }
        }
        Ok(())
    }

    /// Makes every page of the wizard's strip reachable (once): the wizard opened from the seeded
    /// plan restores its stored photos and fit, which opens only Compare and Accept, so the rig,
    /// its alignment, the photo files and the calibration a capture session leaves behind are
    /// filled in (`harness_support::prime_rough_color_wizard`). The stored fit stays. Two small
    /// synthetic stone photos and white frames are written for the Photos page.
    fn rough_color_prime(&self) -> Result<(), String> {
        let wizard =
            harness_support::rough_color_window().ok_or("the Rough Color wizard is not open")?;
        if wizard.get_step_ok().iter().all(|open| open) {
            return Ok(());
        }
        let photos = write_synthetic_photos(&output_path(&self.id, "rough-color-photos"))?;
        harness_support::prime_rough_color_wizard(&photos)?;
        pump(100);
        let open: Vec<bool> = wizard.get_step_ok().iter().collect();
        if open.len() != WIZARD_PAGES.len() || !open.iter().all(|open| *open) {
            return Err(format!(
                "the wizard still locks steps after the capture state was filled in: {open:?}"
            ));
        }
        Ok(())
    }

    /// Opens the pages `pages` (0-based, in the order given) of the wizard by clicking their pills
    /// in the step strip, and holds each one on screen. The steps of the lesson that describe the
    /// pages (Photos and calibration, Masks and surfaces, Zones and the fit, Read the result, Keep
    /// the result) each call this with their own pages.
    pub(crate) fn film_rough_color_pages(&mut self, pages: &[usize]) -> Result<(), String> {
        self.rough_color_prime()?;
        let wizard =
            harness_support::rough_color_window().ok_or("the Rough Color wizard is not open")?;
        let origin = self
            .rec
            .attached_origin()
            .ok_or("the Rough Color wizard is not attached to the video")?;
        for &page in pages {
            let label = WIZARD_PAGES
                .get(page)
                .ok_or_else(|| format!("the wizard has no page {page}"))?;
            let wanted = i32::try_from(page).map_err(|_| "page index out of range")?;
            let view = WindowView::of("RoughColourWindow", &wizard);
            let pill = targets::role_label(&view, label, AccessibleRole::Button, None)
                .ok_or_else(|| format!("the step pill {label:?} is not on screen or is locked"))?;
            self.click_secondary(wizard.window(), origin, &pill)?;
            if !pacing::wait_real(pacing::STEP_TIMEOUT, || wizard.get_step() == wanted) {
                return Err(format!(
                    "the wizard did not open {label:?}: it shows page {} ({:?})",
                    wizard.get_step() + 1,
                    wizard.get_error_text()
                ));
            }
            // The page draws its pictures and lists on the next pushes.
            pump(100);
            self.rough_color_privacy_scan(&WindowView::of("RoughColourWindow", &wizard))?;
            self.rec.caption(WIZARD_CAPTIONS[page]);
            self.hold(pacing::RESULT_HOLD)?;
            // Debug mode: a picture of every page, not only of the step's last one.
            let slug = label.to_lowercase().replace(' ', "-");
            crate::debug_log::page_shot(&format!("page-{slug}"), "rough-color");
        }
        Ok(())
    }

    /// Step "Choose Best Color": the Best Color pill under Stone Orientation, on the seeded plan.
    pub(crate) fn film_rough_color_best(&mut self) -> Result<(), String> {
        let (rough, origin) = self.open_seeded_plan("rough_color_best_chosen")?;
        let view = WindowView::of("RoughPlannerWindow", &rough);
        let pill = targets::run_label(&view, "Best Color")
            .ok_or("the Best Color pill is not visible (are there results on screen?)")?;
        self.click_secondary(rough.window(), origin, &pill)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            rough.global::<Zoning>().get_pose_option() == 1
        }) {
            return Err("Best Color did not become the stone orientation".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }

    /// Step "Use the color of a design": Use Color on the first design row, then the planner
    /// leaves the video so the editor shows the design with its color.
    pub(crate) fn film_rough_color_use(&mut self) -> Result<(), String> {
        let (rough, origin) = self.open_seeded_plan("rough_color_used")?;
        let view = WindowView::of("RoughPlannerWindow", &rough);
        let pill = targets::run_label_start(&view, "Use the Color of")
            .ok_or("no Use Color button is visible on a design row")?;
        self.click_secondary(rough.window(), origin, &pill)?;
        self.hold(Duration::from_millis(500))?;
        if self.ui.global::<EditorModel>().get_unsaved_dialog_open() {
            return Err("Use Color raised an unsaved-changes question".into());
        }
        let guide = self.ui.global::<GuideModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || guide.get_step_done()) {
            return Err(format!(
                "the color of the design did not reach the editor: toast {:?}",
                self.ui.get_toast_message()
            ));
        }
        // The main window shows the design now; the planner is not part of the picture any more.
        self.rec.detach_window();
        self.attached_window = None;
        self.hold(pacing::RESULT_HOLD)?;
        Ok(())
    }
}
