//! The exact single-stone fit under an orientation rule, and the one-pose re-scale behind
//! "Free Orientation".
//!
//! # The rule in the search
//!
//! `fit/orient.rs` is the only orientation search of the planner, so the rule reaches it here:
//!
//! - **Required**: the orientation tables are filtered to the cone of the rule BEFORE the spins
//!   (a table direction is kept or dropped as a whole, the order of the rest is unchanged, so
//!   the result is deterministic), and the polish rejects every step that leaves the cone.
//! - **Preferred**: the unrestricted fit runs as always, and so does the restricted one (the
//!   cone of the rule at its tolerance, a small part of the orientations, so it is cheap);
//!   per design the pose with the larger `volume * factor` is kept, and designs are ranked by
//!   that value ([`merge_fits_scored`]). A pose inside the tolerance has factor 1, so the
//!   restricted fit competes at its true volume against a larger fit that pays the penalty.
//!
//! With no rule every entry point here is the plain fit, bit for bit.

use glam::DVec3;

use super::{
    Check, DesignHull, FitMesh, SingleFit, StonePose, fit_shortlisted_over, merge_fits,
    orient::{
        EXACT_SPINS, Orientation, SCREEN_SPINS, generate_orientations, orientations_for,
        screening_orientations,
    },
    screen_designs_over, shortlist,
    support::{StoneGuard, SupportWorkspace},
    usable_hull,
};
use crate::{
    rough_plan::{
        orientation::{ResolvedRule, RuleHold},
        types::{PlanProgress, PlanSettings},
    },
    yield_metrics::carat_weight,
};

/// What a fit must satisfy besides the convex region: the rough's mesh and, for a rule-restricted
/// fit, the cone of its Required orientation rule.
#[derive(Clone, Copy)]
pub(super) struct FitLimits<'a> {
    /// The rough's mesh, when it is not convex.
    pub(super) mesh: Option<FitMesh<'a>>,
    /// The cone a pose must stay in.
    pub(super) cone: Option<&'a ResolvedRule>,
}

/// What one pose must satisfy while it is polished: the mesh guard of its design and the cone.
#[derive(Clone, Copy)]
pub(super) struct PoseLimits<'a, 'g> {
    /// The mesh check of the design.
    pub(super) guard: Option<&'a StoneGuard<'g>>,
    /// The cone a pose must stay in.
    pub(super) cone: Option<&'a ResolvedRule>,
}

impl PoseLimits<'_, '_> {
    /// Whether a pose with caliper-frame `axes` stays inside the cone (always, without one).
    pub(super) fn allows(&self, axes: &[[f64; 3]; 3]) -> bool {
        self.cone
            .is_none_or(|rule| rule.factor_for_axes(axes) > 0.0)
    }
}

/// Spins around a table normal the top-up of a rule with an azimuth target takes (2 degrees apart,
/// so a tight azimuth tolerance still finds a spin inside it).
const AZIMUTH_TOP_UP_SPINS: usize = 180;

/// `orients` (a table of `spins` spins per table direction) without the orientations the rule's
/// cone does not allow (order kept).
///
/// A coarse table may have almost no direction inside a tight cone (a few degrees wide): when
/// fewer than [`SCREEN_SPINS`] distinct table directions are left, the orientations of the rule's
/// own table normals (`ResolvedRule::table_directions`) that stay in the cone are appended, so the
/// shortlist and the exact fit are never starved of a pose the rule allows.
fn within_cone(
    mut orients: Vec<Orientation>,
    cone: &ResolvedRule,
    spins: usize,
) -> Vec<Orientation> {
    orients.retain(|orient| cone.factor_for_axes(&orient.axes) > 0.0);
    let mut directions = 0;
    let mut previous: Option<[f64; 3]> = None;
    for orient in &orients {
        if previous != Some(orient.axes[1]) {
            directions += 1;
            previous = Some(orient.axes[1]);
        }
    }
    if directions < SCREEN_SPINS {
        let top_up_spins = if cone.needs_azimuth() {
            spins.max(AZIMUTH_TOP_UP_SPINS)
        } else {
            spins
        };
        let mut extra = orientations_for(&cone.table_directions(), top_up_spins);
        extra.retain(|orient| cone.factor_for_axes(&orient.axes) > 0.0);
        if !cone.needs_azimuth() {
            // The tilt rules do not care about the spin: a normal the table already has adds nothing.
            let has = |orient: &Orientation, known: &[Orientation]| {
                known.iter().any(|other| other.axes[1] == orient.axes[1])
            };
            extra.retain(|orient| !has(orient, &orients));
        }
        orients.extend(extra);
    }
    orients
}

