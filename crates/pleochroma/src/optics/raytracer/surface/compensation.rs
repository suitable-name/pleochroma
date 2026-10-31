//! Energy compensation for the single-scatter GGX interface (Kulla-Conty style).
//!
//! A rough dielectric that samples one micro-normal per event loses the light that the
//! micro-surface shadows or sends to the wrong side of the macro surface; at `α` 0.45 that is
//! 10 to 15 % of the energy, which a cutter would see as a darker girdle than the old Lambertian
//! one. The directional albedo `E(α, cosθ, η)` of the whole event (reflection and transmission,
//! with the exact Fresnel term at each micro-normal) is tabulated once and every event is divided
//! by it, so a lossless rough interface keeps its energy. The table is indexed by the roughness,
//! the cosine of the incidence angle and the index ratio `n2 / n1`, which covers both sides of
//! the interface (a ray entering and a ray leaving the stone); anisotropic lobes use their
//! geometric-mean width.
//!
//! The table is a deterministic stratified quadrature over the visible-normal sampling square,
//! built lazily on first use; the WGSL twin reads the same numbers from a constant array.

use std::sync::OnceLock;

use glam::Vec3;

use super::{
    ggx::GgxLobe,
    micro::{fresnel_unpolarized, reflect_about, refract_about},
};

/// The roughness nodes of the table.
pub const ALPHA_NODES: [f32; 10] = [0.05, 0.1, 0.2, 0.3, 0.4, 0.45, 0.5, 0.6, 0.8, 1.0];

/// The number of incidence-angle nodes (uniform in the angle, `0` to [`MAX_ANGLE_DEG`]).
const ANGLE_NODES: usize = 16;

/// The grazing end of the angle axis in degrees; beyond it the last node holds.
const MAX_ANGLE_DEG: f32 = 86.0;

/// The number of index-ratio nodes (logarithmic from `1/3` to `3`).
const ETA_NODES: usize = 17;

/// The ratio `3` at the end of the index axis.
const ETA_MAX: f32 = 3.0;

/// The grid of the quadrature per axis of the sampling square.
const QUADRATURE: usize = 40;

/// The lowest albedo a division is allowed to use, capping the boost at 20x.
const MIN_ALBEDO: f32 = 0.05;

#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
fn node_eta(index: usize) -> f32 {
    let t = index as f32 / (ETA_NODES - 1) as f32;
    ETA_MAX.powf(2.0 * t - 1.0)
}

fn node_angle_rad(index: usize) -> f32 {
    (MAX_ANGLE_DEG * index as f32 / (ANGLE_NODES - 1) as f32).to_radians()
}

/// The directional albedo of one interface event at `alpha`, `cos_i` and `eta_rel`, by quadrature.
///
/// The incidence cosine is `cos_i` and the index ratio `eta_rel = n2 / n1` (`n1 = 1`). The value
/// is the mean over the visible normals of `F G2/G1` for the reflected samples that stay above the surface plus `(1 − F) G2/G1` for the
/// refracted ones that end below it.
#[must_use]
pub fn albedo_by_quadrature(alpha: f32, cos_i: f32, eta_rel: f32) -> f32 {
    let lobe = GgxLobe::isotropic(alpha);
    let cos_i = cos_i.clamp(1e-3, 1.0);
    let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();
    let wi = Vec3::new(sin_i, 0.0, cos_i);
    let g1_i = lobe.g1(wi);
    let mut sum = 0.0f32;
    for a in 0..QUADRATURE {
        for b in 0..QUADRATURE {
            let u1 = (a as f32 + 0.5) / QUADRATURE as f32;
            let u2 = (b as f32 + 0.5) / QUADRATURE as f32;
            let m = lobe.sample_visible(wi, u1, u2);
            let c = wi.dot(m);
            if c <= 0.0 {
                continue;
            }
            let fresnel = fresnel_unpolarized(1.0, eta_rel, c);
            let o = reflect_about(wi, m);
            if o.z > 0.0 {
                sum += fresnel * lobe.g2(wi, o) / g1_i;
            }
            if let Some(o) = refract_about(wi, m, 1.0, eta_rel)
                && o.z < 0.0
            {
                sum += (1.0 - fresnel) * lobe.g2(wi, o) / g1_i;
            }
        }
    }
    sum / (QUADRATURE * QUADRATURE) as f32
}

