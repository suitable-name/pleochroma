//! Scintillation: the spatial (per-pose grid contrast) and temporal
//! (per-cell, across-pose flicker) terms, combined into `scintillation_pct`.

use super::{
    birefringent::{Line, MODE_WEIGHT, Mode, trace_mode},
    camera::camera_view_basis,
    fan::FanGeometry,
    lighting::ExitLighting,
    ray_trace::{EntryRay, RayFate, StoneArena, trace_entry},
    visibility::ray_is_visibly_returned,
};
use crate::optics::raytracer::Ray;
use glam::Vec3;

/// Small camera-yaw offsets (degrees) around the viewing azimuth, for the TEMPORAL term: how
/// much a grid cell's light return changes as the stone is gently rotated. Five poses (odd
/// count, see `cell_returned_at_yaw_offset`), spanning +/-3 deg, about a hand tilt rather than
/// a full turn.
pub(super) const SCINT_TEMPORAL_YAW_OFFSETS_DEG: [f32; 5] = [-3.0, -1.5, 0.0, 1.5, 3.0];

/// Weight of the spatial (per-pose, per-grid-cell contrast) term in the combined
/// `scintillation_pct`. See `combine_scintillation_pct` for the full weighting rationale.
const SCINT_SPATIAL_WEIGHT: f32 = 0.6;
/// Weight of the temporal (per-cell, across-pose flicker) term in the combined
/// `scintillation_pct`. See `combine_scintillation_pct` for the full weighting rationale.
const SCINT_TEMPORAL_WEIGHT: f32 = 0.4;

/// Per-pose context for the Scintillation temporal sub-poses (`cell_returned_at_yaw_offset`):
/// what is needed to fire and classify one ray at a rotated camera azimuth. Bundled to keep
/// that function under clippy's `too_many_arguments` limit.
pub(super) struct TemporalPoseContext<'a> {
    pub(super) stone: StoneArena<'a>,
    pub(super) nd: f32,
    /// The camera frame `(forward, right, up)` at each of the
    /// [`SCINT_TEMPORAL_YAW_OFFSETS_DEG`] yaws, built once per pose rather than per cell.
    bases: [(Vec3, Vec3, Vec3); 5],
    /// Where the fan's rays start, scaled to the stone.
    pub(super) fan: FanGeometry,
    /// The illumination an exit direction is judged against, shared by every pose.
    pub(super) lighting: &'a ExitLighting<'a>,
}

impl<'a> TemporalPoseContext<'a> {
    /// The context for the pose `(cam_yaw, cam_pitch)`; the five offset frames use the
    /// very expression the per-ray code used, so the hoist changes no bit.
    pub(super) fn new(
        stone: StoneArena<'a>,
        nd: f32,
        cam_yaw: f32,
        cam_pitch: f32,
        fan: FanGeometry,
        lighting: &'a ExitLighting<'a>,
    ) -> Self {
        let bases = SCINT_TEMPORAL_YAW_OFFSETS_DEG
            .map(|offset_deg| camera_view_basis(cam_yaw + offset_deg.to_radians(), cam_pitch));
        Self {
            stone,
            nd,
            bases,
            fan,
            lighting,
        }
    }
}

/// Fires one non-jittered ray at grid cell `(u, v)` and reports whether it is visibly returned.
///
/// The ray is fired from the pose `bases[offset_index]` (screen space `[-1, 1]`) and refracts
/// at the d-line only (`ray_is_visibly_returned`). Each sample is independent of the main grid
/// loop: the question is whether the same cell's return flips as the view rotates, so no F/C
/// companion trace is needed.
///
/// Odd offset counts (see [`SCINT_TEMPORAL_YAW_OFFSETS_DEG`]) keep the variance below its
/// continuous maximum 0.25: an even count lets a cell that flips exactly half the time reach
/// 0.25, while 5 samples reach at most 0.24 (2 or 3 of 5).
///
/// Returns the returned WEIGHT of the cell's ray: `1.0` or `0.0` for an isotropic stone; an
/// anisotropic one traces both eigenmodes (see [`super::birefringent`]) and returns `0.5` per
/// visibly returned mode, so the cell's flicker is measured on the weights.
#[must_use]
fn cell_returned_at_yaw_offset(
    ctx: &TemporalPoseContext,
    offset_index: usize,
    u: f32,
    v: f32,
) -> f32 {
    let (forward, right, up) = ctx.bases[offset_index];
    let ray = Ray {
        origin: ctx.fan.origin(forward, right, up, u, v),
        dir: forward,
    };
    let Some(hit_rec) = ctx.stone.intersect(ray) else {
        return 0.0;
    };
    let hit_point = ray.origin + hit_rec.t * ray.dir;
    let n_entry = hit_rec.normal;
    let cos_i = (-ray.dir).dot(n_entry).clamp(0.0, 1.0);

    let entry = EntryRay {
        point: hit_point,
        dir: ray.dir,
        normal: n_entry,
        cos_i,
        facet: hit_rec.facet_idx,
    };
    if ctx.stone.modes.is_some() {
        return Mode::BOTH
            .map(
                |mode| match trace_mode(entry, ctx.stone, Line::D, mode, ctx.nd, None) {
                    RayFate::ExitedUpward(exit)
                        if ray_is_visibly_returned(exit.dir, forward, ctx.lighting) =>
                    {
                        MODE_WEIGHT
                    }
                    _ => 0.0,
                },
            )
            .iter()
            .sum();
    }
    match trace_entry(entry, ctx.stone, ctx.nd, None) {
        RayFate::ExitedUpward(exit) if ray_is_visibly_returned(exit.dir, forward, ctx.lighting) => {
            1.0
        }
        _ => 0.0,
    }
}

