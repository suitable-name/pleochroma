//! The Cut slider: where it stands, what it shows and what it is called.
//!
//! The slider walks a design's cutting steps ([`Design::preview_steps`]): position `0`
//! is the rough (the preform alone, whatever its shape: block, cylinder or a planned
//! piece), position `k` the stone after the first `k` steps,
//! and the last position the finished design. Flat and concave tiers each count as one
//! step, in the order the cutting sheet lists them for every design
//! ([`Design::cutting_order`]: the pavilion and girdle tiers, the crown, the table last).
//!
//! `SolidPreviewModel.tier_cutoff` is the one source of truth, in these terms:
//! [`MODEL_FINISHED`] (`-1`) for the finished design, [`MODEL_ROUGH`] (`-2`) for the
//! rough, and `n >= 0` for the stone after steps `0..=n` -- the first `n + 1` steps of the
//! cutting order, which the Slice tool reads. [`CutPosition`] decodes it;
//! everything else here is derived from the position and the design, so the label, the
//! slider handle, the planner and every full redraw cannot disagree.
//!
//! [`CutPosition`], [`cut_label`], [`MODEL_FINISHED`] and [`MODEL_ROUGH`] live in
//! `pleochroma_solid::cut_position`, shared with the web app, and are re-exported here.
//! The decisions that need the solver are pure functions here ([`cut_geometry`]); the
//! few functions that touch the window ([`sync_model`], [`current_steps`],
//! [`reset_to_finished`]) only read and write the model.

use crate::{MainWindow, SolidPreviewModel};
use pleochroma::geometry::{GpuFacetPlane, meet_solver::SolvedTier};
use pleochroma_cut_core::Design;
pub use pleochroma_solid::cut_position::{CutPosition, MODEL_FINISHED, MODEL_ROUGH, cut_label};
use pleochroma_solid::{
    live_update::{CutLimit, display_geometry},
    preview::StoneGeometryBuf,
};
use slint::ComponentHandle;

/// The geometry every full-plane redraw path draws for `design` at `steps` cutting steps.
///
/// `None` is the finished design. The result holds the planes, the concave tools cut into
/// them and the `(tier, placement)` of each tool. This is the one place the Cut slider is
/// applied outside the planner, so the background-solve push, the full refresh and the
/// optimise ghost agree with it, and the path tracer receives the same planes as the
/// solid raster.
///
/// `solved` is the design's own mast list, or `None` to solve it here (an unsolvable
/// design then draws nothing, except the rough, which needs no masts). Never panics: a
/// mast list that does not fit the design draws nothing.
///
/// `None` runs `Design::solve` on the calling thread, which takes seconds for a large
/// design: never pass it from the UI thread. Use [`cut_geometry_no_solve`] there.
#[must_use]
pub fn cut_geometry(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    steps: Option<usize>,
) -> StoneGeometryBuf {
    let own = if solved.is_none() {
        design.solve().unwrap_or_default()
    } else {
        Vec::new()
    };
    let masts = solved.unwrap_or(&own);
    let limit = steps.map_or(CutLimit::Finished, CutLimit::Steps);
    let geometry = display_geometry(design, masts, limit);
    // Empty at once for a design without frosting; otherwise the frosted planes among the
    // ones drawn (the Cut slider's partial stones included).
    let frosted = design.frosted_planes_in(masts, &geometry.planes);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the GPU plane precision; offsets are in the normal float range"
    )]
    let planes = geometry
        .planes
        .into_iter()
        .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
        .collect();
    StoneGeometryBuf {
        planes,
        tools: geometry.tools,
        placements: geometry.placements,
        frosted,
    }
}

/// [`cut_geometry`] for a caller on the UI thread: it never solves.
///
/// `solved` is the mast list the caller already holds, and `None` means the design does
/// not solve (the caller's own solve failed). Retrying that failed solve here would only
/// fail again, on the UI thread, so the stone is drawn as for an empty mast list: nothing,
/// except the rough, which needs no masts.
#[must_use]
pub fn cut_geometry_no_solve(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    steps: Option<usize>,
) -> StoneGeometryBuf {
    cut_geometry(design, Some(solved.unwrap_or(&[])), steps)
}

/// How many more 16 ms ticks a Cut slider replan waits for the editor state to be free.
///
/// About 130 ms in all, then it gives up. A longer-lived writer (a blocking file dialog
/// that pumps the event loop) submits its own replan on its way out, and that replan
/// reads the slider fresh.
pub const REPLAN_RETRIES: u8 = 8;

