//! Tests of the per-node GPU fragment path.
//!
//! What runs everywhere: naga on the assembled kernels, the WGSL rules of the fragment units
//! (no function-scope arrays, every function-scope `var` initialised), the packed per-node data
//! (`node_count` 1 to 48, the f16 weights and deltas), the rebasing of a sub-batch, and the pure
//! half of the bin rule. What needs an adapter (skipped cleanly without one;
//! `PLEOCHROMA_REQUIRE_GPU=1` turns the skip into a failure): GPU against the CPU fragment twin on
//! dispersive leaves under three lightings, the bit-identity of two runs, the effect of the deltas,
//! and the bin rule of `kept.wgsl` against `SpectralNode::of_wavelength` on every bin edge ± 1 ULP.
//!
//! The twin tolerances `FRAGMENT_TWIN_REL` and `GLINT_TWIN_REL` cover the partition difference:
//! the CPU bisects the longest edge of a polygon, the GPU splits an n-gon into a centroid fan of
//! quads and then quads into four, so the pieces that reach the 7-point rule differ. Both
//! partitions are exact. The tests print the worst errors; a glint error above `GLINT_TWIN_REL`
//! means `bm_quad_of` should bisect the longest edge like the CPU.

use std::collections::BTreeMap;

use bytemuck::{Pod, Zeroable};
use glam::{DVec2, DVec3, Vec3};
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::{
    buffers::{FragmentPlan, LeafBufferSet, split_into_batches, sub_batch},
    fragments::{
        FRAGMENTS_ENTRY, FRAGMENTS_WGSL, FragmentOptions, REFINE_ENTRY, REFINE_WGSL,
        dispatch_fragments_with, fragment_kernel_source, refine_kernel_source,
    },
    kept::kept_wgsl,
    reference_source::BRANCH_SIG_SRC,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, LightingPreset, color::integrate_nodes_to_xyz},
        studio_rig::StudioRig,
    },
    renderer::{
        beam::{
            beam::{
                BranchPrefix, LEAF_FLAG_DELTAS, Leaf, PackedLeaves, SpectralInterval, SpectralNode,
            },
            geom::polygon::ConvexPolygon,
            leaf::pack_leaves,
            lighting::{FragmentInput, FragmentWork, LightingScene, integrate_fragment_counted},
        },
        gpu::{GpuContext, compute},
        gpu_backend::GpuSceneRef,
        path_cache::record::{f16_bits_to_f32, f32_to_f16_bits},
    },
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// Relative GPU-vs-CPU tolerance of a fragment's XYZ away from glints (the same value and meaning
/// as `FRAGMENT_TWIN_REL` of `tests_fragments`, which is private to that module).
const FRAGMENT_TWIN_REL: f32 = 2e-3;

/// Relative tolerance on sun-disc pixels (the same as `GLINT_TWIN_REL` of `tests_fragments`).
const GLINT_TWIN_REL: f32 = 5e-2;

/// Frame edge in pixels of the twin tests.
const FRAME: u32 = 24;

/// The exit-direction gradient of the test leaf as a fraction of the mirrored camera field's
/// angle per pixel (`4 * fov_tan / FRAME`), so the leaf follows the camera's field of view.
const GRADIENT_RATIO: f64 = 0.2;

/// Gradient ratio of the delta test. The sun disc (angular radius 4.7e-3 rad) must have an edge
/// inside the leaf for a per-node shift to change a pixel, so the field climbs one camera pixel of
/// angle per leaf pixel; the end nodes then shift by one pixel of the disc edge.
const DELTA_GRADIENT_RATIO: f64 = 1.0;

/// The dispersion of the test leaf per node step from the hero, as a fraction of the gradient.
const SPREAD_RATIO: f64 = 0.25;

/// The subdivision depth cap of both twins (a steep fixture still terminates).
const TWIN_MAX_LEVELS: u32 = 3;

/// The narrow twin camera: 1.5 degrees of field of view over 24 pixels is 0.0011 rad per pixel
/// (0.0022 after the mirror), the angular scale `tau_L` = 1e-3 is designed for.
fn twin_camera() -> Camera {
    Camera::new(0.6, 0.45, 3.0, 1.5)
}

/// Above this Y a pixel counts as a sun glint (the tolerance of the disc edge applies).
const GLINT_Y: f32 = 50.0;

/// The fraction of the peak CPU `Y` below which a pixel is compared on the peak's scale. The GPU's
/// f32 geometry (vertex positions, the cell clipping, the sun's sector sum) carries an absolute
/// error of a few 1e-6 of the brightest pixel, so a flat floor would ask for more dynamic range
/// than f32 gives. A pixel above the floor is held to `FRAGMENT_TWIN_REL` of its own value (as
/// `TWIN_FLOOR_REL` in `tests_fragments`).
const TWIN_FLOOR_REL: f32 = 1e-2;

/// The denominator of a pixel's relative error: its own CPU value, or the scene floor below it.
fn error_scale(expected: Vec3, peak: f32) -> f32 {
    expected
        .max_element()
        .max((TWIN_FLOOR_REL * peak).max(1e-6))
}

