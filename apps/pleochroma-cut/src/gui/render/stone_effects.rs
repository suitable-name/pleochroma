//! The bridge between the per-design stone render effects
//! ([`pleochroma_cut_core::StoneRenderEffects`], stored on the `Design` and in the native
//! file) and the live render context plus the controls that mirror it.
//!
//! The Stone tab drives the picture live (`render::material`), and a debounced commit writes
//! the controls' values into the design as one undoable `Edit::SetRenderEffects`
//! (`editor::callbacks::tier_actions::viewport_material`). The editor refresh pushes the
//! design's own effects back into the context and the controls, except while a live edit
//! waits for its commit ([`live_edit_pending`]) so a slider drag is never overwritten.
//! Nothing here touches the settings file: the effects live in the design, and the old
//! `AppSettings` fields are read-only legacy values.

use crate::{
    CrystalAxisModel, MainWindow, SettingsModel,
    bridge::{
        render_effects::inclusion_second_axis,
        render_thread::{RenderContext, resolve_material_with_override},
    },
    gui::{optics::c_axis, tutorial_events::raise},
};
use glam::Vec3;
use pleochroma::{optics::materials::GemMaterial, render_setup::inclusion_scale};
use pleochroma_cut_core::{Design, Edit, StoneRenderEffects};
use pleochroma_editor::{guide::viewing_events as events, stone_orientation::orientation_override};
use pleochroma_net::scene::effects::EdgeSpec;
use slint::ComponentHandle;
use std::sync::atomic::{AtomicBool, Ordering};

/// A stored render-effect value (f64 in the design) at the f32 precision the render
/// context and the Slint controls use.
#[expect(
    clippy::cast_possible_truncation,
    reason = "render effects are small bounded values (angles, radii, sigma); f32 is the render and control precision"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

/// Set by a Stone-tab control while its change is shown but not yet written to the design.
static LIVE_EDIT_PENDING: AtomicBool = AtomicBool::new(false);

/// Serialises the tests that touch the process-wide pending flag.
#[cfg(test)]
pub(in crate::gui) static PENDING_FLAG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A control changed the picture and its commit to the design is still to come.
pub(in crate::gui) fn mark_live_edit_pending() {
    LIVE_EDIT_PENDING.store(true, Ordering::SeqCst);
}

/// The pending live edit was committed (or dropped).
pub(in crate::gui) fn clear_live_edit_pending() {
    LIVE_EDIT_PENDING.store(false, Ordering::SeqCst);
}

/// Whether a live edit waits for its commit: the editor refresh must not push the design's
/// effects over the controls meanwhile.
#[must_use]
pub(in crate::gui) fn live_edit_pending() -> bool {
    LIVE_EDIT_PENDING.load(Ordering::SeqCst)
}

/// The effects the Stone tab's controls show, as the design's own type. Read from the controls
/// (their exact `f32` values), not from the render context, so a committed value converts back
/// to exactly what the context already holds.
///
/// The render-physics parts (oriented inclusions, play-of-color, abraded edges) are read from
/// their own controls ([`super::physics_effects_ui::physics_from_controls`]); `base` (the open
/// design's own effects) only supplies the exact stored values a control still holds, so an
/// unchanged control writes no history entry.
#[must_use]
pub(in crate::gui) fn effects_from_controls(
    ui: &MainWindow,
    base: &StoneRenderEffects,
) -> StoneRenderEffects {
    let settings = ui.global::<SettingsModel>();
    let axis = ui.global::<CrystalAxisModel>();
    StoneRenderEffects::from_render_values(
        settings.get_inclusion_sigma_s(),
        settings.get_c_axis_override_enabled(),
        settings.get_c_axis_tilt_deg(),
        settings.get_c_axis_azimuth_deg(),
        second_turn_from_controls(axis.get_second_set(), axis.get_second_deg()),
        settings.get_girdle_frosted(),
        settings.get_edge_rounding_radius(),
    )
    .with_physics_of(&super::physics_effects_ui::physics_from_controls(ui, base))
}

/// The Second Axis turn the Stone tab's controls stand for: the value of the turn control while
/// its "set" switch is on, nothing while it is off (the material's own second direction).
#[must_use]
pub(in crate::gui) const fn second_turn_from_controls(set: bool, degrees: f32) -> Option<f32> {
    if set { Some(degrees) } else { None }
}