/// The best single-stone fit of every design of `hulls` whose pose stays in the cone of
/// `cone` (a Required rule): [`fit_single_stones_with`](super::fit_single_stones_with) over
/// the filtered orientation tables.
#[expect(
    clippy::too_many_arguments,
    reason = "the stages of the fit take the region, the coarse region, the designs, the settings, the cap, the mesh, the rule and the progress callback"
)]
fn fit_in_cone(
    region: &[(DVec3, f64)],
    coarse_region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    keep: usize,
    mesh: Option<FitMesh<'_>>,
    cone: &ResolvedRule,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<SingleFit>> {
    if hulls.is_empty() || keep == 0 {
        return Some(Vec::new());
    }
    let screening = within_cone(screening_orientations(), cone, SCREEN_SPINS);
    let scores = screen_designs_over(coarse_region, hulls, mesh, &screening, on_progress)?;
    let mut chosen = shortlist(&scores, keep);
    chosen.sort_unstable();
    let subset: Vec<DesignHull> = hulls
        .iter()
        .filter(|hull| chosen.binary_search(&hull.entry_id).is_ok())
        .cloned()
        .collect();
    let exact = within_cone(
        generate_orientations(super::EXACT_LEVEL, EXACT_SPINS),
        cone,
        EXACT_SPINS,
    );
    let fits = fit_shortlisted_over(
        region,
        &subset,
        settings,
        mesh,
        &exact,
        Some(cone),
        on_progress,
    )?;
    Some(merge_fits(fits, keep))
}

/// The orientation passes a design under `rule` is fitted with.
///
/// Each pass is a cone (a Required rule at the rule's tolerance) or `None` for the unrestricted
/// fit: no rule is one unrestricted pass, a Required rule one pass in its cone, a Preferred rule
/// the pass in its cone and the unrestricted one. [`fit_single_stones_ruled`] runs exactly these;
/// the parallel fit of the window runs them chunk by chunk with [`screen_designs_in_cone`] and
/// [`fit_shortlisted_in_cone`] and joins the passes with [`merge_rule_passes`].
#[must_use]
pub fn rule_passes(rule: Option<&ResolvedRule>) -> Vec<Option<ResolvedRule>> {
    rule.map_or_else(
        || vec![None],
        |rule| match rule.hold {
            RuleHold::Required => vec![Some(rule.as_required())],
            RuleHold::Preferred { .. } => vec![Some(rule.as_required()), None],
        },
    )
}

/// [`screen_designs_with`](super::screen_designs_with) restricted to the cone `cone`.
///
/// `None` is the plain screening. The scores of several chunks of designs can be concatenated.
#[must_use]
pub fn screen_designs_in_cone(
    coarse_region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    mesh: Option<FitMesh<'_>>,
    cone: Option<&ResolvedRule>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<(i64, f64)>> {
    let table = cone.map_or_else(screening_orientations, |cone| {
        within_cone(screening_orientations(), cone, SCREEN_SPINS)
    });
    screen_designs_over(coarse_region, hulls, mesh, &table, on_progress)
}

/// [`fit_shortlisted_with`](super::fit_shortlisted_with) restricted to the cone `cone`.
///
/// `None` is the plain exact fit; with a cone the polish stays in it too. The fits of several
/// chunks can be concatenated.
#[must_use]
pub fn fit_shortlisted_in_cone(
    region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    mesh: Option<FitMesh<'_>>,
    cone: Option<&ResolvedRule>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<SingleFit>> {
    let table = generate_orientations(super::EXACT_LEVEL, super::EXACT_SPINS);
    let table = match cone {
        Some(cone) => within_cone(table, cone, super::EXACT_SPINS),
        None => table,
    };
    fit_shortlisted_over(region, hulls, settings, mesh, &table, cone, on_progress)
}

/// Joins the fits of the passes of [`rule_passes`] into the result of the rule.
///
/// `passes[i]` holds the merged fits of pass `i`, at most `keep` each. The one pass stands as it
/// is, the two passes of a Preferred rule are joined by `volume * factor`, as
/// [`fit_single_stones_ruled`] does.
#[must_use]
pub fn merge_rule_passes(
    rule: Option<&ResolvedRule>,
    passes: Vec<Vec<SingleFit>>,
    keep: usize,
) -> Vec<SingleFit> {
    match rule {
        Some(rule) if rule.is_preferred() => {
            let own = *rule;
            merge_fits_scored(passes.into_iter().flatten().collect(), keep, &move |_| {
                Some(own)
            })
        }
        _ => passes.into_iter().flatten().collect(),
    }
}

/// The value a fit counts for in the ranking: its volume times the rule's factor for its pose.
fn scored_volume(fit: &SingleFit, rule: Option<&ResolvedRule>) -> f64 {
    fit.volume_mm3 * rule.map_or(1.0, |rule| rule.factor_for_axes(&fit.pose.axes))
}

/// Keeps the best fit of every design and ranks the designs by it, best first.
///
/// The value of a fit is its `volume * factor` of its rule (`rule_of` gives the rule of an entry
/// id); ties go by ascending `entry_id`; at most `keep` fits. The fits keep their true volume and
/// carat.
#[must_use]
pub fn merge_fits_scored(
    fits: Vec<SingleFit>,
    keep: usize,
    rule_of: &dyn Fn(i64) -> Option<ResolvedRule>,
) -> Vec<SingleFit> {
    let mut scored: Vec<(f64, SingleFit)> = fits
        .into_iter()
        .map(|fit| (scored_volume(&fit, rule_of(fit.entry_id).as_ref()), fit))
        .collect();
    // Best of each design first, so the dedup below keeps it.
    scored.sort_by(|(a, fit_a), (b, fit_b)| {
        fit_a
            .entry_id
            .cmp(&fit_b.entry_id)
            .then(b.total_cmp(a))
            .then(fit_b.volume_mm3.total_cmp(&fit_a.volume_mm3))
    });
    scored.dedup_by(|later, earlier| later.1.entry_id == earlier.1.entry_id);
    scored
        .sort_by(|(a, fit_a), (b, fit_b)| b.total_cmp(a).then(fit_a.entry_id.cmp(&fit_b.entry_id)));
    scored.truncate(keep);
    scored.into_iter().map(|(_, fit)| fit).collect()
}

/// [`fit_single_stones_with`](super::fit_single_stones_with) under one orientation `rule`.
///
/// It gives the best single-stone fit of each design of `hulls`, best first, at most `keep`
/// results. `None` when cancelled.
///
/// With `rule == None` this IS the plain fit. A Required rule fits inside its cone only; a
/// Preferred one runs the plain fit and the cone fit and keeps, per design, the pose with the
/// larger `volume * factor` (see the module documentation). The fits keep their true volume;
/// the ranking uses the value with the factor.
#[expect(
    clippy::too_many_arguments,
    reason = "the stages of the fit take the region, the coarse region, the designs, the settings, the cap, the mesh, the rule and the progress callback"
)]
pub fn fit_single_stones_ruled(
    region: &[(DVec3, f64)],
    coarse_region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    keep: usize,
    mesh: Option<FitMesh<'_>>,
    rule: Option<&ResolvedRule>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<SingleFit>> {
    let Some(rule) = rule else {
        return super::fit_single_stones_with(
            region,
            coarse_region,
            hulls,
            settings,
            keep,
            mesh,
            on_progress,
        );
    };
    let cone = rule.as_required();
    let restricted = fit_in_cone(
        region,
        coarse_region,
        hulls,
        settings,
        keep,
        mesh,
        &cone,
        on_progress,
    )?;
    match rule.hold {
        RuleHold::Required => Some(restricted),
        RuleHold::Preferred { .. } => {
            let mut all = super::fit_single_stones_with(
                region,
                coarse_region,
                hulls,
                settings,
                keep,
                mesh,
                on_progress,
            )?;
            all.extend(restricted);
            let own = *rule;
            Some(merge_fits_scored(all, keep, &move |_| Some(own)))
        }
    }
}

/// [`fit_single_stones_ruled`] when the designs have their own rules.
///
/// `rule_of` gives the resolved rule of an entry id. The designs are grouped by rule, in the order
/// of their first appearance, each group is fitted under its rule (a group is screened and
/// shortlisted on its own, so the shortlist's cut-off is per group), and the fits are merged by
/// `volume * factor`.
///
/// With one group, or with no rules at all, this is exactly [`fit_single_stones_ruled`].
#[expect(
    clippy::too_many_arguments,
    reason = "the stages of the fit take the region, the coarse region, the designs, the settings, the cap, the mesh, the rule and the progress callback"
)]
pub fn fit_single_stones_by_rule(
    region: &[(DVec3, f64)],
    coarse_region: &[(DVec3, f64)],
    hulls: &[DesignHull],
    settings: &PlanSettings,
    keep: usize,
    mesh: Option<FitMesh<'_>>,
    rule_of: &dyn Fn(i64) -> Option<ResolvedRule>,
    on_progress: &mut dyn FnMut(PlanProgress) -> bool,
) -> Option<Vec<SingleFit>> {
    let mut groups: Vec<(Option<ResolvedRule>, Vec<DesignHull>)> = Vec::new();
    for hull in hulls {
        let rule = rule_of(hull.entry_id);
        match groups.iter_mut().find(|(known, _)| *known == rule) {
            Some((_, members)) => members.push(hull.clone()),
            None => groups.push((rule, vec![hull.clone()])),
        }
    }
    if let [(rule, members)] = groups.as_slice() {
        return fit_single_stones_ruled(
            region,
            coarse_region,
            members,
            settings,
            keep,
            mesh,
            rule.as_ref(),
            on_progress,
        );
    }
    let mut all = Vec::new();
    for (rule, members) in &groups {
        all.extend(fit_single_stones_ruled(
            region,
            coarse_region,
            members,
            settings,
            keep,
            mesh,
            rule.as_ref(),
            on_progress,
        )?);
    }
    Some(merge_fits_scored(all, keep, rule_of))
}

