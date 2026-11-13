//! Tests of the reference tracer.
//!
//! They cover the `pack_sig` pin, the signature of one fixed cube path,
//! reference equal to production on a constant-index material, exact matching never ending the
//! hero, only bitwise-equal channels live on a dispersive material, and the `kept` zeroing.
//!
//! The reference-versus-production test allows up to `MAX_ULPS` ulps of difference per component.

use super::{
    super::{
        NUM_CHANNELS, camera::Ray, intersect::build_plane_soa, sampling::hash_u32,
        surface::SurfaceScene,
    },
    PreparedMaterial,
    reference::{KeptFn, ReferenceSample, trace_spectral_ray_reference_prepared},
    trace_spectral_ray_prepared, wrapped_hero_wavelengths,
};
use crate::{
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{LightingPreset, fluorescence::Fluorescence, materials::GemMaterial},
    renderer::path_cache::{
        bounce::EventKind,
        signature::{BRANCH_SIG_SEED, branch_step, pack_sig},
    },
};
use glam::Vec3;
use std::cell::Cell;

/// The live mask with every channel set.
const ALL_LIVE: u8 = u8::MAX;

// `ALL_LIVE` names eight channels.
const _: () = assert!(NUM_CHANNELS == 8);

/// How many ulps apart reference and production may be on a constant-index material.
const MAX_ULPS: u32 = 1;

/// A stone, its intersection arena and a material.
struct Rig {
    planes: Vec<GpuFacetPlane>,
    plane_soa: crate::simd::PlanesSoA32,
    material: GemMaterial,
}

impl Rig {
    fn new(planes: Vec<GpuFacetPlane>, material: GemMaterial) -> Self {
        let plane_soa = build_plane_soa(&planes);
        Self {
            planes,
            plane_soa,
            material,
        }
    }

    /// The standard round brilliant.
    fn brilliant(material: GemMaterial) -> Self {
        Self::new(StandardGemCuts::standard_round_brilliant(), material)
    }

    /// A cube of half-width one; facets `+X, -X, +Y, -Y, +Z, -Z` are indices 0 to 5.
    fn cube(material: GemMaterial) -> Self {
        let axes = [
            Vec3::X,
            Vec3::NEG_X,
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::NEG_Z,
        ];
        Self::new(
            axes.iter()
                .map(|axis| GpuFacetPlane::new(*axis, -1.0))
                .collect(),
            material,
        )
    }

    fn geom(&self) -> StoneGeometry<'_> {
        StoneGeometry {
            planes: &self.planes,
            tools: &[],
        }
    }

    /// The reference tracer on `ray` with the Studio ring lights.
    fn reference(
        &self,
        ray: Ray,
        max_bounces: u32,
        (seed, hero_rand): (u32, f32),
        kept: Option<&KeptFn<'_>>,
    ) -> ReferenceSample {
        let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        trace_spectral_ray_reference_prepared(
            ray,
            self.geom(),
            &self.plane_soa,
            &[],
            &PreparedMaterial::new(&self.material),
            Fluorescence::none(),
            max_bounces,
            env,
            seed,
            hero_rand,
            None,
            SurfaceScene::IDEAL,
            kept,
        )
    }

    /// The production tracer on `ray` with the Studio ring lights.
    fn production(&self, ray: Ray, max_bounces: u32, (seed, hero_rand): (u32, f32)) -> Vec3 {
        let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
        trace_spectral_ray_prepared(
            ray,
            self.geom(),
            &self.plane_soa,
            &[],
            &PreparedMaterial::new(&self.material),
            Fluorescence::none(),
            max_bounces,
            env,
            seed,
            hero_rand,
            None,
            SurfaceScene::IDEAL,
        )
    }
}

