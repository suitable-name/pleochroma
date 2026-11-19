//! The planning half of a redraw: [`build_planned_frame`], the only pipeline function
//! that calls the potentially multi-second [`live_update::plan_preview`].
//!
//! Called by the desktop's plan worker and by the web app's main thread, each with its
//! own clock (`Instant`-backed on the desktop, `performance.now()` on the web).

use super::request::{PlanJob, PlannedFrame};
use crate::{
    facet_map::FacetMap,
    live_update::{self, CutLimit},
    raster::SolidStyle,
};
use pleochroma::geometry::{meet_solver::SolvedTier, tool::ToolPrimitive};
use pleochroma_cut_core::design::Design;
use std::time::Duration;

/// Runs [`live_update::plan_preview`] (the expensive, potentially multi-second
/// call) and builds the facet-level [`SolidStyle`] from its result via
/// [`FacetMap::overlay_flags`].
///
/// `budget` is a parameter (the desktop and web both pass
/// [`live_update::DEFAULT_PREVIEW_BUDGET`]) so tests can force the `Stale` branch
/// deterministically (`Duration::ZERO`); `clock` is the caller's monotonic clock
/// (see [`live_update::Clock`]).
///
/// The `Stale` pending set is forwarded unchanged: every tier of a batch edit is
/// outlined as pending.
///
/// Does not decide `Unbounded` vs. closed, which needs a
/// [`crate::mesh_cache::MeshCache`]; [`super::render_request`] finishes that part.
#[must_use]
pub fn build_planned_frame(
    job: PlanJob,
    budget: Duration,
    clock: &dyn live_update::Clock,
) -> PlannedFrame {
    build_planned_frame_themed(job, budget, clock, false)
}

/// The viewport style for the theme.
///
/// [`SolidStyle::high_contrast`]'s palette when `high_contrast` is set, otherwise `style`
/// unchanged. The per-facet flags, the preform settings and everything else the planner filled
/// in carry over, so only the palette changes.
///
/// Only the live viewports use this (the desktop's Solid view and Compare, the Android
/// viewport). The thumbnails, variants, history, rough plan and pose preview keep the
/// default style on purpose: they are content shown small or exported, not interface.
#[must_use]
pub fn restyle_for_theme(style: SolidStyle, high_contrast: bool) -> SolidStyle {
    if !high_contrast {
        return style;
    }
    SolidStyle {
        flagged: style.flagged,
        pending: style.pending,
        selected: style.selected,
        preform_plane_count: style.preform_plane_count,
        show_preform: style.show_preform,
        ..SolidStyle::high_contrast()
    }
}

/// [`build_planned_frame`] with the theme: `high_contrast` draws the frame's style with
/// [`SolidStyle::high_contrast`] (see [`restyle_for_theme`]). `false` is exactly
/// [`build_planned_frame`].
#[must_use]
pub fn build_planned_frame_themed(
    job: PlanJob,
    budget: Duration,
    clock: &dyn live_update::Clock,
    high_contrast: bool,
) -> PlannedFrame {
    let limit = job.cut_limit();
    let PlanJob {
        design,
        dirty,
        last_solved,
        camera,
        size,
        selected_tier,
        n_d,
        view_mode,
        generation,
        show_preform,
        enlarged_panel,
        tier_cutoff: _,
        cut_steps: _,
    } = job;
    let plan = live_update::plan_preview_limited(
        &design,
        last_solved.as_deref(),
        &dirty,
        budget,
        &live_update::RealSolver,
        clock,
        limit,
    );
    // The whole pending set is outlined. `empty_pending` gives the non-`Stale` arm
    // something to borrow.
    let empty_pending = std::collections::BTreeSet::new();
    let pending_tiers = match &plan.freshness {
        live_update::Freshness::Stale { pending } => pending,
        _ => &empty_pending,
    };
    // The masts for the tools and the facet map: the plan's own solve, or for an
    // `Unsolvable` plan the previous masts the held-over stone is drawn from. These follow
    // the stone on screen; the frame itself carries no masts (see `solved` below).
    let drawn = drawn_masts(plan.solved.as_deref(), last_solved.as_deref(), &design);
    let (tools, placements) = concave_tools_for_display(&design, drawn, limit);
    // The planes of a partly cut stone are the finished stone's with the hidden tiers'
    // slices removed, so the facet ids must be counted the same way.
    let visible_tiers = live_update::limit_visible_tiers(&design, limit);
    let facet_map = FacetMap::from_design_cut(
        &design,
        drawn.unwrap_or(&[]),
        &placements,
        visible_tiers.as_deref(),
    );
    let overlay = facet_map.overlay_flags(&design, n_d, selected_tier, pending_tiers);
    let unsolvable_status = match &plan.freshness {
        live_update::Freshness::Unsolvable(err) => {
            let text = err.to_string();
            let text = text.trim_end_matches('.');
            Some(format!("Preview cannot be solved: {text}."))
        }
        _ => None,
    };
    // No `MeshCache` here: the render step makes the `Unbounded` check. `style` stays
    // undimmed whatever that check finds.
    let preform_plane_count = facet_map.preform_plane_count();
    let style = restyle_for_theme(
        SolidStyle {
            flagged: overlay.flagged,
            pending: overlay.pending,
            selected: overlay.selected,
            preform_plane_count,
            show_preform,
            ..SolidStyle::viewport()
        },
        high_contrast,
    );
    let out_of_date = matches!(plan.freshness, live_update::Freshness::Stale { .. });
    // An `Unsolvable` plan carries no masts (`plan.solved` is `None`, see
    // `live_update::plan_preview`). The frame is stamped with the new generation, so
    // masts carried over from the previous solve would read as the solve of a design
    // they do not describe. A caller keeps its own cache when a frame brings no masts.
    let solved = plan.solved;
    PlannedFrame {
        design,
        planes: plan.planes,
        tools,
        placements,
        style,
        solved,
        stale: out_of_date,
        unsolvable_status,
        camera,
        size,
        view_mode,
        generation,
        n_d,
        enlarged_panel,
        visible_tiers,
        overlay: None,
    }
}