/// The retries left after one more failed attempt, or `None` when they are used up.
#[must_use]
pub const fn retries_after_busy(retries_left: u8) -> Option<u8> {
    retries_left.checked_sub(1)
}

/// The stone to push when a redraw arrives with the finished stone in hand.
///
/// That is `finished` -- built for the whole design -- unless the slider has the design cut
/// back to `steps`, when it is the stone at that cut, so the cut survives the background
/// solve's result. With no cut the stone passes through untouched.
///
/// Runs on the UI thread, so it never solves: `solved` is `None` only when the solve that
/// produced `finished` failed (see [`cut_geometry_no_solve`]).
#[must_use]
pub fn stone_at_cut(
    finished: StoneGeometryBuf,
    design: &Design,
    solved: Option<&[SolvedTier]>,
    steps: Option<usize>,
) -> StoneGeometryBuf {
    steps.map_or(finished, |steps| {
        cut_geometry_no_solve(design, solved, Some(steps))
    })
}

/// The cut the viewport is showing for `design` right now; `None` is the finished design.
///
/// Reads the model without changing it, so it is safe on any redraw path, including the
/// Slice tool's provisional replans (whose design has one extra tier).
#[must_use]
pub fn current_steps(ui: &MainWindow, design: &Design) -> Option<usize> {
    let cutoff = ui.global::<SolidPreviewModel>().get_tier_cutoff();
    CutPosition::from_model(cutoff, design.preview_step_count()).steps()
}

/// Writes the model's slider fields for `design` and returns the position they show: the
/// number of steps (the slider's maximum), the slider handle, the label, and a cutoff that
/// no longer fits (a tier was removed) normalised back to finished. Setting a property to
/// the value it already has changes nothing, so this is cheap to call on every replan.
fn publish(ui: &MainWindow, design: &Design) -> CutPosition {
    let model = ui.global::<SolidPreviewModel>();
    let step_count = design.preview_step_count();
    let position = CutPosition::from_model(model.get_tier_cutoff(), step_count);
    model.set_cut_step_count(i32::try_from(step_count).unwrap_or(i32::MAX));
    model.set_tier_cutoff(position.to_model());
    model.set_cut_position(position.slider_value(step_count));
    model.set_cut_label(cut_label(design, position).into());
    position
}

/// Brings the model's slider fields in line with `design` (see [`publish`]) and returns
/// the cut to draw. Called for every replan of the committed design.
#[must_use]
pub fn sync_model(ui: &MainWindow, design: &Design) -> Option<usize> {
    publish(ui, design).steps()
}