/// Relative change of some pixel that proves the GPU read the per-node deltas.
const DELTA_EFFECT_REL: f32 = 1e-2;

/// One spectral interval of the twin tests.
struct Case {
    label: &'static str,
    n_lambda: u32,
    first: u16,
    nodes: usize,
    /// Whether the daylight-sun lighting is part of the case.
    sun: bool,
}

impl Case {
    fn last(&self) -> u16 {
        self.first + u16::try_from(self.nodes).expect("few nodes") - 1
    }

    fn interval(&self) -> SpectralInterval {
        SpectralInterval {
            first: self.first,
            last: self.last(),
        }
    }

    fn hero(&self) -> usize {
        let interval = self.interval();
        usize::from(interval.centre() - interval.first)
    }
}

/// Eight nodes of eight, a single node, a partial group, and the whole-band 48-node interval.
const CASES: [Case; 5] = [
    Case {
        label: "8 nodes of 8",
        n_lambda: 8,
        first: 0,
        nodes: 8,
        sun: true,
    },
    Case {
        label: "single node 5 of 8",
        n_lambda: 8,
        first: 5,
        nodes: 1,
        sun: true,
    },
    Case {
        label: "8 nodes of 48 from 20",
        n_lambda: 48,
        first: 20,
        nodes: 8,
        sun: true,
    },
    Case {
        label: "5 nodes of 24 from 7",
        n_lambda: 24,
        first: 7,
        nodes: 5,
        sun: true,
    },
    Case {
        label: "48 nodes of 48",
        n_lambda: 48,
        first: 0,
        nodes: 48,
        sun: false,
    },
];

/// Parses and validates a source; panics with the diagnostics otherwise.
fn validate(source: &str, what: &str) {
    let module = wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{what} does not parse: {}", e.emit_to_string(source)));
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{what} does not validate: {}", e.emit_to_string(source)));
}

fn context_or_skip(name: &str) -> Option<GpuContext> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(ctx),
        Err(error) => {
            assert!(
                !std::env::var(REQUIRE_GPU_ENV).is_ok_and(|v| v == "1"),
                "{name}: {REQUIRE_GPU_ENV}=1 but no usable GPU adapter: {error}"
            );
            println!("skipping {name}: no GPU adapter");
            None
        }
    }
}

// ---- static checks of the sources ----

#[test]
fn the_per_node_kernels_are_naga_clean() {
    let fragments = fragment_kernel_source().expect("every anchor occurs exactly once");
    let refine = refine_kernel_source().expect("every anchor occurs exactly once");
    validate(&fragments, "the fragment kernel");
    validate(&refine, "the refine kernel");
    assert!(fragments.contains(FRAGMENTS_ENTRY));
    assert!(refine.contains(REFINE_ENTRY));
}

#[test]
fn the_fragment_units_have_no_function_scope_arrays_and_initialise_every_var() {
    for (name, source) in [("fragments", FRAGMENTS_WGSL), ("refine", REFINE_WGSL)] {
        for (number, line) in source.lines().enumerate() {
            let line = line.trim_start();
            // `var<private>` / `var<workgroup>` start with `var<`: only function-scope `var ` here.
            if !line.starts_with("var ") {
                continue;
            }
            assert!(
                !line.contains("array<"),
                "{name}.wgsl:{}: a function-scope array (AMD/FXC crash)",
                number + 1
            );
            assert!(
                line.contains(" = "),
                "{name}.wgsl:{}: a function-scope var without an initialiser",
                number + 1
            );
        }
    }
}

// ---- the packed per-node data ----

/// The direction of vertex `v`, node `n` of the plain leaves.
fn plain_dir(v: usize, n: usize) -> Vec3 {
    Vec3::new(
        0.01 * n as f32,
        0.2_f32.mul_add(v as f32, -0.3),
        0.001f32.mul_add(n as f32, -1.0),
    )
}

/// The weight of vertex `v`, node `n` of the plain leaves.
fn plain_weight(v: usize, n: usize) -> f32 {
    0.01_f32.mul_add(v as f32, 0.001_f32.mul_add(n as f32, 0.05))
}

/// A quadrilateral leaf at horizontal offset `shift` with `nodes` nodes from spectral node `first`,
/// whose directions differ between nodes (so a leaf of two or more nodes is dispersive).
fn plain_leaf(first: u16, nodes: usize, shift: f64) -> Leaf {
    let corners = [
        DVec2::new(shift, 0.0),
        DVec2::new(shift + 4.0, 0.2),
        DVec2::new(shift + 4.1, 4.0),
        DVec2::new(shift + 0.1, 3.8),
    ];
    let image = ConvexPolygon::from_image_points(&corners).expect("a convex leaf");
    let mut dirs = Vec::new();
    let mut weights = Vec::new();
    for v in 0..image.len() {
        for n in 0..nodes {
            dirs.push(plain_dir(v, n));
            weights.push(plain_weight(v, n));
        }
    }
    Leaf {
        tile: 0,
        image,
        dirs,
        weights,
        spectrum: SpectralInterval {
            first,
            last: first + u16::try_from(nodes).expect("few nodes") - 1,
        },
        sig: [0, 0],
        prefix: BranchPrefix::EMPTY,
        glare: false,
        reflection: false,
        tau_omega: 1e-4,
        pre_clip: None,
    }
}

