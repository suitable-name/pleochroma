//! Polyhedron ray intersection (scalar and SIMD/SoA twins) and facet-edge
//! shading-normal rounding.

use super::camera::{HitRecord, Ray};
use crate::geometry::plane::GpuFacetPlane;
use glam::Vec3;

/// Intersects a ray with the convex solid bounded by `planes`, returning the entry hit.
#[must_use]
pub fn intersect_polyhedron(ray: Ray, planes: &[GpuFacetPlane]) -> Option<HitRecord> {
    let mut t_near = -1e30f32;
    let mut t_far = 1e30f32;
    let mut near_facet = None;
    let mut near_normal = Vec3::ZERO;
    let mut far_facet = None;
    let mut far_normal = Vec3::ZERO;

    for (i, p) in planes.iter().enumerate() {
        let n = Vec3::from_array(p.normal);
        let denom = n.dot(ray.dir);
        let side = p.d + n.dot(ray.origin);
        let numer = -side;

        if denom.abs() > 1e-7 {
            let t = numer / denom;
            if denom < 0.0 {
                if t > t_near {
                    t_near = t;
                    near_facet = Some(i);
                    near_normal = n;
                }
            } else if t < t_far {
                t_far = t;
                far_facet = Some(i);
                far_normal = n;
            }
        } else if side > 0.0 {
            // Parallel to this plane and outside its half-space: the ray can never enter the
            // solid, so the slab method rejects it outright. Skipping the plane would instead
            // risk a false hit through the remaining planes. An empty plane list never runs
            // this loop, so it keeps the sentinel hit that the furnace check relies on.
            return None;
        }
    }

    if t_near > t_far {
        // Ray direction is entirely outside the solid's half-space intersection.
        return None;
    }

    if t_near > 1e-4 {
        // Origin is outside the solid: the ray enters through the near (entry) plane.
        Some(HitRecord {
            t: t_near,
            normal: near_normal,
            facet_idx: near_facet.unwrap_or(0),
        })
    } else if t_far > 1e-4 {
        // Origin is inside the solid (every entry plane lies behind the ray): the ray
        // exits through the far (exit) plane. Without this branch, any ray currently
        // inside the gem (e.g. after a refraction) would never find its exit facet and
        // intersect_polyhedron would incorrectly report a miss.
        Some(HitRecord {
            t: t_far,
            normal: far_normal,
            facet_idx: far_facet.unwrap_or(0),
        })
    } else {
        None
    }
}

/// Builds the [`crate::simd::PlanesSoA32`] arena `intersect_polyhedron_soa` scans.
///
/// The bounce loop builds it once per sample, not once per bounce. It is `pub` so a batch
/// caller tracing many samples against the same `planes` can build it once and pass it to
/// [`crate::optics::raytracer::trace_spectral_ray_with_finish_soa`].
#[must_use]
pub fn build_plane_soa(planes: &[GpuFacetPlane]) -> crate::simd::PlanesSoA32 {
    crate::simd::PlanesSoA32::from_normals_d(planes.iter().map(|p| (p.normal, p.d)), planes.len())
}

/// Bit-identical SoA/SIMD twin of [`intersect_polyhedron`], see `src/simd.rs`'s
/// determinism contract.
#[must_use]
pub(crate) fn intersect_polyhedron_soa(
    ray: Ray,
    soa: &crate::simd::PlanesSoA32,
) -> Option<HitRecord> {
    match crate::simd::slab_scan(soa, ray.origin, ray.dir) {
        crate::simd::SlabScan::Outside => None,
        crate::simd::SlabScan::Slab {
            t_near,
            near_idx,
            t_far,
            far_idx,
        } => {
            if t_near > t_far {
                None
            } else if t_near > 1e-4 {
                Some(HitRecord {
                    t: t_near,
                    normal: usize::try_from(near_idx).map_or(Vec3::ZERO, |idx| soa.normal(idx)),
                    facet_idx: usize::try_from(near_idx).unwrap_or(0),
                })
            } else if t_far > 1e-4 {
                Some(HitRecord {
                    t: t_far,
                    normal: usize::try_from(far_idx).map_or(Vec3::ZERO, |idx| soa.normal(idx)),
                    facet_idx: usize::try_from(far_idx).unwrap_or(0),
                })
            } else {
                None
            }
        }
    }
}