/// A material with a bitwise-constant index (Cauchy `b = c = 0`, cubic, no absorption).
fn constant_index_material() -> GemMaterial {
    GemMaterial::new_custom("constant probe", 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
}

/// A strongly dispersive cubic material: every channel of a comb has its own index.
fn dispersive_material() -> GemMaterial {
    GemMaterial::new_custom("dispersive probe", 2.4, 0.05, 0.0, [0.0, 0.0, 0.0])
}

fn centred(i: u32, salt: u32) -> f32 {
    (hash_u32(i ^ salt) as f32) / 4_294_967_295.0 - 0.5
}

/// An oblique camera ray from above that mostly meets the stone, varied per `i`.
fn ray_for(i: u32) -> Ray {
    Ray {
        origin: Vec3::new(centred(i, 0x1111) * 1.4, 2.5, centred(i, 0x2222) * 1.4),
        dir: Vec3::new(centred(i, 0x3333) * 0.8, -1.0, centred(i, 0x4444) * 0.8).normalize(),
    }
}

/// The `(seed, hero_rand)` pair of sample `i`.
fn draw(i: u32) -> (u32, f32) {
    let seed = 77_000 + i;
    (seed, (hash_u32(seed) as f32) / 4_294_967_295.0)
}

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

/// The distance of two finite floats in representable steps.
fn ulps(a: f32, b: f32) -> u32 {
    let ordered = |x: f32| {
        let v = i32::from_ne_bytes(x.to_ne_bytes());
        if v < 0 { i32::MIN.wrapping_sub(v) } else { v }
    };
    ordered(a).abs_diff(ordered(b))
}

#[test]
fn pack_sig_pin() {
    assert_eq!(
        pack_sig([0x0707_A745, 0x3F6E_6B9A]),
        0x3F6E_6B9A_0707_A745_u64
    );
}

/// One fixed ray on a cube. It meets the +Y facet (index 2) at x = 0.5 and, refracted, the +X
/// facet (index 0) at about y = -0.14 under total internal reflection (incidence 66 degrees
/// against a critical angle of 35 degrees). With two bounces the loop ends there, so the chain is
/// either the exterior reflection alone, `(2, Reflect)`, or `(2, Transmit)` then `(0, TIR)`.
#[test]
fn cube_path_signature_chain() {
    let rig = Rig::cube(constant_index_material());
    let ray = Ray {
        origin: Vec3::new(-0.5, 2.0, 0.0),
        dir: Vec3::new(1.0, -1.0, 0.0).normalize(),
    };
    let reflected = branch_step(BRANCH_SIG_SEED, 2, EventKind::Reflect, false);
    let entered = branch_step(BRANCH_SIG_SEED, 2, EventKind::Transmit, false);
    let trapped = branch_step(entered, 0, EventKind::TotalReflect, false);
    let (mut saw_reflected, mut saw_trapped) = (false, false);
    for i in 0..64 {
        let sample = rig.reference(ray, 2, draw(i), None);
        if sample.signature == reflected {
            saw_reflected = true;
        } else {
            assert_eq!(
                sample.signature, trapped,
                "sample {i}: the chain must be (2, Reflect) or (2, Transmit) + (0, TIR)"
            );
            saw_trapped = true;
        }
        assert_eq!(sample.live_mask, ALL_LIVE, "sample {i}: constant index");
    }
    assert!(saw_trapped, "no sample entered the cube in 64 draws");
    eprintln!("cube chain: exterior reflection seen = {saw_reflected}");
}

/// A path that meets no facet carries the seed.
#[test]
fn a_miss_keeps_the_seed_signature() {
    let rig = Rig::cube(constant_index_material());
    let ray = Ray {
        origin: Vec3::new(5.0, 5.0, 0.0),
        dir: Vec3::Y,
    };
    let sample = rig.reference(ray, 8, draw(0), None);
    assert_eq!(sample.signature, BRANCH_SIG_SEED);
    assert_eq!(sample.live_mask, ALL_LIVE);
}

/// On a constant-index material the reference and the production tracer agree (guide §16.1).
#[test]
fn reference_equals_production_on_constant_index() {
    let rig = Rig::brilliant(constant_index_material());
    let (mut exact, mut max_ulps) = (0usize, 0u32);
    let samples = 256;
    for i in 0..samples {
        let (ray, id) = (ray_for(i), draw(i));
        let reference = rig.reference(ray, 12, id, None).xyz;
        let production = rig.production(ray, 12, id);
        if bits(reference) == bits(production) {
            exact += 1;
        }
        for axis in 0..3 {
            let distance = ulps(reference[axis], production[axis]);
            max_ulps = max_ulps.max(distance);
            assert!(
                distance <= MAX_ULPS,
                "sample {i} axis {axis}: reference {reference:?} and production {production:?} \
                 differ by {distance} ulps (allowed {MAX_ULPS}); bit identity was expected"
            );
        }
    }
    eprintln!(
        "reference = production: {exact}/{samples} bit-identical, max {max_ulps} ulps \
         (bit identity {})",
        if exact == samples as usize {
            "holds"
        } else {
            "does NOT hold; only the ulp fallback does"
        }
    );
}

/// `DirectionMatch::Exact` never ends the hero, on a constant and on a dispersive material.
#[test]
fn exact_matching_never_terminates_the_hero() {
    for material in [constant_index_material(), dispersive_material()] {
        let name = material.name.clone();
        let rig = Rig::brilliant(material);
        for i in 0..256 {
            let sample = rig.reference(ray_for(i), 12, draw(i), None);
            assert_eq!(
                sample.live_mask & 1,
                1,
                "{name}, sample {i}: the hero channel must stay live"
            );
        }
    }
}

/// On a dispersive material no live channel's index differs in bits from the hero's. A path that
/// never refracted keeps every channel (they share its geometry), so those samples have a full mask.
#[test]
fn dispersive_live_channels_share_the_hero_index() {
    let material = dispersive_material();
    let rig = Rig::brilliant(material.clone());
    let (mut narrowed, mut full) = (0usize, 0usize);
    for i in 0..256 {
        let id = draw(i);
        let sample = rig.reference(ray_for(i), 12, id, None);
        let lambdas: [f32; 8] = wrapped_hero_wavelengths(id.1);
        let hero_bits = material.dispersion.evaluate(lambdas[0]).to_bits();
        if sample.live_mask == ALL_LIVE {
            full += 1;
            continue;
        }
        narrowed += 1;
        for (k, lambda) in lambdas.iter().enumerate() {
            if sample.live_mask & (1 << k) != 0 {
                assert_eq!(
                    material.dispersion.evaluate(*lambda).to_bits(),
                    hero_bits,
                    "sample {i}: live channel {k} ({lambda} nm) has an index that differs in bits \
                     from the hero's"
                );
            }
        }
    }
    assert!(
        narrowed > 0,
        "no sample refracted on a dispersive stone in 256 draws ({full} full masks)"
    );
    eprintln!("dispersive: {narrowed} narrowed masks, {full} full masks");
}

/// With `kept` always true every sample that met the stone has zero XYZ; the samples that did not
/// are untouched. Also: `kept` is offered exactly the live channels, at the sample's own
/// wavelengths and signature.
#[test]
fn kept_always_true_zeroes_stone_hits() {
    let material = dispersive_material();
    let rig = Rig::brilliant(material);
    let (mut zeroed, mut missed) = (0usize, 0usize);
    for i in 0..256 {
        let (ray, id) = (ray_for(i), draw(i));
        let plain = rig.reference(ray, 12, id, None);
        let calls = Cell::new(0u32);
        let lambdas: [f32; 8] = wrapped_hero_wavelengths(id.1);
        let packed = pack_sig(plain.signature);
        let always = |signature: u64, lambda: f32| {
            assert_eq!(signature, packed, "sample {i}: kept saw a wrong signature");
            assert!(
                lambdas.iter().any(|l| l.to_bits() == lambda.to_bits()),
                "sample {i}: kept saw wavelength {lambda}, not one of the comb"
            );
            calls.set(calls.get() + 1);
            true
        };
        let kept: &KeptFn<'_> = &always;
        let zeroed_sample = rig.reference(ray, 12, id, Some(kept));
        assert_eq!(zeroed_sample.signature, plain.signature);
        assert_eq!(zeroed_sample.live_mask, plain.live_mask);
        if plain.signature == BRANCH_SIG_SEED {
            missed += 1;
            assert_eq!(
                calls.get(),
                0,
                "sample {i}: a miss is never offered to kept"
            );
            assert_eq!(bits(zeroed_sample.xyz), bits(plain.xyz));
        } else {
            zeroed += 1;
            assert_eq!(
                calls.get(),
                plain.live_mask.count_ones(),
                "sample {i}: kept must be asked once per live channel"
            );
            assert_eq!(
                zeroed_sample.xyz,
                Vec3::ZERO,
                "sample {i}: all channels kept"
            );
        }
    }
    assert!(zeroed > 0, "no sample met the stone ({missed} misses)");
}

/// `kept = None` and a `kept` that never claims anything give the same sample bit for bit.
#[test]
fn kept_none_changes_nothing() {
    let rig = Rig::brilliant(dispersive_material());
    let never = |_: u64, _: f32| false;
    let kept: &KeptFn<'_> = &never;
    for i in 0..128 {
        let (ray, id) = (ray_for(i), draw(i));
        let without = rig.reference(ray, 12, id, None);
        let with = rig.reference(ray, 12, id, Some(kept));
        assert_eq!(bits(with.xyz), bits(without.xyz), "sample {i}");
        assert_eq!(with.signature, without.signature, "sample {i}");
        assert_eq!(with.live_mask, without.live_mask, "sample {i}");
    }
}