/// Puts the slider back to the finished design and refreshes the label for `design`: a
/// new, opened or loaded design starts uncut, and the label and the view must agree.
pub fn reset_to_finished(ui: &MainWindow, design: &Design) {
    ui.global::<SolidPreviewModel>()
        .set_tier_cutoff(MODEL_FINISHED);
    publish(ui, design);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    fn round_brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    #[test]
    fn the_rough_geometry_is_the_preform_alone_with_or_without_masts() {
        let design = round_brilliant();
        let preform = design.preform.planes().len();
        let solved = design.solve().expect("every tier is pinned");
        assert_eq!(
            cut_geometry(&design, Some(&solved), Some(0)).planes.len(),
            preform
        );
        assert_eq!(cut_geometry(&design, None, Some(0)).planes.len(), preform);
        assert_eq!(
            cut_geometry(&design, Some(&[]), Some(0)).planes.len(),
            preform
        );
    }

    #[test]
    fn the_finished_geometry_matches_the_uncut_conversion() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        let expected =
            pleochroma_editor::solve_policy::design_to_gpu_planes_from_solved(&design, &solved);
        let finished = cut_geometry(&design, Some(&solved), None);
        assert_eq!(finished.planes, expected);
        assert!(finished.tools.is_empty() && finished.placements.is_empty());
        assert_eq!(
            cut_geometry(&design, None, None).planes,
            expected,
            "without masts the design is solved here"
        );
    }

    #[test]
    fn a_cut_geometry_grows_one_tier_at_a_time() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        let mut previous = cut_geometry(&design, Some(&solved), Some(0)).planes.len();
        for steps in 1..=design.preview_step_count() {
            let now = cut_geometry(&design, Some(&solved), Some(steps))
                .planes
                .len();
            assert!(now >= previous, "step {steps}");
            previous = now;
        }
        assert_eq!(
            previous,
            cut_geometry(&design, Some(&solved), None).planes.len()
        );
    }

    #[test]
    fn an_unsolvable_design_draws_nothing_but_its_rough() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            vec![ConstraintTier {
                angle_deg: 30.0,
                name: "C1".to_string(),
                indices: vec![0.0],
                constraint: pleochroma::geometry::meet_solver::MeetConstraint::MeetExisting,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            }],
        );
        assert!(
            cut_geometry(&design, None, None).planes.is_empty(),
            "an unsolvable design has no finished stone"
        );
        assert!(
            cut_geometry(&design, None, Some(1)).planes.is_empty(),
            "nor one cut back after a step"
        );
        assert_eq!(
            cut_geometry(&design, None, Some(0)).planes.len(),
            design.preform.planes().len()
        );
    }

    /// The UI-thread form never solves: with masts it is `cut_geometry`, and without masts
    /// (a design whose solve failed) the stone is the rough and nothing else.
    #[test]
    fn the_no_solve_form_draws_the_rough_for_a_design_without_masts() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        for steps in [None, Some(0), Some(3)] {
            assert_eq!(
                cut_geometry_no_solve(&design, Some(&solved), steps),
                cut_geometry(&design, Some(&solved), steps),
                "with masts, cut {steps:?}"
            );
        }
        // The same design, but the caller holds no masts: `cut_geometry(.., None, ..)` would
        // solve it here and draw the whole stone, which is exactly what this must not do.
        assert!(
            !cut_geometry(&design, None, None).planes.is_empty(),
            "the solving form draws the whole stone"
        );
        assert!(
            cut_geometry_no_solve(&design, None, None).planes.is_empty(),
            "no masts, no stone"
        );
        assert!(
            cut_geometry_no_solve(&design, None, Some(3))
                .planes
                .is_empty(),
            "nor one cut back to step 3"
        );
        assert_eq!(
            cut_geometry_no_solve(&design, None, Some(0)).planes.len(),
            design.preform.planes().len()
        );
    }

    /// A background solve arrives with the finished stone: with no cut it is pushed as it
    /// is, with a cut the truncated stone replaces it (the regression: the finished gem
    /// overwrote the cut after every edit).
    #[test]
    fn a_background_solves_stone_is_cut_back_only_when_the_slider_is() {
        let design = round_brilliant();
        let solved = design.solve().expect("every tier is pinned");
        let finished = cut_geometry(&design, Some(&solved), None);
        let untouched = stone_at_cut(finished.clone(), &design, Some(&solved), None);
        assert_eq!(untouched, finished);

        let cut = stone_at_cut(finished.clone(), &design, Some(&solved), Some(3));
        assert!(cut.planes.len() < finished.planes.len());
        assert_eq!(cut, cut_geometry(&design, Some(&solved), Some(3)));

        let rough = stone_at_cut(finished, &design, Some(&solved), Some(0));
        assert_eq!(rough.planes.len(), design.preform.planes().len());
    }

    #[test]
    fn a_busy_editor_is_retried_a_bounded_number_of_times() {
        let mut left = REPLAN_RETRIES;
        let mut retries = 0;
        while let Some(next) = retries_after_busy(left) {
            left = next;
            retries += 1;
        }
        assert_eq!(retries, REPLAN_RETRIES);
        assert_eq!(retries_after_busy(0), None);
        assert_eq!(retries_after_busy(1), Some(0));
    }

    #[test]
    fn the_concave_geometry_carries_the_tools_of_the_cut_steps() {
        let design = Design::concave_fixture();
        let solved = design.solve().expect("the fixture solves");
        let count = design.preview_step_count();
        assert!(
            cut_geometry(&design, Some(&solved), Some(3))
                .tools
                .is_empty(),
            "the groove is step 4"
        );
        let grooved = cut_geometry(&design, Some(&solved), Some(4));
        assert_eq!(grooved.tools.len(), 8);
        assert_eq!(grooved.placements.len(), 8);
        let finished = cut_geometry(&design, Some(&solved), None);
        assert_eq!(finished.tools.len(), 12);
        assert_eq!(
            cut_geometry(&design, Some(&solved), Some(count))
                .tools
                .len(),
            12
        );
    }
}
