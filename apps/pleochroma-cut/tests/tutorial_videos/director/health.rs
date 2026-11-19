//! The health check after every step: what makes a video dishonest.

use super::Film;
use crate::{
    pacing,
    perform_actions::AUTO_SOLVE_SETTLE,
    routes,
    support::{WindowView, app::pump},
};
use pleochroma_cut::{
    EditorModel, SolidPreviewModel, VerdictModel, ViewportModel, harness_support,
};
use slint::ComponentHandle;
use std::time::{Duration, Instant};
use uiprobe::take_panics;

/// How long the camera pose must stay unchanged before it counts as settled.
const CAMERA_STILL: Duration = Duration::from_millis(400);
/// The least time a started path trace is waited for before "it never started" is believed.
const TRACE_START_GRACE: Duration = Duration::from_secs(2);

impl Film<'_> {
    /// Waits, in real time (a cut in a video), for what the step just did to finish: the auto-solve
    /// its edit armed (its debounce runs on mock time, so that is pumped first) and the solve it
    /// started, then the traced picture of a path or ray view, which reads "Nothing rendered yet"
    /// until its first frame and noisy until its samples are in. Every wait has a cap; a timeout
    /// is not an error here, the picture is then taken as it is.
    pub(crate) fn settle_solve_and_render(&self) {
        let ui = self.ui;
        let editor = ui.global::<EditorModel>();
        let debounce = u64::try_from(AUTO_SOLVE_SETTLE.as_millis()).unwrap_or(350);
        for _ in 0..2 {
            pump(debounce);
            let _ = pacing::wait_real(Duration::from_secs(20), || !editor.get_solve_running());
        }
        // A tick after the solve landed: the solid worker and the activity strip catch up.
        pump(100);
        // The verdict (badge, Bril / Wind / Ext figures) is worked out by a worker after the
        // solve; a design with no verdict to wait for (or one left "Not solved", whose badge
        // stays dimmed) is held up for the short cap only.
        let verdict = ui.global::<VerdictModel>();
        let _ = pacing::wait_real(Duration::from_secs(4), || {
            !editor.get_solve_running() && !verdict.get_busy() && !verdict.get_stale()
        });
        let viewport = ui.global::<ViewportModel>();
        let edit = ui.get_active_tab() == 0 && ui.get_render_view_tab() == 1;
        let live = ui.get_active_tab() == 0 && ui.get_render_view_tab() != 1;
        let traced = if edit {
            matches!(
                ui.global::<SolidPreviewModel>().get_view_mode(),
                1 | 2 | 4 | 5
            )
        } else {
            live && matches!(viewport.get_live_view_mode(), 1 | 2)
        };
        if traced && editor.get_has_design() {
            let start = Instant::now();
            let mut seen_running = false;
            let _ = pacing::wait_real(pacing::STEP_TIMEOUT, || {
                let running = routes::trace_running(ui);
                seen_running |= running;
                let age = start.elapsed();
                !viewport.get_trace_refusal().is_empty()
                    || (viewport.get_has_render()
                        && !running
                        && ((seen_running && age >= Duration::from_millis(300))
                            || age >= TRACE_START_GRACE))
            });
            pump(300);
        }
    }

    /// Waits, in real time, until the camera pose and the fit distance of the stone stay
    /// unchanged for [`CAMERA_STILL`] (a re-fit after a solve, a drag still being shown).
    pub(crate) fn settle_camera(&self) {
        if !self.ui.global::<EditorModel>().get_has_design() {
            return;
        }
        let mut last: Option<(f32, f32, f32, f32)> = None;
        let mut since = Instant::now();
        let _ = pacing::wait_real(Duration::from_secs(3), || {
            let (yaw, pitch, distance) = harness_support::camera_pose();
            let now = (yaw, pitch, distance, harness_support::fit_distance_now());
            if last != Some(now) {
                last = Some(now);
                since = Instant::now();
                return false;
            }
            since.elapsed() >= CAMERA_STILL
        });
    }

    /// Abandons the video on a panic, an unexpected file dialog, an error or warning toast, or
    /// private text on screen.
    pub(super) fn health(&self, label: &str) -> Result<(), String> {
        let panics = take_panics();
        if !panics.is_empty() {
            return Err(format!("{label}: panic: {}", panics.join("; ")));
        }
        let dialogs = harness_support::take_picker_failures();
        if !dialogs.is_empty() {
            return Err(format!(
                "{label}: unexpected file dialog: {}",
                dialogs.join("; ")
            ));
        }
        if self.ui.get_toast_visible() {
            let kind = self.ui.get_toast_type().to_string();
            if kind == "error" || kind == "warning" {
                return Err(format!(
                    "{label}: {kind} toast: {}",
                    self.ui.get_toast_message()
                ));
            }
        }
        let mut windows = vec![("main", routes::visible_texts(&self.view()))];
        if let Some(help) = harness_support::help_window() {
            windows.push((
                "Help",
                routes::visible_texts(&WindowView::of("HelpWindow", &help)),
            ));
        }
        if let Some(rough) = harness_support::rough_planner_window() {
            windows.push((
                "Rough Planner",
                routes::visible_texts(&WindowView::of("RoughPlannerWindow", &rough)),
            ));
        }
        for (window, texts) in windows {
            for text in texts {
                if let Some(word) = self.cfg.private_hit(&text) {
                    return Err(format!(
                        "{label}: private text in the {window} window ({word:?}): {text:?}"
                    ));
                }
            }
        }
        Ok(())
    }
}