/// The masts a frame's concave tools and facet map are built from: the plan's own `solved`
/// masts, or, when the plan solved nothing (an unsolvable design), the `previous` masts the
/// held-over stone is drawn from -- as long as they still line up with `design`'s tiers, the
/// same test [`live_update::plan_preview`] applies before it draws the previous planes.
fn drawn_masts<'a>(
    solved: Option<&'a [SolvedTier]>,
    previous: Option<&'a [SolvedTier]>,
    design: &Design,
) -> Option<&'a [SolvedTier]> {
    solved.or_else(|| previous.filter(|masts| masts.len() == design.tiers.len()))
}

/// The concave tools to draw with `design`'s planes, and their `(tier, placement)`
/// list. Empty for a design without concave tiers (and so for every planar design,
/// leaving the frame byte-identical), when there is no solve to measure the stone
/// against, and when the concave tiers do not resolve (an invalid tier, too many
/// placements): the preview then shows the flat stone rather than failing, and the
/// editor's own validation reports why.
///
/// `limit` is the viewport's slider; the tools are the ones
/// [`live_update::display_tools`] picks for the planes of the same frame, so a groove is
/// never drawn into planes the slider has removed.
fn concave_tools_for_display(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    limit: CutLimit,
) -> (Vec<ToolPrimitive>, Vec<(usize, usize)>) {
    solved.map_or_else(
        || (Vec::new(), Vec::new()),
        |solved| live_update::display_tools(design, solved, limit),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::CameraPose;
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use std::sync::Arc;

    /// With the slider after the stored-first crown tier of a crown-first design, the
    /// groove (a pavilion tool, cut before every crown tier) is drawn together with the
    /// pavilion planes it cuts, and the crown dimple, cut after the hidden crown tier, is not.
    #[test]
    fn concave_tools_follow_the_same_visible_tiers_as_the_planes() {
        let mut design = Design::concave_fixture();
        design.tiers.reverse();
        let solved = design.solve().expect("the fixture solves");
        let (all, _) = concave_tools_for_display(&design, Some(&solved), CutLimit::Finished);
        let (through, placements) =
            concave_tools_for_display(&design, Some(&solved), CutLimit::ThroughTier(0));
        assert_eq!(all.len(), 12, "eight groove placements and four dimples");
        assert_eq!(through.len(), 8);
        assert!(placements.iter().all(|&(tier, _)| tier == 0));
    }

    /// No solve, no tools: with no masts at all the frame draws the flat stone alone.
    #[test]
    fn no_masts_means_no_tools() {
        let design = Design::concave_fixture();
        let (tools, placements) = concave_tools_for_display(&design, None, CutLimit::Finished);
        assert!(tools.is_empty() && placements.is_empty());
    }

    /// The plan's own masts win; without them the previous masts stand in, but only
    /// while they line up with the design's tiers.
    #[test]
    fn a_plan_that_solved_nothing_draws_from_the_previous_masts() {
        let design = Design::concave_fixture();
        let own = design.solve().expect("the fixture solves");
        let previous = design.solve().expect("the fixture solves");

        let drawn = drawn_masts(Some(&own), Some(&previous[..1]), &design);
        assert!(
            drawn.is_some_and(|masts| std::ptr::eq(masts, own.as_slice())),
            "the plan's own solve wins"
        );
        let held_over = drawn_masts(None, Some(&previous), &design);
        assert!(held_over.is_some_and(|masts| std::ptr::eq(masts, previous.as_slice())));
        assert!(
            drawn_masts(None, Some(&previous[..1]), &design).is_none(),
            "a list left over from before a tier was added or removed draws nothing"
        );
        assert!(drawn_masts(None, None, &design).is_none());
    }

    /// An unsolvable frame still shows the previous stone, so it keeps that stone's
    /// concave tools (grooves, dimples and their hover entries). The frame carries no
    /// masts of its own.
    #[test]
    fn an_unsolvable_frame_keeps_the_concave_tools_of_the_stone_it_shows() {
        let fixture = Design::concave_fixture();
        let previous = fixture.solve().expect("the fixture solves");
        // The same design with no tier anchored: it does not solve.
        let mut broken = fixture.clone();
        for tier in &mut broken.tiers {
            tier.constraint = MeetConstraint::MeetExisting;
        }
        let tier_count = broken.tiers.len();
        let frame = build_planned_frame(
            PlanJob {
                design: Arc::new(broken),
                dirty: (0..tier_count).collect(),
                last_solved: Some(previous),
                camera: CameraPose {
                    roll: 0.0,
                    pan: [0.0, 0.0],
                    yaw: 0.0,
                    pitch: 0.0,
                    distance: 5.0,
                },
                size: (16, 16),
                selected_tier: None,
                n_d: fixture.effective_refractive_index(),
                view_mode: 0,
                generation: 9,
                show_preform: true,
                enlarged_panel: -1,
                tier_cutoff: None,
                cut_steps: None,
            },
            Duration::from_secs(60),
            &ZeroClock,
        );
        assert!(
            frame.unsolvable_status.is_some(),
            "the premise: the design does not solve"
        );
        assert!(
            frame.solved.is_none(),
            "an unsolvable frame carries no masts"
        );
        assert!(!frame.planes.is_empty(), "the held-over stone is drawn");
        assert_eq!(
            frame.tools.len(),
            12,
            "eight groove placements and four dimples stay with the stone"
        );
        assert_eq!(frame.placements.len(), frame.tools.len());
    }

    /// The high-contrast flag swaps the palette to `SolidStyle::high_contrast()` and keeps
    /// the facet flags and preform settings; off, the style is untouched.
    #[test]
    fn the_theme_flag_selects_the_high_contrast_palette() {
        let planned = SolidStyle {
            flagged: vec![true, false],
            pending: vec![false, true],
            selected: vec![true, true],
            preform_plane_count: 3,
            show_preform: false,
            ..SolidStyle::viewport()
        };
        let default = restyle_for_theme(planned.clone(), false);
        assert_eq!(default.edge_color, SolidStyle::viewport().edge_color);
        assert_eq!(
            default.selected_color,
            SolidStyle::viewport().selected_color
        );

        let high = restyle_for_theme(planned, true);
        let expected = SolidStyle::high_contrast();
        assert_eq!(high.base_color, expected.base_color);
        assert_eq!(high.edge_color, expected.edge_color);
        assert_eq!(high.selected_color, expected.selected_color);
        assert_eq!(high.multi_selected_color, expected.multi_selected_color);
        assert_ne!(high.edge_color, SolidStyle::viewport().edge_color);
        assert_eq!(high.flagged, vec![true, false]);
        assert_eq!(high.pending, vec![false, true]);
        assert_eq!(high.selected, vec![true, true]);
        assert_eq!(high.preform_plane_count, 3);
        assert!(!high.show_preform);
    }

    /// A clock that never advances: every solve is "within budget".
    struct ZeroClock;

    impl live_update::Clock for ZeroClock {
        fn now_ms(&self) -> f64 {
            0.0
        }
    }
}