/// Facet edge rounding: perturbs a flat facet's shading normal near a meet edge, so the
/// soft glint of a rounded cut edge appears. The solid stays flat: [`intersect_polyhedron`]
/// still intersects the sharp planes, and only the normal fed to the Fresnel and refraction
/// calculation changes.
///
/// `planes` are the inward half-space constraints (`n.hit_point + d <= 0`) that define the
/// solid. The nearest plane other than the hit facet's, by signed distance from `hit_point`,
/// is the neighbour whose shared edge the hit approaches. The normal blends toward the angle
/// bisector of the two flat normals as that distance falls to zero, with a `smoothstep` ease
/// over `rounding_radius`, so the facet interior stays exactly flat.
///
/// The bisector is the shared value at the edge: evaluated from either facet, the result
/// converges to the same `normalize(A + B)`, so the shading has no seam.
///
/// `rounding_radius <= 0.0` (the default of every built-in material) returns `hit_normal`
/// unchanged before any work, so existing scenes stay bit-identical.
///
/// `pub(crate)` for the GPU check `renderer::gpu::shading_normal_check`, which compares the
/// WGSL twin `shaders/shading_normal.wgsl` against this function.
#[must_use]
pub(crate) fn shading_normal_near_edge(
    planes: &[GpuFacetPlane],
    hit_point: Vec3,
    hit_facet_idx: usize,
    hit_normal: Vec3,
    rounding_radius: f32,
) -> Vec3 {
    if rounding_radius <= 0.0 {
        return hit_normal;
    }
    let mut nearest_dist = f32::INFINITY;
    let mut nearest_normal = hit_normal;
    for (i, p) in planes.iter().enumerate() {
        if i == hit_facet_idx {
            continue;
        }
        let n = Vec3::from_array(p.normal);
        // Signed distance from `hit_point` to plane `i`, using `intersect_polyhedron`'s
        // own `side = p.d + n.dot(x)` convention (`<= 0` inside/on-boundary): negated so
        // `dist >= 0`, reaching exactly `0.0` at a shared edge with facet `i`.
        let dist = -(p.d + n.dot(hit_point));
        if dist < nearest_dist {
            nearest_dist = dist;
            nearest_normal = n;
        }
    }
    if nearest_dist >= rounding_radius {
        return hit_normal;
    }
    let t = (1.0 - nearest_dist / rounding_radius).clamp(0.0, 1.0);
    let smooth_t = t * t * 2.0f32.mul_add(-t, 3.0);
    let bisector = (hit_normal + nearest_normal).normalize_or_zero();
    (hit_normal * (1.0 - smooth_t) + bisector * smooth_t).normalize_or_zero()
}

#[cfg(test)]
mod intersect_polyhedron_parallel_ray_tests {
    use super::*;

    /// A unit cube `[-0.5, 0.5]^3`, as the six axis-aligned facet planes.
    fn unit_cube_planes() -> Vec<GpuFacetPlane> {
        [
            (Vec3::X, -0.5),
            (Vec3::NEG_X, -0.5),
            (Vec3::Y, -0.5),
            (Vec3::NEG_Y, -0.5),
            (Vec3::Z, -0.5),
            (Vec3::NEG_Z, -0.5),
        ]
        .into_iter()
        .map(|(n, d)| GpuFacetPlane::new(n, d))
        .collect()
    }

    /// A ray parallel to the cube's top face (+Y, at y=0.5) and starting above it never enters
    /// the cube. The parallel-plane guard must reject it rather than fall through to the -X face.
    #[test]
    fn ray_parallel_to_and_outside_a_face_never_hits() {
        let planes = unit_cube_planes();
        let ray = Ray {
            origin: Vec3::new(-5.0, 2.0, 0.0),
            dir: Vec3::new(1.0, 0.0, 0.0),
        };
        let hit = intersect_polyhedron(ray, &planes);
        assert!(
            hit.is_none(),
            "a ray parallel to the top face and starting outside it should never hit the cube, got {hit:?}"
        );
    }

