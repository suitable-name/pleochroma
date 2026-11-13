//! The kept index and the full beam tree, and the branch-filtered equivalence of the beams
//! with the reference tracer (beam guide §16.2).
//!
//! The reference side traces the reference tracer twice per sample, with `kept = None` and with
//! `kept = index.kept` bound to the sample's image point: the difference is the radiance of the
//! branches the beams claim, which the beam image must equal (z-tests with the CPU helpers of
//! `renderer/test_stats.rs`). Only pixels the primary beams cover fully are compared (a ray that
//! meets no facet is never offered to `kept`). The reference samples are white-balanced with the
//! environment's balance, like the beam image (the identity for the daylight presets and an HDR
//! map).
//!
//! The thresholds and sample counts are named constants. The counts the tree logs (entry pieces,
//! partitions, snapped vertices, TIR splits, drops by reason) are printed with `eprintln!` and
//! never asserted.

use std::{cell::RefCell, collections::BTreeMap, sync::atomic::AtomicBool};

use glam::{DVec2, Vec3};

use super::{
    BeamSettings,
    beam::{BranchPrefix, KeptRegion, Leaf, SpectralInterval, SpectralNode},
    frame::{BeamFrame, BeamImage},
    geom::polygon::ConvexPolygon,
    leaf::sample_points,
    primary::{TileRect, image_polygon, primary_beams},
    raster::rasterise_polygon,
    residual::{KeptIndex, pack_kept},
    retrace::{RetraceKit, exact_ray_at, step_code, step_facet},
    snap::exit_facet,
    spectral::leaf_peak,
    stone::BeamStone,
    tests_support::{BeamRig, MAX_BOUNCES, glass},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            BACKDROP_GREY, EnvironmentSource, LightingPreset, apply_von_kries_white_balance,
            cranley_patterson_rotate,
            environment::environment_white_balance,
            hash_u32, radical_inverse_base,
            surface::SurfaceScene,
            transport::{
                PreparedMaterial,
                reference::{KeptFn, trace_spectral_ray_reference_prepared},
            },
        },
    },
    renderer::{
        env_map::EnvironmentMap,
        frame_scene::FrameScene,
        path_cache::signature::pack_sig,
        test_stats::{assert_z_stats, compare_streams, draw},
    },
};

/// The side of the square test frame in pixels.
const FRAME: u32 = 32;

/// The tile side of the beam render in pixels (16 tiles, so threads have work to share).
const TILE_SIZE: u32 = 8;

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
    /// The beam settings of the case.
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
    nodes: 48,
};

/// The small rows that stay in a default run: a 16 pixel frame, 256 samples, 8 pixels and 16
/// nodes. The z-statistics are normalised by the estimated spread and the over-count bound has its
/// own standard error, so every threshold holds unchanged at the smaller sample count; these rows
/// only guard against gross breakage. The structural tests (bit identity, budgets, counts) run at
/// this size too: they are exact at any size.
const SMALL: Case = Case {
    frame: 16,
    samples: 256,
    max_pixels: 8,
    nodes: 16,
};

/// The small interior Pentalume row: the 32 pixel frame, 256 samples, 8 pixels and 16 nodes.
///
/// The 16 pixel frame of [`SMALL`] offers the Pentalume no fully covered interior pixel, so this
/// row keeps the full frame of [`FULL`] and only cuts the samples, pixels and nodes.
const PENTALUME_SMALL: Case = Case {
    frame: FRAME,
    samples: 256,
    max_pixels: 8,
    nodes: 16,
};

/// The fewest fully covered interior pixels a case must offer.
const MIN_PIXELS: usize = 4;

/// Samples per candidate pixel for the "every sample hits the stone" screen.
const SCREEN_SAMPLES: u32 = 64;

/// The largest |z| allowed on interior pixels (`assert_z_stats` also bounds the fraction above 3).
const Z_LIMIT: f64 = 5.0;

/// The largest |z| allowed on boundary pixels, where the `2 ε_chord` over-count of the snapped
/// boundaries (guide §12.3) is added to the statistical noise.
const BOUNDARY_Z_LIMIT: f64 = 8.0;

/// Standard errors the aggregated over-count bound allows on top of its relative part.
const AGGREGATE_SIGMAS: f64 = 3.0;

/// The tolerance of a pixel counting as fully covered, in square pixels.
const COVERAGE_EPS: f64 = 1.0e-9;

/// A piece covering less than one pixel by more than this marks a boundary pixel.
const PARTIAL_EPS: f64 = 1.0e-6;

/// The radiance of the constant environment of the furnace row.
const FURNACE_RADIANCE: f32 = 2.5;

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
    /// The white balance of a studio environment (`None` for an HDR map), which the beam image
    /// carries and the raw reference samples do not.
    white_balance: Option<Vec3>,
}

