//! Primary beams, reflection leaves and the branch-filtered equivalence of the beams'
//! first-surface reflections with the reference tracer (beam guide §16.2).
//!
//! The reference side traces the reference tracer with `kept = |sig, _| !sig_in(&d1, sig)`,
//! which zeroes every channel except the depth-1 reflections. Only pixels the beams cover fully
//! are compared (a ray that meets no facet is never offered to `kept`, so the background of a
//! boundary pixel would stay in the reference). The statistics are the CPU helpers of
//! `renderer/test_stats.rs`.
//!
//! The thresholds and sample counts are named constants. No synthetic HDR map is built here (an
//! `EnvironmentMap` with a sharp feature is costly in a test); the three analytic presets stand
//! for the lighting models.

use std::sync::atomic::AtomicBool;

use glam::{DVec2, DVec3, Vec3};

use super::{
    BeamOff, BeamSettings,
    beam::SpectralInterval,
    frame::BeamFrame,
    geom::homography::camera_direction_map,
    leaf::{
        ReflectionKit, exit_direction, exit_weight, node_count, reflection_interpolates,
        reflection_leaf,
    },
    primary::{TileRect, image_polygon, primary_beams},
    raster::rasterise_polygon,
    stone::BeamStone,
    tests_support::{
        BeamRig, MAX_BOUNCES, d1_reflection_signatures, diamond, glass, pentalume_planes, sapphire,
        sig_in,
    },
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            BACKDROP_GREY, EnvironmentSource, LightingPreset, cranley_patterson_rotate, hash_u32,
            radical_inverse_base,
            surface::SurfaceScene,
            transport::{
                PreparedMaterial,
                reference::{KeptFn, trace_spectral_ray_reference_prepared},
            },
        },
    },
    renderer::{
        frame_scene::FrameScene,
        path_cache::{
            bounce::EventKind,
            signature::{BRANCH_SIG_SEED, branch_step},
        },
        test_stats::{assert_z_stats, compare_streams, draw},
    },
};

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;

/// The tile side of the beam render in pixels (16 tiles, so threads have work to share).
const TILE_SIZE: u32 = 8;

/// The size of one equivalence case: the frame side, the reference samples per compared pixel and
/// the most pixels compared.
#[derive(Clone, Copy)]
struct CaseSize {
    /// The side of the square frame in pixels.
    frame: u32,
    /// Reference samples per compared pixel.
    samples: u32,
    /// The most pixels compared per case.
    max_pixels: usize,
}

/// The full rows: the 32 pixel frame, 2048 samples, 16 pixels.
const FULL: CaseSize = CaseSize {
    frame: FRAME,
    samples: 2048,
    max_pixels: 16,
};

/// The smoke rows: a 16 pixel frame, 256 samples, 8 pixels. The z-statistics are normalised by
/// the estimated spread, so `Z_LIMIT` holds unchanged at the smaller sample count; the rows only
/// guard against gross breakage and a default run still exercises the comparison path.
const SMOKE: CaseSize = CaseSize {
    frame: 16,
    samples: 256,
    max_pixels: 8,
};

/// The fewest fully covered pixels a case must offer.
const MIN_PIXELS: usize = 4;

/// Samples per candidate pixel for the "every sample hits the stone" screen.
const SCREEN_SAMPLES: u32 = 64;

/// The largest |z| allowed (`assert_z_stats` also bounds the fraction above 3).
const Z_LIMIT: f64 = 5.0;

/// A compared pixel whose beam and reference Y are both below this fraction of the brightest
/// compared beam pixel is dark: it is checked against that absolute bound instead of by z. Inside
/// the head-shadow core of the lit presets the visibility `1 - smoothstep(..)` is an `f32` near
/// zero, quantised in steps of about `6e-8` of the sky; the reference's spread there is that
/// quantisation, so a z score overstates small gaps.
const DARK_FLOOR: f64 = 1.0e-3;

/// The normal-incidence reflectance of the test glass (n = 1.6), `((n-1)/(n+1))^2`.
const GLASS_NORMAL_REFLECTANCE: f32 = 0.053_254_4;

/// The tolerance of the weight against the normal-incidence reflectance at the face centre.
const GLASS_WEIGHT_TOLERANCE: f32 = 2.0e-3;

/// The tolerance of an exit direction against the analytic mirror (an `f32` result).
const DIRECTION_TOLERANCE: f64 = 1.0e-6;