    /// The same direction from inside the top and bottom slab (y=0) never trips the guard and
    /// still hits through the -X face: the guard must not reject legitimate hits.
    #[test]
    fn ray_parallel_to_a_face_but_inside_its_half_space_still_hits() {
        let planes = unit_cube_planes();
        let ray = Ray {
            origin: Vec3::new(-5.0, 0.0, 0.0),
            dir: Vec3::new(1.0, 0.0, 0.0),
        };
        let hit = intersect_polyhedron(ray, &planes).expect("ray aimed at the cube should hit");
        assert!((hit.t - 4.5).abs() < 1e-4, "expected t=4.5, got {}", hit.t);
        assert!(
            (hit.normal - Vec3::NEG_X).length() < 1e-5,
            "expected entry through the -X face, got normal {:?}",
            hit.normal
        );
    }

    /// An empty plane list returns the `t = 1e30` sentinel hit, never `None`: the furnace
    /// check depends on it (see `renderer::gpu::furnace_check`).
    #[test]
    fn empty_planes_still_returns_the_sentinel_hit_not_none() {
        let ray = Ray {
            origin: Vec3::ZERO,
            dir: Vec3::new(0.0, -1.0, 0.0),
        };
        let hit = intersect_polyhedron(ray, &[]).expect("empty planes must still be Some");
        assert!((hit.t - 1e30).abs() < 1.0);
    }
}

/// Facet edge rounding: tests for
/// [`shading_normal_near_edge`] and the `GemMaterial::edge_rounding_radius` gate in
/// `trace_spectral_ray_inner`'s bounce loop.
#[cfg(test)]
mod edge_rounding_tests {
    use super::{
        super::{
            camera::Camera,
            color::cie_1931_cmf,
            environment::{EnvironmentSource, LightingPreset},
            sampling::hash_u32,
            transport::trace_spectral_ray,
        },
        *,
    };
    use crate::{
        geometry::cuts::StandardGemCuts, optics::materials::GemMaterial,
        renderer::env_map::EnvironmentMap,
    };

    /// A minimal two-plane "corner" -- normal `(0,1,0)` (`y <= 0`) and normal `(1,0,0)`
    /// (`x <= 0`) -- whose shared edge is the entire line `x=0, y=0`. Not a closed
    /// polyhedron (this function never calls `intersect_polyhedron`, only loops over
    /// `planes`), just enough geometry to exercise the "nearest OTHER plane" logic
    /// directly and predictably.
    fn corner_planes() -> [GpuFacetPlane; 2] {
        [
            GpuFacetPlane {
                normal: [0.0, 1.0, 0.0],
                d: 0.0,
            },
            GpuFacetPlane {
                normal: [1.0, 0.0, 0.0],
                d: 0.0,
            },
        ]
    }

    #[test]
    fn edge_rounding_disabled_by_default_returns_flat_normal_unchanged() {
        let planes = corner_planes();
        let hit_point = Vec3::new(0.0, 0.0, 5.0); // exactly on the shared edge
        for radius in [0.0f32, -1.0, -0.001] {
            let out =
                shading_normal_near_edge(&planes, hit_point, 0, Vec3::new(0.0, 1.0, 0.0), radius);
            assert_eq!(
                out,
                Vec3::new(0.0, 1.0, 0.0),
                "radius <= 0.0 (radius={radius}) must return the flat normal completely \
                 unchanged"
            );
        }
    }

    /// Evaluated from either facet, a hit point on their shared edge gives the same shading
    /// normal: no seam.
    #[test]
    fn edge_rounding_is_continuous_across_a_shared_edge() {
        let planes = corner_planes();
        let hit_point = Vec3::new(0.0, 0.0, 5.0); // on both planes exactly (dist == 0)
        let from_a = shading_normal_near_edge(&planes, hit_point, 0, Vec3::new(0.0, 1.0, 0.0), 1.0);
        let from_b = shading_normal_near_edge(&planes, hit_point, 1, Vec3::new(1.0, 0.0, 0.0), 1.0);
        assert!(
            (from_a - from_b).length() < 1e-5,
            "shading normal at a shared edge must agree regardless of which facet was hit: \
             from_a={from_a:?}, from_b={from_b:?}"
        );
        let expected_bisector = Vec3::new(1.0, 1.0, 0.0).normalize();
        assert!(
            (from_a - expected_bisector).length() < 1e-5,
            "at the exact edge, the shading normal should be the angle bisector \
             {expected_bisector:?}, got {from_a:?}"
        );
    }

