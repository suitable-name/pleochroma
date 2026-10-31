//! The browser's Cut slider without the window: which stone a handle position shows, what it is
//! called, and what the planner is asked to draw.
//!
//! The slider walks the design's cutting steps in cutting order (`Design::preview_steps`, the
//! order of the cutting sheet and of Cutting Mode), exactly as the desktop's slider does: handle
//! `0` is the rough, handle `k` the stone after the first `k` steps, the last handle (`N`, the
//! step count) the finished design. Flat and concave tiers each count as one step, so `N` is
//! `Design::preview_step_count`, not the number of rows of the tier table.
//!
//! `SolidModel.tier-cutoff` stays the one stored value, in the desktop's terms
//! (`pleochroma_solid::cut_position`): `-1` finished, `-2` rough, `n >= 0` the stone through
//! steps `0..=n`. [`CutSlider`] decodes it for a step count, and everything the toolbar shows (the
//! handle, the label, the amber border) and everything the planner is given
//! ([`CutSlider::cut_steps`]) derives from that one decoding, so they cannot disagree. The Finished
//! stone is `cut_steps: None`, which is the planner's untruncated path and so the very picture the
//! slider has always drawn at its right end.
//!
//! The tier index is never handed to the planner: `ThroughTier(n)` truncates by stored table
//! position, which is not the cutting order for a design stored top-down or one with concave
//! tiers.

use pleochroma_solid::cut_position::CutPosition;

/// A design needs at least this many steps for the slider to be worth showing.
const MIN_STEPS_FOR_SLIDER: usize = 2;

/// Where the Cut slider stands for a design with `step_count` cutting steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CutSlider {
    /// `Design::preview_step_count` of the design the slider is for.
    pub step_count: usize,
    /// The decoded position.
    pub position: CutPosition,
}

impl CutSlider {
    /// Decodes the stored `tier_cutoff` for a design with `step_count` steps. A stale value past
    /// the last step (a tier was removed since it was set) and anything below
    /// [`MODEL_ROUGH`](pleochroma_solid::cut_position::MODEL_ROUGH) read as the finished stone.
    #[must_use]
    pub fn new(tier_cutoff: i32, step_count: usize) -> Self {
        Self {
            step_count,
            position: CutPosition::from_model(tier_cutoff, step_count),
        }
    }

    /// The slider for a handle value (rounded to the nearest step, clamped to the ends).
    #[must_use]
    pub const fn from_handle(value: f32, step_count: usize) -> Self {
        Self {
            step_count,
            position: CutPosition::from_slider_value(value, step_count),
        }
    }

    /// The `tier_cutoff` to store for this position (always normalised: a stale cutoff comes back
    /// as [`MODEL_FINISHED`](pleochroma_solid::cut_position::MODEL_FINISHED)).
    #[must_use]
    pub fn tier_cutoff(self) -> i32 {
        self.position.to_model()
    }

    /// The handle's value: `0` for the rough up to `step_count` for the finished design.
    #[must_use]
    pub const fn handle(self) -> f32 {
        self.position.slider_value(self.step_count)
    }

    /// How many steps are cut, the planner's `cut_steps`: `None` for the finished design,
    /// `Some(0)` for the rough.
    #[must_use]
    pub const fn cut_steps(self) -> Option<usize> {
        self.position.steps()
    }

    /// Whether the slider shows part of the design (the toolbar's amber border).
    #[must_use]
    pub const fn is_partial(self) -> bool {
        self.cut_steps().is_some()
    }

    /// The toolbar label: `Cut: Rough`, `Cut: k/N` after `k` of `N` steps, `Cut: Finished`.
    #[must_use]
    pub fn label(self) -> String {
        match self.position {
            CutPosition::Finished => "Cut: Finished".to_string(),
            CutPosition::Rough => "Cut: Rough".to_string(),
            CutPosition::After(steps) => format!("Cut: {steps}/{}", self.step_count),
        }
    }
}