/// The whole test frame of `frame` pixels as one tile.
const fn whole_frame_of(frame: u32) -> TileRect {
    TileRect {
        x0: 0,
        y0: 0,
        x1: frame,
        y1: frame,
    }
}

/// The whole 32 pixel test frame as one tile.
const fn whole_frame() -> TileRect {
    whole_frame_of(FRAME)
}

/// The studio environment of a preset, with the grey tray under it.
fn environment_for(preset: LightingPreset) -> EnvironmentSource<'static> {
    preset.studio(1.0, 0.4, 0.35).with_backdrop(BACKDROP_GREY)
}

/// The reference tracer on the pixels of a rig.
struct Reference<'r> {
    /// The rig (stone, camera).
    rig: &'r BeamRig,
    /// The prepared material.
    prepared: PreparedMaterial<'r>,
    /// The lighting.
    environment: EnvironmentSource<'r>,
    /// The side of the square frame in pixels.
    frame: u32,
}

impl Reference<'_> {
    /// Sample `i` of pixel `pixel` through the reference tracer with the `kept` filter.
    fn sample(&self, pixel: (u32, u32), i: u32, kept: &KeptFn<'_>) -> Vec3 {
        let frame = self.frame;
        let salt = pixel.1 * frame + pixel.0;
        let (seed, hero_rand) = draw(i, salt);
        let unit = |h: u32| hash_u32(h) as f32 / 4_294_967_295.0;
        let jitter_x =
            cranley_patterson_rotate(radical_inverse_base(i, 2), unit(salt ^ 0x1357_9BDF)) - 0.5;
        let jitter_y =
            cranley_patterson_rotate(radical_inverse_base(i, 3), unit(salt ^ 0x2468_ACE0)) - 0.5;
        let ray = self.rig.camera.generate_ray(
            pixel.0 as f32,
            pixel.1 as f32,
            frame as f32,
            frame as f32,
            jitter_x,
            jitter_y,
        );
        trace_spectral_ray_reference_prepared(
            ray,
            StoneGeometry::planes_only(&self.rig.planes),
            &self.rig.soa,
            &[],
            &self.prepared,
            Fluorescence::none(),
            MAX_BOUNCES,
            self.environment,
            seed,
            hero_rand,
            None,
            SurfaceScene::IDEAL,
            Some(kept),
        )
        .xyz
    }
}

/// The pixels whose square the primary beams cover completely, row-major.
fn fully_covered_pixels(rig: &BeamRig, frame: u32) -> Vec<(u32, u32)> {
    let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
    let tile = whole_frame_of(frame);
    let mut area = vec![0.0_f64; (frame * frame) as usize];
    let beams = primary_beams(
        &stone,
        &rig.camera,
        (frame, frame),
        tile,
        SpectralInterval::all(48),
    );
    for beam in beams {
        let image = image_polygon(&beam).expect("an image polygon");
        for ((x, y), piece) in rasterise_polygon(&image, tile) {
            area[(y * frame + x) as usize] += piece.area();
        }
    }
    (0..frame * frame)
        .filter(|&i| area[i as usize] >= 1.0 - 1.0e-9)
        .map(|i| (i % frame, i / frame))
        .collect()
}