/// The f16 round trip of a value.
fn round_f16(value: f32) -> f32 {
    f16_bits_to_f32(f32_to_f16_bits(value))
}

#[test]
fn the_packed_nodes_round_trip_for_one_to_forty_eight_nodes() {
    for nodes in [1_usize, 2, 3, 7, 8, 9, 16, 47, 48] {
        let first = u16::try_from((48 - nodes) / 2).expect("small");
        let leaf = plain_leaf(first, nodes, 3.0);
        let packed = pack_leaves(&[(0, vec![leaf.clone()])]);
        assert_eq!(packed.headers.len(), 1, "{nodes} nodes");
        let header = packed.headers[0];
        let count = leaf.image.len();
        assert_eq!(header.node_count as usize, nodes);
        assert_eq!(header.first_node, u32::from(first));
        assert_eq!(header.vertex_count as usize, count);
        assert_eq!(packed.nodes.len(), count * nodes);
        assert_eq!(
            header.flags & LEAF_FLAG_DELTAS != 0,
            nodes > 1,
            "{nodes} nodes: the deltas flag"
        );
        for (v, vertex) in packed.vertices.iter().enumerate() {
            assert_eq!(vertex.first_node as usize, v * nodes);
            for n in 0..nodes {
                let word = packed.nodes[vertex.first_node as usize + n];
                let weight = f16_bits_to_f32((word.weight_dx & 0xFFFF) as u16);
                assert_eq!(weight, round_f16(leaf.weights[v * nodes + n]));
                let delta = Vec3::new(
                    f16_bits_to_f32((word.weight_dx >> 16) as u16),
                    f16_bits_to_f32((word.dy_dz & 0xFFFF) as u16),
                    f16_bits_to_f32((word.dy_dz >> 16) as u16),
                );
                let dir = Vec3::from_array(vertex.hero_dir) + delta;
                let want = leaf.dirs[v * nodes + n];
                assert!(
                    (dir - want).abs().max_element() <= 1e-3,
                    "{nodes} nodes, vertex {v}, node {n}: {dir} against {want}"
                );
            }
        }
        // A one-leaf batch is its own sub-batch, and the plan covers the leaf.
        assert_eq!(sub_batch(&packed, 0..1), Some(packed.clone()));
        let plan = FragmentPlan::from_leaves(&packed, (64, 64));
        assert_eq!(plan.first_fragment.len(), 2);
        assert!(plan.total > 0);
    }
}

/// The `(first_node, node_count)` of every header of `batches`, in order.
fn spectral_ranges<'a>(batches: impl IntoIterator<Item = &'a PackedLeaves>) -> Vec<(u32, u32)> {
    batches
        .into_iter()
        .flat_map(|batch| batch.headers.iter().map(|h| (h.first_node, h.node_count)))
        .collect()
}

#[test]
fn a_sub_batch_keeps_the_spectral_first_node_and_the_node_blocks() {
    // Three tiles with 1, 8 and 48 nodes starting at different spectral nodes.
    let leaves = vec![
        (0, vec![plain_leaf(7, 1, 0.0)]),
        (1, vec![plain_leaf(3, 8, 6.0)]),
        (2, vec![plain_leaf(0, 48, 12.0)]),
    ];
    let packed = pack_leaves(&leaves);
    let plan = FragmentPlan::from_leaves(&packed, (64, 64));
    let ranges = split_into_batches(&packed, &plan, 1600).expect("every tile fits alone");
    assert!(ranges.len() >= 2, "the limit forces a split: {ranges:?}");
    assert_eq!(ranges[0].start, 0);
    assert_eq!(ranges.last().map(|r| r.end), Some(3));
    for pair in ranges.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
    }
    let subs: Vec<PackedLeaves> = ranges
        .iter()
        .map(|range| sub_batch(&packed, range.clone()).expect("well-formed"))
        .collect();
    assert_eq!(
        spectral_ranges(&subs),
        spectral_ranges([&packed]),
        "the spectral first nodes and node counts survive the split"
    );
    // The node blocks survive: every vertex of every sub-batch points at the same node words.
    let mut original = packed.vertices.iter();
    for (sub, range) in subs.iter().zip(&ranges) {
        let mut vertices = sub.vertices.iter();
        for header in &sub.headers {
            for _ in 0..header.vertex_count {
                let moved = vertices.next().expect("vertex");
                let was = original.next().expect("vertex");
                assert_eq!(moved.image, was.image);
                assert_eq!(moved.hero_dir, was.hero_dir);
                let (a, b) = (moved.first_node as usize, was.first_node as usize);
                let count = header.node_count as usize;
                assert_eq!(
                    sub.nodes[a..a + count],
                    packed.nodes[b..b + count],
                    "tiles {range:?}"
                );
            }
        }
    }
}

