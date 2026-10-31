//! Material resolution and the render-time override stack.
//!
//! By-name lookup and every opt-in override layered on top of it
//! (inclusion/subsurface scattering, crystal-axis orientation, facet edge rounding,
//! physical stone size). Moved out of the desktop viewer unchanged so the browser app
//! resolves a design's rendered material exactly the same way for the same settings --
//! see `crate::render_setup`'s own doc comment.

use super::stone_width::measure_model_width;
use crate::{
    geometry::plane::GpuFacetPlane,
    optics::{
        inclusions::{CrystalFrame, spec::InclusionSpec},
        materials::{AbsorptionUnit, GemMaterial, OpticalCharacter},
        zoning::ZoneFrame,
    },
};
use glam::Vec3;

/// Resolves the current gem material by name: custom materials take priority over the
/// built-in presets.
///
/// Returns `None` when `material_name` matches neither table -- deliberately no
/// `materials[0]` (Diamond) fallback, so a caller cannot silently
/// trace/export/tilt-sweep an unrecognized or unset name as a different stone.
#[must_use]
pub fn resolve_material(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_name: &str,
) -> Option<GemMaterial> {
    custom_materials
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(material_name))
        .or_else(|| {
            materials
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(material_name))
        })
        .cloned()
}

/// Prefers `material_override` over the plain by-name lookup [`resolve_material`]
/// already does.
///
/// Like `resolve_material` -- returns `None` rather than substituting a different
/// stone when neither the override nor the name resolves.
#[must_use]
pub fn resolve_material_with_override(
    materials: &[GemMaterial],
    custom_materials: &[GemMaterial],
    material_override: Option<&GemMaterial>,
    material_name: &str,
) -> Option<GemMaterial> {
    material_override
        .cloned()
        .or_else(|| resolve_material(materials, custom_materials, material_name))
}

/// Every opt-in render-time material override bundled into one struct, to keep call
/// sites' argument lists short.
#[derive(Clone, Copy)]
pub struct MaterialOverrides {
    /// Inclusion/subsurface scattering: the Henyey-Greenstein `sigma_s` applied via
    /// `GemMaterial::with_scattering_amount`. Per model unit, independent of the stone's
    /// size. `0.0` is off.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation: `Some(axis)` replaces the resolved material's
    /// `c_axis` (skipped for isotropic materials); `None` ("as cut") leaves it
    /// untouched.
    pub c_axis_override: Option<Vec3>,
    /// Second crystal-axis direction: `Some(alpha)` replaces the resolved material's
    /// `alpha_axis`, only for a genuinely biaxial material
    /// (`biaxial_delta_beta_alpha.is_some()`); uniaxial and isotropic materials ignore
    /// it. `None` leaves the material's own (normally absent) `alpha_axis` untouched.
    pub alpha_axis_override: Option<Vec3>,
    /// Facet edge (meet-point) rounding radius, via `GemMaterial::with_edge_rounding`.
    /// `0.0` is off (sharp edges).
    pub edge_rounding_radius: f32,
    /// Physical stone size: girdle width in millimetres for the absorption scale. `0.0` is
    /// "not set": a [`AbsorptionUnit::ModelUnit`] material then renders as it was tuned (7 mm,
    /// its swatch color face-up, see [`MODEL_UNIT_FACE_UP_PATH`]; for a design normalised to the
    /// [`REFERENCE_MODEL_WIDTH`]) and a [`AbsorptionUnit::PerMm`] one assumes
    /// [`PHYSICS_DEFAULT_STONE_WIDTH_MM`].
    pub stone_width_mm: f32,
}

