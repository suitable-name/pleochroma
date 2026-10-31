//! Shared pieces of the render-effects lessons (`surface_finish.rs` and `render_effects.rs`): the
//! groups they unlock, the two steps every one of them opens with, and the goals that read the
//! design's surface finish, inclusions and play-of-color.
//!
//! What these lessons wait for is in the design, so the goals are [`check`]s on it: every route
//! to the result counts (the tier table's menu, the inspector, the Stone tab, Undo), and no new
//! UI event is needed. A choice that only changes the view (Show Surface Finish, the Faceting
//! Machine in Preferences) leaves no trace in the design, so the lessons keep those as reading
//! steps.

use super::{VIEWPORT, path_traced_view};
use crate::guide::{Goal, GoalContext, Group, GuideStep, Perform};
use pleochroma::optics::inclusions::bragg::{DEFAULT_SEED, StructurePattern, StructureSpec};
use pleochroma_cut_core::{Design, StructureEffects, SurfaceGrade, TierSurface};

/// The Stone tab's controls (the `DesignSettings` group gates them) and Undo.
pub(super) const EFFECTS: &[Group] = &[Group::DesignSettings, Group::History];

/// The tier table, the Tier tab of the inspector (its Surface Finish section is an Advanced
/// control, which the Simple interface hides unless a step unlocks the group) and Undo.
pub(super) const SURFACE: &[Group] = &[
    Group::TierForm,
    Group::TierTable,
    Group::Advanced,
    Group::History,
];

/// The step that picks `material` in the Stone tab (the step every lesson of these files opens
/// its action steps with).
pub(super) fn pick_material(material: &str, intro: &'static str, why: &'static str) -> GuideStep {
    GuideStep::new(format!("Choose {material}"), intro)
        .actions([
            "Open the Stone tab of the Parameters dock (View > Stone Properties, or Ctrl+Alt+1)."
                .to_owned(),
            format!("Set Material to {material}. It applies at once."),
        ])
        .check(format!("The Stone tab shows {material}."))
        .why(why)
        .goal(
            Goal::Material(material.to_owned()),
            format!("the material set to {material}"),
        )
        .highlight("stone_tab")
        .allow(EFFECTS)
}

/// The step that switches the Path-Traced view on, where polish, silk and flashes of color show.
pub(super) fn path_traced(intro: &'static str) -> GuideStep {
    GuideStep::new("Look at it in Path-Traced", intro)
        .actions([
            "Click Path-Traced in the row of view pills above the picture, and give the picture a moment to settle.",
        ])
        .check("The picture shows the stone lit with traced light, and the pills show Path-Traced as chosen, with Solid chosen as well or on its own.")
        .why("The Solid view is flat-shaded and shows none of this. From Solid, the click gives Solid combined with Path-Traced, which shows it too; click Solid to turn the flat layer off.")
        .goal(path_traced_view(), "The Path-Traced view")
        .perform(Perform::ViewMode(1))
        .highlight("view_modes")
        .allow(VIEWPORT)
}

// --- Surface finish ---------------------------------------------------------------------

/// The row of the tier called `name` (case and surrounding spaces do not matter).
fn tier_index(design: &Design, name: &str) -> Option<usize> {
    design.tiers.iter().position(|tier| {
        tier.names()
            .iter()
            .any(|known| known.trim().eq_ignore_ascii_case(name))
    })
}

/// The surface finish stored for the tier called `name`, `None` for an ideal tier.
pub(super) fn tier_surface_of<'a>(design: &'a Design, name: &str) -> Option<&'a TierSurface> {
    design.tier_surface(tier_index(design, name)?)
}

/// The grade of the tier called `name`; `None` when it has no finish (the ideal polish).
pub(super) fn grade_of(design: &Design, name: &str) -> Option<SurfaceGrade> {
    tier_surface_of(design, name).map(|surface| surface.finish.grade)
}

/// Crown Main has the Commercial grade.
pub(super) fn crown_commercial(ctx: &GoalContext<'_>) -> bool {
    grade_of(ctx.design, "Crown Main") == Some(SurfaceGrade::Commercial)
}

/// Crown Main has the Prepolish grade.
pub(super) fn crown_prepolish(ctx: &GoalContext<'_>) -> bool {
    grade_of(ctx.design, "Crown Main") == Some(SurfaceGrade::Prepolish)
}

/// Crown Main is a perfect polish again.
pub(super) fn crown_ideal(ctx: &GoalContext<'_>) -> bool {
    grade_of(ctx.design, "Crown Main").is_none_or(|grade| grade == SurfaceGrade::Ideal)
}

/// Pavilion Main has the Prepolish grade.
pub(super) fn pavilion_prepolish(ctx: &GoalContext<'_>) -> bool {
    grade_of(ctx.design, "Pavilion Main") == Some(SurfaceGrade::Prepolish)
}

/// Pavilion Main has a Custom Angle of its own.
pub(super) fn pavilion_angle_fixed(ctx: &GoalContext<'_>) -> bool {
    tier_surface_of(ctx.design, "Pavilion Main")
        .is_some_and(|surface| surface.finish.lap_deg.is_some())
}

// --- Inclusions -------------------------------------------------------------------------

/// The design has an inclusion preset.
pub(super) const fn inclusions_chosen(ctx: &GoalContext<'_>) -> bool {
    ctx.design.render_effects.inclusions.is_some()
}

/// The inclusions have a Silk Turn of their own.
pub(super) fn silk_turned(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .render_effects
        .inclusions
        .is_some_and(|inclusions| inclusions.turn_deg.is_some())
}

/// The design has the Cat's-Eye Silk preset.
pub(super) fn cats_eye_chosen(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .render_effects
        .inclusions
        .is_some_and(|inclusions| inclusions.preset == "Cat's-Eye Silk")
}

// --- Play-of-color ----------------------------------------------------------------------

/// The design's play-of-color choice and the material's own default for it, judged by `moved`.
fn structure_against_default(
    ctx: &GoalContext<'_>,
    moved: fn(&StructureEffects, &StructureEffects) -> bool,
) -> bool {
    let Some(own) = ctx.design.render_effects.structure else {
        return false;
    };
    let Some(name) = ctx.design.material.name.as_deref() else {
        return false;
    };
    StructureSpec::default_for(name)
        .is_some_and(|spec| moved(&own, &StructureEffects::from_spec(&spec)))
}

/// Play-of-Color (Schiller) is switched on.
pub(super) const fn structure_on(ctx: &GoalContext<'_>) -> bool {
    ctx.design.render_effects.structure.is_some()
}

/// The pattern is Pinfire.
pub(super) fn pattern_pinfire(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .render_effects
        .structure
        .is_some_and(|structure| structure.pattern == StructurePattern::Pinfire)
}

/// The Color Range is not the material's own any more.
pub(super) fn range_moved(ctx: &GoalContext<'_>) -> bool {
    structure_against_default(ctx, |own, default| {
        (own.color_low_nm - default.color_low_nm).abs() > 0.5
            || (own.color_high_nm - default.color_high_nm).abs() > 0.5
    })
}

/// The Coverage is not the material's own any more.
pub(super) fn coverage_moved(ctx: &GoalContext<'_>) -> bool {
    structure_against_default(ctx, |own, default| {
        (own.coverage - default.coverage).abs() > 0.005
    })
}

/// The patches were drawn again with a new seed.
pub(super) fn reseeded(ctx: &GoalContext<'_>) -> bool {
    ctx.design
        .render_effects
        .structure
        .is_some_and(|structure| structure.seed != DEFAULT_SEED)
}
