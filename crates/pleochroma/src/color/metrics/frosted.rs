//! Frosted facets in the analytic metrics tracer.
//!
//! The metrics fan ([`super::ray_trace`]) is its own single-ray tracer, not the render tracer, so
//! a facet finish has to be taught to it separately. A [`FacetFinish::Frosted`] facet scatters
//! the way [`apply_ground_bounce`] scatters it in the render tracer (decision D11: the GGX ground
//! lobe, `alpha` 0.45, replaced the Lambertian bounce):
//!
//! - one broadband unpolarized Fresnel reflect probability `r_unpol` (the polished interface's
//!   sharp per-wavelength structure is averaged away), clamped to the polished dispatch's
//!   selection range and drawn against `FRESNEL_BRANCH_STREAM`;
//! - total internal reflection, where the polished facet would reflect specularly, takes the top
//!   of that range, so a tilted micro-normal can still let the ray out;
//! - the new direction is the exact mirror (or Snell) direction at a micro-normal drawn from the
//!   visible normals of the GGX lobe about the macroscopic normal of the side the ray comes from
//!   (`FROSTED_DIR_U_STREAM` / `FROSTED_DIR_V_STREAM`);
//! - a sample the micro-surface shadows, or that ends on the wrong side of the facet, becomes the
//!   mirror direction, which keeps the ray where it was; the render tracer gives such a sample
//!   weight `0` and the fan, which counts rays rather than weights, ignores the weight;
//! - the branch itself carries the energy: the path weight stays `1`, so the Fresnel
//!   transmittance of the entry and the exit that a polished path multiplies in is `1` here.
//!
//! The render tracer draws those numbers from a per-path seed. The fan has no path seed, so
//! [`path_seed`] hashes the bits of the entry point and direction: a pure function of the ray,
//! the same for the d-line trace and its F and C companions (they share a direction sample, as
//! the render's channels do), and independent of the thread that traces it. A frosted stone's
//! metrics are therefore a Monte Carlo estimate over the fan's rays (about 900 per pose), but a
//! reproducible one: same stone, same pose, same bits.
//!
//! What the fan returns is the same for polished and frosted facets: only light that went
//! through the stone. A polished facet's specular reflection at the entry is dropped, so a
//! frosted entry facet's diffuse reflect, which heads back out without entering, is dropped
//! too (`RayFate::Absorbed`, `ExtinctionReason::SideExit`); there is no "surface sheen". A
//! diffuse reflect at an interior facet only keeps the ray inside, as total internal
//! reflection does. The per-event draws above are unaffected by this rule.
//!
//! [`apply_ground_bounce`]: crate::optics::raytracer::surface::apply_ground_bounce

use glam::Vec3;

use crate::optics::raytracer::{
    FRESNEL_BRANCH_STREAM, FROSTED_DIR_U_STREAM, FROSTED_DIR_V_STREAM, hash_u32,
    surface::{GroundCtx, GroundInterface, GroundParams},
};

/// What one frosted facet did to a ray.
#[derive(Clone, Copy, Debug)]
pub(super) struct FrostedEvent {
    /// The new propagation direction, a unit vector.
    pub(super) dir: Vec3,
    /// `true` when the ray went through the facet into the other medium; `false` when it
    /// reflected diffusely and stayed where it was.
    pub(super) crossed: bool,
}

/// The per-path seed of the frosted draws for a ray that enters the stone at `origin` heading
/// along `dir`: a hash of their bits.
pub(super) fn path_seed(origin: Vec3, dir: Vec3) -> u32 {
    [origin.x, origin.y, origin.z, dir.x, dir.y, dir.z]
        .into_iter()
        .fold(0x9E37_79B9_u32, |seed, component| {
            hash_u32(seed ^ component.to_bits())
        })
}

/// One uniform `[0, 1)` draw of the stream `salt` for `bounce` of the path `seed`.
const fn draw(seed: u32, bounce: u32, salt: u32) -> f32 {
    (hash_u32(seed ^ hash_u32(bounce ^ salt)) as f32) / 4_294_967_295.0
}

