//! Tier 3 statistical image comparisons of stones with oriented inclusions.
//!
//! Their pipelines compile the oriented estimator in (`ORIENTED_INCLUSIONS`), which a stone
//! without inclusions does not since 2026-10-22: AMD's RADV compiler returned garbage for
//! uniaxial stones while that code was reachable. A uniaxial and an isotropic stone with the
//! same silk tell whether the fault follows the material class.

use crate::optics::{
    inclusions::{CrystalFrame, bragg::StructureSpec, catalogue},
    materials::GemMaterial,
    raytracer::LightingPreset,
};

use super::image_comparison::{
    ImageComparisonResult, run_image_comparison_for, run_image_comparison_hdr_for,
};

/// `material` with the built-in preset `preset` of `preset_owner`'s catalogue arm, resolved in
/// the material's own crystal frame.
pub(super) fn with_preset(material: GemMaterial, preset_owner: &str, preset: &str) -> GemMaterial {
    let def = catalogue::find_preset(preset_owner, preset)
        .unwrap_or_else(|| panic!("{preset_owner:?} has the built-in preset {preset:?}"));
    let set = CrystalFrame::new(material.c_axis, None).resolve(def.families);
    material.with_inclusions(set)
}

/// Ruby with Star Silk: a uniaxial stone with three needle families in the basal plane.
///
/// # Panics
///
/// Panics if `"Ruby"` or its `"Star Silk"` preset is ever removed.
#[must_use]
pub fn run_image_comparison_star_ruby(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let ruby = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()");
    let material = with_preset(ruby, "Ruby", "Star Silk");
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// Ruby with Star Silk under the non-uniform HDR map, where the scattering-point next-event
/// estimate and its MIS partner (the phase density carried to the escape) are both live.
///
/// # Panics
///
/// Panics if `"Ruby"` or its `"Star Silk"` preset is ever removed.
#[must_use]
pub fn run_image_comparison_star_ruby_hdr(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let ruby = GemMaterial::by_name("Ruby")
        .expect("\"Ruby\" is a built-in material in GemMaterial::all_materials()");
    run_image_comparison_hdr_for(ctx, &with_preset(ruby, "Ruby", "Star Silk"))
}

/// The built-in `name` with its default play-of-color structure.
fn with_default_structure(name: &str) -> GemMaterial {
    let spec = StructureSpec::default_for(name)
        .unwrap_or_else(|| panic!("{name:?} has a default play-of-color structure"));
    GemMaterial::by_name(name)
        .unwrap_or_else(|| panic!("{name:?} is a built-in material"))
        .with_structure_spec(&spec)
}

/// Precious Opal (White), the Bragg domain tracker with random lattice normals, under the
/// non-uniform HDR map.
///
/// # Panics
///
/// Panics if `"Precious Opal (White)"` is ever removed.
#[must_use]
pub fn run_image_comparison_opal_hdr(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_hdr_for(ctx, &with_default_structure("Precious Opal (White)"))
}

/// Labradorite, the Bragg domain tracker with one lamella normal in the crystal frame (a
/// biaxial host), under the non-uniform HDR map.
///
/// # Panics
///
/// Panics if `"Labradorite"` is ever removed.
#[must_use]
pub fn run_image_comparison_labradorite_hdr(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    run_image_comparison_hdr_for(ctx, &with_default_structure("Labradorite"))
}

/// The zoned dichroic rim (a uniaxial zoned host) with corundum's Star Silk: the one kind of
/// stone whose pipeline keeps the zoned oriented estimator compiled in (`ZONED_ORIENTED`).
#[must_use]
pub fn run_image_comparison_star_zoned_rim(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let rim = crate::renderer::gpu::zoned_cases::ZonedImageCase::DichroicRim.material();
    let material = with_preset(rim, "Sapphire", "Star Silk");
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// The bicolour zoned opal (a zoned host that also holds a Bragg domain structure).
///
/// The zoned Bragg estimator renders it (`bounce_scatter_sample_zoned_bragg`): the zoned and the
/// structure paths together, under the Daylight studio rig.
#[must_use]
pub fn run_image_comparison_zoned_opal(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = crate::renderer::gpu::zoned_cases::zoned_opal_material();
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}

/// [`run_image_comparison_zoned_opal`] under the non-uniform HDR map.
#[must_use]
pub fn run_image_comparison_zoned_opal_hdr(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let material = crate::renderer::gpu::zoned_cases::zoned_opal_material();
    run_image_comparison_hdr_for(ctx, &material)
}

/// Diamond with corundum's Star Silk: the same families in an isotropic stone, the control for
/// [`run_image_comparison_star_ruby`].
///
/// # Panics
///
/// Panics if `"Diamond"` or corundum's `"Star Silk"` preset is ever removed.
#[must_use]
pub fn run_image_comparison_star_diamond(
    ctx: &crate::renderer::gpu::GpuContext,
) -> ImageComparisonResult {
    let diamond = GemMaterial::by_name("Diamond")
        .expect("\"Diamond\" is a built-in material in GemMaterial::all_materials()");
    let material = with_preset(diamond, "Sapphire", "Star Silk");
    run_image_comparison_for(ctx, &material, &[], LightingPreset::Daylight)
}
