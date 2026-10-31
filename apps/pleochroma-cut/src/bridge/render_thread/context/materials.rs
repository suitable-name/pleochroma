//! Material resolution and per-frame quality derivation: by-name lookup, the
//! opt-in render-time overrides layered on top of it, and the samples-per-frame the
//! render loop derives from the user's target sample count.
//!
//! By-name resolution ([`resolve_material`]/[`resolve_material_with_override`]) and
//! the override stack ([`MaterialOverrides`]) now live in
//! `pleochroma::render_setup::materials` (re-exported here so nothing else in this
//! crate changes) so the browser app resolves a design's rendered material exactly
//! the same way for the same settings -- see that module's own doc comment.
//! [`apply_material_overrides`] stays a thin adapter here: the pleochroma version is
//! pure (it takes an already-resolved model width), while this one still owns the
//! `StoneWidthCache` lookup, matching this crate's live render loop, which wants to
//! reuse one persistent cache across frames.

use crate::bridge::frame_cache::stone_width::StoneWidthCache;
pub use pleochroma::render_setup::{
    MaterialOverrides, resolve_material, resolve_material_with_override,
};
use pleochroma::{geometry::plane::GpuFacetPlane, optics::materials::GemMaterial, render_setup};

/// Applies every [`MaterialOverrides`] field on top of a resolved base material -- see
/// `pleochroma::render_setup::apply_material_overrides`'s own doc comment for what
/// each field does and why a material with nothing dialled in renders bit-identical to
/// before these controls existed.
///
/// `active_planes`/`width_cache` are consulted whenever the material's absorption unit needs
/// the design's model width (`render_setup::needs_model_width`, now true for every material: a
/// per-millimetre band color or physics recipe for millimetres per model unit, a per-model-unit
/// material -- the built-ins, legacy color triples -- for the reference-width rescale
/// `REFERENCE_MODEL_WIDTH / model_width`) -- passed in rather than looked up internally so the
/// live render loop can reuse one persistent `StoneWidthCache` across frames while a one-shot
/// caller can hand in a fresh one. The `else` branch below is kept for the gate and is never
/// taken today.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
) -> GemMaterial {
    let model_width = if render_setup::needs_model_width(&material) {
        width_cache.ensure(active_planes)
    } else {
        None
    };
    let overrides = reconciled_second_axis(&material, overrides);
    render_setup::apply_material_overrides(material, &overrides, model_width)
}

/// `overrides` with the Second Axis made consistent with the Crystal Axis it travels with.
///
/// A design that sets a Crystal Axis but no Second Axis, on a biaxial material that holds a
/// Second Axis of its own (a custom material's stored default, baked into `alpha_axis`), would
/// keep an alpha direction that belongs to a different Crystal Axis. Such a stone takes the
/// material's own completion about its new axis instead (a turn of zero). Every other case,
/// and so every design that never set an axis, is returned unchanged.
fn reconciled_second_axis(
    material: &GemMaterial,
    overrides: &MaterialOverrides,
) -> MaterialOverrides {
    match overrides.c_axis_override {
        Some(gamma)
            if overrides.alpha_axis_override.is_none()
                && material.alpha_axis.is_some()
                && material.biaxial_delta_beta_alpha.is_some() =>
        {
            MaterialOverrides {
                alpha_axis_override: Some(pleochroma::optics::birefringence::alpha_axis_from_turn(
                    gamma, 0.0,
                )),
                ..*overrides
            }
        }
        _ => *overrides,
    }
}

/// Everything needed to name the material for a frame: the two tables to look a
/// name up in, the name itself, and the editor's already-resolved override that
/// beats both when it is set.
pub struct MaterialSources<'a> {
    /// Built-in gem materials.
    pub materials: &'a [GemMaterial],
    /// User-defined gem materials.
    pub custom_materials: &'a [GemMaterial],
    /// Explicit material that takes precedence over `material_name`.
    pub material_override: Option<&'a GemMaterial>,
    /// Name of the selected material.
    pub material_name: &'a str,
}