/// What the Second Axis controls show for `effects`: `(set, degrees)`. The inverse of
/// [`second_turn_from_controls`], so a Second Axis loaded from a file survives the write-back of
/// the controls (`effects_from_controls`) unchanged.
#[must_use]
pub(in crate::gui) fn second_controls_for(effects: &StoneRenderEffects) -> (bool, f32) {
    effects
        .c_axis_second_azimuth_deg
        .map_or((false, 0.0), |turn| (true, to_f32(turn)))
}

/// The orientation `effects` gives the material the context traces (`None` unless the design's
/// override is on and the material is anisotropic): the Crystal Axis and the optional Second
/// Axis, exactly as the scoring scenes resolve them (`stone_orientation::orientation_override`).
/// Resolving the material is only paid for while the override is on.
fn context_orientation(
    ctx: &RenderContext,
    effects: &StoneRenderEffects,
) -> Option<(Vec3, Option<Vec3>)> {
    if !effects.c_axis_override_enabled {
        return None;
    }
    let material: GemMaterial = resolve_material_with_override(
        &GemMaterial::all_materials(),
        &ctx.custom_materials,
        ctx.material_override.as_ref(),
        &ctx.material_name,
    )?;
    // Only a biaxial stone has a Second Axis for the optics, and a uniaxial stone whose material
    // carries inclusion families reads it as the Silk Turn those families are rebased with
    // (`render_setup::apply_material_overrides`; the web passes it for every stone). For any
    // other material the render ignores it, so keeping it out of the context keeps a turn from
    // restarting an unchanged render.
    let reads_alpha = material.biaxial_delta_beta_alpha.is_some() || material.inclusions.is_some();
    orientation_override(effects, &material)
        .map(|(gamma, alpha)| (gamma, alpha.filter(|_| reads_alpha)))
}

/// The Second Axis (alpha) the render context should carry for `effects`: only a biaxial stone,
/// or one whose material carries inclusion families, with the override on has one. The live
/// Stone-tab handlers call this after changing the Crystal Axis, because alpha is derived from
/// it.
#[must_use]
pub(in crate::gui) fn alpha_override_for(
    ctx: &RenderContext,
    effects: &StoneRenderEffects,
) -> Option<Vec3> {
    context_orientation(ctx, effects).and_then(|(_, alpha)| alpha)
}

/// The stored Second Axis the silk of a uniaxial stone reads as its turn, for the effects'
/// Crystal Axis: `None` unless the override is on and the material is uniaxial (a biaxial stone
/// carries its own).
fn inclusion_second_axis_for(
    ctx: &RenderContext,
    effects: &StoneRenderEffects,
) -> Option<[f32; 3]> {
    if !effects.c_axis_override_enabled {
        return None;
    }
    let material: GemMaterial = resolve_material_with_override(
        &GemMaterial::all_materials(),
        &ctx.custom_materials,
        ctx.material_override.as_ref(),
        &ctx.material_name,
    )?;
    inclusion_second_axis(effects, &material).map(|axis| axis.to_array())
}

/// Brings the Second Axis the context's silk reads in line with `effects`' Crystal Axis and the
/// material the context traces (a no-op without inclusions). The Second Axis derives from both,
/// so the live Stone-tab handlers and a material change call this after changing either.
pub(in crate::gui) fn refresh_inclusion_axis(
    ctx: &mut RenderContext,
    effects: &StoneRenderEffects,
) {
    if ctx.scene_effects.inclusions.is_some() {
        ctx.scene_effects.inclusion_second_axis = inclusion_second_axis_for(ctx, effects);
    }
}

/// Whether the render physics the context carries is `effects`' own: the inclusions, the
/// play-of-color, the abraded edge and (while inclusions are on) the Second Axis they read.
fn physics_matches(ctx: &RenderContext, effects: &StoneRenderEffects) -> bool {
    let held = &ctx.scene_effects;
    held.inclusions == effects.inclusion_spec()
        && held.structure == effects.structure_spec()
        && held.edge == EdgeSpec::from_abrasion(&effects.edge_abrasion())
        && (effects.inclusions.is_none()
            || held.inclusion_second_axis == inclusion_second_axis_for(ctx, effects))
}

