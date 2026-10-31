//! The weighted grid pass of an anisotropic stone, and the **Doubling** figure.
//!
//! Every camera ray of the grid is traced as both eigenmodes (see [`super::birefringent`]),
//! each carrying [`MODE_WEIGHT`] of the ray. The brilliance, windowing and extinction shares,
//! the Fire sum, the tone and the scintillation cell fractions all take the mode weights
//! instead of the integer counts; an isotropic stone never comes here (it keeps the integer
//! path of `evaluate::sample_cell`, bit for bit).
//!
//! **Doubling** is the mean angular separation, in arc-minutes, between the o- and e-mode exit
//! directions of the pairs that both return visibly and leave through the same facet, weighted
//! like Fire: by the product of the two modes' Fresnel transmittances and exit cosines. It is
//! what a cutter looks for through the table ("how far apart are the two images of a back
//! facet"). It is `0.0` for an isotropic stone and for an evaluation without a single
//! qualifying pair.

use super::{
    accumulate::MetricsAccumulators,
    birefringent::MODE_WEIGHT,
    classify::{ApertureSampleContext, RayClassification, aperture_ray, classify_ray_pair},
    tone::ToneAccumulator,
};

/// The ray counts of an anisotropic evaluation as mode weights: each camera ray contributes
/// `MODE_WEIGHT` per mode, so every share is a multiple of one half. The total is
/// `MetricsAccumulators::total_rays`, which still counts camera rays (the two halves of a ray
/// always add to one).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct WeightedCounts {
    pub(super) windowed: f32,
    pub(super) extinct: f32,
    pub(super) returned: f32,
}

/// The sums behind the Doubling figure (see the module docs).
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct DoublingSums {
    /// `sum w * separation` (arc-minutes).
    pub(super) angle_weighted: f32,
    /// `sum w`.
    pub(super) weight: f32,
}

impl DoublingSums {
    /// The weighted mean separation in arc-minutes, `0.0` without a qualifying pair.
    pub(super) fn arcmin(self) -> f32 {
        if self.weight > 0.0 {
            self.angle_weighted / self.weight
        } else {
            0.0
        }
    }
}

/// Angular separation of two exit directions in arc-minutes, accurate for the tiny angles a
/// doubling produces (`atan2` of the cross and dot products, not `acos` of a dot product
/// rounded to one).
fn separation_arcmin(a: glam::Vec3, b: glam::Vec3) -> f32 {
    a.cross(b).length().atan2(a.dot(b)).to_degrees() * 60.0
}

/// The `(separation_arcmin, weight)` of one camera ray's mode pair: both modes visibly returned
/// and through the same exit facet, else `None`.
fn doubling_sample(pair: &[RayClassification; 2]) -> Option<(f32, f32)> {
    let [
        RayClassification::Returned(o),
        RayClassification::Returned(e),
    ] = pair
    else {
        return None;
    };
    if o.exit_facet != e.exit_facet {
        return None;
    }
    let weight =
        o.transmittance * e.transmittance * o.exit_cos_theta.max(0.0) * e.exit_cos_theta.max(0.0);
    Some((separation_arcmin(o.exit_dir, e.exit_dir), weight))
}

/// Classifies one aperture sample of the cell `(u, v)` as its two modes and folds both into
/// `acc` (the weighted counts, the Fire sum, the Doubling sums) and `tone`. Returns the share of
/// the ray that was visibly returned (`0`, one half or `1`), or `None` when the ray misses the
/// stone.
///
/// `tone` is told about two traced rays per camera ray (one per mode) and offered each returned
/// mode with half its Fresnel weight, so its returned fraction and its weighted mixture are the
/// 50/50 split of the two modes.
pub(super) fn accumulate_sample_pair(
    ctx: &ApertureSampleContext,
    acc: &mut MetricsAccumulators,
    mut tone: Option<&mut ToneAccumulator>,
    diag_fire_debug: bool,
    (u, v): (f32, f32),
    (sub_right, sub_up): (f32, f32),
) -> Option<f32> {
    let ray = aperture_ray(ctx, u, v, sub_right, sub_up);
    // The fan's rays run along the camera axis, so the eye is straight back along it.
    let pair = classify_ray_pair(ctx, ray, -ctx.cam_forward)?;
    acc.total_rays += 1;

    let mut returned_share = 0.0f32;
    for class in &pair {
        if let Some(t) = tone.as_deref_mut() {
            t.count_total();
        }
        let counts = acc.weighted.get_or_insert_default();
        match class {
            RayClassification::EntryBlocked | RayClassification::Invalid => {}
            RayClassification::Windowed => counts.windowed += MODE_WEIGHT,
            RayClassification::Extinct => counts.extinct += MODE_WEIGHT,
            RayClassification::Returned(ray) => {
                counts.returned += MODE_WEIGHT;
                returned_share += MODE_WEIGHT;
                if let Some(t) = tone.as_deref_mut() {
                    t.offer_principal(
                        MODE_WEIGHT * ray.transmittance,
                        ray.path_len,
                        ray.principal_path,
                    );
                }
                if let Some((angle_deg, weight)) = ray.fire {
                    let weight = MODE_WEIGHT * weight;
                    acc.fire_energy_weighted_sum_deg =
                        f32::mul_add(angle_deg, weight, acc.fire_energy_weighted_sum_deg);
                    if diag_fire_debug {
                        acc.dbg_fire_qualifying += 1;
                        acc.dbg_fire_angle_sum_unweighted += angle_deg;
                        acc.dbg_fire_transmittance_sum += weight;
                    }
                }
            }
        }
    }
    if let Some((arcmin, weight)) = doubling_sample(&pair) {
        acc.doubling.angle_weighted = f32::mul_add(arcmin, weight, acc.doubling.angle_weighted);
        acc.doubling.weight += weight;
    }
    Some(returned_share)
}
