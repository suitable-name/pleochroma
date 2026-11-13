//! Equivalence of the dispersive beam tree with the reference tracer, plus the sub-node wiring and
//! the aliasing check.
//!
//! The reference side works as in `tests_d2_equivalence`: the reference tracer is traced twice per
//! sample, with `kept = None` and with the kept index of the beam tree bound to the sample's image
//! point; the difference is the radiance of the branches the beams claim. Only pixels the primary
//! beams cover fully are compared. The rows are the dispersive stones (diamond and an isotropic
//! sapphire-like material on the cube and the brilliant, under the studio rig and the sun), the
//! narrow-band row (a 15 nm absorption band), the clamp row (diamond under the warmest Planckian
//! preset), the diamond boundary-pixel row and the wiring checks (depth bookkeeping, hero contexts,
//! glass unchanged).
//!
//! The sapphire-like material is isotropic; birefringent tracing has its own tests
//! (`tests_d4_biaxial`, `tests_d4_uniaxial`). The thresholds and sample counts are named constants.
//! Numbers that only a run can give are printed with `eprintln!` and not asserted.

use std::sync::{Arc, atomic::AtomicBool};

use glam::{DVec2, Vec3};

use super::{
    BeamSettings,
    aliasing::{aliasing_risk, worst_node_step},
    beam::{Beam, BranchPrefix, Leaf, SeamStore, SpectralInterval},
    entry::refract_entry,
    frame::{BeamFrame, BeamImage},
    geom::{Tolerances, polygon::ConvexPolygon},
    interact::{
        InteractKit, KitHeroes, build_subnodes, hero_node, hero_slot, internal_hit, on_group_grid,
    },
    leaf::{GROUP_HERO, ReflectionKit, node_count},
    lighting::sigma_l,
    primary::{TileRect, image_polygon, primary_beams},
    propagate::{partition, project_to, retrace_state},
    raster::rasterise_polygon,
    residual::KeptIndex,
    retrace::RetraceKit,
    snap::snap_partition,
    stats::BeamStats,
    stone::BeamStone,
    subnodes::depth_len,
    tests_support::{BeamRig, MAX_BOUNCES, diamond, glass},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
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
        test_stats::{assert_z_stats, compare_streams, draw},
    },
};

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;

/// The tile side of the beam render in pixels.
const TILE_SIZE: u32 = 8;

/// The worker threads of the beam renders.
const THREADS: usize = 4;

/// The spectral node count of the tests.
const N_LAMBDA: u32 = 48;

/// The node count as an index type.
const NODES: usize = 48;

/// The absorption sub-nodes per node of the default settings.
const SUB: usize = 4;

/// The size of one equivalence case.
#[derive(Clone, Copy)]
struct Case {
    /// The side of the square frame in pixels.
    frame: u32,
    /// Reference samples per compared pixel.
    samples: u32,
    /// The most pixels compared per case.
    max_pixels: usize,
    /// The spectral nodes of the beams.
    nodes: u32,
}

impl Case {
    /// The default beam settings at the case's node count.
    fn settings(self) -> BeamSettings {
        BeamSettings {
            spectral_nodes: self.nodes,
            ..BeamSettings::default()
        }
    }

    /// The whole frame as one tile.
    const fn whole_frame(self) -> TileRect {
        TileRect {
            x0: 0,
            y0: 0,
            x1: self.frame,
            y1: self.frame,
        }
    }
}

/// The full rows: the 32 pixel frame, 2048 samples, 16 pixels and the default 48 nodes. Their
/// statistical tests are `#[ignore]`d (a default run takes minutes per row).
const FULL: Case = Case {
    frame: FRAME,
    samples: 2048,
    max_pixels: 16,
    nodes: N_LAMBDA,
};

/// The small rows that stay in a default run: a 16 pixel frame, 256 samples, 8 pixels, all 48
/// nodes (the narrow-band row needs the node resolution). The z-statistics are normalised by the
/// estimated spread and the aggregate bounds carry their own standard error, so every threshold
/// holds unchanged at the smaller sample count; these rows only guard against gross breakage.
/// The structural tests (node step, bit identity) run at this frame size too: they are exact at
/// any size.
const SMALL: Case = Case {
    frame: 16,
    samples: 256,
    max_pixels: 8,
    nodes: N_LAMBDA,
};

/// The small rows of the brilliant: the 32 pixel frame (the faceted stone's kept regions cross so
/// many pixels that a 16 pixel frame may not offer [`MIN_PIXELS`] interior ones), 256 samples, 8
/// pixels and 16 nodes (two groups, still dispersive).
const BRILLIANT_SMALL: Case = Case {
    frame: FRAME,
    samples: 256,
    max_pixels: 8,
    nodes: 16,
};

/// The fewest fully covered interior pixels a case must offer.
const MIN_PIXELS: usize = 4;

/// Samples per candidate pixel for the "every sample hits the stone" screen.
const SCREEN_SAMPLES: u32 = 64;

/// The largest |z| allowed on interior pixels.
const Z_LIMIT: f64 = 5.0;

/// The largest |z| allowed on boundary pixels (the `2 ε_chord` over-count is added to the noise).
const BOUNDARY_Z_LIMIT: f64 = 8.0;