impl Reference<'_> {
    /// Sample `i` of pixel `pixel` through the reference tracer with `filter`, white-balanced like
    /// the beam image (the identity for the daylight presets and an HDR map).
    fn sample(&self, pixel: (u32, u32), i: u32, filter: Filter<'_>) -> Vec3 {
        let xyz = self.raw_sample(pixel, i, filter);
        self.white_balance
            .map_or(xyz, |scale| apply_von_kries_white_balance(xyz, scale))
    }

    /// Sample `i` of pixel `pixel` through the reference tracer with `filter`, unbalanced.
    fn raw_sample(&self, pixel: (u32, u32), i: u32, filter: Filter<'_>) -> Vec3 {
        self.traced(pixel, i, filter, None)
    }

    /// The signatures the kept `index` claims on sample `i` of `pixel`, in the order the tracer
    /// asks for them.
    fn kept_signatures(&self, pixel: (u32, u32), i: u32, index: &KeptIndex) -> Vec<u64> {
        let recorded = RefCell::new(Vec::new());
        self.traced(pixel, i, Filter::Kept(index), Some(&recorded));
        recorded.into_inner()
    }

    /// The reference tracer on sample `i` of `pixel`; every signature `filter` zeroes is pushed
    /// to `record` when given.
    fn traced(
        &self,
        pixel: (u32, u32),
        i: u32,
        filter: Filter<'_>,
        record: Option<&RefCell<Vec<u64>>>,
    ) -> Vec3 {
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
        let kept = |sig: u64, lambda: f32| {
            let zeroed = match filter {
                Filter::Plain => false,
                Filter::Kept(index) => index.kept(point, sig, lambda, n_lambda),
                Filter::Everything => true,
            };
            if zeroed && let Some(record) = record {
                record.borrow_mut().push(sig);
            }
            zeroed
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
        SpectralInterval::all(48),
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
/// snapped facet boundary or a TIR curve (and by the silhouette, which the callers exclude).
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

/// How much of `pixel` the beams count twice, and the largest number of extra counts.
///
/// The kept test is a union (a sample is kept when any region of its branch holds it) while the
/// beams add every leaf, so two leaves of one branch that overlap at a point and node over-count
/// there. On a grid of points and every node this returns the extra regions (same branch, same
/// point and node) as a fraction of the covered (point, node) pairs, and the largest extra
/// count at one pair. Zero for both means the over-count is not an overlap of the leaves.
fn overlap_excess(image: &BeamImage, pixel: (u32, u32), case: Case) -> (f64, u32) {
    /// Grid points per side.
    const GRID: u32 = 16;
    let index = index_of(image, pixel, case);
    let regions = index.regions();
    let polygons: Vec<(usize, ConvexPolygon)> = index
        .pixel_regions(pixel)
        .iter()
        .filter_map(|&r| {
            let at = r as usize;
            let region = regions.get(at)?;
            let ring: Vec<DVec2> = region.polygon[..usize::from(region.len)]
                .iter()
                .map(|p| p.as_dvec2())
                .collect();
            ConvexPolygon::from_image_points(&ring).map(|polygon| (at, polygon))
        })
        .collect();
    let n_lambda = super::leaf::node_count(&case.settings());
    let nodes = u16::try_from(n_lambda).unwrap_or(u16::MAX);
    let (mut extra, mut covered, mut worst) = (0_u32, 0_u32, 0_u32);
    for gy in 0..GRID {
        for gx in 0..GRID {
            let q = DVec2::new(
                f64::from(pixel.0) - 0.5 + (f64::from(gx) + 0.5) / f64::from(GRID),
                f64::from(pixel.1) - 0.5 + (f64::from(gy) + 0.5) / f64::from(GRID),
            );
            let holding: Vec<&KeptRegion> = polygons
                .iter()
                .filter(|(_, polygon)| polygon.contains_2d(q, 0.0))
                .map(|&(at, _)| &regions[at])
                .collect();
            for node in 0..nodes {
                let mut sigs: Vec<u64> = holding
                    .iter()
                    .filter(|region| region.interval.contains(node))
                    .map(|region| pack_sig(region.sig))
                    .collect();
                let total = sigs.len();
                sigs.sort_unstable();
                sigs.dedup();
                if total > 0 {
                    covered += 1;
                }
                let more = u32::try_from(total - sigs.len()).unwrap_or(u32::MAX);
                extra += more;
                worst = worst.max(more);
            }
        }
    }
    (f64::from(extra) / f64::from(covered.max(1)), worst)
}

/// The kept index of the tile holding `pixel`.
fn index_of(image: &BeamImage, pixel: (u32, u32), case: Case) -> &KeptIndex {
    let columns = case.frame.div_ceil(TILE_SIZE);
    let tile = (pixel.1 / TILE_SIZE) * columns + pixel.0 / TILE_SIZE;
    &image.kept[tile as usize].1
}

/// The full beam tree of a rig under `environment` on `threads` threads.
fn render(
    rig: &BeamRig,
    environment: EnvironmentSource<'_>,
    label: &str,
    threads: usize,
    case: Case,
) -> BeamImage {
    let scene = FrameScene {
        environment,
        ..rig.scene(case.frame, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, &case.settings())
        .unwrap_or_else(|off| panic!("{label}: beams are off: {off:?}"));
    let cancel = AtomicBool::new(false);
    let image = frame
        .render_beams_with_threads(TILE_SIZE, threads, &cancel)
        .expect("not cancelled");
    log_counts(label, &image);
    image
}

/// Prints the counts of a beam render.
fn log_counts(label: &str, image: &BeamImage) {
    let stats = &image.stats;
    eprintln!("{label}: {}", stats.summary());
    eprintln!(
        "{label}: primaries {}, entry pieces {}, apex splits {}, partitions {}, re-partitions {}, \
         snapped vertices {}, bracket failures {}, TIR splits {}, conservation failures {}",
        stats.primary_beams,
        stats.entry_pieces,
        stats.apex_splits,
        stats.partitions,
        stats.repartitions,
        stats.snapped_vertices,
        stats.bracket_failures,
        stats.tir_splits,
        stats.conservation_failures
    );
    eprintln!(
        "{label}: drops by reason (cutoff, sliver, depth cap, min probability, fragment cap, \
         interpolation) {:?}, dropped energy {:?}",
        stats.drops, stats.dropped_energy
    );
    eprintln!("{label}: depth histogram {:?}", stats.depth_histogram);
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

/// The fully covered pixels on a kept boundary when `boundary` is set, the others otherwise.
fn candidates(rig: &BeamRig, image: &BeamImage, boundary: bool, case: Case) -> Vec<(u32, u32)> {
    let edges = boundary_pixels(image);
    fully_covered_pixels(rig, case)
        .into_iter()
        .filter(|pixel| edges.binary_search(pixel).is_ok() == boundary)
        .collect()
}

/// What decided the interior pixels of a case, for the failure message: the pixels the primary
/// beams cover completely, the pixels some kept region crosses partly, and the covered pixels
/// left over
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

/// A reference for `rig` under `environment`.
fn reference_for<'r>(
    rig: &'r BeamRig,
    environment: EnvironmentSource<'r>,
    case: Case,
) -> Reference<'r> {
    Reference {
        rig,
        prepared: PreparedMaterial::new(&rig.material),
        environment,
        n_lambda: super::leaf::node_count(&case.settings()),
        frame: case.frame,
        white_balance: match environment {
            EnvironmentSource::Studio { .. } => Some(environment_white_balance(environment)),
            EnvironmentSource::HdrMap { .. } => None,
        },
    }
}

/// Beams against the reference restricted to the kept branches, on interior pixels.
fn check_interior(rig: &BeamRig, preset: LightingPreset, label: &str, case: Case) {
    let environment = environment_for(preset);
    let image = render(rig, environment, label, 4, case);
    assert!(image.stats.leaves > 0, "{label}: no leaves");
    assert!(image.stats.entry_pieces > 0, "{label}: no entry pieces");
    let reference = reference_for(rig, environment, case);
    let pixels = screened(&reference, candidates(rig, &image, false, case), case);
    assert!(
        pixels.len() >= MIN_PIXELS,
        "{label}: only {} fully covered interior pixels ({})",
        pixels.len(),
        interior_report(rig, &image, case)
    );
    let beam_value = |pixel: (u32, u32)| image.xyz[(pixel.1 * case.frame + pixel.0) as usize];
    let mut reference_sum = 0.0_f64;
    let zs = compare_streams(
        pixels.len(),
        (case.samples, 2),
        |k, i| {
            let value = reference.kept_branches(pixels[k], i, index_of(&image, pixels[k], case));
            reference_sum += f64::from(value.y);
            value
        },
        |k, _| beam_value(pixels[k]),
    );
    let beam_sum: f64 = pixels.iter().map(|&p| f64::from(beam_value(p).y)).sum();
    let reference_mean = reference_sum / f64::from(case.samples);
    eprintln!(
        "{label}: {} interior pixels, beam Y {beam_sum:.6}, reference Y {reference_mean:.6}, \
         relative {:+.5}",
        pixels.len(),
        beam_sum / reference_mean.max(1.0e-12) - 1.0
    );
    assert!(beam_sum > 0.0, "{label}: the beams produced no light");
    assert_z_stats(&zs, Z_LIMIT, label);
}

/// What makes a pixel a boundary pixel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Crossing {
    /// A branch ends inside the pixel: the `2 ε_chord` argument of guide §12.3 applies.
    Branch = 0,
    /// Only a bisection seam inside one branch crosses the pixel: no over-count is expected.
    Seam = 1,
}

/// Whether a boundary `pixel` is crossed by a branch boundary or only by seams.
///
/// For every signature the kept regions in the pixel are summed by area, each weighted by its
/// share of the nodes. A pixel is a seam pixel when every signature covers it completely (the
/// regions of one branch tile it), and a branch-boundary pixel otherwise.
fn crossing_of(image: &BeamImage, pixel: (u32, u32), case: Case) -> Crossing {
    let index = index_of(image, pixel, case);
    let regions = index.regions();
    let nodes = f64::from(super::leaf::node_count(&case.settings()));
    let rect = TileRect {
        x0: pixel.0,
        y0: pixel.1,
        x1: pixel.0 + 1,
        y1: pixel.1 + 1,
    };
    let mut covered: BTreeMap<u64, f64> = BTreeMap::new();
    for &r in index.pixel_regions(pixel) {
        let Some(region) = regions.get(r as usize) else {
            continue;
        };
        let ring: Vec<DVec2> = region.polygon[..usize::from(region.len)]
            .iter()
            .map(|p| p.as_dvec2())
            .collect();
        let Some(polygon) = ConvexPolygon::from_image_points(&ring) else {
            continue;
        };
        let share = f64::from(u32::try_from(region.interval.len()).unwrap_or(u32::MAX)) / nodes;
        for (_, piece) in rasterise_polygon(&polygon, rect) {
            let total = covered.entry(pack_sig(region.sig)).or_insert(0.0);
            *total = piece.area().mul_add(share, *total);
        }
    }
    if !covered.is_empty() && covered.values().all(|&area| area >= 1.0 - PARTIAL_EPS) {
        Crossing::Seam
    } else {
        Crossing::Branch
    }
}

/// The reference's kept-branch tally of one signature at one pixel.
#[derive(Clone, Copy, Debug, Default)]
struct SignatureTally {
    /// The sum of the samples' Y.
    sum: f64,
    /// The samples that claimed the signature.
    count: u32,
    /// The largest sample Y.
    largest: f64,
}

impl SignatureTally {
    /// Adds one sample of Y `y`.
    fn add(&mut self, y: f64) {
        self.sum += y;
        self.count += 1;
        self.largest = self.largest.max(y);
    }
}

/// The beam's XYZ and leaf count per signature at `pixel`, shading each leaf of the pixel alone.
fn beam_by_signature(
    frame: &BeamFrame<'_, '_>,
    leaves: &[Leaf],
    index: &KeptIndex,
    pixel: (u32, u32),
) -> BTreeMap<u64, (Vec3, u32)> {
    let rect = TileRect {
        x0: pixel.0,
        y0: pixel.1,
        x1: pixel.0 + 1,
        y1: pixel.1 + 1,
    };
    let mut out: BTreeMap<u64, (Vec3, u32)> = BTreeMap::new();
    for &r in index.pixel_regions(pixel) {
        let Some(leaf) = leaves.get(r as usize) else {
            continue;
        };
        let xyz = frame
            .shade_leaves(std::slice::from_ref(leaf), rect)
            .first()
            .copied()
            .unwrap_or(Vec3::ZERO);
        let entry = out.entry(pack_sig(leaf.sig)).or_insert((Vec3::ZERO, 0));
        *entry = (entry.0 + xyz, entry.1 + 1);
    }
    out
}

/// Prints, per signature of `pixel`, the reference's kept-branch mean with its sample count and
/// largest sample share beside the beam's value and leaf count.
fn print_signatures(
    label: &str,
    pixel: (u32, u32),
    reference: &BTreeMap<u64, SignatureTally>,
    beam: &BTreeMap<u64, (Vec3, u32)>,
    samples: u32,
) {
    let sigs: std::collections::BTreeSet<u64> =
        reference.keys().chain(beam.keys()).copied().collect();
    for sig in sigs {
        let tally = reference.get(&sig).copied().unwrap_or_default();
        let (beam_xyz, leaves) = beam.get(&sig).copied().unwrap_or((Vec3::ZERO, 0));
        let beam_y = f64::from(beam_xyz.y);
        let mean = tally.sum / f64::from(samples);
        eprintln!(
            "{label}: pixel {pixel:?} signature {sig:#018x}: reference mean Y {mean:.6} from {} \
             samples, largest {:.3} of the sum; beam Y {beam_y:.6} from {leaves} leaves, relative \
             {:+.5}",
            tally.count,
            tally.largest / tally.sum.max(1.0e-12),
            beam_y / mean.max(1.0e-12) - 1.0
        );
    }
}

/// The most reference hits a branch may expect and still be unsampled by chance.
///
/// Poisson: zero hits at this mean has probability e^-5, 0.7 %.
const RARE_BRANCH_MAX_EXPECTED_HITS: f64 = 5.0;

/// How far inside a kept piece a check point lies, in chord tolerances.
///
/// A leaf edge that is a branch boundary lies within one chord tolerance of the exact curve (the
/// snap of guide section 7.4), within two at a vertex the snap leaves at its model position
/// (`propagate::project_to`); a point on the edge is off the branch for one of the two leaves that
/// share it. Four chord tolerances is twice the designed error.
const CHECK_INSET_CHORDS: f64 = 4.0;

/// The check points of `piece` moved `inset` pixels inside it, or `None` when it is thinner.
///
/// The points are the vertices, the centroid and the edge midpoints, scaled towards the centroid
/// until each lies at least `inset` pixels inside every edge.
fn inset_points(piece: &ConvexPolygon, inset: f64) -> Option<Vec<DVec2>> {
    let (points, _) = sample_points(piece);
    let centre = piece.centroid().truncate();
    let (ring, n) = piece.points_2d();
    // Scaling about the centroid by `1 - shrink` moves an edge inwards by `shrink` times its
    // distance from the centroid, so the nearest edge sets the factor.
    let nearest = (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let len = a.distance(b);
            if len > 0.0 {
                ((b - a).perp_dot(centre - a) / len).abs()
            } else {
                f64::INFINITY
            }
        })
        .fold(f64::INFINITY, f64::min);
    let shrink = inset / nearest;
    (shrink.is_finite() && shrink < 1.0).then(|| {
        points
            .iter()
            .map(|&q| (q - centre).mul_add(DVec2::splat(1.0 - shrink), centre))
            .collect()
    })
}