/// Applies every [`MaterialOverrides`] field on top of a resolved base material.
///
/// Each one is opt-in and skips its underlying `GemMaterial::with_*` call entirely at
/// its off position, so a material with nothing dialled in renders bit-identical to
/// before these controls existed.
///
/// # Absorption units
///
/// The stone size sets the material's `absorption_path_scale` through
/// [`absorption_path_scale_for`], by the material's own [`AbsorptionUnit`]: a
/// `ModelUnit` material (the built-in table apart from the zoned Ametrine, legacy color
/// triples, fantasy customs) scales by
/// `stone_width_mm / 7 / MODEL_UNIT_FACE_UP_PATH * (REFERENCE_MODEL_WIDTH / model_width)`
/// (the width factor is 1 while unset, so the scale is never left at 1.0: the calibration always
/// applies); a `PerMm` one (band colors, physics recipes, the zoned Ametrine) by
/// `stone_width_mm / model_width`, with 7 mm for an unset width. See [`AbsorptionUnit`].
///
/// The tracer multiplies model-space segment lengths by the scale, and `MODEL_UNIT_FACE_UP_PATH`
/// is measured in the units of a [`REFERENCE_MODEL_WIDTH`]-wide stone, so a `ModelUnit` material
/// needs the design's own width too: a design normalised twice as large has twice the model-space
/// path for the same physical stone and must get half the scale to render the same color.
///
/// `model_width` is the active design's own girdle width in model units, already
/// resolved by the caller (see [`crate::render_setup::measure_model_width`]) -- this
/// function does no plane-arrangement measurement of its own, so a caller with a
/// persistent cache keyed on the design's geometry (the desktop's `StoneWidthCache`)
/// only pays for a remeasure when the design actually changes, and a one-shot caller
/// can just measure fresh and pass the result straight through. Every material reads it
/// ([`needs_model_width`] is always true); a `ModelUnit` material with no measurable width
/// (`None`) is taken as [`REFERENCE_MODEL_WIDTH`] wide, a `PerMm` one keeps its scale.
#[must_use]
pub fn apply_material_overrides(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    model_width: Option<f64>,
) -> GemMaterial {
    apply_material_overrides_with_inclusions(material, overrides, model_width, None)
}