/// The number of standard errors the aggregated comparisons allow on top of their relative part.
const AGGREGATE_SIGMAS: f64 = 3.0;

/// The tolerance of a pixel counting as fully covered, in square pixels.
const COVERAGE_EPS: f64 = 1.0e-9;

/// A piece covering less than one pixel by more than this marks a boundary pixel.
const PARTIAL_EPS: f64 = 1.0e-6;

/// The narrow-band row's relative tolerance (guide §16.3).
const NARROW_BAND_TOLERANCE: f64 = 0.01;

/// The centre of the ruby-like band in nm.
const BAND_CENTRE_NM: f32 = 650.0;

/// The FWHM of the ruby-like band in nm.
const BAND_FWHM_NM: f32 = 15.0;

/// The FWHM of a Gaussian in units of its standard deviation.
const FWHM_PER_SIGMA: f32 = 2.354_82;

/// The peak absorption of the band, per model unit of path.
const BAND_PEAK: f32 = 1.0;

/// The node inside the band (its centre is 650.8 nm).
const RED_NODE: usize = 32;

/// A node far from the band (its centre is 431.3 nm).
const BLUE_NODE: usize = 6;

/// The relative tolerance of the replayed depths against the propagated ones.
const DEPTH_RELATIVE_TOLERANCE: f32 = 1.0e-3;

/// The absolute tolerance of the replayed depths against the propagated ones.
const DEPTH_ABSOLUTE_TOLERANCE: f32 = 1.0e-4;

/// The tolerance of the angles of the synthetic leaf.
const ANGLE_TOLERANCE: f64 = 1.0e-6;

/// The whole 32 pixel test frame as one tile.
const fn whole_frame() -> TileRect {
    TileRect {
        x0: 0,
        y0: 0,
        x1: FRAME,
        y1: FRAME,
    }
}

// ---- materials ----

/// A dispersive isotropic material with the index and the dispersion of sapphire (below the
/// single-node gate, so it starts with six intervals of eight nodes).
fn sapphire_like() -> GemMaterial {
    GemMaterial::new_custom("beam test sapphire-like", 1.768, 0.0106, 0.0, [0.0; 3])
}

/// `material` with the ruby-like absorption band for both eigenmodes.
fn with_band(mut material: GemMaterial) -> GemMaterial {
    let band = AbsorptionBand::new(BAND_CENTRE_NM, BAND_FWHM_NM / FWHM_PER_SIGMA, BAND_PEAK);
    material.absorption = AbsorptionTensor::isotropic(vec![band]);
    material
}

// ---- the reference side ----

/// The studio environment of a preset, with the grey tray under it.
fn environment_for(preset: LightingPreset) -> EnvironmentSource<'static> {
    preset.studio(1.0, 0.4, 0.35).with_backdrop(BACKDROP_GREY)
}

/// Which branches of a reference sample are zeroed.
#[derive(Clone, Copy)]
enum Filter<'i> {
    /// The plain reference tracer (`kept = None`).
    Plain,
    /// The branches the index keeps.
    Kept(&'i KeptIndex),
    /// Everything (the screen for "every sample hits the stone").
    Everything,
}

/// The reference tracer on the pixels of a rig.
struct Reference<'r> {
    /// The rig (stone, camera).
    rig: &'r BeamRig,
    /// The prepared material.
    prepared: PreparedMaterial<'r>,
    /// The lighting.
    environment: EnvironmentSource<'r>,
    /// The spectral node count of the beams.
    n_lambda: u32,
    /// The side of the square frame in pixels.
    frame: u32,
}

impl Reference<'_> {
    /// Sample `i` of pixel `pixel` through the reference tracer with `filter`.
    fn sample(&self, pixel: (u32, u32), i: u32, filter: Filter<'_>) -> Vec3 {
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
        let point = DVec2::new(
            f64::from(pixel.0) + f64::from(jitter_x),
            f64::from(pixel.1) + f64::from(jitter_y),
        );
        let n_lambda = self.n_lambda;
        let kept = |sig: u64, lambda: f32| match filter {
            Filter::Plain => false,
            Filter::Kept(index) => index.kept(point, sig, lambda, n_lambda),
            Filter::Everything => true,
        };
        let callback: Option<&KeptFn<'_>> = match filter {
            Filter::Plain => None,
            _ => Some(&kept as &KeptFn<'_>),
        };
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
            callback,
        )
        .xyz
    }

    /// The radiance of the kept branches: the plain sample minus the sample with the kept branches
    /// zeroed.
    fn kept_branches(&self, pixel: (u32, u32), i: u32, index: &KeptIndex) -> Vec3 {
        self.sample(pixel, i, Filter::Plain) - self.sample(pixel, i, Filter::Kept(index))
    }
}