/// The first prefix step the exact ray through `q` at `nm` does not follow, if any.
///
/// The rule of `BranchFollower::follows`, with the step: an inside segment that leaves the stone
/// through another facet than the step names. `Some(0)` when the retracer refuses the point,
/// `None` when the ray follows the prefix.
fn first_off_step(kit: &RetraceKit<'_>, prefix: &BranchPrefix, q: DVec2, nm: f32) -> Option<usize> {
    let Ok(ray) = exact_ray_at(q, nm, prefix, kit) else {
        return Some(0);
    };
    prefix
        .steps()
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(i, &word)| {
            let (from, to) = (ray.hits[i - 1], ray.hits[i]);
            let expected = kit.stone.facet_of_original(step_facet(word));
            let leaves_through = (to - from)
                .try_normalize()
                .and_then(|dir| exit_facet(kit.stone, from, dir));
            (expected.is_none() || leaves_through != expected).then_some(i)
        })
}

/// The beam Y above which a signature the reference never sampled is diagnosed.
const ZERO_SAMPLE_MIN_Y: f64 = 1.0e-4;

/// The per-pixel evidence the reference stream gathered for the report, one entry per pixel.
struct PixelEvidence<'a> {
    /// The sum of the kept-branch Y over all samples.
    per_pixel: &'a [f64],
    /// The sum of the kept-branch Y over the first half of the stream.
    first_half: &'a [f64],
    /// The largest sample Y.
    largest: &'a [f64],
    /// The number of samples that carry a kept branch.
    carrying: &'a [u32],
    /// The kept-branch tally per signature.
    per_signature: &'a [BTreeMap<u64, SignatureTally>],
}