// ---- the dispersive twin ----

/// The direction field of the test leaf: a tilted field about `target` (the hero node), and per
/// node a rotation along `e1` proportional to the node's distance from the hero.
struct Dispersive {
    target: DVec3,
    e1: DVec3,
    e2: DVec3,
    /// Radians of exit direction per pixel.
    gradient: f64,
    /// Radians of exit direction per node step from the hero.
    spread: f64,
}

impl Dispersive {
    /// The twin field about `target`, scaled from `camera.fov_tan` by [`GRADIENT_RATIO`].
    fn towards(target: Vec3, camera: &Camera) -> Self {
        Self::scaled(target, camera, GRADIENT_RATIO)
    }

    /// The field about `target` with `gradient_ratio` of the camera's angle per pixel.
    fn scaled(target: Vec3, camera: &Camera, gradient_ratio: f64) -> Self {
        let gradient = gradient_ratio * 4.0 * f64::from(camera.fov_tan) / f64::from(FRAME);
        let spread = SPREAD_RATIO * gradient;
        let target = target.as_dvec3().normalize();
        let helper = if target.x.abs() > 0.9 {
            DVec3::Y
        } else {
            DVec3::X
        };
        let e1 = target.cross(helper).normalize();
        let e2 = target.cross(e1);
        Self {
            target,
            e1,
            e2,
            gradient,
            spread,
        }
    }

    fn hero(&self, p: DVec2) -> DVec3 {
        (self.target
            + self.e1 * (self.gradient * (p.x - 12.0))
            + self.e2 * (self.gradient * (p.y - 12.0)))
            .normalize()
    }

    fn at_node(&self, p: DVec2, steps: f64) -> DVec3 {
        (self.hero(p) + self.e1 * (self.spread * steps)).normalize()
    }
}

/// The weight of node `node` at `p`: a Fresnel-like level with a node slope and a spatial ramp,
/// rounded to f16 like the packed weight.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the weight is deliberately rounded from f64 to the f32 the f16 packing starts from"
)]
fn weight(base: f64, slope: f64, node: usize, p: DVec2) -> f32 {
    let w = slope.mul_add(node as f64, base) + 0.01 * (p.x - 12.0) / 8.0;
    round_f16(w as f32)
}

/// The dispersive CPU leaf of `case` and the same leaf in the GPU layout (through `pack_leaves`).
/// Directions are the hero direction plus an f16-rounded delta, so both sides see the same field.
fn build_leaf(field: &Dispersive, case: &Case, base: f64, slope: f64) -> (Leaf, PackedLeaves) {
    let corners = [
        DVec2::new(7.3, 7.9),
        DVec2::new(16.6, 7.2),
        DVec2::new(16.1, 16.4),
        DVec2::new(8.0, 15.8),
    ];
    let (leaf, packed) = build_leaf_on(field, case, base, slope, &corners);
    assert_eq!(packed.headers.len(), 1, "{}", case.label);
    assert_eq!(packed.headers[0].node_count as usize, case.nodes);
    assert_eq!(packed.headers[0].first_node, u32::from(case.first));
    assert_eq!(
        packed.headers[0].flags & LEAF_FLAG_DELTAS != 0,
        case.nodes > 1,
        "{}: the deltas flag",
        case.label
    );
    (leaf, packed)
}

/// A convex ten-vertex leaf (an elongated decagon about pixel `(12, 12)`), which `pack_leaves`
/// pre-splits into pieces of at most 8 vertices.
fn decagon_corners() -> Vec<DVec2> {
    (0..10)
        .map(|k| {
            let angle = 0.13 + std::f64::consts::TAU * f64::from(k) / 10.0;
            DVec2::new(
                angle.cos().mul_add(5.6, 12.0),
                angle.sin().mul_add(4.8, 12.0),
            )
        })
        .collect()
}

/// [`build_leaf`] on arbitrary `corners`: the CPU leaf and its packing (any number of pieces).
fn build_leaf_on(
    field: &Dispersive,
    case: &Case,
    base: f64,
    slope: f64,
    corners: &[DVec2],
) -> (Leaf, PackedLeaves) {
    let image = ConvexPolygon::from_image_points(corners).expect("a convex leaf");
    let (points, count) = image.points_2d();
    let hero = case.hero();
    let mut dirs = Vec::new();
    let mut weights = Vec::new();
    for &p in &points[..count] {
        let hero_dir = field.hero(p).as_vec3();
        for node in 0..case.nodes {
            let dir = if node == hero {
                hero_dir
            } else {
                let exact = field.at_node(p, node as f64 - hero as f64).as_vec3();
                hero_dir + Vec3::from_array((exact - hero_dir).to_array().map(round_f16))
            };
            dirs.push(dir);
            weights.push(weight(base, slope, node, p));
        }
    }
    let leaf = Leaf {
        tile: 0,
        image,
        dirs,
        weights,
        spectrum: case.interval(),
        sig: [0, 0],
        prefix: BranchPrefix::EMPTY,
        glare: false,
        reflection: false,
        tau_omega: 1e-4,
        pre_clip: None,
    };
    let packed = pack_leaves(&[(0, vec![leaf.clone()])]);
    (leaf, packed)
}