/// Whether two optional directions are the same vector (to a tolerance far below a slider step).
fn same_direction(a: Option<Vec3>, b: Option<Vec3>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => (a - b).length() < 1e-5,
        _ => false,
    }
}

/// Whether the render context already traces with `effects`, up to the rounding the design's
/// stored values carry (six decimals), so a committed value does not look "changed" and restart
/// the render when the editor refresh compares it. The Second Axis counts: a design that carries
/// one is not "matched" by a context without it.
#[must_use]
pub(in crate::gui) fn context_matches(ctx: &RenderContext, effects: &StoneRenderEffects) -> bool {
    const EPS: f32 = 2e-6;
    let axis_matches = match (ctx.c_axis_override, effects.c_axis_override_enabled) {
        (None, false) => true,
        (Some(have), true) => {
            let want = c_axis::angles_to_c_axis(
                to_f32(effects.c_axis_tilt_deg),
                to_f32(effects.c_axis_azimuth_deg),
            );
            (have - want).length() < 1e-5
        }
        _ => false,
    };
    axis_matches
        && same_direction(ctx.alpha_axis_override, alpha_override_for(ctx, effects))
        && (ctx.inclusion_sigma_s - to_f32(effects.inclusion_sigma_s)).abs() <= EPS
        && ctx.girdle_frosted == effects.girdle_frosted
        && (ctx.edge_rounding_radius - to_f32(effects.edge_rounding_radius)).abs() <= EPS
        && physics_matches(ctx, effects)
}

/// Pushes `effects` into the render context (and marks it dirty so the render restarts): the
/// controls' values ([`apply_control_values_to_context`]) and the render physics of the design.
pub(in crate::gui) fn apply_effects_to_context(
    ctx: &mut RenderContext,
    effects: &StoneRenderEffects,
) {
    apply_control_values_to_context(ctx, effects);
    let second_axis = if effects.inclusions.is_some() {
        inclusion_second_axis_for(ctx, effects)
    } else {
        None
    };
    ctx.set_design_physics(
        effects.inclusion_spec(),
        second_axis,
        effects.structure_spec(),
        EdgeSpec::from_abrasion(&effects.edge_abrasion()),
    );
}

/// Pushes the values the Stone tab's controls carry into the render context and marks it dirty.
/// The render physics the context holds stays (the controls have no say in it), except that the
/// Second Axis the silk reads follows the Crystal Axis.
pub(in crate::gui) fn apply_control_values_to_context(
    ctx: &mut RenderContext,
    effects: &StoneRenderEffects,
) {
    ctx.inclusion_sigma_s = to_f32(effects.inclusion_sigma_s);
    ctx.c_axis_override = effects.c_axis_override_enabled.then(|| {
        c_axis::angles_to_c_axis(
            to_f32(effects.c_axis_tilt_deg),
            to_f32(effects.c_axis_azimuth_deg),
        )
    });
    ctx.alpha_axis_override = alpha_override_for(ctx, effects);
    refresh_inclusion_axis(ctx, effects);
    ctx.girdle_frosted = effects.girdle_frosted;
    ctx.edge_rounding_radius = to_f32(effects.edge_rounding_radius);
    // Color patterns that follow the Crystal Axis turn with it, whichever path set the axis
    // (the Stone tab, the overlay's handles, a reset, the editor refresh, a library row).
    crate::gui::color_pattern::follow::follow_context(ctx);
    ctx.dirty = true;
}

/// Shows `effects` in the controls that mirror them. Setting a property from Rust does not
/// fire the control's own "changed" callback, so nothing is written back by this.
pub(in crate::gui) fn push_effects_to_controls(ui: &MainWindow, effects: &StoneRenderEffects) {
    let settings = ui.global::<SettingsModel>();
    let sigma_s = to_f32(effects.inclusion_sigma_s);
    settings.set_inclusion_sigma_s(sigma_s);
    settings.set_inclusion_position(inclusion_scale::sigma_s_to_position(sigma_s));
    settings.set_c_axis_override_enabled(effects.c_axis_override_enabled);
    settings.set_c_axis_tilt_deg(to_f32(effects.c_axis_tilt_deg));
    settings.set_c_axis_azimuth_deg(to_f32(effects.c_axis_azimuth_deg));
    let (second_set, second_deg) = second_controls_for(effects);
    let axis = ui.global::<CrystalAxisModel>();
    axis.set_second_set(second_set);
    axis.set_second_deg(second_deg);
    settings.set_girdle_frosted(effects.girdle_frosted);
    settings.set_edge_rounding_radius(to_f32(effects.edge_rounding_radius));
    super::physics_effects_ui::push_physics(ui, effects);
}