/// The largest stone of `hull` that fits `region` in the orientation `axes`: one scaling LP, at
/// once.
///
/// The axes are the caliper-frame axes in the rough frame, `axes[1]` the table normal. This is the
/// re-scale behind Free Orientation: the cutter turns the stone, the planner answers with the
/// stone's new size and position. With locked pieces, pass the region and mesh of the rough with
/// those pieces left out (`rough_with_locked_pieces`), so the stone keeps out of their shells.
///
/// `region` is the rough's usable region, already inset by skin and allowance (as
/// [`fit_single_stones`](super::fit_single_stones) takes it); with a `mesh` the stone is also
/// kept in the material of the mesh. `None` when the hull or the axes are not usable, nothing
/// fits, or the stone would be narrower than the minimum width.
#[must_use]
pub fn rescale_pose(
    region: &[(DVec3, f64)],
    hull: &DesignHull,
    axes: [[f64; 3]; 3],
    settings: &PlanSettings,
    mesh: Option<FitMesh<'_>>,
) -> Option<SingleFit> {
    if !usable_hull(hull) {
        return None;
    }
    let check = Check::of(mesh, hull);
    if check.is_unfit() {
        return None;
    }
    let mut workspace = SupportWorkspace::new(region.len());
    let (k, center_mm) = match check.guard() {
        Some(guard) => workspace.evaluate_in_mesh(&axes, region, &hull.vertices, guard),
        None => workspace.evaluate(&axes, region, &hull.vertices),
    }?;
    if k.is_nan() || k <= 0.0 || k * hull.width < settings.min_width_mm {
        return None;
    }
    let volume_mm3 = k * k * k * hull.volume;
    Some(SingleFit {
        entry_id: hull.entry_id,
        pose: StonePose {
            center_mm,
            axes,
            mm_per_unit: k,
        },
        volume_mm3,
        carat: carat_weight(volume_mm3, settings.specific_gravity),
    })
}