/// The leaf polygon clipped to the half-open cell of pixel `(x, y)`.
fn clip_cell(poly: &[DVec2], x: f64, y: f64) -> Vec<DVec2> {
    let mut out = poly.to_vec();
    for (axis, bound, keep_ge) in [
        (0, x - 0.5, true),
        (0, x + 0.5, false),
        (1, y - 0.5, true),
        (1, y + 0.5, false),
    ] {
        let value = |p: &DVec2| if axis == 0 { p.x } else { p.y };
        let inside = |p: &DVec2| {
            if keep_ge {
                value(p) >= bound
            } else {
                value(p) < bound
            }
        };
        let input = std::mem::take(&mut out);
        for (i, cur) in input.iter().enumerate() {
            let prev = input[(i + input.len() - 1) % input.len()];
            if inside(cur) != inside(&prev) {
                let t = (bound - value(&prev)) / (value(cur) - value(&prev));
                out.push(prev + (*cur - prev) * t);
            }
            if inside(cur) {
                out.push(*cur);
            }
        }
        if out.len() < 3 {
            return Vec::new();
        }
    }
    out
}

/// One GPU run of the packed leaf; the slots and the plan.
fn run_gpu(
    ctx: &GpuContext,
    camera: &Camera,
    environment: EnvironmentSource<'_>,
    packed: &PackedLeaves,
    n_lambda: u32,
) -> (Vec<[f32; 4]>, FragmentPlan) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = GpuSceneRef {
        camera,
        width: FRAME,
        height: FRAME,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment,
        estimator: None,
    };
    let set = LeafBufferSet::upload(ctx, packed).expect("upload");
    let plan = FragmentPlan::from_leaves(packed, (FRAME, FRAME));
    // The same depth cap as the CPU twin's `LightingScene`: the GPU and the CPU refine to
    // the same level, and a steep fixture still finishes.
    let options = FragmentOptions {
        max_levels: TWIN_MAX_LEVELS,
        n_lambda,
        tolerance: 1e-3,
    };
    let slots = dispatch_fragments_with(ctx, &scene, &set, &plan, &options)
        .expect("the fragment kernels run");
    assert_eq!(slots.count, plan.total);
    let words =
        compute::readback::<[f32; 4]>(&ctx.device, &ctx.queue, &slots.slots, plan.total as usize);
    (words, plan)
}

/// The CPU twin of one fragment: the integral over the interval's groups in XYZ, with the work
/// of the integration (the deepest level over the groups, the counts summed).
fn cpu_fragment(
    scene: &LightingScene<'_>,
    leaf: &Leaf,
    polygon: &ConvexPolygon,
    case: &Case,
) -> (Vec3, FragmentWork) {
    let mut xyz = Vec3::ZERO;
    let mut work = FragmentWork::default();
    for group in 0..leaf.spectrum.groups() {
        let (row, group_work) = integrate_fragment_counted(
            scene,
            &FragmentInput {
                leaf,
                polygon,
                group,
            },
        );
        work.evaluations += group_work.evaluations;
        work.deepest = work.deepest.max(group_work.deepest);
        work.centroid_pieces += group_work.centroid_pieces;
        work.rule_pieces += group_work.rule_pieces;
        let first = u32::from(case.first) + u32::try_from(group * 8).expect("few groups");
        xyz += integrate_nodes_to_xyz(&row, first, case.n_lambda);
    }
    (xyz, work)
}

/// The worst relative errors of a GPU run against the CPU twin, with the CPU twin's work.
struct Errors {
    worst: f32,
    worst_glint: f32,
    peak: f32,
    /// The deepest subdivision level the CPU twin reached on any pixel.
    deepest: u32,
    /// The CPU twin's work on the worst non-glint pixel.
    worst_work: FragmentWork,
    /// The worst non-glint pixel: `(x, y, CPU xyz, GPU xyz)`.
    worst_pixel: Option<(u32, u32, Vec3, Vec3)>,
}