/// The number of values in [`table_values`]: the GPU reads this many floats from the tail of the
/// `dist_func` buffer (`GROUND_TABLE_WORDS` in `13_surface_ggx.wgsl`).
pub const TABLE_LEN: usize = ALPHA_NODES.len() * ANGLE_NODES * ETA_NODES;

/// The table, in the order the WGSL twin indexes it: `((alpha * 16 + angle) * 17 + eta)`.
///
/// The roughness nodes of [`ALPHA_NODES`], the angle nodes uniform from 0 to 86 degrees and the index
/// ratios logarithmic from 1/3 to 3. Built on first use.
#[must_use]
pub fn table_values() -> &'static [f32] {
    table()
}

fn table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut values = Vec::with_capacity(ALPHA_NODES.len() * ANGLE_NODES * ETA_NODES);
        for alpha in ALPHA_NODES {
            for angle in 0..ANGLE_NODES {
                let cos_i = node_angle_rad(angle).cos();
                for eta in 0..ETA_NODES {
                    values.push(albedo_by_quadrature(alpha, cos_i, node_eta(eta)));
                }
            }
        }
        values
    })
}

/// The bracketing node index and the interpolation weight of `x` on an ascending node list.
fn bracket(nodes: &[f32], x: f32) -> (usize, f32) {
    let last = nodes.len() - 1;
    if x <= nodes[0] {
        return (0, 0.0);
    }
    if x >= nodes[last] {
        return (last - 1, 1.0);
    }
    let upper = nodes.partition_point(|&node| node <= x).min(last);
    let lower = upper - 1;
    (lower, (x - nodes[lower]) / (nodes[upper] - nodes[lower]))
}

/// The directional albedo of one interface event, by trilinear interpolation of the table.
///
/// The inputs are roughness `alpha`, incidence cosine `cos_i` and index ratio
/// `eta_rel = n2 / n1`. Never below [`MIN_ALBEDO`], so dividing by it stays bounded.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    clippy::manual_midpoint,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the node coordinates are clamped to the table before they are floored"
)]
pub fn ground_albedo(alpha: f32, cos_i: f32, eta_rel: f32) -> f32 {
    let values = table();
    let (a0, ta) = bracket(&ALPHA_NODES, alpha);
    // The angle and index axes are uniform in their own coordinates.
    let angle =
        cos_i.clamp(0.0, 1.0).acos().to_degrees() / MAX_ANGLE_DEG * (ANGLE_NODES - 1) as f32;
    let angle = angle.clamp(0.0, (ANGLE_NODES - 1) as f32);
    let c0 = (angle.floor() as usize).min(ANGLE_NODES - 2);
    let tc = angle - c0 as f32;
    let eta = (eta_rel.max(1e-3).ln() / ETA_MAX.ln() + 1.0) * 0.5 * (ETA_NODES - 1) as f32;
    let eta = eta.clamp(0.0, (ETA_NODES - 1) as f32);
    let e0 = (eta.floor() as usize).min(ETA_NODES - 2);
    let te = eta - e0 as f32;
    let at = |ai: usize, ci: usize, ei: usize| values[(ai * ANGLE_NODES + ci) * ETA_NODES + ei];
    let mut total = 0.0f32;
    for (da, wa) in [(0, 1.0 - ta), (1, ta)] {
        for (dc, wc) in [(0, 1.0 - tc), (1, tc)] {
            for (de, we) in [(0, 1.0 - te), (1, te)] {
                total += wa * wc * we * at(a0 + da, c0 + dc, e0 + de);
            }
        }
    }
    total.max(MIN_ALBEDO)
}