/// Beams against the reference for one rig and lighting.
fn check_equivalence(rig: &BeamRig, preset: LightingPreset, label: &str, size: CaseSize) {
    let environment = environment_for(preset);
    let CaseSize {
        frame: edge,
        samples,
        max_pixels,
    } = size;
    let scene = FrameScene {
        environment,
        ..rig.scene(edge, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, &BeamSettings::default())
        .unwrap_or_else(|off| panic!("{label}: beams are off: {off:?}"));
    let cancel = AtomicBool::new(false);
    let (image, stats) = frame
        .render_reflections_with_threads(TILE_SIZE, 4, &cancel)
        .expect("not cancelled");
    eprintln!("{label}: {}", stats.summary());
    assert!(stats.leaves > 0, "{label}: no leaves");

    let reference = Reference {
        rig,
        prepared: PreparedMaterial::new(&rig.material),
        environment,
        frame: edge,
    };
    let always = |_: u64, _: f32| true;
    let candidates = fully_covered_pixels(rig, edge);
    let stride = (candidates.len() / (2 * max_pixels)).max(1);
    let pixels: Vec<(u32, u32)> = candidates
        .into_iter()
        .step_by(stride)
        .filter(|&pixel| {
            (0..SCREEN_SAMPLES).all(|i| reference.sample(pixel, i, &always) == Vec3::ZERO)
        })
        .take(max_pixels)
        .collect();
    assert!(
        pixels.len() >= MIN_PIXELS,
        "{label}: only {} fully covered pixels",
        pixels.len()
    );

    let d1 = d1_reflection_signatures(
        u32::try_from(rig.planes.len()).expect("the plane count fits in u32"),
    );
    let kept = |sig: u64, _: f32| !sig_in(&d1, sig);
    let beam_value = |pixel: (u32, u32)| image[(pixel.1 * edge + pixel.0) as usize];
    // Per pixel: the reference's mean and the z scores of its stream against the beam value.
    let compared: Vec<(Vec3, Vec<f64>)> = pixels
        .iter()
        .map(|&pixel| {
            let mut sum = DVec3::ZERO;
            let zs = compare_streams(
                1,
                (samples, 2),
                |_, i| {
                    let value = reference.sample(pixel, i, &kept);
                    sum += value.as_dvec3();
                    value
                },
                |_, _| beam_value(pixel),
            );
            ((sum / f64::from(samples)).as_vec3(), zs)
        })
        .collect();
    assert_beams_match(label, &pixels, &compared, beam_value);
}

/// Checks the beam values of `pixels` against the reference means and z scores in `compared`.
fn assert_beams_match(
    label: &str,
    pixels: &[(u32, u32)],
    compared: &[(Vec3, Vec<f64>)],
    beam_value: impl Fn((u32, u32)) -> Vec3,
) {
    let brightest = pixels
        .iter()
        .map(|&p| f64::from(beam_value(p).y))
        .fold(0.0_f64, f64::max);
    let floor = DARK_FLOOR * brightest;
    let mut zs = Vec::new();
    let mut lit = 0_usize;
    // One line per compared pixel, so a failing row shows where the beams and the reference part.
    for (&pixel, (reference_mean, pixel_zs)) in pixels.iter().zip(compared) {
        let beam = beam_value(pixel);
        let (beam_y, reference_y) = (f64::from(beam.y), f64::from(reference_mean.y));
        let dark = beam_y.max(reference_y) < floor;
        eprintln!(
            "{label}: pixel {pixel:?}: beam Y {beam_y:.3e}, reference Y {reference_y:.3e}, relative {:+.4}{}",
            beam_y / reference_y.max(1.0e-12) - 1.0,
            if dark { " (dark)" } else { "" }
        );
        if dark {
            let gap = (beam.as_dvec3() - reference_mean.as_dvec3())
                .abs()
                .max_element();
            assert!(
                gap <= floor,
                "{label}: dark pixel {pixel:?} differs by {gap:.3e} (bound {floor:.3e})"
            );
        } else {
            lit += 1;
            zs.extend_from_slice(pixel_zs);
        }
    }
    assert!(
        lit >= MIN_PIXELS,
        "{label}: only {lit} compared pixels are lit"
    );
    let beam_sum: f64 = pixels.iter().map(|&p| f64::from(beam_value(p).y)).sum();
    let reference_mean: f64 = compared.iter().map(|(m, _)| f64::from(m.y)).sum();
    eprintln!(
        "{label}: {} pixels ({lit} lit), beam Y {beam_sum:.6}, reference Y {reference_mean:.6}, relative {:+.4}",
        pixels.len(),
        beam_sum / reference_mean.max(1.0e-12) - 1.0
    );
    assert!(beam_sum > 0.0, "{label}: the beams produced no light");
    assert_z_stats(&zs, Z_LIMIT, label);
}

/// A named material constructor of the rows.
type NamedMaterial = (&'static str, fn() -> GemMaterial);

/// The three materials of the rows.
fn materials() -> [NamedMaterial; 3] {
    [
        ("glass", glass),
        ("diamond", diamond),
        ("sapphire", sapphire),
    ]
}

/// Runs [`check_equivalence`] for the three materials on one stone and lighting.
fn check_stone(make: fn(GemMaterial) -> BeamRig, stone: &str, preset: LightingPreset, name: &str) {
    for (material_name, material) in materials() {
        let rig = make(material());
        check_equivalence(
            &rig,
            preset,
            &format!("{stone} {material_name} {name}"),
            FULL,
        );
    }
}

#[test]
#[ignore = "beam equivalence: faceted-stone smoke, over 60 s; scripts/run_beam_tests.sh step 03b runs it"]
fn the_pentalume_smoke_row_matches_the_reference_under_the_studio_rig() {
    // The small, fast version of the three ignored Pentalume rows below: one material, a 16 pixel
    // frame, 256 samples. It catches gross breakage of the leaf, lighting and branch-filter path.
    let rig = BeamRig::pentalume(glass());
    check_equivalence(
        &rig,
        LightingPreset::Daylight,
        "pentalume glass studio (smoke)",
        SMOKE,
    );
}

#[test]
fn cube_reflections_match_the_reference_under_the_studio_rig() {
    check_stone(BeamRig::cube, "cube", LightingPreset::Daylight, "studio");
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
fn cube_reflections_match_the_reference_under_the_light_tent() {
    check_stone(
        BeamRig::cube,
        "cube",
        LightingPreset::LightTent,
        "light tent",
    );
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
fn cube_reflections_match_the_reference_under_daylight_sun() {
    check_stone(
        BeamRig::cube,
        "cube",
        LightingPreset::DaylightSun,
        "daylight sun",
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn pentalume_reflections_match_the_reference_under_the_studio_rig() {
    check_stone(
        BeamRig::pentalume,
        "pentalume",
        LightingPreset::Daylight,
        "studio",
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn pentalume_reflections_match_the_reference_under_the_light_tent() {
    check_stone(
        BeamRig::pentalume,
        "pentalume",
        LightingPreset::LightTent,
        "light tent",
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn pentalume_reflections_match_the_reference_under_daylight_sun() {
    check_stone(
        BeamRig::pentalume,
        "pentalume",
        LightingPreset::DaylightSun,
        "daylight sun",
    );
}

#[test]
fn the_reflection_leaf_is_the_exact_mirror_of_the_camera_ray() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let settings = BeamSettings::default();
    let stone = BeamStone::new(&rig.planes, false).expect("the cube");
    let n_lambda = node_count(&settings);
    let kit = ReflectionKit::new(
        &rig.material,
        &rig.planes,
        &rig.camera,
        (FRAME, FRAME),
        &scene.environment,
        n_lambda,
    );
    let beams = primary_beams(
        &stone,
        &rig.camera,
        (FRAME, FRAME),
        whole_frame(),
        SpectralInterval::all(n_lambda),
    );
    assert_eq!(beams.len(), 1, "only the top face is visible");
    let beam = &beams[0];
    assert_eq!(stone.original_facet(beam.facet), 2, "the +Y plane");
    let leaf = reflection_leaf(beam, &stone, &kit, &settings).expect("a reflection leaf");

    assert!(leaf.glare && leaf.reflection);
    assert_eq!(leaf.nodes(), 48);
    assert_eq!(leaf.tile, 0);
    assert_eq!(leaf.tau_omega, kit.tau_omega());
    assert_eq!(leaf.prefix.steps(), &[(2_u32 << 8) | (1 << 1)]);
    assert_eq!(
        leaf.sig,
        branch_step(BRANCH_SIG_SEED, 2, EventKind::Reflect, false)
    );
    assert!(leaf.weights.iter().all(|&w| (0.0..=1.0).contains(&w)));

    // The directions are the mirrored camera directions: exact at any interior point.
    let map = camera_direction_map(&rig.camera, FRAME, FRAME);
    let centroid = leaf.image.centroid().truncate();
    for point in [
        centroid,
        centroid + DVec2::new(1.3, -0.7),
        centroid + DVec2::new(-4.1, 3.3),
    ] {
        let d = (map * point.extend(1.0)).normalize();
        let expected = d - 2.0 * d.dot(DVec3::Y) * DVec3::Y;
        for node in [0, 20, 47] {
            let got = exit_direction(&leaf, point, node).as_dvec3();
            assert!(
                (got - expected).length() < DIRECTION_TOLERANCE,
                "node {node} at {point:?}: {got:?} against {expected:?}"
            );
        }
    }

    // At the face centre the view is normal to the face: the weight is the glass's reflectance.
    let weight = exit_weight(&leaf, centroid, 20);
    assert!(
        (weight - GLASS_NORMAL_REFLECTANCE).abs() < GLASS_WEIGHT_TOLERANCE,
        "weight {weight} against {GLASS_NORMAL_REFLECTANCE}"
    );
    // The check passes or fails by the Fresnel curvature; it is logged, not asserted.
    eprintln!(
        "interpolation check on the whole top face: {}",
        reflection_interpolates(beam, &stone, &kit, &settings, &leaf)
    );
}

#[test]
fn primary_beams_conserve_area_across_tiles() {
    let rig = BeamRig::pentalume(glass());
    let stone = BeamStone::new(&rig.planes, false).expect("the Pentalume");
    assert_eq!(rig.planes.len(), pentalume_planes().len());
    let spectrum = SpectralInterval::all(48);
    let area_of = |tile: TileRect| -> f64 {
        primary_beams(&stone, &rig.camera, (FRAME, FRAME), tile, spectrum)
            .iter()
            .map(|beam| image_polygon(beam).expect("an image polygon").area())
            .sum()
    };
    let whole = area_of(whole_frame());
    assert!(whole > 100.0, "the stone covers {whole} px^2");
    let mut parts = 0.0;
    for (x0, y0) in [(0, 0), (16, 0), (0, 16), (16, 16)] {
        let tile = TileRect {
            x0,
            y0,
            x1: x0 + 16,
            y1: y0 + 16,
        };
        let beams = primary_beams(&stone, &rig.camera, (FRAME, FRAME), tile, spectrum);
        // Canonical order and inside the tile.
        assert!(beams.windows(2).all(|w| w[0].facet <= w[1].facet));
        let (lo, hi) = (tile.image_min(), tile.image_max());
        for beam in &beams {
            for v in image_polygon(beam).expect("an image polygon").vertices() {
                assert!(
                    v.x >= lo.x - 1e-9
                        && v.x <= hi.x + 1e-9
                        && v.y >= lo.y - 1e-9
                        && v.y <= hi.y + 1e-9,
                    "vertex {v:?} outside the tile"
                );
            }
        }
        parts += area_of(tile);
    }
    assert!(
        (whole - parts).abs() <= 1.0e-9 * whole,
        "the four tiles hold {parts}, the frame {whole}"
    );
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_image_is_bit_identical_for_one_and_eight_threads() {
    // Bit identity is exact at any size: a 16 pixel frame (four tiles) and 16 spectral nodes.
    let side = SMOKE.frame;
    let rig = BeamRig::pentalume(diamond());
    let scene = FrameScene {
        environment: environment_for(LightingPreset::LightTent),
        ..rig.scene(side, MAX_BOUNCES)
    };
    let settings = BeamSettings {
        spectral_nodes: 16,
        ..BeamSettings::default()
    };
    let frame = BeamFrame::new(&scene, &settings).expect("eligible");
    let cancel = AtomicBool::new(false);
    let (one, one_stats) = frame
        .render_reflections_with_threads(TILE_SIZE, 1, &cancel)
        .expect("not cancelled");
    let (eight, eight_stats) = frame
        .render_reflections_with_threads(TILE_SIZE, 8, &cancel)
        .expect("not cancelled");
    assert_eq!(one.len(), (side * side) as usize);
    let bits = |image: &[Vec3]| -> Vec<[u32; 3]> {
        image
            .iter()
            .map(|p| p.to_array().map(f32::to_bits))
            .collect()
    };
    assert_eq!(bits(&one), bits(&eight));
    assert_eq!(one_stats, eight_stats);
    assert!(
        one.iter().any(|p| p.y > 0.0),
        "an empty image proves nothing"
    );
}

#[test]
fn a_cancelled_render_returns_nothing() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let frame = BeamFrame::new(&scene, &BeamSettings::default()).expect("eligible");
    let cancel = AtomicBool::new(true);
    assert!(frame.render_reflections(TILE_SIZE, &cancel).is_none());
}

#[test]
fn a_reflection_needs_two_bounces_of_budget() {
    let rig = BeamRig::cube(glass());
    let scene = FrameScene {
        max_bounces: 1,
        ..rig.scene(FRAME, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, &BeamSettings::default()).expect("eligible");
    let cancel = AtomicBool::new(false);
    let (image, stats) = frame.render_reflections(TILE_SIZE, &cancel).expect("done");
    assert_eq!(stats.leaves, 0);
    assert!(image.iter().all(|p| *p == Vec3::ZERO));
}

#[test]
fn a_uv_lamp_falls_back_silently() {
    let rig = BeamRig::cube(glass());
    let scene = FrameScene {
        environment: environment_for(LightingPreset::UvLamp365),
        ..rig.scene(FRAME, MAX_BOUNCES)
    };
    let result = BeamFrame::new(&scene, &BeamSettings::default());
    assert!(matches!(result, Err(BeamOff::OutOfScopeLighting)));
}