/// Compares every pixel the leaf's pieces cover: a leaf of more than eight vertices reaches the
/// GPU as several pieces, each with its own rectangle and slots, so a pixel's GPU value is the sum
/// of its pieces' slots. The CPU twin integrates the whole leaf over the pixel cell.
fn compare(
    scene: &LightingScene<'_>,
    leaf: &Leaf,
    case: &Case,
    gpu: &[[f32; 4]],
    plan: &FragmentPlan,
) -> Errors {
    let mut sums: BTreeMap<(u32, u32), Vec3> = BTreeMap::new();
    for (piece, rect) in plan.pixel_rects.iter().enumerate() {
        let width = rect[2] - rect[0];
        let first = plan.first_fragment[piece];
        for py in rect[1]..rect[3] {
            for px in rect[0]..rect[2] {
                let slot = first + (py - rect[1]) * width + (px - rect[0]);
                *sums.entry((px, py)).or_insert(Vec3::ZERO) +=
                    Vec3::from_slice(&gpu[slot as usize][..3]);
            }
        }
    }
    let corners: Vec<DVec2> = leaf
        .image
        .vertices()
        .iter()
        .map(|v| DVec2::new(v.x, v.y))
        .collect();
    let mut pairs = Vec::new();
    for (&(px, py), &got) in &sums {
        let cell = clip_cell(&corners, f64::from(px), f64::from(py));
        let Some(polygon) = ConvexPolygon::from_image_points(&cell) else {
            continue;
        };
        let (expected, work) = cpu_fragment(scene, leaf, &polygon, case);
        pairs.push((px, py, expected, got, work));
    }
    let peak = pairs.iter().map(|p| p.2.y).fold(0.0, f32::max);
    let mut errors = Errors {
        worst: 0.0,
        worst_glint: 0.0,
        peak,
        deepest: 0,
        worst_work: FragmentWork::default(),
        worst_pixel: None,
    };
    for (px, py, expected, got, work) in pairs {
        errors.deepest = errors.deepest.max(work.deepest);
        let rel = (got - expected).abs().max_element() / error_scale(expected, peak);
        if expected.y > GLINT_Y {
            errors.worst_glint = errors.worst_glint.max(rel);
        } else if rel >= errors.worst {
            errors.worst = rel;
            errors.worst_work = work;
            errors.worst_pixel = Some((px, py, expected, got));
        }
    }
    errors
}

#[test]
fn gpu_fragments_equal_the_cpu_twin_on_dispersive_leaves() {
    let name = "gpu_fragments_equal_the_cpu_twin_on_dispersive_leaves";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let camera = twin_camera();
    let (yaw, pitch) = (0.4, 0.6);
    let rig = StudioRig::new(yaw, pitch);
    // (label, preset, the direction the leaf's hero field is centred on, sun)
    let lightings = [
        (
            "studio rings",
            LightingPreset::RingLights,
            Vec3::new(0.3, 0.8, 0.2).normalize(),
            false,
        ),
        (
            "light tent",
            LightingPreset::LightTent,
            Vec3::new(0.3, 0.8, 0.2).normalize(),
            false,
        ),
        (
            "daylight sun",
            LightingPreset::DaylightSun,
            rig.key_dir,
            true,
        ),
    ];
    // (label, base weight, node slope): a glass-like and a diamond-like Fresnel level.
    let stones = [("glass", 0.04, 0.0004), ("diamond", 0.17, 0.0012)];
    for case in &CASES {
        for (light_label, preset, target, sun) in &lightings {
            if *sun && !case.sun {
                continue;
            }
            let environment = preset.studio(1.0, yaw, pitch);
            let field = Dispersive::towards(*target, &camera);
            for (stone_label, base, slope) in stones {
                let label = format!("{} / {light_label} / {stone_label}", case.label);
                let (leaf, packed) = build_leaf(&field, case, base, slope);
                let scene = LightingScene {
                    environment,
                    rig: Some(StudioRig::new(yaw, pitch)),
                    camera: &camera,
                    size: (FRAME, FRAME),
                    tolerance: 1e-3,
                    max_levels: TWIN_MAX_LEVELS,
                    n_lambda: case.n_lambda,
                };
                let (gpu, plan) = run_gpu(&ctx, &camera, environment, &packed, case.n_lambda);
                let (again, _) = run_gpu(&ctx, &camera, environment, &packed, case.n_lambda);
                assert_eq!(gpu, again, "{label}: two runs differ");
                let errors = compare(&scene, &leaf, case, &gpu, &plan);
                println!(
                    "{label}: worst rel {:.3e} (FRAGMENT_TWIN_REL {FRAGMENT_TWIN_REL}), worst glint rel {:.3e} (GLINT_TWIN_REL {GLINT_TWIN_REL}), peak Y {}, deepest level {}, work of the worst non-glint pixel {:?}, worst pixel {:?}",
                    errors.worst,
                    errors.worst_glint,
                    errors.peak,
                    errors.deepest,
                    errors.worst_work,
                    errors.worst_pixel
                );
                assert!(errors.peak > 0.0, "{label}: the leaf is dark");
                assert!(
                    errors.worst <= FRAGMENT_TWIN_REL,
                    "{label}: {}",
                    errors.worst
                );
                assert!(
                    errors.worst_glint <= GLINT_TWIN_REL,
                    "{label}: glint {}",
                    errors.worst_glint
                );
            }
        }
    }
}

