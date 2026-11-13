//! Forward photons carrying the redirected fraction to the tray.
use super::{
    TRAY_SHADOW_BANDS,
    geometry::{StoneFrame, StoneHit},
    lights::LightTable,
    rng::SplitMix64,
};
use crate::renderer::whitted::{
    fresnel::{fresnel_unpolarised_f32, reflect, refract},
    tint::BinnedAbsorption,
};
use glam::Vec3;

/// One spectral photon landing before the ordered bilinear splat.
pub(super) struct Deposit {
    /// Landing x coordinate.
    pub(super) x: f32,
    /// Landing z coordinate.
    pub(super) z: f32,
    /// Spectral band index.
    pub(super) band: u8,
    /// Radiant weight of this landing.
    pub(super) weight: f32,
}
/// Immutable transport inputs shared by photon chunks.
pub(super) struct PhotonCtx<'a> {
    /// Emission sphere and receiving plane.
    pub(super) frame: &'a StoneFrame,
    /// Stone intersection accelerator.
    pub(super) stone: &'a StoneHit<'a>,
    /// Incident-flux distribution.
    pub(super) table: &'a LightTable,
    /// Refractive index at each band centre.
    pub(super) n: [f32; TRAY_SHADOW_BANDS],
    /// Absorption coefficients for the same bands.
    pub(super) absorption: &'a BinnedAbsorption,
    /// Scattering material treated as an opaque body.
    pub(super) opaque_body: bool,
    /// Whether the stone has no tools, so an outward ray never re-meets it.
    pub(super) convex: bool,
    /// Maximum internal surface interactions.
    pub(super) max_bounces: u32,
    /// Incident flux per emitted photon.
    pub(super) weight: f32,
}

/// Transmittance averaged over the absorption bins and modes of each shadow band.
pub(super) fn band_transmittance(
    abs: &BinnedAbsorption,
    dir: Vec3,
    length: f32,
) -> [f32; TRAY_SHADOW_BANDS] {
    if abs.modes == 0 {
        return [1.0; TRAY_SHADOW_BANDS];
    }
    let axial = abs.axial(dir, length);
    let modes = usize::from(abs.modes);
    std::array::from_fn(|b| {
        let mut sum = 0.0;
        for m in 0..modes {
            for bin in [2 * b, 2 * b + 1] {
                sum += (-abs.p[m][bin].mul_add(length, abs.q[m][bin] * axial)).exp();
            }
        }
        sum / (2 * modes) as f32
    })
}

fn branch(
    dir: Vec3,
    normal: Vec3,
    n: &[f32; TRAY_SHADOW_BANDS],
    inside: bool,
    tau: &mut [f32; TRAY_SHADOW_BANDS],
    rng: &mut SplitMix64,
) -> bool {
    let cosine = -dir.dot(normal);
    let r: [f32; TRAY_SHADOW_BANDS] = std::array::from_fn(|b| {
        fresnel_unpolarised_f32(cosine, if inside { n[b] } else { 1.0 / n[b] })
    });
    let p = if r.iter().all(|&v| v == 1.0) {
        1.0
    } else if r.iter().all(|&v| v == 0.0) {
        0.0
    } else {
        (r.iter().sum::<f32>() / TRAY_SHADOW_BANDS as f32).clamp(0.02, 0.98)
    };
    let reflected = rng.next_f32() < p;
    for (weight, &reflectance) in tau.iter_mut().zip(&r) {
        *weight *= if reflected {
            reflectance / p
        } else {
            (1.0 - reflectance) / (1.0 - p)
        };
    }
    reflected
}

fn land(
    ctx: &PhotonCtx<'_>,
    point: Vec3,
    dir: Vec3,
    band: usize,
    tau: f32,
    out: &mut Vec<Deposit>,
) {
    if tau <= 0.0 {
        return;
    }
    let Some([x, z]) = ctx.frame.plane_hit(point, dir) else {
        return;
    };
    // A ray leaving a convex polytope cannot meet it again, so only concave stones re-test.
    if !ctx.convex && ctx.stone.hits(point + dir * ctx.frame.lift, dir) {
        return;
    }
    out.push(Deposit {
        x,
        z,
        band: u8::try_from(band).expect("band is below 20"),
        weight: ctx.weight * tau,
    });
}

/// Traces a deterministic photon-index range and appends its tray landings.
pub(super) fn trace_chunk(
    ctx: &PhotonCtx<'_>,
    chunk: u64,
    first: u64,
    count: u64,
    out: &mut Vec<Deposit>,
) {
    let mut rng = SplitMix64::for_chunk(chunk);
    for photon in first..first + count {
        let omega = ctx.table.sample(rng.next_f64(), &mut rng);
        let (a, b) = omega.any_orthonormal_pair();
        let r = ctx.frame.sphere_radius * rng.next_f32().sqrt();
        let phi = std::f32::consts::TAU * rng.next_f32();
        let (sin_phi, cos_phi) = phi.sin_cos();
        let origin = ctx.frame.sphere_center
            + r * (a * cos_phi + b * sin_phi)
            + 2.0 * ctx.frame.sphere_radius * omega;
        let mut dir = -omega;
        let Some((_, mut pos, normal)) = ctx.stone.first_hit(origin, dir) else {
            continue;
        };
        if ctx.opaque_body {
            continue;
        }
        let hero =
            usize::try_from(photon % TRAY_SHADOW_BANDS as u64).expect("hero band is below 20");
        let mut tau = [1.0; TRAY_SHADOW_BANDS];
        if branch(dir, normal, &ctx.n, false, &mut tau, &mut rng) {
            dir = reflect(dir, normal);
            for (b, &weight) in tau.iter().enumerate() {
                land(ctx, pos, dir, b, weight, out);
            }
            continue;
        }
        let Some(entry) = refract(dir, normal, 1.0 / ctx.n[hero]) else {
            continue;
        };
        dir = entry;
        for bounce in 1..=ctx.max_bounces {
            let Some((length, hit, normal, entering)) = ctx.stone.next_hit_inside(pos, dir) else {
                break;
            };
            if entering {
                // The restart stepped over a thin rim, so this stretch was air: no absorption and
                // an air-to-stone interface, exactly as the path tracer adopts the state.
                pos = hit;
                if branch(dir, normal, &ctx.n, false, &mut tau, &mut rng) {
                    dir = reflect(dir, normal);
                    for (b, &weight) in tau.iter().enumerate() {
                        land(ctx, pos, dir, b, weight, out);
                    }
                    break;
                }
                let Some(inward) = refract(dir, normal, 1.0 / ctx.n[hero]) else {
                    break;
                };
                dir = inward;
                pos += dir * ctx.frame.lift;
                continue;
            }
            let attenuation = band_transmittance(ctx.absorption, dir, length);
            for (weight, &transmitted) in tau.iter_mut().zip(&attenuation) {
                *weight *= transmitted;
            }
            pos = hit;
            let facing = -normal;
            if branch(dir, facing, &ctx.n, true, &mut tau, &mut rng) {
                dir = reflect(dir, facing);
                pos += dir * ctx.frame.lift;
            } else {
                for (b, &weight) in tau.iter().enumerate() {
                    if let Some(exit) = refract(dir, facing, ctx.n[b]) {
                        land(ctx, pos, exit, b, weight, out);
                    }
                }
                break;
            }
            if bounce >= 8 {
                let q = tau.iter().copied().fold(0.0_f32, f32::max).min(1.0);
                if rng.next_f32() >= q {
                    break;
                }
                for weight in &mut tau {
                    *weight /= q;
                }
            }
        }
    }
}