/// The pixels whose square the primary beams cover completely, row-major.
fn fully_covered_pixels(rig: &BeamRig, case: Case) -> Vec<(u32, u32)> {
    let frame = case.frame;
    let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
    let tile = case.whole_frame();
    let mut area = vec![0.0_f64; (frame * frame) as usize];
    let beams = primary_beams(
        &stone,
        &rig.camera,
        (frame, frame),
        tile,
        SpectralInterval::all(N_LAMBDA),
    );
    for beam in beams {
        let image = image_polygon(&beam).expect("an image polygon");
        for ((x, y), piece) in rasterise_polygon(&image, tile) {
            area[(y * frame + x) as usize] += piece.area();
        }
    }
    (0..frame * frame)
        .filter(|&i| area[i as usize] >= 1.0 - COVERAGE_EPS)
        .map(|i| (i % frame, i / frame))
        .collect()
}

/// The pixels some kept region covers only partly (sorted): the pixels crossed by a leaf edge, a
/// snapped facet boundary, a bin-edge boundary or a TIR curve.
fn boundary_pixels(image: &BeamImage) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for (rect, index) in &image.kept {
        for region in index.regions() {
            let ring: Vec<DVec2> = region.polygon[..usize::from(region.len)]
                .iter()
                .map(|p| p.as_dvec2())
                .collect();
            let Some(polygon) = ConvexPolygon::from_image_points(&ring) else {
                continue;
            };
            for (pixel, piece) in rasterise_polygon(&polygon, *rect) {
                if piece.area() < 1.0 - PARTIAL_EPS {
                    out.push(pixel);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The kept index of the tile holding `pixel`.
fn index_of(image: &BeamImage, pixel: (u32, u32), case: Case) -> &KeptIndex {
    let columns = case.frame.div_ceil(TILE_SIZE);
    let tile = (pixel.1 / TILE_SIZE) * columns + pixel.0 / TILE_SIZE;
    &image.kept[tile as usize].1
}

/// The full beam tree of a rig under `environment` with `settings`.
fn render(
    rig: &BeamRig,
    environment: EnvironmentSource<'_>,
    settings: &BeamSettings,
    label: &str,
    frame_size: u32,
) -> BeamImage {
    let scene = FrameScene {
        environment,
        ..rig.scene(frame_size, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, settings)
        .unwrap_or_else(|off| panic!("{label}: beams are off: {off:?}"));
    let cancel = AtomicBool::new(false);
    let image = frame
        .render_beams_with_threads(TILE_SIZE, THREADS, &cancel)
        .expect("not cancelled");
    log_counts(label, &image);
    image
}

/// Prints the counts of a beam render.
fn log_counts(label: &str, image: &BeamImage) {
    let stats = &image.stats;
    eprintln!("{label}: {}", stats.summary());
    eprintln!(
        "{label}: primaries {}, entry pieces {}, apex splits {}, partitions {}, snapped vertices \
         {}, bracket failures {}, TIR splits {}",
        stats.primary_beams,
        stats.entry_pieces,
        stats.apex_splits,
        stats.partitions,
        stats.snapped_vertices,
        stats.bracket_failures,
        stats.tir_splits
    );
    eprintln!(
        "{label}: interval splits {:?}, residual band area {:.4} px2, worst node step {:.3e} rad",
        stats.interval_splits, stats.residual_band_area, stats.worst_node_step
    );
    eprintln!(
        "{label}: drops by reason {:?}, dropped energy {:?}",
        stats.drops, stats.dropped_energy
    );
}

/// Up to `case.max_pixels` of `candidates`, spread over the list, whose samples all hit the stone.
fn screened(reference: &Reference<'_>, candidates: Vec<(u32, u32)>, case: Case) -> Vec<(u32, u32)> {
    let stride = (candidates.len() / (2 * case.max_pixels)).max(1);
    candidates
        .into_iter()
        .step_by(stride)
        .filter(|&pixel| {
            (0..SCREEN_SAMPLES)
                .all(|i| reference.sample(pixel, i, Filter::Everything) == Vec3::ZERO)
        })
        .take(case.max_pixels)
        .collect()
}

/// The fully covered pixels that are (`boundary`) or are not on a kept boundary.
fn candidates(rig: &BeamRig, image: &BeamImage, boundary: bool, case: Case) -> Vec<(u32, u32)> {
    let edges = boundary_pixels(image);
    fully_covered_pixels(rig, case)
        .into_iter()
        .filter(|pixel| edges.binary_search(pixel).is_ok() == boundary)
        .collect()
}

/// What decided the interior pixels of a case, for the failure message: the pixels the primary beams
/// cover completely, the pixels some kept region crosses partly, and the covered pixels left over
/// (the candidates the "every sample hits the stone" screen then thins out).
fn interior_report(rig: &BeamRig, image: &BeamImage, case: Case) -> String {
    format!(
        "{} fully covered pixels, {} pixels crossed by a kept boundary, {} candidates before the \
         screen, {} leaves, {} frame pixels",
        fully_covered_pixels(rig, case).len(),
        boundary_pixels(image).len(),
        candidates(rig, image, false, case).len(),
        image.stats.leaves,
        case.frame * case.frame
    )
}

/// The mean and the standard error of the mean of `values`.
fn mean_and_se(values: &[f64]) -> (f64, f64) {
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / (n - 1.0);
    (mean, (variance / n).sqrt())
}

/// The scalar the rows aggregate: `X + Y + Z`, the sum the sub-node fold keeps exact (guide §9.1).
fn weight(v: Vec3) -> f64 {
    f64::from(v.x) + f64::from(v.y) + f64::from(v.z)
}

/// Which pixels a row compares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Fully covered pixels away from every kept boundary.
    Interior,
    /// Fully covered pixels crossed by a kept boundary.
    Boundary,
}

/// What a row measured.
struct Row {
    /// The pixels compared.
    pixels: usize,
    /// The beam value summed over the pixels (unclamped, `X + Y + Z`).
    beam: f64,
    /// The same with the end clamp `max(0)` applied per pixel and component.
    clamped: f64,
    /// The compared pixels with a negative component before the clamp.
    negatives: usize,
    /// The mean over the reference samples of the pixel sum of the kept branches.
    mean: f64,
    /// The standard error of that mean.
    se: f64,
}

impl Row {
    /// `beam / mean - 1`.
    fn relative(&self) -> f64 {
        self.beam / self.mean.max(1.0e-12) - 1.0
    }

    /// `clamped / mean - 1`.
    fn relative_clamped(&self) -> f64 {
        self.clamped / self.mean.max(1.0e-12) - 1.0
    }
}

/// Beams against the reference restricted to the kept branches, on the pixels of `kind`.
///
/// `z_limit` bounds the per-pixel z statistics when given. An interior row must offer
/// [`MIN_PIXELS`] pixels; a boundary row may find none (then every field is zero).
fn run_row(
    rig: &BeamRig,
    environment: EnvironmentSource<'_>,
    settings: &BeamSettings,
    label: &str,
    (kind, z_limit, case): (Kind, Option<f64>, Case),
) -> Row {
    let image = render(rig, environment, settings, label, case.frame);
    assert!(image.stats.leaves > 0, "{label}: no leaves");
    assert!(image.stats.entry_pieces > 0, "{label}: no entry pieces");
    let reference = Reference {
        rig,
        prepared: PreparedMaterial::new(&rig.material),
        environment,
        n_lambda: node_count(settings),
        frame: case.frame,
    };
    let pixels = screened(
        &reference,
        candidates(rig, &image, kind == Kind::Boundary, case),
        case,
    );
    eprintln!("{label}: {} pixels compared", pixels.len());
    if kind == Kind::Interior {
        assert!(
            pixels.len() >= MIN_PIXELS,
            "{label}: only {} fully covered interior pixels ({})",
            pixels.len(),
            interior_report(rig, &image, case)
        );
    } else if pixels.is_empty() {
        eprintln!("{label}: no boundary pixel in the frame, nothing to compare");
        return Row {
            pixels: 0,
            beam: 0.0,
            clamped: 0.0,
            negatives: 0,
            mean: 0.0,
            se: 0.0,
        };
    }
    let beam_value = |pixel: (u32, u32)| image.xyz[(pixel.1 * case.frame + pixel.0) as usize];
    let mut aggregate = vec![0.0_f64; case.samples as usize];
    let zs = compare_streams(
        pixels.len(),
        (case.samples, 2),
        |k, i| {
            let value = reference.kept_branches(pixels[k], i, index_of(&image, pixels[k], case));
            aggregate[i as usize] += weight(value);
            value
        },
        |k, _| beam_value(pixels[k]),
    );
    if let Some(limit) = z_limit {
        assert_z_stats(&zs, limit, label);
    }
    let (mean, se) = mean_and_se(&aggregate);
    Row {
        pixels: pixels.len(),
        beam: pixels.iter().map(|&p| weight(beam_value(p))).sum(),
        clamped: pixels
            .iter()
            .map(|&p| weight(beam_value(p).max(Vec3::ZERO)))
            .sum(),
        negatives: pixels
            .iter()
            .filter(|&&p| beam_value(p).min_element() < 0.0)
            .count(),
        mean,
        se,
    }
}

// ---- the dispersive rows ----

/// Beams against the reference on interior pixels, per-pixel z-tests (guide §16.2).
fn check_interior(rig: &BeamRig, preset: LightingPreset, label: &str, case: Case) {
    let row = run_row(
        rig,
        environment_for(preset),
        &case.settings(),
        label,
        (Kind::Interior, Some(Z_LIMIT), case),
    );
    eprintln!(
        "{label}: {} interior pixels, beam {:.6}, reference {:.6} +- {:.6}, relative {:+.5}",
        row.pixels,
        row.beam,
        row.mean,
        row.se,
        row.relative()
    );
    assert!(row.beam > 0.0, "{label}: the beams produced no light");
}

/// The boundary-pixel row (guide §16.3): pixels crossed by a kept boundary, where the boundaries
/// may over-count by about `2 ε_chord` of one branch's value (guide §12.3).
fn check_boundary(rig: &BeamRig, preset: LightingPreset, label: &str, case: Case) {
    let row = run_row(
        rig,
        environment_for(preset),
        &case.settings(),
        label,
        (Kind::Boundary, Some(BOUNDARY_Z_LIMIT), case),
    );
    if row.pixels == 0 {
        return;
    }
    let bound = 2.0 * case.settings().chord_tolerance_px;
    eprintln!(
        "{label}: beam {:.6}, reference {:.6} +- {:.6}, relative {:+.5} \
         (over-count bound {bound:.5})",
        row.beam,
        row.mean,
        row.se,
        row.relative()
    );
    assert!(
        row.beam - row.mean <= bound.mul_add(row.mean, AGGREGATE_SIGMAS * row.se),
        "{label}: the beams over-count the kept branches: beam {}, reference {} +- {}",
        row.beam,
        row.mean,
        row.se
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn diamond_cube_branches_match_the_reference_under_the_studio_rig() {
    check_interior(
        &BeamRig::cube(diamond()),
        LightingPreset::Daylight,
        "cube diamond studio",
        FULL,
    );
}

#[test]
fn diamond_cube_branches_match_the_reference_under_the_studio_rig_smoke() {
    check_interior(
        &BeamRig::cube(diamond()),
        LightingPreset::Daylight,
        "cube diamond studio (small)",
        SMALL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn diamond_cube_branches_match_the_reference_under_daylight_sun() {
    check_interior(
        &BeamRig::cube(diamond()),
        LightingPreset::DaylightSun,
        "cube diamond daylight sun",
        FULL,
    );
}

// The brilliant diamond has no interior row: a diamond is above the dispersion gate, so every node
// has its own tree, and its leaves are so dense that a kept boundary crosses every fully covered
// pixel. Its rows compare the boundary pixels instead, with the boundary row's over-count bound
// (see `diamond_brilliant_boundary_pixels_stay_within_the_over_count_bound` below).

#[test]
#[ignore = "beam equivalence: faceted-stone smoke, over 60 s; scripts/run_beam_tests.sh step 03b runs it"]
fn diamond_brilliant_boundary_pixels_stay_within_the_over_count_bound_smoke() {
    check_boundary(
        &BeamRig::brilliant(diamond()),
        LightingPreset::Daylight,
        "brilliant diamond boundary (small)",
        BRILLIANT_SMALL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn diamond_brilliant_boundary_pixels_stay_within_the_over_count_bound_under_daylight_sun() {
    check_boundary(
        &BeamRig::brilliant(diamond()),
        LightingPreset::DaylightSun,
        "brilliant diamond daylight sun boundary",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn sapphire_like_cube_branches_match_the_reference_under_the_studio_rig() {
    check_interior(
        &BeamRig::cube(sapphire_like()),
        LightingPreset::Daylight,
        "cube sapphire-like studio",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn sapphire_like_cube_branches_match_the_reference_under_daylight_sun() {
    check_interior(
        &BeamRig::cube(sapphire_like()),
        LightingPreset::DaylightSun,
        "cube sapphire-like daylight sun",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn sapphire_like_brilliant_branches_match_the_reference_under_the_studio_rig() {
    check_interior(
        &BeamRig::brilliant(sapphire_like()),
        LightingPreset::Daylight,
        "brilliant sapphire-like studio",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn sapphire_like_brilliant_branches_match_the_reference_under_daylight_sun() {
    check_interior(
        &BeamRig::brilliant(sapphire_like()),
        LightingPreset::DaylightSun,
        "brilliant sapphire-like daylight sun",
        FULL,
    );
}

// ---- the boundary-pixel row ----

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn diamond_brilliant_boundary_pixels_stay_within_the_over_count_bound() {
    check_boundary(
        &BeamRig::brilliant(diamond()),
        LightingPreset::Daylight,
        "brilliant diamond boundary",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn diamond_cube_boundary_pixels_stay_within_the_over_count_bound() {
    check_boundary(
        &BeamRig::cube(diamond()),
        LightingPreset::Daylight,
        "cube diamond boundary",
        FULL,
    );
}

#[test]
fn diamond_cube_boundary_pixels_stay_within_the_over_count_bound_smoke() {
    check_boundary(
        &BeamRig::cube(diamond()),
        LightingPreset::Daylight,
        "cube diamond boundary (small)",
        SMALL,
    );
}

// ---- the narrow-band row ----

/// A 15 nm absorption band: beams with the default sub-nodes against the reference within
/// [`NARROW_BAND_TOLERANCE`] plus the sampling noise of the reference; the same stone with one
/// sub-node per node is printed for comparison (guide §9.1, §16.3).
fn check_narrow_band(rig: &BeamRig, label: &str, case: Case) {
    let environment = environment_for(LightingPreset::Daylight);
    let row = |settings: &BeamSettings, name: &str| {
        run_row(
            rig,
            environment,
            settings,
            &format!("{label} {name}"),
            (Kind::Interior, None, case),
        )
    };
    let four = row(&case.settings(), "four sub-nodes");
    let one = row(
        &BeamSettings {
            absorption_subnodes: 1,
            ..case.settings()
        },
        "one sub-node",
    );
    eprintln!(
        "{label}: reference {:.6} +- {:.6}; four sub-nodes {:+.5}, one sub-node {:+.5}",
        four.mean,
        four.se,
        four.relative(),
        one.relative()
    );
    assert!(four.beam > 0.0, "{label}: the beams produced no light");
    assert!(
        (four.beam - four.mean).abs()
            <= NARROW_BAND_TOLERANCE.mul_add(four.mean, AGGREGATE_SIGMAS * four.se),
        "{label}: beam {} against reference {} +- {} is off by more than {NARROW_BAND_TOLERANCE}",
        four.beam,
        four.mean,
        four.se
    );
    // The sub-node wiring is live: with one sub-node per node the fold is the identity.
    assert_ne!(
        four.beam.to_bits(),
        one.beam.to_bits(),
        "{label}: four sub-nodes gave the same light as one"
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn the_ruby_like_band_in_glass_matches_the_reference_within_one_percent() {
    check_narrow_band(
        &BeamRig::cube(with_band(glass())),
        "cube banded glass narrow band",
        FULL,
    );
}

#[test]
fn the_ruby_like_band_in_glass_matches_the_reference_within_one_percent_smoke() {
    check_narrow_band(
        &BeamRig::cube(with_band(glass())),
        "cube banded glass narrow band (small)",
        SMALL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn the_ruby_like_band_in_a_dispersive_stone_matches_the_reference_within_one_percent() {
    check_narrow_band(
        &BeamRig::cube(with_band(sapphire_like())),
        "cube banded sapphire-like narrow band",
        FULL,
    );
}

// ---- the clamp row ----

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn diamond_under_the_warmest_planckian_preset_reports_the_end_clamp() {
    // The clamp row asserts only finiteness and `clamped >= beam`, both exact at any size and for
    // any stone with interior pixels. It runs on the diamond cube: the brilliant's kept regions
    // cross every pixel of its 32 pixel frame, so it offers the row no interior pixel.
    let label = "cube diamond illuminant A clamp";
    let row = run_row(
        &BeamRig::cube(diamond()),
        environment_for(LightingPreset::IlluminantA),
        &SMALL.settings(),
        label,
        (Kind::Interior, None, SMALL),
    );
    // Guide §16.3 fixes no number here: the per-pixel end clamp is reported, not asserted.
    eprintln!(
        "{label}: reference {:.6} +- {:.6}, beams unclamped {:+.5}, beams clamped {:+.5}, \
         {} of {} pixels had a negative component",
        row.mean,
        row.se,
        row.relative(),
        row.relative_clamped(),
        row.negatives,
        row.pixels
    );
    assert!(row.beam.is_finite() && row.clamped.is_finite());
    assert!(
        row.clamped >= row.beam - 1.0e-9,
        "{label}: the end clamp can only add light"
    );
}

// ---- the aliasing check ----

/// A unit vector tilted by `angle` radians from `+Z` about `+Y`.
fn tilted(angle: f32) -> Vec3 {
    Vec3::new(angle.sin(), 0.0, angle.cos())
}

/// A leaf over a unit square with the given per-vertex, per-node `dirs`.
fn leaf_with(dirs: Vec<Vec3>, last: u16) -> Leaf {
    let points = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    let spectrum = SpectralInterval { first: 0, last };
    let count = dirs.len();
    Leaf {
        tile: 0,
        image: ConvexPolygon::from_image_points(&points).expect("a polygon"),
        dirs,
        weights: vec![1.0; count],
        spectrum,
        sig: [1, 2],
        prefix: BranchPrefix::EMPTY,
        glare: false,
        reflection: false,
        tau_omega: 1.0e-4,
        pre_clip: None,
    }
}

#[test]
fn the_worst_node_step_is_the_largest_neighbour_angle_at_any_vertex() {
    // Four vertices, three nodes: the step between neighbouring nodes per vertex.
    let steps = [[0.001, 0.002], [0.003, 0.004], [0.010, 0.002], [0.0, 0.0]];
    let dirs: Vec<Vec3> = steps
        .iter()
        .flat_map(|[a, b]| [tilted(0.0), tilted(*a), tilted(a + b)])
        .collect();
    let leaf = leaf_with(dirs, 2);
    let worst = worst_node_step(&leaf);
    assert!(
        (worst - 0.010).abs() < ANGLE_TOLERANCE,
        "worst step {worst}"
    );
    // The risk test is `step > sigma_l / 2`.
    assert!(aliasing_risk(&leaf, 0.0199));
    assert!(!aliasing_risk(&leaf, 0.0205));
}

#[test]
fn a_leaf_without_node_dependent_directions_has_no_node_step() {
    let flat = leaf_with(vec![tilted(0.3); 12], 2);
    assert!(worst_node_step(&flat).abs() < f64::MIN_POSITIVE);
    assert!(!aliasing_risk(&flat, 0.0));
    let single = leaf_with(vec![tilted(0.1), tilted(0.2), tilted(0.3), tilted(0.4)], 0);
    assert!(worst_node_step(&single).abs() < f64::MIN_POSITIVE);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_worst_node_step_of_a_dispersive_stone_is_measured_and_logged() {
    // The node step is exact bookkeeping, not a statistic. The stone is the sapphire-like one,
    // whose intervals are groups of 8 nodes (two per tree at 16 nodes). A diamond is above the
    // dispersion gate, so every one of its trees is a single node and its step is 0 by
    // construction.
    let rig = BeamRig::brilliant(sapphire_like());
    let environment = environment_for(LightingPreset::Daylight);
    let settings = BRILLIANT_SMALL.settings();
    let image = render(
        &rig,
        environment,
        &settings,
        "sapphire-like node step",
        SMALL.frame,
    );
    let scene = FrameScene {
        environment,
        ..rig.scene(SMALL.frame, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, &settings).expect("eligible");
    let cancel = AtomicBool::new(false);
    let sigma = f64::from(sigma_l(&environment));
    let (mut worst, mut leaves, mut risky) = (0.0_f64, 0_usize, 0_usize);
    let tiles = (SMALL.frame.div_ceil(TILE_SIZE) * SMALL.frame.div_ceil(TILE_SIZE)) as usize;
    for index in 0..tiles {
        let (tile_leaves, _, _) = frame
            .tile_leaves(index, TILE_SIZE, &cancel)
            .expect("not cancelled");
        for leaf in &tile_leaves {
            worst = worst.max(worst_node_step(leaf));
            leaves += 1;
            risky += usize::from(aliasing_risk(leaf, sigma));
        }
    }
    eprintln!(
        "sapphire-like node step: worst {worst:.3e} rad over {leaves} leaves, sigma_L {sigma:.3e} rad, \
         {risky} leaves above sigma_L / 2"
    );
    assert!(
        worst.is_finite() && worst > 0.0,
        "the stone disperses: worst step {worst} over {leaves} leaves"
    );
    assert_eq!(worst.to_bits(), image.stats.worst_node_step.to_bits());
}

#[test]
fn glass_has_no_node_step_in_its_render() {
    let rig = BeamRig::cube(glass());
    let image = render(
        &rig,
        environment_for(LightingPreset::Daylight),
        &SMALL.settings(),
        "glass node step",
        SMALL.frame,
    );
    assert!(
        image.stats.worst_node_step < ANGLE_TOLERANCE,
        "glass has a node step of {}",
        image.stats.worst_node_step
    );
}

// ---- glass is unchanged by the wiring ----

/// The bit patterns of an image.
fn bits(image: &[Vec3]) -> Vec<[u32; 3]> {
    image
        .iter()
        .map(|p| p.to_array().map(f32::to_bits))
        .collect()
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn glass_renders_bit_for_bit_the_same_with_and_without_sub_nodes() {
    let rig = BeamRig::pentalume(glass());
    let environment = environment_for(LightingPreset::LightTent);
    // Bit identity is exact at any size: the 16 pixel frame.
    let wired = render(
        &rig,
        environment,
        &SMALL.settings(),
        "glass four",
        SMALL.frame,
    );
    let plain = render(
        &rig,
        environment,
        &BeamSettings {
            absorption_subnodes: 1,
            ..SMALL.settings()
        },
        "glass one",
        SMALL.frame,
    );
    assert_eq!(bits(&wired.xyz), bits(&plain.xyz));
    assert_eq!(wired.stats, plain.stats);
    assert_eq!(wired.kept, plain.kept);
    assert!(
        wired.xyz.iter().any(|p| p.y > 0.0),
        "an empty image proves nothing"
    );
}

#[test]
fn only_an_absorbing_material_with_sub_nodes_tracks_depths() {
    let cases = [
        (glass(), 4, false),
        (diamond(), 4, false),
        (with_band(glass()), 4, true),
        (with_band(sapphire_like()), 4, true),
        (with_band(glass()), 1, false),
        (with_band(glass()), 0, false),
    ];
    for (material, sub_nodes, tracked) in cases {
        let prepared = PreparedMaterial::new(&material);
        let settings = BeamSettings {
            absorption_subnodes: sub_nodes,
            ..BeamSettings::default()
        };
        let caches = build_subnodes(&prepared, &[], &material, N_LAMBDA, &settings);
        assert_eq!(
            caches.is_some(),
            tracked,
            "{} with {sub_nodes} sub-nodes",
            material.name
        );
    }
}

// ---- the kit fixture, hero contexts and depths ----

/// Runs `f` with the interaction kit of a cube rig of `material`, built the way the frame builds it.
fn with_kit<R>(
    material: GemMaterial,
    f: impl FnOnce(&InteractKit<'_, '_>, &BeamStone, &BeamRig) -> R,
) -> R {
    let rig = BeamRig::cube(material);
    let stone = BeamStone::new(&rig.planes, false).expect("a closed stone");
    let settings = BeamSettings::default();
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let reflection = ReflectionKit::new(
        &rig.material,
        &rig.planes,
        &rig.camera,
        (FRAME, FRAME),
        &scene.environment,
        N_LAMBDA,
    );
    let retrace = RetraceKit {
        camera: &rig.camera,
        size: (FRAME, FRAME),
        stone: &stone,
        material: &rig.material,
        n_lambda: N_LAMBDA,
    };
    let kit = reflection.interact_kit(retrace, &settings, whole_frame(), MAX_BOUNCES);
    f(&kit, &stone, &rig)
}

#[test]
fn the_hero_of_an_off_grid_interval_is_its_centre_node() {
    let grid = SpectralInterval::all(N_LAMBDA);
    assert!(on_group_grid(grid));
    for g in 0..grid.groups() {
        assert_eq!(hero_slot(grid, g), GROUP_HERO);
        assert_eq!(usize::from(hero_node(grid, g)), g * 8 + GROUP_HERO);
    }
    // Four nodes: the centre is node 1.
    let half = SpectralInterval { first: 0, last: 3 };
    assert!(!on_group_grid(half));
    assert_eq!((hero_slot(half, 0), hero_node(half, 0)), (1, 1));
    // The upper half of a group: nodes 4..=7, centre node 5.
    let upper = SpectralInterval { first: 4, last: 7 };
    assert_eq!((hero_slot(upper, 0), hero_node(upper, 0)), (1, 5));
    // A single node is its own hero.
    let single = SpectralInterval::single(5);
    assert_eq!((hero_slot(single, 0), hero_node(single, 0)), (0, 5));
    // Twelve nodes 12..=23: the centre (17) is in group 0 at slot 5; group 1 keeps the group hero,
    // clamped to the interval.
    let twelve = SpectralInterval {
        first: 12,
        last: 23,
    };
    assert_eq!((hero_slot(twelve, 0), hero_node(twelve, 0)), (5, 17));
    assert_eq!(
        (hero_slot(twelve, 1), hero_node(twelve, 1)),
        (GROUP_HERO, 23)
    );
}

#[test]
fn the_contexts_of_an_off_grid_interval_are_built_once_per_interval() {
    with_kit(glass(), |kit, _, _| {
        assert!(kit.subnodes.is_none(), "glass tracks no depths");
        assert!(matches!(
            kit.group_heroes(SpectralInterval::all(N_LAMBDA)),
            KitHeroes::Shared(_)
        ));
        let half = SpectralInterval { first: 0, last: 3 };
        let (KitHeroes::Own(first), KitHeroes::Own(second)) =
            (kit.group_heroes(half), kit.group_heroes(half))
        else {
            panic!("an interval off the grid has contexts of its own");
        };
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.len(), half.groups());
        let KitHeroes::Own(other) = kit.group_heroes(SpectralInterval::single(5)) else {
            panic!("a single node has contexts of its own");
        };
        assert!(!Arc::ptr_eq(&first, &other));
    });
}

/// The reflected children of the interior hits of the entry beams of the cube, with their kit.
fn reflected_children(kit: &InteractKit<'_, '_>, stone: &BeamStone, rig: &BeamRig) -> Vec<Beam> {
    let spectrum = SpectralInterval::all(N_LAMBDA);
    let mut stats = BeamStats::default();
    let mut seams = SeamStore::default();
    let mut reflected = Vec::new();
    for primary in primary_beams(stone, &rig.camera, (FRAME, FRAME), whole_frame(), spectrum) {
        for child in refract_entry(&primary, kit, &mut stats) {
            // The entry's segment is outside the stone: the depth vector is set up, all zero.
            let expected = depth_len(NODES, SUB);
            for state in &child.state.points {
                assert_eq!(state.depth.len(), expected);
                assert!(state.depth.iter().all(|&d| d == 0.0));
            }
            let split = partition(&child, stone, &Tolerances::DEFAULT);
            if !split.conserved {
                continue;
            }
            let Some(pieces) = snap_partition(&child, &split, kit, &mut seams, &mut stats) else {
                continue;
            };
            for piece in &pieces {
                if let Some(next) = project_to(&child, piece, kit, &mut stats) {
                    reflected.extend(internal_hit(&next, kit, &mut seams, &mut stats).reflected);
                }
            }
        }
    }
    reflected
}

#[test]
fn propagated_and_replayed_depths_agree_and_follow_the_band() {
    with_kit(with_band(glass()), |kit, stone, rig| {
        assert!(
            kit.subnodes.is_some(),
            "an absorbing material tracks depths"
        );
        let reflected = reflected_children(kit, stone, rig);
        assert!(!reflected.is_empty(), "the cube reflects internally");
        let expected = depth_len(NODES, SUB);
        let mut replayed = 0_usize;
        for beam in &reflected {
            for state in &beam.state.points {
                assert_eq!(state.depth.len(), expected);
                // The centre block `depth[NODES * SUB + i]` holds the depth at the node centres.
                let red = state.depth[NODES * SUB + RED_NODE];
                let blue = state.depth[NODES * SUB + BLUE_NODE];
                assert!(red > 0.1, "red depth {red}");
                assert!(blue < 1.0e-3 * red, "blue depth {blue} against red {red}");
                // The state a retrace builds for the same ray carries the same depths.
                let Some(again) = retrace_state(state.image, beam, kit) else {
                    continue;
                };
                assert_eq!(again.depth.len(), state.depth.len());
                for (a, b) in again.depth.iter().zip(&state.depth) {
                    let tolerance = DEPTH_RELATIVE_TOLERANCE
                        .mul_add(a.abs().max(b.abs()), DEPTH_ABSOLUTE_TOLERANCE);
                    assert!(
                        (a - b).abs() <= tolerance,
                        "replayed {a} against propagated {b}"
                    );
                }
                replayed += 1;
            }
        }
        eprintln!(
            "depths: {} reflected beams, {replayed} sample states replayed",
            reflected.len()
        );
        assert!(replayed > 0, "nothing was replayed");
    });
}