    #[test]
    fn edge_rounding_leaves_facet_interior_exactly_flat_far_from_any_edge() {
        let planes = corner_planes();
        // On facet 0's own plane (y=0) but far (distance 10) from facet 1's plane.
        let hit_point = Vec3::new(-10.0, 0.0, 5.0);
        let out = shading_normal_near_edge(&planes, hit_point, 0, Vec3::new(0.0, 1.0, 0.0), 1.0);
        assert_eq!(
            out,
            Vec3::new(0.0, 1.0, 0.0),
            "a point far from every other plane (beyond rounding_radius) must return the \
             flat normal exactly unperturbed"
        );
    }

    /// A partial-distance case checked against the documented smoothstep formula,
    /// independently re-derived here (not copied from the implementation).
    #[test]
    fn edge_rounding_partial_blend_matches_documented_smoothstep_formula() {
        let planes = corner_planes();
        let radius = 1.0f32;
        let dist_to_other = 0.5f32; // hit_point = (-0.5, 0, 5) is 0.5 from plane 1
        let hit_point = Vec3::new(-dist_to_other, 0.0, 5.0);
        let hit_normal = Vec3::new(0.0, 1.0, 0.0);
        let neighbor_normal = Vec3::new(1.0, 0.0, 0.0);

        let t = (1.0 - dist_to_other / radius).clamp(0.0, 1.0);
        let smooth_t = t * t * 2.0f32.mul_add(-t, 3.0);
        let bisector = (hit_normal + neighbor_normal).normalize();
        let expected = (hit_normal * (1.0 - smooth_t) + bisector * smooth_t).normalize();

        let actual = shading_normal_near_edge(&planes, hit_point, 0, hit_normal, radius);
        assert!(
            (actual - expected).length() < 1e-5,
            "partial-blend result should match the documented smoothstep formula: \
             expected={expected:?}, actual={actual:?}"
        );
    }

    /// A material with `edge_rounding_radius == 0.0` traces bit-identically whether the radius
    /// is the default or set explicitly.
    #[test]
    fn default_off_edge_rounding_is_bit_identical() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material_default = GemMaterial::diamond();
        assert!(material_default.edge_rounding_radius <= 0.0);
        let material_explicit_zero = material_default.clone().with_edge_rounding(0.0);
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let env = || LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