/// [`apply_material_overrides`] plus the design's oriented-inclusion choice.
///
/// `inclusions` (the design's `[render_effects.inclusions]`) is resolved against the material
/// after the crystal orientation has been applied, so the silk follows the Crystal Axis: the
/// families are rotated into the stone frame spanned by the final `c_axis` and the second
/// direction (the biaxial `alpha_axis`, or for a uniaxial stone the stored Second Axis in
/// `overrides.alpha_axis_override`, which is the "Silk Turn" and which the optics never read).
/// A species preset also sets the haze asymmetry `g`.
///
/// # Which stones take the Second Axis
///
/// `alpha_axis_override` reaches the optics only for a genuinely biaxial material; a uniaxial or
/// isotropic one never reads it. The silk and the custom families read it all the same, as the
/// second direction of their crystal frame ([`CrystalFrame`]): a biaxial stone takes its own
/// (overridden) `alpha_axis`, any other stone the stored Second Axis in `alpha_axis_override`
/// (a caller passes it for uniaxial stones too, which is what makes the Silk Turn work there).
/// Families a custom material already carries are turned into the design's frame only when the
/// design turned the stone (a Crystal Axis override, or a Second Axis override on a biaxial
/// stone); a stored Second Axis alone on a uniaxial stone does not move them.
///
/// # Zoned hosts with a Bragg structure
///
/// A zoned host with a Bragg structure (opal or labradorite colour on a zoned stone) takes
/// `maybe_scatter_oriented_zoned_bragg` (CPU, `scattering::oriented_zoned`) and
/// `bounce_scatter_sample_zoned_bragg` (WGSL, `08b_extend_scatter.wgsl`): the structure reflects,
/// the zones keep their absorption per segment, and scattering-point next-event estimation stays
/// off, as for every zoned host.
///
/// `None`, or a spec that resolves to nothing
/// (unknown preset for the material, zero density), is exactly [`apply_material_overrides`], bit
/// for bit.
#[must_use]
pub fn apply_material_overrides_with_inclusions(
    material: GemMaterial,
    overrides: &MaterialOverrides,
    model_width: Option<f64>,
    inclusions: Option<&InclusionSpec>,
) -> GemMaterial {
    // Opt-in only: skipped entirely (not called with 0.0) at the off position.
    let material = if overrides.inclusion_sigma_s > 0.0 {
        material.with_scattering_amount(overrides.inclusion_sigma_s)
    } else {
        material
    };

    // An isotropic material's optic axis is physically meaningless (no birefringence
    // to orient). This guard is what stops a leftover override from a previously
    // selected anisotropic material reaching an isotropic one's `c_axis`.
    let mut material = material;
    let c_turned = if let Some(axis) = overrides.c_axis_override
        && material.optical_character != OpticalCharacter::Isotropic
    {
        material.c_axis = axis;
        true
    } else {
        false
    };
    // The second direction only means something for a genuinely biaxial material (alpha
    // is the `n_alpha` principal axis); a uniaxial or isotropic one has no such axis, so
    // a leftover override can never reach it.
    let alpha_turned = if let Some(alpha) = overrides.alpha_axis_override
        && material.biaxial_delta_beta_alpha.is_some()
    {
        material.alpha_axis = Some(alpha);
        true
    } else {
        false
    };
    let turned = c_turned || alpha_turned;
    // Zones that follow the crystal axis (the built-in Ametrine's sector boundary, a pattern set
    // to Follows Crystal Axis) take the final axis here, so every render path turns them, a
    // built-in included, not only the callers that re-frame a custom material themselves.
    if let Some(zoning) = material.zoning.as_mut()
        && zoning.follows_axis
    {
        zoning.frame = ZoneFrame::along_axis(material.c_axis.as_dvec3());
    }
    // Families a custom material already carries (resolved in its stored orientation) turn with
    // the design's own axes, like the species presets below do by being resolved afterwards.
    if turned && let Some(set) = material.inclusions {
        let second = if material.biaxial_delta_beta_alpha.is_some() {
            material.alpha_axis
        } else {
            overrides.alpha_axis_override
        };
        let to = CrystalFrame::new(material.c_axis, second);
        material = material.with_inclusions(set.rebased(&to));
    }
    // After the orientation (the families live in its frame) and after the haze (a species
    // preset replaces the haze's asymmetry).
    let material = match inclusions {
        Some(spec) => material.with_inclusion_spec(spec, overrides.alpha_axis_override),
        None => material,
    };

    let material = if overrides.edge_rounding_radius > 0.0 {
        material.with_edge_rounding(overrides.edge_rounding_radius)
    } else {
        material
    };

    // A degenerate/unmeasurable plane arrangement (`model_width` is `None`) or a
    // non-finite/non-positive scale leaves the material untouched, rather than risking
    // a NaN/negative path-length multiplier reaching the tracer.
    if let Some(scale) = absorption_path_scale_for(
        material.absorption_unit,
        overrides.stone_width_mm,
        model_width,
    ) {
        return material.with_absorption_path_scale(scale);
    }
    material
}

/// `material` sized for a stone of `stone_width_mm` on `planes` and nothing else dialled in.
///
/// The stone-size step of [`apply_material_overrides`] alone, measuring the plane arrangement
/// when the material's unit needs it ([`needs_model_width`]: every unit does).
///
/// For the callers that score or sweep a stone outside the render loop (tone/tilt metrics, the
/// optimizer, comparisons): they must see the absorption the render shows, so they take the
/// same size rule instead of tracing the bare resolved material. `stone_width_mm == 0.0` is the
/// render's "not set" (a `ModelUnit` material then gets the calibration scale
/// `1 / MODEL_UNIT_FACE_UP_PATH`, times `REFERENCE_MODEL_WIDTH / model_width` of `planes`).
#[must_use]
pub fn material_for_stone(
    material: GemMaterial,
    stone_width_mm: f32,
    planes: &[GpuFacetPlane],
) -> GemMaterial {
    material_for_stone_oriented(material, stone_width_mm, planes, None)
}