/// The `tier_cutoff` a dragged handle value stands for.
///
/// The value is rounded to a whole step and clamped, `0` is
/// [`MODEL_ROUGH`](pleochroma_solid::cut_position::MODEL_ROUGH) and `step_count` (or past it)
/// [`MODEL_FINISHED`](pleochroma_solid::cut_position::MODEL_FINISHED).
#[must_use]
pub fn tier_cutoff_for_handle(value: f32, step_count: usize) -> i32 {
    CutSlider::from_handle(value, step_count).tier_cutoff()
}

/// Whether the toolbar shows the slider: it needs two steps or more to have anything between the
/// rough and the finished stone.
#[must_use]
pub const fn is_shown(step_count: usize) -> bool {
    step_count >= MIN_STEPS_FOR_SLIDER
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta, design::TierRef};
    use pleochroma_solid::{
        cut_position::{MODEL_FINISHED, MODEL_ROUGH},
        live_update::{CutLimit, display_geometry},
    };

    fn stored_top_down() -> Design {
        // The standard round brilliant is stored table first, culet last; it is cut pavilion
        // first and the table last.
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    #[test]
    fn a_dragged_handle_maps_to_the_desktops_cutoff_encoding() {
        // Four steps: 0 rough, 1..=3 after that many steps, 4 finished.
        assert_eq!(tier_cutoff_for_handle(0.0, 4), MODEL_ROUGH);
        assert_eq!(tier_cutoff_for_handle(0.4, 4), MODEL_ROUGH);
        assert_eq!(tier_cutoff_for_handle(1.0, 4), 0);
        assert_eq!(tier_cutoff_for_handle(2.0, 4), 1);
        assert_eq!(tier_cutoff_for_handle(2.6, 4), 2);
        assert_eq!(tier_cutoff_for_handle(3.0, 4), 2);
        assert_eq!(tier_cutoff_for_handle(3.6, 4), MODEL_FINISHED);
        assert_eq!(tier_cutoff_for_handle(4.0, 4), MODEL_FINISHED);
        assert_eq!(tier_cutoff_for_handle(99.0, 4), MODEL_FINISHED);
        assert_eq!(tier_cutoff_for_handle(-3.0, 4), MODEL_ROUGH);
        assert_eq!(tier_cutoff_for_handle(f32::NAN, 4), MODEL_ROUGH);
    }

    #[test]
    fn every_handle_position_round_trips_through_the_stored_cutoff() {
        for step_count in [2_usize, 4, 8, 13] {
            for handle in 0..=step_count {
                let value = u16::try_from(handle).map(f32::from).expect("small");
                let slider = CutSlider::new(tier_cutoff_for_handle(value, step_count), step_count);
                assert_eq!(slider.handle(), value, "{handle} of {step_count}");
            }
        }
    }

    #[test]
    fn the_three_labels_keep_their_wording() {
        assert_eq!(CutSlider::new(MODEL_ROUGH, 4).label(), "Cut: Rough");
        assert_eq!(CutSlider::new(MODEL_FINISHED, 4).label(), "Cut: Finished");
        // Stored cutoff 1 is the stone through steps 0..=1: two steps cut.
        assert_eq!(CutSlider::new(1, 4).label(), "Cut: 2/4");
        assert_eq!(CutSlider::new(0, 4).label(), "Cut: 1/4");
        assert_eq!(CutSlider::new(2, 4).label(), "Cut: 3/4");
        // The last step is the finished stone, never a bogus "4/4".
        assert_eq!(CutSlider::new(3, 4).label(), "Cut: Finished");
    }

    #[test]
    fn a_stale_cutoff_reads_as_finished_and_is_normalised() {
        for stale in [4, 5, 99, -3, i32::MIN] {
            let slider = CutSlider::new(stale, 4);
            assert_eq!(slider.position, CutPosition::Finished, "{stale}");
            assert_eq!(slider.tier_cutoff(), MODEL_FINISHED, "{stale}");
            assert_eq!(slider.cut_steps(), None, "{stale}");
            assert!(!slider.is_partial(), "{stale}");
        }
        // A design that lost all its steps has nothing to cut back.
        assert_eq!(CutSlider::new(MODEL_ROUGH, 0).cut_steps(), None);
    }

    #[test]
    fn the_planner_is_asked_for_the_steps_cut_and_nothing_for_the_finished_stone() {
        assert_eq!(CutSlider::new(MODEL_FINISHED, 8).cut_steps(), None);
        assert_eq!(CutSlider::new(MODEL_ROUGH, 8).cut_steps(), Some(0));
        for steps in 1..8 {
            let handle = u16::try_from(steps).map(f32::from).expect("small");
            let cutoff = tier_cutoff_for_handle(handle, 8);
            let slider = CutSlider::new(cutoff, 8);
            assert_eq!(slider.cut_steps(), Some(steps), "handle {steps}");
            assert!(slider.is_partial());
        }
        assert!(!CutSlider::new(MODEL_FINISHED, 8).is_partial());
        assert!(CutSlider::new(MODEL_ROUGH, 8).is_partial());
    }

    #[test]
    fn the_slider_needs_two_steps() {
        assert!(!is_shown(0));
        assert!(!is_shown(1));
        assert!(is_shown(2));
        assert!(is_shown(8));
    }

    /// The case that was wrong: a design whose cutting order is not its table order. Handle `k`
    /// must show the stone after the first `k` steps in cutting order, which for this stone is
    /// the pavilion side first -- not the first `k` rows of the table (table, star, crown...).
    #[test]
    fn handle_k_shows_the_first_k_steps_in_cutting_order_not_in_table_order() {
        let design = stored_top_down();
        let step_count = design.preview_step_count();
        let order = design.preview_steps();
        assert_ne!(
            order[0],
            TierRef::Flat(0),
            "the fixture is cut pavilion first, but stored table first"
        );
        let solved = design.solve().expect("every tier is pinned");

        for steps in 1..step_count {
            let handle = u16::try_from(steps).map(f32::from).expect("small");
            let slider = CutSlider::new(tier_cutoff_for_handle(handle, step_count), step_count);
            let cut = slider.cut_steps().expect("a partial stone");
            assert_eq!(cut, steps);

            // What the planner draws for the slider's `cut_steps`: exactly the flat tiers among
            // the first `steps` of the cutting order (plus the rough).
            let mut visible = vec![false; design.tiers.len()];
            for step in &order[..steps] {
                if let TierRef::Flat(index) = step {
                    visible[*index] = true;
                }
            }
            let shown = display_geometry(&design, &solved, CutLimit::Steps(cut)).planes;
            let expected = design
                .try_planes_for_visible_tiers(&solved, &visible)
                .expect("aligned masts");
            assert_eq!(shown, expected, "after {steps} steps");

            // What the slider used to draw: the first `steps` rows of the table. Not the same
            // stone for any partial position of this design.
            let by_table = display_geometry(&design, &solved, CutLimit::ThroughTier(steps - 1));
            assert_ne!(
                shown, by_table.planes,
                "after {steps} steps the table-order stone is a different stone"
            );
        }

        // The ends: the rough is the preform alone, the last handle the untruncated design.
        let rough = CutSlider::new(MODEL_ROUGH, step_count);
        assert_eq!(
            display_geometry(
                &design,
                &solved,
                CutLimit::Steps(rough.cut_steps().unwrap_or(0))
            )
            .planes
            .len(),
            design.preform.planes().len()
        );
        assert_eq!(CutSlider::new(MODEL_FINISHED, step_count).cut_steps(), None);
        assert_eq!(
            display_geometry(&design, &solved, CutLimit::Finished).planes,
            design.planes_from_solved(&solved),
            "Finished draws the whole design, as before"
        );
    }
}