/// The undoable edit that sets `design`'s render effects to `effects`, or `None` when they
/// already are exactly that (so an unchanged control writes no history entry).
#[must_use]
pub(in crate::gui) fn effects_edit(design: &Design, effects: StoneRenderEffects) -> Option<Edit> {
    let effects = effects.sanitised();
    (design.render_effects != effects).then_some(Edit::SetRenderEffects { effects })
}

/// Tells the open tutorial which Stone-tab effects a committed edit changed: the inclusion
/// haze, the frosted girdle and the edge rounding each have their own event. Called once the
/// edit is in the design, so the lesson step completes on the change that can be undone.
pub(in crate::gui) fn raise_effect_events(
    ui: &MainWindow,
    before: &StoneRenderEffects,
    after: &StoneRenderEffects,
) {
    if before.inclusion_sigma_s.to_bits() != after.inclusion_sigma_s.to_bits() {
        raise(ui, events::STONE_HAZE_CHANGED);
    }
    if before.girdle_frosted != after.girdle_frosted {
        raise(ui, events::STONE_GIRDLE_FROSTED_CHANGED);
    }
    if before.edge_rounding_radius.to_bits() != after.edge_rounding_radius.to_bits() {
        raise(ui, events::STONE_EDGE_ROUNDING_CHANGED);
    }
}

