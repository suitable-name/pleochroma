//! `PreparedMaterial` / `trace_spectral_ray_prepared`: the zone kernels are built once per frame,
//! and the radiance is bit-identical to the unprepared entry point (which prepares per call).

use super::{
    super::{
        camera::Ray, environment::LightingPreset, intersect::build_plane_soa, sampling::hash_u32,
        surface::SurfaceScene, zoned::KERNEL_BUILDS,
    },
    PreparedMaterial, trace_spectral_ray_prepared, trace_spectral_ray_with_finish_soa_geom_surface,
};
use crate::{
    geometry::{cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::Camera,
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
    renderer::{cpu_frame::trace_pixels_interleaved, frame_scene::FrameScene},
};
use glam::{DVec3, Vec3};

/// Model-to-millimetre scale of every test material (zones are in millimetres).
const PATH_SCALE: f32 = 3.0;

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

fn seed_and_hero(i: u32) -> (u32, f32) {
    let seed = 52_000 + i;
    (seed, (hash_u32(seed) as f32) / 4_294_967_295.0)
}

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

fn band_zone(center_nm: f32, width_nm: f32, peak_per_mm: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        center_nm,
        width_nm,
        peak_per_mm,
    )]))
}

/// A base zone plus one coloured zone per `shapes` entry.
fn zoning_of(shapes: Vec<ZoneShape>) -> ZonedAbsorption {
    let mut zoning = ZonedAbsorption::new(band_zone(520.0, 60.0, 0.4));
    for (i, shape) in shapes.into_iter().enumerate() {
        zoning.zones.push(Zone {
            shape,
            absorption: band_zone((-40.0_f32).mul_add(i as f32, 620.0), 50.0, 0.9),
        });
    }
    zoning
}

fn half_space() -> ZoneShape {
    ZoneShape::HalfSpace {
        normal: DVec3::X,
        offset: 0.0,
    }
}

/// A closed, consistently wound cube shell of half-width `half` mm centred on the origin.
fn cube_shell(half: f64) -> ZoneShape {
    let h = half;
    ZoneShape::MeshShell {
        vertices: vec![
            [-h, -h, -h],
            [h, -h, -h],
            [h, h, -h],
            [-h, h, -h],
            [-h, -h, h],
            [h, -h, h],
            [h, h, h],
            [-h, h, h],
        ],
        triangles: vec![
            [0, 3, 2],
            [0, 2, 1],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [3, 7, 6],
            [3, 6, 2],
            [0, 4, 7],
            [0, 7, 3],
            [1, 2, 6],
            [1, 6, 5],
        ],
    }
}

/// A cubic probe stone with the given zoning installed.
fn zoned_probe(zoning: ZonedAbsorption) -> GemMaterial {
    GemMaterial::new_custom("zoned probe", 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
        .with_zoning(zoning)
        .with_absorption_path_scale(PATH_SCALE)
}

/// How many `PreparedZoneKernels` this thread built while `work` ran.
fn kernel_builds_during(work: impl FnOnce()) -> usize {
    let before = KERNEL_BUILDS.with(std::cell::Cell::get);
    work();
    KERNEL_BUILDS.with(std::cell::Cell::get) - before
}

/// 64 `(seed, hero_rand)` pairs on a brilliant: the prepared entry point and the unprepared one
/// agree to the bit, per component, and the prepared loop builds no kernel at all.
fn assert_prepared_is_bit_identical(material: &GemMaterial, label: &str) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let geom = StoneGeometry::planes_only(&planes);
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let fluorescence = Fluorescence::none();
    let prepared = PreparedMaterial::new(material);
    let mut lit = 0;
    for i in 0..64 {
        let (seed, hero) = seed_and_hero(i);
        let mut with_prepared = Vec3::ZERO;
        let builds = kernel_builds_during(|| {
            with_prepared = trace_spectral_ray_prepared(
                ray_for(i),
                geom,
                &plane_soa,
                &[],
                &prepared,
                fluorescence,
                12,
                environment,
                seed,
                hero,
                None,
                SurfaceScene::IDEAL,
            );
        });
        assert_eq!(builds, 0, "{label}: sample {i} rebuilt the zone kernels");
        let unprepared = trace_spectral_ray_with_finish_soa_geom_surface(
            ray_for(i),
            geom,
            &plane_soa,
            &[],
            material,
            fluorescence,
            12,
            environment,
            seed,
            hero,
            None,
            SurfaceScene::IDEAL,
        );
        assert_eq!(bits(with_prepared), bits(unprepared), "{label}: sample {i}");
        lit += usize::from(with_prepared.max_element() > 0.0);
    }
    assert!(lit > 0, "{label}: no sample returned any light");
}

#[test]
fn prepared_matches_unprepared_for_analytic_zones() {
    let material = zoned_probe(zoning_of(vec![half_space()]));
    assert_prepared_is_bit_identical(&material, "analytic zones");
}

#[test]
fn prepared_matches_unprepared_for_a_mesh_shell_zone() {
    let material = zoned_probe(zoning_of(vec![cube_shell(1.2)]));
    assert_prepared_is_bit_identical(&material, "mesh shell");
}

#[test]
fn prepared_matches_unprepared_on_a_biaxial_host() {
    let host = GemMaterial::by_name("Tanzanite").expect("built-in material");
    assert!(host.biaxial_indicatrix(550.0).is_some(), "a biaxial host");
    let material = host
        .with_zoning(zoning_of(vec![half_space()]))
        .with_absorption_path_scale(PATH_SCALE);
    assert_prepared_is_bit_identical(&material, "biaxial host");
}

#[test]
fn prepared_matches_unprepared_for_empty_zones() {
    let material = zoned_probe(zoning_of(Vec::new()));
    assert_prepared_is_bit_identical(&material, "empty zones");
}

/// A 4x4 frame at 4 spp is 64 traced samples; a mesh-shell material builds its kernels once.
#[test]
fn a_frame_builds_the_mesh_shell_kernels_once() {
    let material = zoned_probe(zoning_of(vec![cube_shell(1.2)]));
    let builds = frame_kernel_builds(&material);
    assert_eq!(builds, 1, "one build per frame, not one per sample");
}

#[test]
fn a_frame_of_an_unzoned_material_builds_no_kernel() {
    let material = GemMaterial::new_custom("plain probe", 1.76, 0.0, 0.0, [0.0, 0.0, 0.0]);
    assert_eq!(frame_kernel_builds(&material), 0);
}

/// Kernel builds of one `trace_pixels_interleaved` call over a 4x4 frame at 4 spp.
fn frame_kernel_builds(material: &GemMaterial) -> usize {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let camera = Camera::new(0.0, std::f32::consts::FRAC_PI_2 - 0.01, 3.2, 42.0);
    let scene = FrameScene {
        camera: &camera,
        width: 4,
        height: 4,
        planes: &planes,
        facet_finishes: &[],
        material,
        max_bounces: 12,
        environment: LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
    };
    let mut pixels = 0;
    let builds = kernel_builds_during(|| {
        pixels = trace_pixels_interleaved(&scene, &plane_soa, 0, 1, 0, 4).len();
    });
    assert_eq!(pixels, 16, "every pixel of the 4x4 frame was traced");
    builds
}