/// The signatures the beams light above [`ZERO_SAMPLE_MIN_Y`] that no reference sample claimed.
fn zero_sample_signatures(
    reference: &BTreeMap<u64, SignatureTally>,
    beam: &BTreeMap<u64, (Vec3, u32)>,
) -> Vec<u64> {
    beam.iter()
        .filter(|&(sig, &(xyz, _))| {
            f64::from(xyz.y) > ZERO_SAMPLE_MIN_Y
                && reference.get(sig).is_none_or(|tally| tally.count == 0)
        })
        .map(|(&sig, _)| sig)
        .collect()
}

/// The zero-sample signatures of one pixel and the reference sample count they are judged by.
struct Suspects<'a> {
    /// The signatures the beams light that no reference sample claimed.
    sigs: &'a [u64],
    /// The reference samples per pixel.
    samples: u32,
    /// How far inside a piece its check points lie, in pixels.
    inset: f64,
}

/// Prints, per leaf of a suspect signature at `pixel`, the covered pixel fraction, the Y per
/// covered area, the check points off the branch and the expected reference hits.
///
/// Returns the largest expected hit count among the leaves and whether any leaf has an interior
/// check point off its branch. A check point is a sample point of the piece moved
/// [`CHECK_INSET_CHORDS`] chord tolerances inside it (a point on the leaf's own edge is on the
/// branch boundary and decides nothing); it is on the branch when [`first_off_step`] finds no step
/// off. The expected hits are `samples * covered * peak`, because a path claims the branch only
/// when every reflect/transmit coin flip goes its way.
fn print_zero_sample_verdicts(
    label: &str,
    pixel: (u32, u32),
    kit: &RetraceKit<'_>,
    frame: &BeamFrame<'_, '_>,
    leaves: &[Leaf],
    index: &KeptIndex,
    suspects: &Suspects<'_>,
) -> (f64, bool) {
    let mut expected_max = 0.0_f64;
    let mut any_off = false;
    let rect = TileRect {
        x0: pixel.0,
        y0: pixel.1,
        x1: pixel.0 + 1,
        y1: pixel.1 + 1,
    };
    for &r in index.pixel_regions(pixel) {
        let Some(leaf) = leaves.get(r as usize) else {
            continue;
        };
        let sig = pack_sig(leaf.sig);
        if !suspects.sigs.contains(&sig) {
            continue;
        }
        let pieces = rasterise_polygon(&leaf.image, rect);
        let covered: f64 = pieces.iter().map(|(_, piece)| piece.area()).sum();
        let Some((_, largest)) = pieces
            .iter()
            .max_by(|a, b| a.1.area().total_cmp(&b.1.area()))
        else {
            eprintln!("{label}: pixel {pixel:?} signature {sig:#018x}: a leaf covers nothing");
            continue;
        };
        let y = frame
            .shade_leaves(std::slice::from_ref(leaf), rect)
            .first()
            .map_or(0.0, |value| f64::from(value.y));
        let nm = SpectralNode(leaf.spectrum.centre()).centre_nm(kit.n_lambda);
        // A piece thinner than the inset has no interior check point: nothing to judge there.
        let points = inset_points(largest, suspects.inset).unwrap_or_default();
        let off_steps: Vec<(usize, usize)> = points
            .iter()
            .enumerate()
            .filter_map(|(at, &q)| first_off_step(kit, &leaf.prefix, q, nm).map(|step| (at, step)))
            .collect();
        let off = off_steps.len();
        let peak = leaf_peak(leaf);
        let expected = f64::from(suspects.samples) * covered * peak;
        any_off |= off > 0;
        expected_max = expected_max.max(expected);
        let prefix: Vec<String> = leaf
            .prefix
            .steps()
            .iter()
            .map(|&w| {
                format!(
                    "{}{}",
                    step_facet(w),
                    if step_code(w) == 3 { 'T' } else { 'R' }
                )
            })
            .collect();
        let reading = if points.is_empty() {
            "too thin to judge"
        } else if off > 0 {
            "the leaf is off its branch (a)"
        } else if expected <= RARE_BRANCH_MAX_EXPECTED_HITS {
            "a rare branch the reference did not sample (b)"
        } else {
            "undecided: on its branch at every check point, yet the reference should have hit it"
        };
        eprintln!(
            "{label}: pixel {pixel:?} signature {sig:#018x} prefix [{}]: zero-sample leaf covers \
             {covered:.6} of the pixel, Y {y:.6} ({:.6} per covered area), peak weight {peak:.3e}, \
             expected reference hits {expected:.2}, {off} of {} interior check points off the branch \
             {off_steps:?} (point, first step off): {reading}",
            prefix.join(" "),
            y / covered.max(1.0e-12),
            points.len()
        );
    }
    (expected_max, any_off)
}