/// Scintillation TEMPORAL contribution of one grid cell `(u, v)`: does its return status
/// flip as the camera nudges through the small azimuth offsets in
/// [`SCINT_TEMPORAL_YAW_OFFSETS_DEG`]? Computed via independent single-ray samples (see
/// `cell_returned_at_yaw_offset`), reduced to the Bernoulli variance `p * (1 - p)` of the
/// fraction `p` of offsets at which the cell returned light: 0 when never flipping
/// (bright or dark, not sparkling), up to 0.24 when it flips close to half the time.
#[must_use]
pub(super) fn cell_temporal_variance(ctx: &TemporalPoseContext, u: f32, v: f32) -> f32 {
    let mut temporal_returned = 0.0f32;
    for offset_index in 0..SCINT_TEMPORAL_YAW_OFFSETS_DEG.len() {
        temporal_returned += cell_returned_at_yaw_offset(ctx, offset_index, u, v);
    }
    let temporal_p = temporal_returned / SCINT_TEMPORAL_YAW_OFFSETS_DEG.len() as f32;
    temporal_p.mul_add(-temporal_p, temporal_p)
}

/// Scintillation SPATIAL term: the contrast of the per-cell light-return fraction, as a 0-100
/// percentage.
///
/// The contrast is the coefficient of variation (std-dev / mean) across the 18x18 grid. CV = 0
/// means no contrast, no sparkle. CV often exceeds 1.0 for sparse bright cells, so it is
/// squashed rather than clamped: a linear `cv * 100` clamp saturates at 100 for most stones (19 of
/// 26 material/cut combinations measured).
///
/// The squash `cv / (1 + cv)` is a monotone map from [0, inf) onto [0, 1). A stone with no
/// visited cells reports 0.
#[must_use]
pub(super) fn spatial_scintillation_pct(
    cell_fraction_sum: f32,
    cell_fraction_sum_sq: f32,
    cell_count: u32,
) -> f32 {
    if cell_count == 0 {
        return 0.0;
    }
    let mean_frac = cell_fraction_sum / cell_count as f32;
    if mean_frac <= 1e-4 {
        return 0.0;
    }
    let variance = mean_frac
        .mul_add(-mean_frac, cell_fraction_sum_sq / cell_count as f32)
        .max(0.0);
    let std_dev = variance.sqrt();
    let coefficient_of_variation = std_dev / mean_frac;
    (coefficient_of_variation / (1.0 + coefficient_of_variation) * 100.0).clamp(0.0, 100.0)
}

/// Scintillation TEMPORAL term: mean per-cell Bernoulli variance across the same
/// visited-cell set as the spatial term, normalized by its own achievable maximum (0.24,
/// not the continuous 0.25 -- see `cell_returned_at_yaw_offset`) into a 0-100
/// percentage. Needs no separate saturating squash: it is bounded by construction, and
/// averaging many cells makes the whole grid landing on that per-cell ceiling
/// simultaneously vanishingly unlikely in practice.
#[must_use]
pub(super) fn temporal_scintillation_pct(temporal_variance_sum: f32, cell_count: u32) -> f32 {
    if cell_count == 0 {
        return 0.0;
    }
    ((temporal_variance_sum / cell_count as f32) / 0.24 * 100.0).clamp(0.0, 100.0)
}

/// Combines the Scintillation spatial and temporal terms (both already 0-100 percentages)
/// into the final displayed `scintillation_pct`.
///
/// Scintillation needs both: a static bright/dark pattern is not sparkle, and uniform flicker
/// has nothing to flicker between. The 60/40 weights favour the spatial term, which has the
/// larger sample (5 sub-aperture rays x 18x18 grid); the temporal term takes 5 single-ray
/// samples per cell to keep the cost down.
#[must_use]
pub(super) fn combine_scintillation_pct(spatial_pct: f32, temporal_pct: f32) -> f32 {
    SCINT_TEMPORAL_WEIGHT
        .mul_add(temporal_pct, SCINT_SPATIAL_WEIGHT * spatial_pct)
        .clamp(0.0, 100.0)
}