        for iy in 0..6usize {
            for ix in 0..6usize {
                let ray = camera.generate_ray(ix as f32, iy as f32, 6.0, 6.0, 0.5, 0.5);
                for s in 0..4u32 {
                    let pixel_id = u32::try_from(iy * 6 + ix).expect("a 6 x 6 grid");
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x2468_ACE0));
                    let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                    let a = trace_spectral_ray(
                        ray,
                        &planes,
                        &material_default,
                        10,
                        env(),
                        seed,
                        hero_rand,
                        None,
                    );
                    let b = trace_spectral_ray(
                        ray,
                        &planes,
                        &material_explicit_zero,
                        10,
                        env(),
                        seed,
                        hero_rand,
                        None,
                    );
                    assert_eq!(
                        a.to_array(),
                        b.to_array(),
                        "edge_rounding_radius=0.0 (default) vs 0.0 (explicit) must be BIT \
                         identical at pixel ({ix},{iy}) sample {s}"
                    );
                }
            }
        }
    }

    /// A rounded edge must change the face-up appearance measurably, not merely run.
    #[test]
    fn edge_rounding_measurably_changes_face_up_appearance() {
        const SAMPLES_PER_PIXEL: u32 = 128;
        const GRID: usize = 16;

        let planes = StandardGemCuts::standard_round_brilliant();
        let sharp = GemMaterial::diamond();
        let rounded = sharp.clone().with_edge_rounding(0.02);
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let env = || LightingPreset::RingLights.studio(1.0, 0.85, 0.95);

        let mut sum_sharp = Vec3::ZERO;
        let mut sum_rounded = Vec3::ZERO;
        let mut count = 0u32;
        for iy in 0..GRID {
            for ix in 0..GRID {
                let ray =
                    camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
                for s in 0..SAMPLES_PER_PIXEL {
                    let pixel_id = u32::try_from(iy * GRID + ix).expect("a small grid");
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0xF00D_CAFE));
                    let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                    sum_sharp +=
                        trace_spectral_ray(ray, &planes, &sharp, 12, env(), seed, hero_rand, None);
                    sum_rounded += trace_spectral_ray(
                        ray,
                        &planes,
                        &rounded,
                        12,
                        env(),
                        seed,
                        hero_rand,
                        None,
                    );
                    count += 1;
                }
            }
        }
        let mean_sharp = sum_sharp / count as f32;
        let mean_rounded = sum_rounded / count as f32;
        let delta_y = (mean_rounded.y - mean_sharp.y).abs();
        let relative_change = delta_y / mean_sharp.y.max(1e-6);
        println!(
            "[edge-rounding face-up] sharp Y={:.5} rounded Y={:.5} delta_y={:.5} ({:.2}%) over {count} samples",
            mean_sharp.y,
            mean_rounded.y,
            delta_y,
            100.0 * relative_change
        );
        assert!(
            relative_change > 0.005,
            "rounding meet-point edges should measurably change face-up brightness \
             (>0.5%), not render identically to a perfectly sharp stone -- got {:.4}% \
             (sharp Y={:.5}, rounded Y={:.5})",
            100.0 * relative_change,
            mean_sharp.y,
            mean_rounded.y
        );
    }

    /// White furnace, applied to edge rounding: a perfectly colorless, non-absorbing gem with
    /// rounded edges immersed in a uniform environment must still render at exactly
    /// that environment's own radiance -- edge rounding only perturbs the shading
    /// normal fed into the ALREADY energy-conserving Fresnel reflect/transmit split, it
    /// introduces no new energy source or sink of its own.
    #[test]
    fn edge_rounding_white_furnace_energy_conservation_holds() {
        const L0: f32 = 2.5;
        const SAMPLES_PER_PIXEL: u32 = 96;
        const GRID: usize = 12;
        // Covers the 0.6 percent truncation loss at cap 16 and under 0.1 percent noise at this
        // sample budget, with five standard deviations of headroom.
        const TOLERANCE: f32 = 0.01;

        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::new_custom(
            "edge rounding furnace probe",
            1.5,
            0.0,
            0.0,
            [0.0, 0.0, 0.0],
        )
        .with_edge_rounding(0.03);
        let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let mut sum = Vec3::ZERO;
        let mut count = 0u32;
        for iy in 0..GRID {
            for ix in 0..GRID {
                let ray =
                    camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
                for s in 0..SAMPLES_PER_PIXEL {
                    let pixel_id = u32::try_from(iy * GRID + ix).expect("a small grid");
                    let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0xABAD_1DEA));
                    sum += trace_spectral_ray(
                        ray,
                        &planes,
                        &material,
                        16,
                        EnvironmentSource::hdr(&env_map),
                        seed,
                        (hash_u32(seed) as f32) / 4_294_967_295.0,
                        None,
                    );
                    count += 1;
                }
            }
        }
        let mean = sum / count as f32;

        let mut target = Vec3::ZERO;
        for step in 0..=(780 - 380) {
            let lambda = 380.0f32 + step as f32;
            let spec = crate::renderer::env_map::rgb_to_spectral_radiance([L0, L0, L0], lambda);
            target += cie_1931_cmf(lambda) * spec;
        }
        target /= crate::color::cie1931::CIE_1931_Y_INTEGRAL_5NM;

        let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
        let (ex, ey, ez) = (
            rel_err(mean.x, target.x),
            rel_err(mean.y, target.y),
            rel_err(mean.z, target.z),
        );
        println!(
            "[edge-rounding furnace] mean={mean:?} target={target:?} rel_err=({ex:.4}, {ey:.4}, {ez:.4}) over {count} samples"
        );
        assert!(
            ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
            "rounded-edge shading should still converge to the uniform environment's own \
             radiance (mean={mean:?}, target={target:?}, rel_err=({ex}, {ey}, {ez}), \
             tolerance={TOLERANCE})"
        );
    }
}
