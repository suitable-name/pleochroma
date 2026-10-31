//! Filmed routes of the Two Tiers sweep, Keep Trials and Tune by Hand lessons, kept out of the
//! registry in `scripts.rs` so that its match stays a list of arms.
//!
//! Every route clicks the controls the lesson names, with the real pointer, and then reads one
//! model value back, so a renamed button or a view that did not open stops the video with a
//! sentence instead of filming something else.

use crate::{
    director::Film,
    pacing,
    support::app::pump,
    targets::{self, Spot},
};
use pleochroma_cut::{SweepModel, TuneModel};
use slint::ComponentHandle;
use std::time::Duration;

/// How long a two-tier sweep of the default grid (81 pairs) may take.
const SWEEP_TIMEOUT: Duration = Duration::from_secs(240);

/// How many wheel turns bring a row or a button of the Trials view into its visible area.
const TRIALS_REVEAL_TRIES: usize = 12;

/// The arrow keys that may move the selection off the design's own pair, in the order they are
/// tried: the first neighbor that gives a stone is used.
const NEIGHBOR_KEYS: [&str; 4] = ["Right", "Down", "Left", "Up"];

impl Film<'_> {
    /// Angle Sweep, Two Tiers: runs the default grid, selects a pair other than the design's own
    /// with the arrow keys, uses it and closes the dialog.
    pub(super) fn film_two_tier_sweep(&mut self) -> Result<(), String> {
        self.open_sweep()?;
        let sweep = self.ui.global::<SweepModel>();
        let pill = self.scoped_button("SweepDialog", "Two Tiers")?;
        self.click_and_invoke(&pill)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            sweep.get_mode() == 1 && sweep.get_form_ok_2d()
        }) {
            return Err(format!(
                "the Two Tiers form did not open or cannot run: {:?}",
                sweep.get_cells_summary()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        let run = self.scoped_button("SweepDialog", "Run")?;
        self.click_and_invoke(&run)?;
        if !pacing::wait_real(SWEEP_TIMEOUT, || {
            !sweep.get_running() && sweep.get_has_results_2d()
        }) {
            return Err("the two-tier sweep did not finish in time".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        // A click on the middle of the map selects the cell under it (and gives the map the
        // keyboard); the arrow keys then move off the design's own pair.
        let map = self.scoped_button("SweepDialog", "Angle Pair on the Heat Map")?;
        self.click_and_invoke(&map)?;
        for key in NEIGHBOR_KEYS {
            if sweep.get_cells_usable() {
                break;
            }
            self.key(key)?;
            self.hold(Duration::from_millis(250))?;
        }
        if !sweep.get_cells_usable() {
            return Err("no neighboring angle pair can be used".into());
        }
        self.hold(pacing::RESULT_HOLD)?;
        let use_pair = self.scoped_button("SweepDialog", "Use These Angles")?;
        self.click_and_invoke(&use_pair)?;
        self.hold(pacing::RESULT_HOLD)?;
        let close = self.scoped_button("SweepDialog", "Close")?;
        self.click_and_invoke(&close)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !sweep.get_dialog_open()) {
            return Err("Close did not close the Angle Sweep dialog".into());
        }
        Ok(())
    }

    /// Keep as Trial: the button the guide outlines (the strip's, or the Trials view's).
    pub(super) fn film_keep_trial(&mut self) -> Result<(), String> {
        let targets::Resolved::Found(button) = targets::resolve(&self.view(), "keep_trial_button")
        else {
            return Err("the Keep as Trial button is not visible".into());
        };
        self.click_and_invoke(&button)?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// The History tab, then its Trials view.
    pub(super) fn film_open_trials_view(&mut self) -> Result<(), String> {
        self.film_inspector_tab(4)?;
        self.press_label("Trials")
    }

    /// The Trials view scrolls as a whole when the Inspector is short (the list keeps two rows
    /// and the buttons of a chosen trial sit below it), so a row or a button can lie under the
    /// dock's lower edge. Turns the wheel over the view's own scroll bar, as a learner would,
    /// until the control found by `find` lies wholly inside the view's visible area, and returns
    /// where it is then.
    fn reveal_in_trials_view(
        &mut self,
        what: &str,
        find: &dyn Fn(&Self) -> Option<Spot>,
    ) -> Result<Spot, String> {
        let mut seen_top = false;
        for _ in 0..TRIALS_REVEAL_TRIES {
            let view = self.view();
            let scroll = targets::run(&view, &targets::Query::Id("TrialsView::trials_scroll"))
                .ok_or("the Trials view's scroll area is missing")?;
            let dock = targets::run(&view, &targets::Query::Id("EditorView::dock_scroll"))
                .ok_or("editor dock scroll area is missing")?;
            let visible = scroll
                .clipped_to(&dock, view.size)
                .ok_or("the Trials view is outside the editor dock")?;
            let found = find(self);
            let delta = if let Some(spot) = &found {
                if spot.y >= visible.y - 1.0 && spot.y + spot.h <= visible.y + visible.h + 1.0 {
                    return Ok(spot.clone());
                }
                -(spot.center().1 - visible.center().1).clamp(-160.0, 160.0)
            } else if seen_top {
                // Not in the tree: it is further down than what the view shows now.
                -(visible.h * 0.65).clamp(24.0, 120.0)
            } else {
                seen_top = true;
                3000.0
            };
            // The scroll bar is the one safe place to turn the wheel: over the list the wheel
            // would move the list instead.
            let bar = scroll.handle.as_ref().and_then(|handle| {
                handle
                    .query_descendants()
                    .match_inherits("ScrollBar")
                    .find_all()
                    .into_iter()
                    .map(|element| Spot::of(&element))
                    .filter(|bar| bar.h > bar.w && bar.h >= visible.h * 0.65)
                    .filter_map(|bar| bar.clipped_to(&visible, view.size))
                    .max_by(|left, right| left.x.total_cmp(&right.x))
            });
            let target = bar.unwrap_or_else(|| Spot {
                x: visible.x + 2.0,
                y: visible.y,
                w: 2.0,
                h: visible.h,
                handle: None,
            });
            self.wheel_over(&target, delta)?;
            pump(100);
            self.hold(Duration::from_millis(180))?;
        }
        Err(format!(
            "{what} did not come wholly into view of the Trials view in {TRIALS_REVEAL_TRIES} wheel turns"
        ))
    }

    /// A click on the row of trial `number` in the Trials view.
    fn film_trial_row(&mut self, number: u32) -> Result<(), String> {
        let prefix = format!("Trial {number},");
        let row = self.reveal_in_trials_view(&format!("the row of Trial {number}"), &|film| {
            targets::run_label_start(&film.view(), &prefix)
        })?;
        self.click_and_invoke(&row)?;
        self.hold(Duration::from_millis(400))
    }

    /// The button `label` of the chosen trial's detail row, scrolled wholly into view.
    fn trial_detail_button(&mut self, label: &str) -> Result<Spot, String> {
        self.reveal_in_trials_view(&format!("{label:?} in TrialsView"), &|film| {
            let mut spots = film.scoped_spots("TrialsView", label);
            if spots.len() == 1 { spots.pop() } else { None }
        })
    }

    /// Restore: picks the row of trial `number`, then presses Restore under the table.
    pub(super) fn film_restore_trial(&mut self, number: u32) -> Result<(), String> {
        self.film_trial_row(number)?;
        let restore = self.trial_detail_button("Restore")?;
        self.click_and_invoke(&restore)
    }

    /// Set as Baseline: picks the row of trial `number`, then presses Set as Baseline under the
    /// table (the strip has a button of the same name, so the Trials view is asked for it).
    pub(super) fn film_trial_to_baseline(&mut self, number: u32) -> Result<(), String> {
        self.film_trial_row(number)?;
        let baseline = self.trial_detail_button("Set as Baseline")?;
        self.click_and_invoke(&baseline)
    }

    /// The Optimize tab, then its Tune view.
    pub(super) fn film_open_tune_view(&mut self) -> Result<(), String> {
        self.film_inspector_tab(2)?;
        self.press_label("Tune")?;
        let tune = self.ui.global::<TuneModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            tune.get_active() && tune.get_has_design()
        }) {
            return Err("the Tune view did not open on a design".into());
        }
        Ok(())
    }

    /// The row of the Tune list for tier `name`: the list rows read "code name, angle", the tier
    /// table's rows start with "Tier", so the two cannot be mixed up.
    fn tune_row(&self, name: &str) -> Result<Spot, String> {
        let view = self.view();
        let tab = view
            .root
            .query_descendants()
            .match_inherits("TuneTab")
            .find_first()
            .ok_or("the Tune tab is not on screen")?;
        tab.query_descendants()
            .find_all()
            .into_iter()
            .filter(|element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label.contains(name) && !label.starts_with("Tier"))
            })
            .map(|element| Spot::of(&element))
            .find(|spot| spot.usable(view.size))
            .ok_or_else(|| format!("the Tune list has no visible row for {name:?}"))
    }

    /// Clicks the Tune row of tier `name` and waits for the title to name it.
    pub(super) fn film_tune_pick(&mut self, name: &str) -> Result<(), String> {
        let row = self.tune_row(name)?;
        self.click_and_invoke(&row)?;
        let tune = self.ui.global::<TuneModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            // The title is set in capitals ("PAVILION MAIN (P1) 40.00°").
            tune.get_has_selection()
                && tune
                    .get_title()
                    .to_uppercase()
                    .contains(&name.to_uppercase())
        }) {
            return Err(format!(
                "the Tune title did not name {name:?}: {:?}",
                tune.get_title()
            ));
        }
        self.hold(Duration::from_millis(300))
    }

    /// The Tune tab's button `label`, scrolled into the inspector's view first: with the mini
    /// sweep open, the nudges and Accept sit below the fold of a short inspector.
    fn tune_button(&mut self, label: &str) -> Result<Spot, String> {
        if let Some(spot) = self.inspector_form_control(label, uiprobe::AccessibleRole::Button)? {
            return Ok(spot);
        }
        self.scoped_button("TuneTab", label)
    }

    /// Scrolls the guide target `target` into view when its control sits below the fold of the
    /// Tune tab (the metric curves and the legend make the tab taller than the inspector, and the
    /// picked tier's nudges, Pair With and Accept are the last rows). Returns whether `target` is
    /// one this can reveal, so the caller asks for it again.
    pub(crate) fn film_reveal_target(&mut self, target: &str) -> Result<bool, String> {
        if target != "tune_accept_button" {
            return Ok(false);
        }
        self.inspector_form_control("Accept", uiprobe::AccessibleRole::Button)?;
        Ok(true)
    }

    /// The +0.1 nudge button of the picked tier.
    pub(super) fn film_tune_nudge(&mut self) -> Result<(), String> {
        let nudge = self.tune_button("+0.1")?;
        self.click_and_invoke(&nudge)?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// Accept, once the view says the picked tier can be accepted.
    pub(super) fn film_tune_accept(&mut self) -> Result<(), String> {
        let tune = self.ui.global::<TuneModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || tune.get_can_accept()) {
            return Err("Accept stayed disabled in the Tune view".into());
        }
        let accept = self.tune_button("Accept")?;
        self.click_and_invoke(&accept)?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// The last, reading step: the mini sweep's chart (below Accept, Pair With and the Objective
    /// list) is scrolled wholly into the inspector's view and held, so the picture the text
    /// describes is on screen. Nothing is clicked: a click on the chart would set an angle.
    pub(super) fn film_tune_show_chart(&mut self) -> Result<(), String> {
        self.inspector_form_control("Tune Metric Curves", uiprobe::AccessibleRole::Image)?
            .ok_or("the Tune chart is not on screen (the mini sweep is drawn in Advanced only)")?;
        self.hold(pacing::RESULT_HOLD)
    }

    /// Next Tier, then the +0.1 nudge of the tier it picked.
    pub(super) fn film_tune_next_and_nudge(&mut self) -> Result<(), String> {
        let next = self.tune_button("Next Tier")?;
        self.click_and_invoke(&next)?;
        self.hold(Duration::from_millis(400))?;
        self.film_tune_nudge()
    }
}
