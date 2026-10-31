//! The index-chip and Reset Progress demonstration in the Cutting Mode lesson.
use crate::{director::Film, pacing, targets, targets::Spot};
use pleochroma_cut::CuttingModeModel;
use slint::{ComponentHandle, Model};
use std::time::Duration;
use uiprobe::AccessibleRole;

const SCREEN: &str = "CuttingModeScreen";
const WAIT: Duration = Duration::from_millis(100);

impl Film<'_> {
    /// Opens the real Cutting Mode screen, demonstrates chip undo/redo and reset confirmation,
    /// then leaves the lesson in the editor with every cutting mark cleared.
    #[expect(clippy::too_many_lines, reason = "one filmed route step by step")]
    pub(crate) fn film_cutting_mode_ticks_and_reset(&mut self) -> Result<(), String> {
        let model = self.ui.global::<CuttingModeModel>();
        if !model.get_open() {
            let button = targets::role_label(
                &self.view(),
                "Cut Mode",
                AccessibleRole::Button,
                Some("AnalysisGroup"),
            )
            .ok_or("enabled Cut Mode command button is not visible")?;
            self.click_and_invoke(&button)?;
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            model.get_open() && !model.get_busy() && model.get_error().is_empty()
        }) {
            return Err(format!(
                "Cutting Mode did not become ready: open={}, busy={}, error={:?}",
                model.get_open(),
                model.get_busy(),
                model.get_error()
            ));
        }
        let mut index = usize::try_from(model.get_step_index()).map_err(|_| {
            format!(
                "Cutting Mode exposed negative step index {}",
                model.get_step_index()
            )
        })?;
        let step_count = usize::try_from(model.get_step_count()).unwrap_or(0);
        let initial_done_count = model.get_done_count();
        if index >= step_count || initial_done_count < 1 {
            return Err(format!(
                "expected the saved first mark and a valid current step for the tick lesson: step={}/{step_count}, state={}, done_count={}, progress={:?}",
                index + 1,
                model.get_step_state(),
                initial_done_count,
                model.get_progress_text()
            ));
        }

        // Reopening resumes at the first unfinished tier. Return to the tier marked by the
        // previous lesson action so the manual chip demonstration needs only the two clicks the
        // text describes. Each Left press must move exactly one step without changing progress.
        while index > 0 {
            let before = index;
            self.key("Left")?;
            let expected = before - 1;
            if !pacing::wait_real(pacing::START_TIMEOUT, || {
                usize::try_from(model.get_step_index()).ok() == Some(expected)
                    && model.get_done_count() == initial_done_count
            }) {
                return Err(self.state_error(&format!(
                    "Left from step {} did not move exactly to step {} without changing done_count={initial_done_count}",
                    before + 1,
                    expected + 1
                )));
            }
            index = expected;
            self.hold(WAIT)?;
        }
        if model.get_step_state() != 1 {
            return Err(self.state_error(
                "the previously marked first tier is not done after returning with Left",
            ));
        }
        let chips = model.get_chips();
        if chips.row_count() == 0 || chips.iter().any(|chip| !chip.ticked) {
            return Err(format!(
                "previously marked first tier {} ({}) must have every index ticked; chips={:?}",
                index + 1,
                model.get_tier_name(),
                chips
                    .iter()
                    .map(|chip| (chip.text.to_string(), chip.ticked))
                    .collect::<Vec<_>>()
            ));
        }

        if model.get_done_count() != initial_done_count || !self.chip_ticked(0)? {
            return Err(self.state_error(
                "the previously marked first tier did not reopen with its index ticks intact",
            ));
        }

        // This click is the lesson's requested un-tick. The previously marked step becomes
        // incomplete; restoring this last index tick marks it done again.
        let done_count = model.get_done_count();
        self.click_chip(0, false)?;
        self.wait_step_done(false, done_count - 1)?;
        self.click_chip(0, true)?;
        self.wait_step_done(true, done_count)?;

        let unmark = self.visible_control("Mark Not Done", AccessibleRole::Button)?;
        self.click_and_invoke(&unmark)?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            model.get_step_state() != 1
                && model.get_done_count() == done_count - 1
                && model.get_chips().iter().all(|chip| !chip.ticked)
        }) {
            return Err(self.state_error(
                "Mark Not Done did not clear this step's done mark and all its index ticks",
            ));
        }

        // Leave a partial tick for Reset Progress to clear, and verify the confirmation itself
        // before confirming it with the real visible button.
        self.click_chip(0, true)?;
        let reset_progress = self.visible_control("Reset Progress", AccessibleRole::Button)?;
        self.click_and_invoke(&reset_progress)?;
        if !model.get_confirm_reset() {
            return Err(self.state_error("Reset Progress did not open its confirmation"));
        }
        let reset = self.visible_control("Reset", AccessibleRole::Button)?;
        self.click_and_invoke(&reset)?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            model.get_done_count() == 0
                && (0..model.get_schedule().row_count()).all(|row| {
                    model
                        .get_schedule()
                        .row_data(row)
                        .is_some_and(|step| step.state == 0)
                })
                && model.get_chips().iter().all(|chip| !chip.ticked)
        }) {
            return Err(self.state_error(
                "confirming Reset did not clear all done marks and current index ticks",
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.key("Escape")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || !model.get_open()) {
            return Err(self.state_error("Escape did not close Cutting Mode after reset"));
        }
        Ok(())
    }

    fn click_chip(&mut self, index: usize, ticked: bool) -> Result<(), String> {
        let row = self
            .ui
            .global::<CuttingModeModel>()
            .get_chips()
            .row_data(index)
            .ok_or_else(|| format!("Cutting Mode chip {index} disappeared"))?;
        let label = format!("Index {}", row.text);
        let spot = self.reveal_chip(&label)?;
        self.click_and_invoke(&spot)?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            self.ui
                .global::<CuttingModeModel>()
                .get_chips()
                .row_data(index)
                .is_some_and(|chip| chip.ticked == ticked)
        }) {
            return Err(self.state_error(&format!(
                "real click on {label:?} did not set ticked={ticked}"
            )));
        }
        self.hold(WAIT)
    }

    fn chip_ticked(&self, index: usize) -> Result<bool, String> {
        self.ui
            .global::<CuttingModeModel>()
            .get_chips()
            .row_data(index)
            .map(|chip| chip.ticked)
            .ok_or_else(|| format!("Cutting Mode chip {index} is missing"))
    }

    fn wait_step_done(&self, done: bool, count: i32) -> Result<(), String> {
        let model = self.ui.global::<CuttingModeModel>();
        if pacing::wait_real(pacing::START_TIMEOUT, || {
            (model.get_step_state() == 1) == done && model.get_done_count() == count
        }) {
            Ok(())
        } else {
            Err(self.state_error(&format!(
                "expected step_done={done}, done_count={count} after the chip click"
            )))
        }
    }

    fn visible_control(&self, label: &str, role: AccessibleRole) -> Result<Spot, String> {
        let view = self.view();
        let screen = view
            .root
            .query_descendants()
            .match_inherits(SCREEN)
            .find_first()
            .ok_or("CuttingModeScreen is not mounted")?;
        let wanted = label.to_owned();
        let spots: Vec<_> = screen
            .query_descendants()
            .match_accessible_role(role)
            .match_predicate(move |element| {
                element
                    .accessible_label()
                    .is_some_and(|name| name == wanted)
                    && element.accessible_enabled() != Some(false)
            })
            .find_all()
            .iter()
            .map(Spot::of)
            .filter(|spot| spot.usable(view.size))
            .collect();
        match spots.as_slice() {
            [spot] => Ok(spot.clone()),
            _ => Err(format!(
                "CuttingModeScreen control {label:?} ({role:?}) has {} visible matches",
                spots.len()
            )),
        }
    }

    /// Scrolls only the step-card viewport until the named checkbox has a hit point in its
    /// actual clipped area. In wide layout the first `ScrollView` is the step page; in narrow
    /// layout it is the page's one outer scroll area.
    fn reveal_chip(&mut self, label: &str) -> Result<Spot, String> {
        let viewport = self.chip_viewport()?;
        self.wheel_over(&viewport, 3000.0)?;
        for _ in 0..8 {
            let view = self.view();
            let screen = view
                .root
                .query_descendants()
                .match_inherits(SCREEN)
                .find_first()
                .ok_or("CuttingModeScreen disappeared while scrolling to an index")?;
            let wanted = label.to_owned();
            let spots: Vec<_> = screen
                .query_descendants()
                .match_accessible_role(AccessibleRole::Checkbox)
                .match_predicate(move |element| {
                    element
                        .accessible_label()
                        .is_some_and(|name| name == wanted)
                        && element.accessible_enabled() != Some(false)
                })
                .find_all()
                .iter()
                .map(Spot::of)
                .collect();
            if spots.len() != 1 {
                return Err(format!(
                    "CuttingModeScreen checkbox {label:?} has {} accessible matches",
                    spots.len()
                ));
            }
            if let Some(clipped) = spots[0].clipped_to(&viewport, view.size) {
                return Ok(clipped);
            }
            let delta = if spots[0].y + spots[0].h > viewport.y + viewport.h {
                -450.0
            } else {
                450.0
            };
            self.wheel_over(&viewport, delta)?;
        }
        Err(format!(
            "index checkbox {label:?} stayed outside the Cutting Mode step viewport after scrolling"
        ))
    }

    fn chip_viewport(&self) -> Result<Spot, String> {
        let view = self.view();
        let screen = view
            .root
            .query_descendants()
            .match_inherits(SCREEN)
            .find_first()
            .ok_or("CuttingModeScreen is not mounted")?;
        screen
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .map(|element| Spot::of(&element))
            .filter(|spot| spot.usable(view.size))
            .ok_or_else(|| "Cutting Mode step scroll viewport is missing or clipped".to_owned())
    }

    fn state_error(&self, reason: &str) -> String {
        let model = self.ui.global::<CuttingModeModel>();
        format!(
            "{reason}: open={}, busy={}, error={:?}, step={}/{}, tier={}, step_state={}, done_count={}, progress={:?}",
            model.get_open(),
            model.get_busy(),
            model.get_error(),
            model.get_step_index().saturating_add(1),
            model.get_step_count(),
            model.get_tier_name(),
            model.get_step_state(),
            model.get_done_count(),
            model.get_progress_text()
        )
    }
}