/// A refracted leaf of ten vertices reaches the GPU as pre-split pieces whose Wachspress
/// interpolation runs over the piece, while the CPU interpolates over the whole leaf (guide §14.7
/// item 1). Exact for an affine field; a refracted field is not, so the twin error here is the
/// measure of the pre-split. The error is printed; the tolerances are those of the other twins.
#[test]
fn gpu_pieces_equal_the_cpu_twin_on_a_ten_vertex_refracted_leaf() {
    let name = "gpu_pieces_equal_the_cpu_twin_on_a_ten_vertex_refracted_leaf";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let camera = twin_camera();
    let (yaw, pitch) = (0.4, 0.6);
    let rig = StudioRig::new(yaw, pitch);
    // (label, preset, the direction the leaf's hero field is centred on)
    let lightings = [
        (
            "light tent",
            LightingPreset::LightTent,
            Vec3::new(0.3, 0.8, 0.2).normalize(),
        ),
        ("daylight sun", LightingPreset::DaylightSun, rig.key_dir),
    ];
    let case = &CASES[0];
    for (light_label, preset, target) in lightings {
        let label = format!("10 vertices / {light_label}");
        let environment = preset.studio(1.0, yaw, pitch);
        let field = Dispersive::towards(target, &camera);
        let (leaf, packed) = build_leaf_on(&field, case, 0.17, 0.0012, &decagon_corners());
        assert_eq!(leaf.image.len(), 10, "{label}: the decagon survives");
        assert!(
            packed.headers.len() >= 2,
            "{label}: a ten-vertex leaf is pre-split into pieces"
        );
        let scene = LightingScene {
            environment,
            rig: Some(StudioRig::new(yaw, pitch)),
            camera: &camera,
            size: (FRAME, FRAME),
            tolerance: 1e-3,
            max_levels: TWIN_MAX_LEVELS,
            n_lambda: case.n_lambda,
        };
        let (gpu, plan) = run_gpu(&ctx, &camera, environment, &packed, case.n_lambda);
        let errors = compare(&scene, &leaf, case, &gpu, &plan);
        println!(
            "{label}: {} pieces, worst rel {:.3e} (FRAGMENT_TWIN_REL {FRAGMENT_TWIN_REL}), worst glint rel {:.3e} (GLINT_TWIN_REL {GLINT_TWIN_REL}), peak Y {}, deepest level {}, worst pixel {:?}",
            packed.headers.len(),
            errors.worst,
            errors.worst_glint,
            errors.peak,
            errors.deepest,
            errors.worst_pixel
        );
        assert!(errors.peak > 0.0, "{label}: the leaf is dark");
        assert!(
            errors.worst <= FRAGMENT_TWIN_REL,
            "{label}: {}",
            errors.worst
        );
        assert!(
            errors.worst_glint <= GLINT_TWIN_REL,
            "{label}: glint {}",
            errors.worst_glint
        );
    }
}

#[test]
fn the_gpu_reads_the_per_node_deltas() {
    let name = "the_gpu_reads_the_per_node_deltas";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let camera = twin_camera();
    let (yaw, pitch) = (0.4, 0.6);
    let rig = StudioRig::new(yaw, pitch);
    let environment = LightingPreset::DaylightSun.studio(1.0, yaw, pitch);
    let field = Dispersive::scaled(rig.key_dir, &camera, DELTA_GRADIENT_RATIO);
    let case = &CASES[0];
    let (_, packed) = build_leaf(&field, case, 0.17, 0.0012);
    assert_ne!(packed.headers[0].flags & LEAF_FLAG_DELTAS, 0);
    // The same leaf with every delta cleared: all nodes then share the hero direction.
    let mut flat = packed.clone();
    flat.headers[0].flags &= !LEAF_FLAG_DELTAS;
    for node in &mut flat.nodes {
        node.weight_dx &= 0xFFFF;
        node.dy_dz = 0;
    }
    let (with, _) = run_gpu(&ctx, &camera, environment, &packed, case.n_lambda);
    let (without, _) = run_gpu(&ctx, &camera, environment, &flat, case.n_lambda);
    assert_eq!(with.len(), without.len());
    let mut largest = 0.0_f32;
    for (a, b) in with.iter().zip(&without) {
        let (a, b) = (Vec3::from_slice(&a[..3]), Vec3::from_slice(&b[..3]));
        largest = largest.max((a - b).abs().max_element() / b.max_element().max(1e-6));
    }
    println!("largest relative effect of the deltas: {largest:.3e} (need > {DELTA_EFFECT_REL})");
    assert!(largest > DELTA_EFFECT_REL, "the deltas change nothing");
}

// ---- the bin rule of `kept.wgsl` against `SpectralNode::of_wavelength` ----

/// The node counts of the cross-check.
const BIN_COUNTS: [u32; 3] = [8, 48, 96];

/// The probe kernel's own bindings (inside the beam range, after the kept ones).
const BIN_BINDING_IN: u32 = 214;
const BIN_BINDING_OUT: u32 = 215;

/// The entry point of [`BIN_KERNEL`].
const BIN_ENTRY: &str = "bin_probe_main";

/// A tiny kernel that runs the bin rule of `kept.wgsl` on a list of probes.
const BIN_KERNEL: &str = "
struct BinProbe {
    lambda: f32,
    n_lambda: u32,
}

@group(0) @binding(214) var<storage, read> bin_probes: array<BinProbe>;
@group(0) @binding(215) var<storage, read_write> bin_nodes: array<u32>;

@compute @workgroup_size(64)
fn bin_probe_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&bin_probes)) {
        return;
    }
    let p = bin_probes[i];
    bin_nodes[i] = kept_node_of(p.lambda, p.n_lambda);
}
";