/// A ray meets a frosted facet from the medium of index `n1`, the other side being `n2`.
///
/// `normal` is the facet's outward normal turned to face the medium the ray is in (the
/// outward normal itself for a ray in air about to enter, its negation for a ray inside the
/// stone), exactly as the render tracer hands it to `apply_ground_bounce`, and `incident` the
/// propagation direction of the ray (the lobe is anisotropic in the ray's own geometry, so the
/// direction matters, not just the angle); `cos_i` is the cosine of the angle of incidence and
/// `sin2_t` the squared sine of the refraction angle (greater than `1` for total internal
/// reflection). `bounce` numbers the interface events of the path so that every event draws from
/// its own streams.
///
/// The sample is `apply_ground_bounce`'s: both call [`GroundCtx::sample`] with the same draws;
/// the test `a_frosted_event_is_the_render_tracers_frosted_bounce` pins the two together.
pub(super) fn frosted_event(
    (n1, n2): (f32, f32),
    cos_i: f32,
    sin2_t: f32,
    (normal, incident): (Vec3, Vec3),
    seed: u32,
    bounce: u32,
) -> FrostedEvent {
    let u1 = draw(seed, bounce, FROSTED_DIR_U_STREAM);
    let u2 = draw(seed, bounce, FROSTED_DIR_V_STREAM);
    let branch = draw(seed, bounce, FRESNEL_BRANCH_STREAM);
    let ground = GroundCtx::new(
        normal,
        incident,
        GroundParams::DEFAULT,
        GroundInterface {
            n1,
            n2,
            cos_i,
            sin2_t,
        },
    );
    let sample = ground.sample(u1, u2, branch);
    FrostedEvent {
        dir: sample.dir,
        crossed: sample.crossed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::{
        materials::GemMaterial,
        polarization::StokesVector,
        raytracer::{
            BounceRefractionGeometry, LightingPreset, RayMaterialContext, build_plane_soa,
            scattering::NeeContext, surface::apply_ground_bounce,
        },
    };

    /// The bit patterns of a vector, so that "the same direction" means the same bits.
    fn bits(v: Vec3) -> [u32; 3] {
        v.to_array().map(f32::to_bits)
    }

    /// The propagation direction of a ray that meets a facet with the normal `normal` (facing
    /// the ray) at incidence cosine `cos_i`, arriving from the side of the `phi` azimuth.
    fn incident_direction(normal: Vec3, cos_i: f32, phi: f32) -> Vec3 {
        let t = normal.any_orthonormal_vector();
        let b = normal.cross(t);
        let sin_i = cos_i.mul_add(-cos_i, 1.0).max(0.0).sqrt();
        -(normal * cos_i + (t * phi.cos() + b * phi.sin()) * sin_i)
    }

    /// `apply_ground_bounce` with next-event estimation off: its new direction and whether the
    /// ray changed side.
    fn render_tracer(
        (n1, n2): (f32, f32),
        cos_i: f32,
        sin2_t: f32,
        (normal, incident): (Vec3, Vec3),
        inside_gem: bool,
        seed: u32,
        bounce: u32,
    ) -> (Vec3, bool) {
        let material = GemMaterial::new_custom("frosted metrics", 1.8, 0.02, 0.0, [0.0; 3]);
        let geo = BounceRefractionGeometry {
            cos_i,
            n1,
            n2,
            sin2_t,
            ..Default::default()
        };
        let ctx = RayMaterialContext {
            material: &material,
            lambdas: [550.0; 8],
            hero_idx: 0,
            c_axis: Vec3::Y,
            is_anisotropic: false,
            enable_internal_mode_coupling: true,
        };
        let mut stokes: [StokesVector; 8] = std::array::from_fn(|_| StokesVector::unpolarized(1.0));
        let mut path_pdf = [1.0f32; 8];
        let plane_soa = build_plane_soa(&[]);
        let nee = NeeContext {
            environment: LightingPreset::Daylight.studio(1.0, 0.0, 0.0),
            plane_soa: &plane_soa,
            tools: &[],
            enabled: false,
            edge_band: None,
        };
        let mut radiance = [0.0f32; 8];
        let (dir, new_inside_gem, _, _) = apply_ground_bounce(
            &ctx,
            &geo,
            normal,
            (incident, Vec3::ZERO),
            GroundParams::DEFAULT,
            inside_gem,
            (seed, bounce),
            &mut stokes,
            &mut path_pdf,
            nee,
            &ctx.lambdas,
            &mut radiance,
        );
        (dir, new_inside_gem != inside_gem)
    }

    /// The metrics' frosted event is the render tracer's frosted bounce: the same branch and
    /// the same direction bits, entering from air and leaving from inside the stone, including
    /// total internal reflection, over a spread of angles, indices, seeds and bounce numbers.
    #[test]
    fn a_frosted_event_is_the_render_tracers_frosted_bounce() {
        let outward = Vec3::new(0.3, 0.8, 0.2).normalize();
        let mut crossed = 0;
        let mut stayed = 0;
        for index in [1.45f32, 1.77, 2.42] {
            for cos_i in [0.15f32, 0.4, 0.7, 0.95] {
                for seed in [1u32, 7, 0xDEAD_BEEF, 123_456_789] {
                    for bounce in 0..4u32 {
                        let phi = 0.7 + bounce as f32;
                        // From air: the facet's own outward normal.
                        let sin2_in = (1.0 / (index * index)) * cos_i.mul_add(-cos_i, 1.0);
                        let from_air = (outward, incident_direction(outward, cos_i, phi));
                        let ours =
                            frosted_event((1.0, index), cos_i, sin2_in, from_air, seed, bounce);
                        let reference = render_tracer(
                            (1.0, index),
                            cos_i,
                            sin2_in,
                            from_air,
                            false,
                            seed,
                            bounce,
                        );
                        assert_eq!(bits(ours.dir), bits(reference.0));
                        assert_eq!(ours.crossed, reference.1);

                        // From inside: the normal turned to face the stone.
                        let sin2_out = (index * index) * cos_i.mul_add(-cos_i, 1.0);
                        let from_inside = (-outward, incident_direction(-outward, cos_i, phi));
                        let ours =
                            frosted_event((index, 1.0), cos_i, sin2_out, from_inside, seed, bounce);
                        let reference = render_tracer(
                            (index, 1.0),
                            cos_i,
                            sin2_out,
                            from_inside,
                            true,
                            seed,
                            bounce,
                        );
                        assert_eq!(bits(ours.dir), bits(reference.0));
                        assert_eq!(ours.crossed, reference.1);
                        if ours.crossed {
                            crossed += 1;
                        } else {
                            stayed += 1;
                        }
                    }
                }
            }
        }
        assert!(
            crossed > 0 && stayed > 0,
            "{crossed} crossed, {stayed} stayed"
        );
    }

    /// The seed is a pure function of the ray and changes with it.
    #[test]
    fn the_path_seed_is_deterministic_and_ray_dependent() {
        let (origin, dir) = (Vec3::new(0.1, 2.0, -0.3), Vec3::NEG_Y);
        assert_eq!(path_seed(origin, dir), path_seed(origin, dir));
        assert_ne!(
            path_seed(origin, dir),
            path_seed(origin + Vec3::X * 1e-3, dir)
        );
        assert_ne!(path_seed(origin, dir), path_seed(origin, Vec3::NEG_X));
    }
}