/// What the per-pixel report of the boundary row hands back to the assertions.
struct PixelVerdicts {
    /// The compared pixels split by what crosses them: `(beam Y, reference Y, pixels)` for a
    /// branch boundary and for a seam.
    by_crossing: [(f64, f64, u32); 2],
    /// Per pixel, the zero-sample signatures judged by the Poisson test, not the z-test.
    excluded_sigs: Vec<Vec<u64>>,
    /// Per pixel, the beam's XYZ over the excluded signatures.
    excluded_xyz: Vec<Vec3>,
    /// The largest expected reference hits of a zero-sample leaf.
    rare_expected_max: f64,
    /// Whether any zero-sample leaf has an interior check point off its branch.
    any_off_branch: bool,
}

/// Prints the per-pixel report of the boundary row and returns its verdicts.
///
/// Names the pixels that carry the over-count (the z list alone does not): the beam value, the
/// reference mean and the relative gap of every compared pixel, then the per-signature rows and
/// the zero-sample verdicts.
fn report_pixels(
    rig: &BeamRig,
    environment: EnvironmentSource<'_>,
    label: &str,
    case: Case,
    pixels: &[(u32, u32)],
    evidence: &PixelEvidence<'_>,
    image: &BeamImage,
) -> PixelVerdicts {
    let scene = FrameScene {
        environment,
        ..rig.scene(case.frame, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, &case.settings())
        .unwrap_or_else(|off| panic!("{label}: beams are off: {off:?}"));
    let stone = BeamStone::new(&rig.planes, false).expect("a stone");
    let kit = RetraceKit {
        camera: &rig.camera,
        size: (case.frame, case.frame),
        stone: &stone,
        material: &rig.material,
        n_lambda: super::leaf::node_count(&case.settings()),
    };
    let mut tile_leaves: BTreeMap<usize, Vec<Leaf>> = BTreeMap::new();
    let mut verdicts = PixelVerdicts {
        by_crossing: [(0.0_f64, 0.0_f64, 0_u32); 2],
        excluded_sigs: Vec::with_capacity(pixels.len()),
        excluded_xyz: Vec::with_capacity(pixels.len()),
        rare_expected_max: 0.0,
        any_off_branch: false,
    };
    for (k, (&pixel, total)) in pixels.iter().zip(evidence.per_pixel).enumerate() {
        let reference_mean = total / f64::from(case.samples);
        let beam_y = f64::from(image.xyz[(pixel.1 * case.frame + pixel.0) as usize].y);
        let half = f64::from(case.samples / 2);
        let first_half = evidence.first_half[k];
        let (first, second) = (first_half / half, (total - first_half) / half);
        let (extra, worst) = overlap_excess(image, pixel, case);
        let crossing = crossing_of(image, pixel, case);
        let slot = &mut verdicts.by_crossing[crossing as usize];
        *slot = (slot.0 + beam_y, slot.1 + reference_mean, slot.2 + 1);
        eprintln!(
            "{label}: pixel {pixel:?} ({crossing:?}): beam Y {beam_y:.6}, reference Y \
             {reference_mean:.6}, relative {:+.5}; reference halves {first:.6} / {second:.6}, \
             largest sample is {:.3} of the sum, {} of {} samples carry a kept branch; \
             same-branch overlap {extra:.5} of the covered (point, node) pairs, at most {worst} \
             extra",
            beam_y / reference_mean.max(1.0e-12) - 1.0,
            evidence.largest[k] / total.max(1.0e-12),
            evidence.carrying[k],
            case.samples
        );
        let index = index_of(image, pixel, case);
        let tile =
            (pixel.1 / TILE_SIZE * case.frame.div_ceil(TILE_SIZE) + pixel.0 / TILE_SIZE) as usize;
        let leaves = tile_leaves.entry(tile).or_insert_with(|| {
            let cancel = AtomicBool::new(false);
            frame
                .tile_leaves(tile, TILE_SIZE, &cancel)
                .map_or_else(Vec::new, |(leaves, _, _)| leaves)
        });
        let beam = beam_by_signature(&frame, leaves, index, pixel);
        print_signatures(
            label,
            pixel,
            &evidence.per_signature[k],
            &beam,
            case.samples,
        );
        let sigs = zero_sample_signatures(&evidence.per_signature[k], &beam);
        let suspects = Suspects {
            sigs: &sigs,
            samples: case.samples,
            inset: CHECK_INSET_CHORDS * case.settings().chord_tolerance_px,
        };
        let (expected, off) =
            print_zero_sample_verdicts(label, pixel, &kit, &frame, leaves, index, &suspects);
        verdicts.rare_expected_max = verdicts.rare_expected_max.max(expected);
        verdicts.any_off_branch |= off;
        let excluded_xyz: Vec3 = sigs
            .iter()
            .filter_map(|sig| beam.get(sig))
            .map(|&(xyz, _)| xyz)
            .sum();
        eprintln!(
            "{label}: pixel {pixel:?} excluded Y {:.6} of beam Y {beam_y:.6}",
            excluded_xyz.y
        );
        verdicts.excluded_sigs.push(sigs);
        verdicts.excluded_xyz.push(excluded_xyz);
    }
    verdicts
}

/// The reference's kept-branch stream of the compared pixels and the evidence gathered from it.
struct ReferenceStream {
    /// The kept-branch Y of every sample summed over the pixels.
    aggregate: Vec<f64>,
    /// The sum of the kept-branch Y over all samples, per pixel.
    per_pixel: Vec<f64>,
    /// The largest sample Y per pixel (heavy-tail evidence).
    largest: Vec<f64>,
    /// The sum of the kept-branch Y over the first half of the stream, per pixel.
    first_half: Vec<f64>,
    /// The number of samples that carry a kept branch, per pixel.
    carrying: Vec<u32>,
    /// The kept-branch tally per signature of every pixel (the sample's first claimed signature
    /// carries its value).
    per_signature: Vec<BTreeMap<u64, SignatureTally>>,
    /// The kept-branch value of every sample, per pixel.
    kept_values: Vec<Vec<Vec3>>,
    /// The first kept signature every sample claimed, per pixel.
    first_sig: Vec<Vec<Option<u64>>>,
}

/// Runs the reference's kept-branch stream over `pixels` once and stores it.
fn stream_reference(
    reference: &Reference<'_>,
    image: &BeamImage,
    pixels: &[(u32, u32)],
    case: Case,
) -> ReferenceStream {
    let n = pixels.len();
    let mut stream = ReferenceStream {
        aggregate: vec![0.0; case.samples as usize],
        per_pixel: vec![0.0; n],
        largest: vec![0.0; n],
        first_half: vec![0.0; n],
        carrying: vec![0; n],
        per_signature: vec![BTreeMap::new(); n],
        kept_values: vec![Vec::with_capacity(case.samples as usize); n],
        first_sig: vec![Vec::with_capacity(case.samples as usize); n],
    };
    for (k, &pixel) in pixels.iter().enumerate() {
        let index = index_of(image, pixel, case);
        for i in 0..case.samples {
            let value = reference.kept_branches(pixel, i, index);
            let y = f64::from(value.y);
            let mut first = None;
            if value != Vec3::ZERO {
                first = reference.kept_signatures(pixel, i, index).first().copied();
                if let Some(sig) = first {
                    stream.per_signature[k].entry(sig).or_default().add(y);
                }
                stream.carrying[k] += 1;
            }
            stream.aggregate[i as usize] += y;
            stream.per_pixel[k] += y;
            stream.largest[k] = stream.largest[k].max(y);
            if i < case.samples / 2 {
                stream.first_half[k] += y;
            }
            stream.kept_values[k].push(value);
            stream.first_sig[k].push(first);
        }
    }
    stream
}

/// The boundary-pixel row (guide §16.3): pixels crossed by a kept boundary, where the snapped
/// boundaries may over-count by about `2 ε_chord` of one branch's value (guide §12.3).
fn check_boundary(rig: &BeamRig, preset: LightingPreset, label: &str, case: Case) {
    let environment = environment_for(preset);
    let image = render(rig, environment, label, 4, case);
    let reference = reference_for(rig, environment, case);
    let pixels = screened(&reference, candidates(rig, &image, true, case), case);
    eprintln!("{label}: {} boundary pixels compared", pixels.len());
    if pixels.is_empty() {
        eprintln!("{label}: no boundary pixel in the frame, nothing to compare");
        return;
    }
    let beam_value = |pixel: (u32, u32)| image.xyz[(pixel.1 * case.frame + pixel.0) as usize];
    let stream = stream_reference(&reference, &image, &pixels, case);
    let beam_sum: f64 = pixels.iter().map(|&p| f64::from(beam_value(p).y)).sum();
    let (mean, se) = mean_and_se(&stream.aggregate);
    let chord = case.settings().chord_tolerance_px;
    let evidence = PixelEvidence {
        per_pixel: &stream.per_pixel,
        first_half: &stream.first_half,
        largest: &stream.largest,
        carrying: &stream.carrying,
        per_signature: &stream.per_signature,
    };
    let verdicts = report_pixels(rig, environment, label, case, &pixels, &evidence, &image);
    // The zero-sample signatures are judged by the Poisson assertions below, so the z-test sees
    // neither their samples (none exist) nor their beam value; every other signature stays in.
    let zs = compare_streams(
        pixels.len(),
        (case.samples, 2),
        |k, i| match stream.first_sig[k][i as usize] {
            Some(sig) if verdicts.excluded_sigs[k].contains(&sig) => Vec3::ZERO,
            _ => stream.kept_values[k][i as usize],
        },
        |k, _| beam_value(pixels[k]) - verdicts.excluded_xyz[k],
    );
    for (name, (beam_total, reference_total, count)) in
        ["branch boundary", "seam"].iter().zip(verdicts.by_crossing)
    {
        eprintln!(
            "{label}: {count} {name} pixels: beam Y {beam_total:.6}, reference Y \
             {reference_total:.6}, relative {:+.5}",
            beam_total / reference_total.max(1.0e-12) - 1.0
        );
    }
    eprintln!(
        "{label}: beam Y {beam_sum:.6}, reference Y {mean:.6} +- {se:.6}, relative {:+.5} \
         (over-count bound {:.5})",
        beam_sum / mean.max(1.0e-12) - 1.0,
        2.0 * chord
    );
    assert!(
        beam_sum - mean <= (2.0 * chord).mul_add(mean, AGGREGATE_SIGMAS * se),
        "{label}: the beams over-count the kept branches: beam {beam_sum}, reference {mean} +- {se}"
    );
    assert!(
        !verdicts.any_off_branch,
        "{label}: a kept leaf is off its branch at an interior check point (see the zero-sample verdicts above)"
    );
    assert!(
        verdicts.rare_expected_max <= RARE_BRANCH_MAX_EXPECTED_HITS,
        "{label}: a branch the reference never sampled expected {:.2} hits: the leaf is misplaced or \
         the index misses it",
        verdicts.rare_expected_max
    );
    assert_z_stats(&zs, BOUNDARY_Z_LIMIT, label);
}

/// The furnace row: in a constant environment the beams plus the reference with the kept branches
/// zeroed equal the plain reference, per pixel within z.
fn check_furnace(rig: &BeamRig, label: &str, case: Case) {
    let samples = case.samples;
    let map = EnvironmentMap::uniform(1, 1, [FURNACE_RADIANCE; 3]);
    let environment = EnvironmentSource::hdr(&map);
    let image = render(rig, environment, label, 4, case);
    let reference = reference_for(rig, environment, case);
    let pixels = screened(&reference, candidates(rig, &image, false, case), case);
    assert!(
        pixels.len() >= MIN_PIXELS,
        "{label}: only {} fully covered interior pixels ({})",
        pixels.len(),
        interior_report(rig, &image, case)
    );
    let beam_value = |pixel: (u32, u32)| image.xyz[(pixel.1 * case.frame + pixel.0) as usize];
    let mut plain_sum = 0.0_f64;
    let mut total_sum = 0.0_f64;
    let zs = compare_streams(
        pixels.len(),
        (samples, samples),
        |k, i| {
            let value = reference.sample(pixels[k], i, Filter::Plain);
            plain_sum += f64::from(value.y);
            value
        },
        |k, i| {
            // An independent stream: the residual samples are not the plain ones.
            let residual = reference.sample(
                pixels[k],
                i + samples,
                Filter::Kept(index_of(&image, pixels[k], case)),
            );
            let value = beam_value(pixels[k]) + residual;
            total_sum += f64::from(value.y);
            value
        },
    );
    eprintln!(
        "{label}: furnace plain Y {:.6}, beams + residual Y {:.6}",
        plain_sum / f64::from(samples),
        total_sum / f64::from(samples)
    );
    assert_z_stats(&zs, Z_LIMIT, label);
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn glass_cube_branches_match_the_reference_under_the_studio_rig() {
    check_interior(
        &BeamRig::cube(glass()),
        LightingPreset::Daylight,
        "cube glass studio",
        FULL,
    );
}

#[test]
fn glass_cube_branches_match_the_reference_under_the_studio_rig_smoke() {
    check_interior(
        &BeamRig::cube(glass()),
        LightingPreset::Daylight,
        "cube glass studio (small)",
        SMALL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn glass_cube_branches_match_the_reference_under_daylight_sun() {
    check_interior(
        &BeamRig::cube(glass()),
        LightingPreset::DaylightSun,
        "cube glass daylight sun",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn glass_pentalume_branches_match_the_reference_under_the_studio_rig() {
    check_interior(
        &BeamRig::pentalume(glass()),
        LightingPreset::Daylight,
        "pentalume glass studio",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: Pentalume smoke, over 60 s; scripts/run_beam_tests.sh step 03b runs it"]
fn glass_pentalume_branches_match_the_reference_under_the_studio_rig_smoke() {
    check_interior(
        &BeamRig::pentalume(glass()),
        LightingPreset::Daylight,
        "pentalume glass studio (small)",
        PENTALUME_SMALL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn glass_pentalume_branches_match_the_reference_under_daylight_sun() {
    check_interior(
        &BeamRig::pentalume(glass()),
        LightingPreset::DaylightSun,
        "pentalume glass daylight sun",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn boundary_pixels_of_the_cube_stay_within_the_over_count_bound() {
    check_boundary(
        &BeamRig::cube(glass()),
        LightingPreset::Daylight,
        "cube glass boundary",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn boundary_pixels_of_the_pentalume_stay_within_the_over_count_bound() {
    check_boundary(
        &BeamRig::pentalume(glass()),
        LightingPreset::Daylight,
        "pentalume glass boundary",
        FULL,
    );
}

#[test]
#[ignore = "beam equivalence: Pentalume smoke, over 60 s; scripts/run_beam_tests.sh step 03b runs it"]
fn boundary_pixels_of_the_pentalume_stay_within_the_over_count_bound_smoke() {
    check_boundary(
        &BeamRig::pentalume(glass()),
        LightingPreset::Daylight,
        "pentalume glass boundary (small)",
        SMALL,
    );
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn the_cube_furnace_beams_plus_residual_equal_the_reference() {
    check_furnace(&BeamRig::cube(glass()), "cube glass furnace", FULL);
}

#[test]
fn the_cube_furnace_beams_plus_residual_equal_the_reference_smoke() {
    check_furnace(&BeamRig::cube(glass()), "cube glass furnace (small)", SMALL);
}

#[test]
#[ignore = "beam equivalence: long; scripts/run_beam_tests.sh --with-long (step 09) runs it"]
fn the_pentalume_furnace_beams_plus_residual_equal_the_reference() {
    check_furnace(
        &BeamRig::pentalume(glass()),
        "pentalume glass furnace",
        FULL,
    );
}

/// The bit patterns of an image.
fn bits(image: &[Vec3]) -> Vec<[u32; 3]> {
    image
        .iter()
        .map(|p| p.to_array().map(f32::to_bits))
        .collect()
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_full_tree_is_bit_identical_for_one_and_eight_threads_and_across_runs() {
    // Bit identity is exact at any size: the small case.
    let rig = BeamRig::pentalume(glass());
    let environment = environment_for(LightingPreset::LightTent);
    let one = render(&rig, environment, "one thread", 1, SMALL);
    let eight = render(&rig, environment, "eight threads", 8, SMALL);
    let again = render(&rig, environment, "eight threads again", 8, SMALL);
    assert_eq!(one.xyz.len(), (SMALL.frame * SMALL.frame) as usize);
    assert_eq!(bits(&one.xyz), bits(&eight.xyz));
    assert_eq!(bits(&eight.xyz), bits(&again.xyz));
    assert_eq!(one.stats, eight.stats);
    assert_eq!(eight.stats, again.stats);
    assert_eq!(one.kept, eight.kept);
    assert_eq!(eight.kept, again.kept);
    assert!(
        one.xyz.iter().any(|p| p.y > 0.0),
        "an empty image proves nothing"
    );
}

#[test]
fn a_cancelled_tree_returns_nothing() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, MAX_BOUNCES);
    let frame = BeamFrame::new(&scene, &BeamSettings::default()).expect("eligible");
    let cancel = AtomicBool::new(true);
    assert!(frame.render_beams(TILE_SIZE, &cancel).is_none());
}

#[test]
#[ignore = "beam: over 60 s in the workspace run; scripts/run_beam_tests.sh step 03b runs it"]
fn a_two_bounce_budget_gives_the_reflection_image() {
    let rig = BeamRig::pentalume(glass());
    let scene = FrameScene {
        max_bounces: 2,
        environment: environment_for(LightingPreset::Daylight),
        ..rig.scene(SMALL.frame, MAX_BOUNCES)
    };
    let frame = BeamFrame::new(&scene, &SMALL.settings()).expect("eligible");
    let cancel = AtomicBool::new(false);
    let tree = frame
        .render_beams_with_threads(TILE_SIZE, 2, &cancel)
        .expect("done");
    let (reflections, _) = frame
        .render_reflections_with_threads(TILE_SIZE, 2, &cancel)
        .expect("done");
    let clamped: Vec<Vec3> = tree.xyz.iter().map(|p| p.max(Vec3::ZERO)).collect();
    assert_eq!(bits(&clamped), bits(&reflections));
    assert_eq!(tree.stats.entry_pieces, 0);
    assert_eq!(tree.stats.partitions, 0);
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 03"]
fn the_tree_logs_its_counts() {
    let rig = BeamRig::pentalume(glass());
    let image = render(
        &rig,
        environment_for(LightingPreset::Daylight),
        "pentalume glass counts",
        4,
        SMALL,
    );
    let regions: usize = image.kept.iter().map(|(_, index)| index.len()).sum();
    eprintln!(
        "pentalume glass counts: {} kept regions in {} tiles, {} leaves",
        regions,
        image.kept.len(),
        image.stats.leaves
    );
    assert!(regions > 0);
    assert!(image.stats.primary_beams > 0);
    assert!(image.stats.partitions > 0);
}

// ---- the kept index and its adapters ----

/// A leaf over the image polygon `points` (counter-clockwise) with signature `sig` and nodes
/// `first..=last`.
fn leaf_over(points: &[DVec2], sig: [u32; 2], first: u16, last: u16) -> Leaf {
    let spectrum = SpectralInterval { first, last };
    let count = points.len() * spectrum.len();
    Leaf {
        tile: 0,
        image: ConvexPolygon::from_image_points(points).expect("a polygon"),
        dirs: vec![Vec3::Z; count],
        weights: vec![1.0; count],
        spectrum,
        sig,
        prefix: BranchPrefix::EMPTY,
        glare: false,
        reflection: false,
        tau_omega: 1.0e-4,
        pre_clip: None,
    }
}

/// The corners of the axis-aligned square `[lo, hi]²`, counter-clockwise.
fn square(lo: f64, hi: f64) -> Vec<DVec2> {
    vec![
        DVec2::new(lo, lo),
        DVec2::new(hi, lo),
        DVec2::new(hi, hi),
        DVec2::new(lo, hi),
    ]
}

/// The tile `[x0, x1) x [y0, y1)`.
const fn tile(x0: u32, y0: u32, x1: u32, y1: u32) -> TileRect {
    TileRect { x0, y0, x1, y1 }
}

#[test]
fn the_kept_adapter_applies_signature_node_range_and_polygon() {
    let sig = [1, 2];
    // Nodes 8..=15 of 48 are the bins from 446.7 nm to 513.3 nm.
    let leaf = leaf_over(&square(2.0, 5.0), sig, 8, 15);
    let index = KeptIndex::from_leaves(tile(0, 0, 8, 8), &[leaf]);
    let packed = pack_sig(sig);
    assert!(index.kept(DVec2::new(3.0, 3.0), packed, 480.0, 48));
    // A boundary point and a vertex count as inside (edges inclusive).
    assert!(index.kept(DVec2::new(5.0, 3.0), packed, 480.0, 48));
    assert!(index.kept(DVec2::new(2.0, 2.0), packed, 480.0, 48));
    // Another signature, a wavelength outside the node range, a point outside the polygon and
    // points outside the tile are not kept.
    assert!(!index.kept(DVec2::new(3.0, 3.0), pack_sig([1, 3]), 480.0, 48));
    assert!(!index.kept(DVec2::new(3.0, 3.0), packed, 420.0, 48));
    assert!(!index.kept(DVec2::new(3.0, 3.0), packed, 560.0, 48));
    assert!(!index.kept(DVec2::new(6.0, 3.0), packed, 480.0, 48));
    assert!(!index.kept(DVec2::new(9.0, 3.0), packed, 480.0, 48));
    assert!(!index.kept(DVec2::new(f64::NAN, 3.0), packed, 480.0, 48));
}

#[test]
fn the_kept_index_lists_overlapping_regions_per_pixel_in_ascending_order() {
    let first = leaf_over(&square(1.0, 4.0), [1, 1], 0, 47);
    let second = leaf_over(&square(3.0, 6.0), [2, 2], 0, 47);
    let index = KeptIndex::from_leaves(tile(0, 0, 8, 8), &[first, second]);
    assert_eq!(index.len(), 2);
    assert_eq!(index.tile(), tile(0, 0, 8, 8));
    assert_eq!(index.pixel_regions((3, 3)), [0_u32, 1].as_slice());
    assert_eq!(index.pixel_regions((1, 1)), [0_u32].as_slice());
    assert_eq!(index.pixel_regions((5, 5)), [1_u32].as_slice());
    assert_eq!(index.pixel_regions((7, 7)), [0_u32; 0].as_slice());
    assert_eq!(index.pixel_regions((9, 9)), [0_u32; 0].as_slice());
    assert!(!KeptIndex::from_leaves(tile(0, 0, 8, 8), &[]).kept(
        DVec2::new(3.0, 3.0),
        pack_sig([1, 1]),
        480.0,
        48
    ));
}

#[test]
fn pack_kept_lays_out_the_regions_and_the_per_pixel_csr() {
    let sig = [7, 9];
    let left = KeptIndex::from_leaves(
        tile(0, 0, 4, 4),
        &[leaf_over(&square(1.0, 3.0), sig, 8, 15)],
    );
    // The right region lies inside its tile. A polygon outside the tile's rows is listed for
    // none of its pixels.
    let right_corners = [
        DVec2::new(5.0, 1.0),
        DVec2::new(7.0, 1.0),
        DVec2::new(7.0, 3.0),
        DVec2::new(5.0, 3.0),
    ];
    let right = KeptIndex::from_leaves(tile(4, 0, 8, 4), &[leaf_over(&right_corners, sig, 0, 47)]);
    let packed = pack_kept(&[(tile(0, 0, 4, 4), left), (tile(4, 0, 8, 4), right)]);
    assert_eq!(packed.regions.len(), 2);
    assert_eq!(packed.origin, (0, 0));
    assert_eq!(packed.size, (8, 4));
    assert_eq!(packed.pixel_offsets.len(), 8 * 4 + 1);
    // Each region overlaps the pixels 1..=3 of three rows.
    assert_eq!(packed.indices.len(), 18);
    assert_eq!(packed.pixel_offsets.last().copied(), Some(18));
    let list = |x: usize, y: usize| {
        let slot = y * 8 + x;
        let (start, end) = (
            packed.pixel_offsets[slot] as usize,
            packed.pixel_offsets[slot + 1] as usize,
        );
        packed.indices[start..end].to_vec()
    };
    assert_eq!(list(1, 1), vec![0]);
    assert_eq!(list(5, 1), vec![1]);
    assert_eq!(list(0, 0), Vec::<u32>::new());
    let region = &packed.regions[0];
    assert_eq!((region.sig, region.len), (sig, 4));
    assert_eq!(
        (region.first_node, region.last_node, region.flags),
        (8, 15, 0)
    );
    assert!(region.polygon[..4].contains(&[1.0, 1.0]));
    assert!(region.polygon[..4].contains(&[3.0, 3.0]));
    assert_eq!(packed.regions[1].last_node, 47);
}

#[test]
fn an_empty_batch_packs_to_an_empty_csr() {
    let packed = pack_kept(&[]);
    assert_eq!(packed.regions.len(), 0);
    assert_eq!(packed.size, (0, 0));
    assert_eq!(packed.pixel_offsets, vec![0]);
    assert_eq!(packed.indices.len(), 0);
}