/// [`material_for_stone`] with the stone's own crystal orientation applied.
///
/// `orientation` is `(gamma, alpha)`, the c-axis and the optional second direction (the
/// pair `CrystalOrientation::to_override` returns). `None` is exactly
/// [`material_for_stone`]. The orientation goes through the same override rules as the
/// render ([`apply_material_overrides`]): an isotropic material ignores the c-axis, and
/// only a biaxial one takes the second direction.
#[must_use]
pub fn material_for_stone_oriented(
    material: GemMaterial,
    stone_width_mm: f32,
    planes: &[GpuFacetPlane],
    orientation: Option<(Vec3, Option<Vec3>)>,
) -> GemMaterial {
    let model_width = if needs_model_width(&material) {
        measure_model_width(planes)
    } else {
        None
    };
    apply_material_overrides(
        material,
        &MaterialOverrides {
            inclusion_sigma_s: 0.0,
            c_axis_override: orientation.map(|(gamma, _)| gamma),
            alpha_axis_override: orientation.and_then(|(_, alpha)| alpha),
            edge_rounding_radius: 0.0,
            stone_width_mm,
        },
        model_width,
    )
}

/// The stone width in millimetres the absorption was calibrated for, and the width assumed
/// when none is set: 7 mm.
///
/// A [`AbsorptionUnit::ModelUnit`] material renders exactly as it was tuned at this width; a
/// [`AbsorptionUnit::PerMm`] material uses it when no stone size is set.
pub const PHYSICS_DEFAULT_STONE_WIDTH_MM: f32 = 7.0;

/// The girdle width, in model units, that every `ModelUnit` absorption number is tuned for.
///
/// It is the standard round brilliant's (girdle radius 1.0). `MODEL_UNIT_FACE_UP_PATH` is measured
/// in the units of a stone this wide; a design normalised to another width is rescaled by
/// `REFERENCE_MODEL_WIDTH / model_width` so the same physical stone renders the same color.
pub const REFERENCE_MODEL_WIDTH: f64 = 2.0;

/// The representative face-up light path, in model units of a [`REFERENCE_MODEL_WIDTH`]-wide
/// stone, that a [`AbsorptionUnit::ModelUnit`] absorption's "one unit" swatch stands for.
///
/// Built-in colored materials, the toolbar color presets, legacy `absorption_rgb` triples and
/// fantasy customs are tuned so their swatch looks right over ONE model unit of path
/// (`pleochroma_cut_core::material::color::FANTASY_PATH_UNITS`), a unit being one unit of a
/// [`REFERENCE_MODEL_WIDTH`]-wide stone. Light that leaves a face-up stone
/// has travelled several chords (the tone metric's `mean_path_units`, about 2.5 for a round
/// brilliant), so rendering those numbers at scale 1.0 is several times too dark (the 2026-10-08
/// 10.87 mm green cubic zirconia came out black). The `ModelUnit` scale is therefore divided by
/// this constant: absorption is linear in the scale, so over `MODEL_UNIT_FACE_UP_PATH` units of
/// path the stone shows exactly the color the swatch shows over one unit, at 7 mm or with no
/// size set, and a larger or smaller stone scales physically from there (`width / 7`). A design
/// whose own model width differs from the reference is rescaled by
/// `REFERENCE_MODEL_WIDTH / model_width` on top, so a file's normalisation never changes the color.
///
/// Chosen from the tone metric (`FaceUpTone::mean_path_units`, light tent, colorless stone) for
/// the standard round brilliant and the emerald cut, each rescaled to the reference width; the test
/// `model_unit_face_up_path_matches_the_measured_cuts` re-measures it and prints the values.
/// Measured 2026-10-08 by that test (before the rescale, the emerald cut being 2.2 wide): round
/// brilliant mean path 2.51 (mixture-equivalent 2.54), emerald cut 2.52 (equivalent 2.49), median
/// 2.52. Re-pinned 2026-10-27 from the owner's Windows run with the rescale in place: round
/// brilliant 2.510 (equivalent 2.54), emerald cut 3.152 at rescale 1.25 (equivalent 3.12), median
/// 2.831 (equivalent median 2.831), so colored `ModelUnit` stones render about 11 % lighter than
/// at 2.52.
/// Stored numbers (vault `absorption_rgb`, `.toml` `body_color_override`, the built-in band peaks)
/// are NOT changed: the constant is applied once, here. The color-to-triple solver
/// (`nearest_legacy_rgb`) keeps solving at one unit, so its outputs stay comparable with stored
/// triples and with the swatch.
pub const MODEL_UNIT_FACE_UP_PATH: f32 = 2.83;