/// Resolves the current gem material and applies the user's material overrides.
///
/// [`resolve_material_with_override`] prefers the explicit material over its name.
/// Pass size is scheduled separately, independently of the final sample target.
///
/// Bounce count is not resolved here -- the Parameters panel's "Max Ray Bounces"
/// selector is the only thing controlling it; callers use `RenderContext::max_bounces`
/// directly.
///
/// # Panics (in practice, never)
///
/// Falls back to `GemMaterial::diamond()` if neither the override nor the name
/// resolves. Both of this function's call sites (`render_thread::mod`'s frame loop)
/// only reach it while `SuspensionFlags::material_unresolved` is `false`, which
/// `RenderContext::material_unresolved` guarantees means `material_override` is set
/// or `material_name` names a real material -- so the fallback is unreachable, not a
/// silent substitution of the kind [`resolve_material`] itself now refuses to make.
pub(in crate::bridge::render_thread) fn resolve_frame_material(
    sources: &MaterialSources<'_>,
    overrides: &MaterialOverrides,
    active_planes: &[GpuFacetPlane],
    width_cache: &mut StoneWidthCache,
) -> GemMaterial {
    let current_mat = resolve_material_with_override(
        sources.materials,
        sources.custom_materials,
        sources.material_override,
        sources.material_name,
    )
    .unwrap_or_else(GemMaterial::diamond);
    apply_material_overrides(current_mat, overrides, active_planes, width_cache)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::cuts::StandardGemCuts;

    /// Review fix (absorption units): the material the live loop traces and the HUD/metrics score
    /// (`resolve_frame_material`) is the very material the scoring consumers
    /// build with `render_setup::material_for_stone` for the same stone size -- for a built-in
    /// (`ModelUnit`) and a band colour (`PerMm`, which reads the plane arrangement).
    #[test]
    fn the_live_material_equals_the_scoring_material_for_a_sized_stone() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let builtins = GemMaterial::all_materials();
        let banded = GemMaterial::sapphire().with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0);
        for (name, override_material) in [("Emerald", None), ("Sapphire", Some(&banded))] {
            for stone_width_mm in [0.0_f32, 10.87] {
                let sources = MaterialSources {
                    materials: &builtins,
                    custom_materials: &[],
                    material_override: override_material,
                    material_name: name,
                };
                let overrides = MaterialOverrides {
                    inclusion_sigma_s: 0.0,
                    c_axis_override: None,
                    alpha_axis_override: None,
                    edge_rounding_radius: 0.0,
                    stone_width_mm,
                };
                let live = resolve_frame_material(
                    &sources,
                    &overrides,
                    &planes,
                    &mut StoneWidthCache::new(),
                );
                let bare = resolve_material_with_override(&builtins, &[], override_material, name)
                    .expect("resolves");
                assert_eq!(
                    live,
                    render_setup::material_for_stone(bare, stone_width_mm, &planes),
                    "{name} at {stone_width_mm} mm"
                );
            }
        }
    }

    fn overrides_with(c_axis: Option<glam::Vec3>, alpha: Option<glam::Vec3>) -> MaterialOverrides {
        MaterialOverrides {
            inclusion_sigma_s: 0.0,
            c_axis_override: c_axis,
            alpha_axis_override: alpha,
            edge_rounding_radius: 0.0,
            stone_width_mm: 0.0,
        }
    }

    /// A design that sets a Crystal Axis but no Second Axis drops the Second Axis a biaxial
    /// material holds for a different axis (its completion about the new axis is used); one that
    /// sets nothing, a material without a Second Axis, and a design with its own Second Axis are
    /// all untouched.
    #[test]
    fn a_design_axis_without_a_second_axis_drops_a_baked_second_axis() {
        use pleochroma::optics::birefringence::alpha_axis_from_turn;
        let planes = StandardGemCuts::standard_round_brilliant();
        let apply = |material: GemMaterial, overrides: MaterialOverrides| {
            apply_material_overrides(material, &overrides, &planes, &mut StoneWidthCache::new())
        };
        let mut baked = GemMaterial::by_name("Tanzanite").expect("built-in");
        baked.alpha_axis = Some(alpha_axis_from_turn(glam::Vec3::Y, 60.0));

        let untouched = apply(baked.clone(), overrides_with(None, None));
        assert_eq!(
            untouched.alpha_axis, baked.alpha_axis,
            "no axis set: nothing moves"
        );

        let axis = glam::Vec3::X;
        let dropped = apply(baked.clone(), overrides_with(Some(axis), None));
        assert_eq!(dropped.alpha_axis, Some(alpha_axis_from_turn(axis, 0.0)));

        let own = alpha_axis_from_turn(axis, 25.0);
        let kept = apply(baked, overrides_with(Some(axis), Some(own)));
        assert_eq!(
            kept.alpha_axis,
            Some(own),
            "the design's own Second Axis wins"
        );

        let plain = apply(
            GemMaterial::by_name("Tanzanite").expect("built-in"),
            overrides_with(Some(axis), None),
        );
        assert_eq!(
            plain.alpha_axis, None,
            "a material with no Second Axis keeps none"
        );
    }
}