/// One probe in the kernel's layout, 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BinProbe {
    lambda: f32,
    n_lambda: u32,
}

/// Every bin edge `380 + k · 400 / n` of every node count, each at the edge and ± 1 and 2 ULP, the
/// node centres, and wavelengths outside the band.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the bin edge is deliberately rounded to the nearest f32 before the ULP probes"
)]
fn bin_probes() -> Vec<BinProbe> {
    let mut probes = Vec::new();
    for n in BIN_COUNTS {
        for k in 0..=n {
            let edge = (380.0_f64 + f64::from(k) * 400.0 / f64::from(n)) as f32;
            for ulps in [-2_i32, -1, 0, 1, 2] {
                probes.push(BinProbe {
                    lambda: f32::from_bits(edge.to_bits().wrapping_add_signed(ulps)),
                    n_lambda: n,
                });
            }
        }
        for node in 0..n {
            let centre = SpectralNode(u16::try_from(node).expect("few nodes")).centre_nm(n);
            probes.push(BinProbe {
                lambda: centre,
                n_lambda: n,
            });
        }
        for lambda in [0.0_f32, 100.0, 379.999, 780.001, 1000.0, 1.0e6] {
            probes.push(BinProbe {
                lambda,
                n_lambda: n,
            });
        }
    }
    probes
}

#[test]
fn the_cpu_bin_rule_is_monotone_clamped_and_maps_centres_to_their_node() {
    let mut probes = bin_probes();
    probes.sort_by(|a, b| {
        a.n_lambda
            .cmp(&b.n_lambda)
            .then(a.lambda.total_cmp(&b.lambda))
    });
    let mut previous: Option<(u32, u16)> = None;
    for probe in &probes {
        let node = SpectralNode::of_wavelength(probe.lambda, probe.n_lambda);
        assert!(u32::from(node.0) < probe.n_lambda, "{}", probe.lambda);
        if probe.lambda < 380.0 {
            assert_eq!(node.0, 0, "below the band: {}", probe.lambda);
        }
        if probe.lambda >= 780.0 {
            assert_eq!(
                u32::from(node.0),
                probe.n_lambda - 1,
                "above the band: {}",
                probe.lambda
            );
        }
        if let Some((n, before)) = previous
            && n == probe.n_lambda
        {
            assert!(node.0 >= before, "not monotone at {}", probe.lambda);
        }
        previous = Some((probe.n_lambda, node.0));
    }
    for n in BIN_COUNTS {
        for index in 0..n {
            let node = SpectralNode(u16::try_from(index).expect("few nodes"));
            assert_eq!(
                SpectralNode::of_wavelength(node.centre_nm(n), n),
                node,
                "the centre of node {index} of {n}"
            );
        }
    }
}

#[test]
fn the_bin_probe_kernel_is_naga_clean() {
    validate(
        &format!("{BRANCH_SIG_SRC}\n{}\n{BIN_KERNEL}", kept_wgsl()),
        "the bin probe kernel",
    );
}

#[test]
fn the_wgsl_bin_rule_equals_of_wavelength_on_every_bin_edge_within_one_ulp() {
    let name = "the_wgsl_bin_rule_equals_of_wavelength_on_every_bin_edge_within_one_ulp";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let probes = bin_probes();
    let device = &ctx.device;
    let source = format!("{BRANCH_SIG_SRC}\n{}\n{BIN_KERNEL}", kept_wgsl());
    let pipeline = compute::create_compute_pipeline(device, "beam bin probe", &source, BIN_ENTRY);
    let input = compute::upload(
        device,
        "beam bin probes",
        &probes,
        wgpu::BufferUsages::STORAGE,
    );
    let output = compute::zeroed_buffer::<u32>(
        device,
        "beam bin nodes",
        probes.len(),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let bind_group = compute::bind_buffers(
        device,
        "beam bin probe bind group",
        &pipeline,
        &[(BIN_BINDING_IN, &input), (BIN_BINDING_OUT, &output)],
    );
    let groups = u32::try_from(probes.len().div_ceil(64)).expect("small probe list");
    compute::dispatch_and_wait(device, &ctx.queue, &pipeline, &bind_group, (groups, 1, 1));
    let nodes = compute::readback::<u32>(device, &ctx.queue, &output, probes.len());
    let mismatches: Vec<String> = probes
        .iter()
        .zip(&nodes)
        .filter_map(|(probe, &gpu)| {
            let cpu = u32::from(SpectralNode::of_wavelength(probe.lambda, probe.n_lambda).0);
            (cpu != gpu).then(|| {
                format!(
                    "n_lambda {} lambda {:?} (bits {:#010x}): cpu {cpu}, gpu {gpu}",
                    probe.n_lambda,
                    probe.lambda,
                    probe.lambda.to_bits()
                )
            })
        })
        .collect();
    assert_eq!(
        mismatches,
        Vec::<String>::new(),
        "the WGSL bin rule disagrees with SpectralNode::of_wavelength"
    );
}