/// Returns the effective stone width in mm: the requested one, or the 7 mm default
/// ([`PHYSICS_DEFAULT_STONE_WIDTH_MM`]) when `requested_mm` is 0.0 (unspecified).
#[must_use]
pub fn effective_stone_width_mm(requested_mm: f32) -> f32 {
    if requested_mm > 0.0 {
        requested_mm
    } else {
        PHYSICS_DEFAULT_STONE_WIDTH_MM
    }
}

/// Whether [`apply_material_overrides`] reads the design's model-unit girdle width for
/// `material`.
///
/// Every material does, a [`AbsorptionUnit::ModelUnit`] one for the
/// [`REFERENCE_MODEL_WIDTH`] rescale and a [`AbsorptionUnit::PerMm`] one for millimetres per
/// unit. Kept as a function so the callers that gate the plane measurement on it compile
/// unchanged (the gate is now always open).
#[must_use]
pub const fn needs_model_width(_material: &GemMaterial) -> bool {
    true
}

/// The `absorption_path_scale` a stone of `stone_width_mm` gets for absorption numbers of
/// `unit`, or `None` when the material keeps the scale it has.
///
/// * [`AbsorptionUnit::ModelUnit`]:
///   `(effective width / 7 mm) / MODEL_UNIT_FACE_UP_PATH * (REFERENCE_MODEL_WIDTH / model_width)`.
///   The tracer multiplies model-space segment lengths by the scale, and the face-up path
///   constant is measured in the units of a [`REFERENCE_MODEL_WIDTH`]-wide stone, so the design's
///   own width rescales it: the same physical stone renders the same color however the file is
///   normalised. A missing or non-positive `model_width` is taken as [`REFERENCE_MODEL_WIDTH`]
///   (today's calibrated look, never the raw scale 1.0 of the 2026-10-08 black stone). While no
///   width is set (`stone_width_mm <= 0`) the width factor is 1.0, so an unset size and 7 mm both
///   give `1 / MODEL_UNIT_FACE_UP_PATH` for a reference-width design: over the face-up path the
///   stone then shows the color the swatch shows over one model unit.
/// * [`AbsorptionUnit::PerMm`]: `effective width / model_width` (millimetres per model unit),
///   the effective width being 7 mm while none is set. `None` when `model_width` is missing or
///   not positive.
///
/// `None` as well for a non-finite or non-positive result.
#[must_use]
pub fn absorption_path_scale_for(
    unit: AbsorptionUnit,
    stone_width_mm: f32,
    model_width: Option<f64>,
) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the render precision; a width ratio is in the normal float range"
    )]
    let scale = match unit {
        AbsorptionUnit::ModelUnit => {
            // `effective_stone_width_mm` maps an unset (0.0) width to 7 mm, i.e. factor 1.0; a
            // NaN or negative width is treated as unset too.
            let width_mm = if stone_width_mm.is_nan() || stone_width_mm <= 0.0 {
                PHYSICS_DEFAULT_STONE_WIDTH_MM
            } else {
                stone_width_mm
            };
            let model_width = model_width
                .filter(|w| *w > 1e-9)
                .unwrap_or(REFERENCE_MODEL_WIDTH);
            // The width ratio is f64 and multiplies last: a model width of exactly 2.0 gives
            // exactly 1.0, so a reference-width design's scale is bit-identical to before.
            let base = width_mm / PHYSICS_DEFAULT_STONE_WIDTH_MM / MODEL_UNIT_FACE_UP_PATH;
            let ratio = REFERENCE_MODEL_WIDTH / model_width;
            (f64::from(base) * ratio) as f32
        }
        AbsorptionUnit::PerMm => {
            let model_width = model_width.filter(|w| *w > 1e-9)?;
            let width_mm = f64::from(effective_stone_width_mm(stone_width_mm));
            (width_mm / model_width) as f32
        }
    };
    (scale.is_finite() && scale > 0.0).then_some(scale)
}

#[cfg(test)]
mod tests;
