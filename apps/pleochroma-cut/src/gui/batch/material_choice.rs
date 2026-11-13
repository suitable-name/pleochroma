//! Which of the materials that fit a design's refractive index its cached previews and
//! tilt curves are rendered in.
//!
//! Refractive index only narrows the field: several presets usually sit within the match
//! tolerance of a design's index and differ in body color, absorption and dispersion.
//! The kept material is the one that serves the stone best across a balanced set of
//! measures, not the one that minimises any single figure: a stone that windows 0% while
//! most of its light is extinguished is a poor match. Windowing, extinction and (flipped)
//! brilliance therefore count equally, each averaged over a table-up and two tilted poses.
//!
//! # Stone size
//!
//! This module only PICKS the material. It scores the candidates unsized
//! (`material_for_stone` at width `0.0`, the 7 mm calibration), because the pick is
//! persisted write-once: it must depend on the design's geometry and index alone.
//!
//! The batches size the picked material by the design's girdle diameter
//! (`super::catalogue_material::catalogue_material`), as the editor's viewport and the
//! exports do. A design whose file names a material is drawn in that material instead (the
//! editor's own rule). A design with neither (an angle-table design, an `.asc` alone, a file
//! without a girdle diameter) keeps the unsized pick.
//!
//! Approximation: for a design that sets a girdle diameter but names no material, the pick
//! was scored at 7 mm, so at a much different size another shortlist candidate could be
//! better. The pick is not redone per size, so it stays stable.

use crate::bridge::preview_render::{PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW};
use pleochroma::{
    color::metrics::evaluate_gem_optical_metrics,
    geometry::plane::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use pleochroma_vault::{db::sqlite::Database, model::material_match::RiPresetCandidate};

/// Camera pitches the candidates are measured at, in degrees: table-up, then two tilts.
const SCORE_PITCHES_DEG: [f32; 3] = [90.0, 60.0, 30.0];

/// The lighting preset the candidates are scored under (the ISO hemisphere).
const SCORE_PRESET: LightingPreset = LightingPreset::IsoHemisphere;

/// `material`'s balanced loss on `planes` (0-100, LOWER is better): the mean over
/// [`SCORE_PITCHES_DEG`] of the equal-weight mean of windowing, extinction and
/// `100 - brilliance`.
#[must_use]
pub fn balanced_loss(planes: &[GpuFacetPlane], material: &GemMaterial) -> f32 {
    let environment = SCORE_PRESET.studio(1.0, PREVIEW_LIGHT_YAW, PREVIEW_LIGHT_PITCH);
    let total: f32 = SCORE_PITCHES_DEG
        .iter()
        .map(|pitch| {
            let m = evaluate_gem_optical_metrics(
                planes,
                material,
                0.0,
                pitch.to_radians(),
                environment,
            );
            (m.windowing_pct + m.extinction_pct + (100.0 - m.brilliance_pct)) / 3.0
        })
        .sum();
    total / SCORE_PITCHES_DEG.len() as f32
}

/// Index of the shortlist entry with the lowest [`balanced_loss`]; the first on a tie
/// and for a name that is not a built-in material, so the choice is deterministic.
#[must_use]
pub fn best_balanced(planes: &[GpuFacetPlane], shortlist: &[&RiPresetCandidate]) -> usize {
    let mut best = (0, f32::INFINITY);
    for (index, candidate) in shortlist.iter().enumerate() {
        let Some(material) = GemMaterial::by_name(&candidate.name) else {
            continue;
        };
        let material = pleochroma::render_setup::material_for_stone(material, 0.0, planes);
        let loss = balanced_loss(planes, &material);
        if loss.is_finite() && loss < best.1 {
            best = (index, loss);
        }
    }
    best.0
}

/// `Database::ensure_preview_material` with the balanced choice among equally fitting
/// materials. Measures nothing when the design already has a material or only one fits.
///
/// # Errors
///
/// The database error of the underlying call.
pub fn ensure_balanced_material(
    db: &Database,
    entry_id: i64,
    target_ri: f64,
    candidates: &[RiPresetCandidate],
    tolerance: f64,
    planes: &[GpuFacetPlane],
) -> anyhow::Result<Option<String>> {
    db.ensure_preview_material_by(
        entry_id,
        target_ri,
        candidates,
        tolerance,
        &mut |shortlist| best_balanced(planes, shortlist),
    )
}