/// The edits that carry the legacy app-level stone values onto a design that has neither a
/// `[render_effects]` table nor a girdle diameter (see
/// `AppSettings::legacy_stone_defaults`): one `SetRenderEffects` when the legacy effects are
/// not the default, one `SetGirdleDiameterMm` when a legacy width was set. Empty for a
/// design that already has its own values.
#[must_use]
pub(in crate::gui) fn legacy_migration_edits(
    design: &Design,
    legacy: &crate::settings::model::LegacyStoneDefaults,
) -> Vec<Edit> {
    let mut edits = Vec::new();
    if design.render_effects.is_default() && !legacy.effects.is_default() {
        edits.push(Edit::SetRenderEffects {
            effects: legacy.effects,
        });
    }
    if design.girdle_diameter_mm.is_none() && legacy.stone_width_mm.is_some() {
        edits.push(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: legacy.stone_width_mm,
        });
    }
    edits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, SettingsFile};
    use pleochroma_editor::EditorSession;

    fn haze() -> StoneRenderEffects {
        StoneRenderEffects::from_render_values(0.5, true, 30.0, 120.0, None, true, 0.01)
    }

    #[test]
    fn a_context_set_from_effects_matches_them_exactly() {
        let mut ctx = RenderContext::default();
        assert!(context_matches(&ctx, &StoneRenderEffects::default()));
        assert!(!context_matches(&ctx, &haze()));
        apply_effects_to_context(&mut ctx, &haze());
        assert!(context_matches(&ctx, &haze()));
    }

    /// A settings file written before the effects and the width moved into the design: all six
    /// legacy effect keys and the width are off their defaults.
    const OLD_FILE: &str = "[settings]\ninclusion_sigma_s = 0.5\ngirdle_frosted = true\n\
        c_axis_override_enabled = true\nc_axis_tilt_deg = 30.0\nc_axis_azimuth_deg = 120.0\n\
        edge_rounding_radius = 0.01\nstone_width_mm = 7.5\n";

    fn old_settings() -> AppSettings {
        toml::from_str::<SettingsFile>(OLD_FILE)
            .expect("an old settings file parses")
            .settings
    }

    #[test]
    fn the_old_app_level_values_are_applied_to_the_first_design_once_and_then_ignored() {
        let mut settings = old_settings();
        let mut first = EditorSession::fresh();
        assert!(first.design.render_effects.is_default());
        assert_eq!(first.design.girdle_diameter_mm, None);

        let legacy = settings
            .legacy_stone_defaults()
            .expect("the old values are offered");
        let edits = legacy_migration_edits(&first.design, &legacy);
        assert_eq!(edits.len(), 2, "the effects and the width");
        first
            .apply(Edit::Batch(edits))
            .expect("the migration applies");
        assert_eq!(first.design.girdle_diameter_mm, Some(7.5));
        let effects = first.design.render_effects;
        assert_eq!(effects.inclusion_sigma_s.to_bits(), 0.5f64.to_bits());
        assert!(effects.girdle_frosted && effects.c_axis_override_enabled);
        assert!(effects.edge_rounding_radius > 0.0);

        // The whole migration is one undo step.
        first.undo().expect("undoes").expect("something to undo");
        assert!(first.design.render_effects.is_default());
        assert_eq!(first.design.girdle_diameter_mm, None);

        // Once the flag is set the settings offer nothing, so a second design starts clean.
        settings.mark_stone_effects_migrated();
        assert_eq!(settings.legacy_stone_defaults(), None);
        let second = EditorSession::fresh();
        let later = settings
            .legacy_stone_defaults()
            .map(|legacy| legacy_migration_edits(&second.design, &legacy))
            .unwrap_or_default();
        assert_eq!(later.len(), 0);
        // The legacy keys stay in the file, untouched and unread.
        assert!(settings.girdle_frosted);
        assert_eq!(settings.stone_width_mm.to_bits(), 7.5f32.to_bits());
    }

    #[test]
    fn the_migration_never_overwrites_values_a_design_already_has() {
        let legacy = old_settings()
            .legacy_stone_defaults()
            .expect("the old values are offered");
        let mut session = EditorSession::fresh();
        session
            .apply(Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(6.0),
            })
            .expect("the width applies");
        let edits = legacy_migration_edits(&session.design, &legacy);
        assert_eq!(edits.len(), 1, "only the effects: the design has its width");
        assert!(matches!(edits[0], Edit::SetRenderEffects { .. }));
        session
            .apply(Edit::SetRenderEffects { effects: haze() })
            .expect("the effects apply");
        assert_eq!(legacy_migration_edits(&session.design, &legacy).len(), 0);
        assert_eq!(session.design.girdle_diameter_mm, Some(6.0));
    }

    #[test]
    fn an_unchanged_control_writes_no_history_entry_and_a_changed_one_is_undoable() {
        let mut session = EditorSession::fresh();
        assert!(effects_edit(&session.design, StoneRenderEffects::default()).is_none());
        let edit = effects_edit(&session.design, haze()).expect("a change is an edit");
        session.apply(edit).expect("the effects apply");
        assert_eq!(session.design.render_effects, haze().sanitised());
        assert!(effects_edit(&session.design, haze()).is_none());
        session.undo().expect("undoes").expect("something to undo");
        assert!(session.design.render_effects.is_default());
    }

    /// A design that carries a Second Axis (loaded from a file) and an axis override.
    fn with_second_axis() -> StoneRenderEffects {
        StoneRenderEffects {
            c_axis_override_enabled: true,
            c_axis_tilt_deg: 30.0,
            c_axis_azimuth_deg: 120.0,
            c_axis_second_azimuth_deg: Some(41.25),
            ..StoneRenderEffects::default()
        }
        .sanitised()
    }

    #[test]
    fn a_second_axis_loaded_from_a_file_survives_the_write_back_of_the_controls() {
        let loaded = with_second_axis();
        // What the Stone tab shows for it, and what the controls hand back on a commit.
        let (set, degrees) = second_controls_for(&loaded);
        assert!(set);
        assert_eq!(degrees.to_bits(), 41.25f32.to_bits());
        let written_back = StoneRenderEffects::from_render_values(
            0.0,
            true,
            30.0,
            120.0,
            second_turn_from_controls(set, degrees),
            false,
            0.0,
        );
        assert_eq!(written_back, loaded);
        // So the GUI's write-back is no edit at all, and the Second Axis stays in the design.
        let mut session = EditorSession::fresh();
        session
            .apply(Edit::SetRenderEffects { effects: loaded })
            .expect("the effects apply");
        assert!(effects_edit(&session.design, written_back).is_none());
        assert_eq!(
            session.design.render_effects.c_axis_second_azimuth_deg,
            Some(41.25)
        );
        // A design without one shows the switch off, and the controls hand back no turn.
        let (set, degrees) = second_controls_for(&StoneRenderEffects::default());
        assert!(!set);
        assert_eq!(second_turn_from_controls(set, degrees), None);
    }

    #[test]
    fn the_context_carries_the_second_axis_and_notices_when_it_differs() {
        let effects = with_second_axis();
        let mut ctx = RenderContext::default();
        assert_eq!(
            ctx.alpha_axis_override, None,
            "a new context has no Second Axis"
        );
        // Tanzanite is biaxial: its Second Axis is the context's alpha override.
        ctx.material_name = "Tanzanite".to_string();
        apply_effects_to_context(&mut ctx, &effects);
        let alpha = ctx.alpha_axis_override.expect("the Second Axis is applied");
        let gamma = ctx.c_axis_override.expect("the Crystal Axis is applied");
        assert!(
            gamma.dot(alpha).abs() < 1e-4,
            "alpha is perpendicular to gamma"
        );
        assert!(context_matches(&ctx, &effects));

        // Another turn is a different stone, and a design without the turn is too.
        let other = StoneRenderEffects {
            c_axis_second_azimuth_deg: Some(100.0),
            ..effects
        };
        assert!(!context_matches(&ctx, &other));
        let without = StoneRenderEffects {
            c_axis_second_azimuth_deg: None,
            ..effects
        };
        assert!(!context_matches(&ctx, &without));

        // Overriding the axis off takes the Second Axis with it.
        apply_effects_to_context(&mut ctx, &StoneRenderEffects::default());
        assert_eq!(ctx.alpha_axis_override, None);
        assert_eq!(ctx.c_axis_override, None);
    }

    #[test]
    fn a_uniaxial_material_has_no_second_axis_in_the_context() {
        let mut ctx = RenderContext::default();
        assert_eq!(
            ctx.c_axis_override, None,
            "a new context has no axis override"
        );
        ctx.material_name = "Sapphire".to_string();
        apply_effects_to_context(&mut ctx, &with_second_axis());
        assert_eq!(ctx.alpha_axis_override, None);
        assert!(ctx.c_axis_override.is_some());
        assert!(context_matches(&ctx, &with_second_axis()));
    }

    /// A uniaxial custom material carrying its own inclusion families.
    fn uniaxial_with_families() -> GemMaterial {
        use pleochroma::optics::inclusions::{CrystalDir, CrystalFrame, Family, FamilyKind};
        let family = Family {
            kind: FamilyKind::Needle,
            dir: CrystalDir::AlongC,
            sigma: 0.2,
            p: 2,
            lobe: 0.05,
            rho: 0.0,
        };
        GemMaterial::new_custom("Silky Corundum", 1.77, 0.018, -0.008, [0.0; 3])
            .with_inclusions(CrystalFrame::new(Vec3::Y, None).resolve(&[family]))
    }

    /// The Silk Turn reaches a uniaxial custom material's stored families on the desktop just as
    /// it does on the web: the context carries the design's Second Axis for such a stone, so
    /// `apply_material_overrides` rebases the families about it instead of `(c, None)`.
    #[test]
    fn a_uniaxial_material_with_families_carries_the_second_axis_in_the_context() {
        let material = uniaxial_with_families();
        assert!(material.biaxial_delta_beta_alpha.is_none());
        let mut ctx = RenderContext {
            material_override: Some(material),
            ..RenderContext::default()
        };
        let effects = with_second_axis();
        apply_effects_to_context(&mut ctx, &effects);
        let alpha = ctx
            .alpha_axis_override
            .expect("the Second Axis is applied to the families");
        let gamma = ctx.c_axis_override.expect("the Crystal Axis is applied");
        assert!(
            gamma.dot(alpha).abs() < 1e-4,
            "alpha is perpendicular to gamma"
        );
        assert!(context_matches(&ctx, &effects));
        // Another turn is a different stone for such a material.
        let other = StoneRenderEffects {
            c_axis_second_azimuth_deg: Some(100.0),
            ..effects
        };
        assert!(!context_matches(&ctx, &other));
    }
}
