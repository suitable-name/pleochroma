//! Tests of the fragment and refine kernels.
//!
//! The source and binding checks run everywhere (naga, no adapter). The twin test compares the
//! GPU slots with the CPU twin (`lighting::integrate_fragment_counted` +
//! `color::integrate_nodes_to_xyz`) per pixel: relative to the pixel, or to `TWIN_FLOOR_REL` of
//! the peak below it. It skips cleanly without an adapter (`PLEOCHROMA_REQUIRE_GPU=1` makes the
//! skip a failure). The CPU bisects the longest edge while the GPU splits an n-gon into a centroid
//! fan of quads and then quads into four, so the pieces that reach the 7-point rule differ; both
//! partitions are exact. The test prints the measured errors; a glint error above `GLINT_TWIN_REL`
//! means `bm_quad_of` should bisect the longest edge like the CPU. Leaves of more than 8 vertices
//! (pre-split pieces interpolating over the piece) are covered by the ten-vertex case of
//! `tests_d3_fragments`.

use glam::{DVec2, DVec3, Vec3};
use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::{
    buffers::{FragmentPlan, LeafBufferSet},
    fragments::{
        BEAM_BINDINGS, FRAGMENTS_ENTRY, FragmentOptions, REFINE_ENTRY, SCENE_BINDINGS,
        STORAGE_BUFFERS, dispatch_fragments, dispatch_fragments_with, fragment_kernel_source,
        refine_kernel_source,
    },
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
                BranchPrefix, GpuLeafHeader, GpuLeafNode, GpuLeafVertex, LEAF_FLAG_GLARE,
                LEAF_FLAG_REFLECTION, Leaf, PackedLeaves, SpectralInterval,
            },
            geom::polygon::ConvexPolygon,
            lighting::{
                FragmentInput, FragmentWork, LightingScene, integrate_fragment_counted,
                sun_disc_term,
            },
        },
        env_map::{EnvRotation, EnvironmentMap},
        gpu::{GpuContext, compute},
        gpu_backend::GpuSceneRef,
        path_cache::record::{f16_bits_to_f32, f32_to_f16_bits},
    },
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// The beam binding range (guide §14.7 item 5).
const BEAM_RANGE: std::ops::RangeInclusive<u32> = 200..=219;

/// Relative GPU-vs-CPU tolerance of a fragment's XYZ away from glints (f32 against f64 inside
/// the CPU twin, plus the WGSL transcendental differences).
const FRAGMENT_TWIN_REL: f32 = 2e-3;

/// Relative tolerance on sun-disc pixels (the disc edge is resolved in f32 on the GPU).
const GLINT_TWIN_REL: f32 = 5e-2;

/// The fraction of the peak CPU `Y` below which a pixel is compared on the peak's scale. The GPU's
/// f32 geometry (vertex positions, the cell clipping, the sun's sector sum) carries an absolute
/// error of a few 1e-6 of the brightest pixel, so a flat floor would ask for more dynamic range
/// than f32 gives. A pixel above the floor is held to `FRAGMENT_TWIN_REL` of its own value.
const TWIN_FLOOR_REL: f32 = 1e-2;

/// The denominator of a pixel's relative error: its own CPU value, or the scene floor below it.
fn error_scale(expected: Vec3, peak: f32) -> f32 {
    expected
        .max_element()
        .max((TWIN_FLOOR_REL * peak).max(1e-6))
}

/// Frame edge in pixels of the twin test.
const FRAME: u32 = 24;

/// The subdivision depth cap of both twins (a steep fixture still terminates).
const TWIN_MAX_LEVELS: u32 = 3;

/// The narrow twin camera: 1.5 degrees of field of view over 24 pixels is 0.0011 rad per pixel
/// (0.0022 after the mirror), the angular scale `tau_L` = 1e-3 is designed for, and the sun disc
/// spans about two pixels.
fn twin_camera() -> Camera {
    Camera::new(0.6, 0.45, 3.0, 1.5)
}

/// Spectral nodes of the twin test (one group).
const NODES: usize = NODE_COUNT as usize;

/// The node count of the test leaves, as the GPU layout stores it.
const NODE_COUNT: u32 = 8;

/// Parses and validates a source; panics with the diagnostics otherwise.
fn validate(source: &str, what: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module = wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("{what} does not parse: {}", e.emit_to_string(source)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("{what} does not validate: {}", e.emit_to_string(source)));
    (module, info)
}

/// The sorted bindings and the storage-buffer count the entry point `entry` reaches.
fn entry_bindings(source: &str, entry: &str) -> (Vec<u32>, usize) {
    let (module, info) = validate(source, entry);
    let index = module
        .entry_points
        .iter()
        .position(|ep| ep.name == entry)
        .unwrap_or_else(|| panic!("entry point {entry} exists"));
    let uses = info.get_entry_point(index);
    let mut bindings = Vec::new();
    let mut storage = 0;
    for (handle, var) in module.global_variables.iter() {
        if uses[handle].is_empty() {
            continue;
        }
        if let Some(binding) = &var.binding {
            bindings.push(binding.binding);
            if matches!(var.space, naga::AddressSpace::Storage { .. }) {
                storage += 1;
            }
        }
    }
    bindings.sort_unstable();
    (bindings, storage)
}

#[test]
fn the_anchors_are_found_and_the_assembled_sources_are_naga_clean() {
    let fragments = fragment_kernel_source().expect("every anchor occurs exactly once");
    let refine = refine_kernel_source().expect("every anchor occurs exactly once");
    assert!(fragments.contains(FRAGMENTS_ENTRY));
    assert!(!fragments.contains(REFINE_ENTRY));
    assert!(refine.contains(REFINE_ENTRY));
    // The megakernel entry is cut away and the zoned material layout is kept.
    assert!(!fragments.contains("fn transport_main("));
    assert!(fragments.contains("zones: GpuZoneTable,"));
    validate(&fragments, "the fragment kernel");
    validate(&refine, "the refine kernel");
}

#[test]
fn both_entry_points_bind_the_scene_subset_and_the_beam_range_only() {
    let mut expected: Vec<u32> = SCENE_BINDINGS
        .iter()
        .chain(&BEAM_BINDINGS)
        .copied()
        .collect();
    expected.sort_unstable();
    let sources = [
        (fragment_kernel_source().expect("source"), FRAGMENTS_ENTRY),
        (refine_kernel_source().expect("source"), REFINE_ENTRY),
    ];
    for (source, entry) in sources {
        let (bindings, storage) = entry_bindings(&source, entry);
        assert_eq!(bindings, expected, "{entry}");
        assert!(
            bindings
                .iter()
                .filter(|b| !SCENE_BINDINGS.contains(b))
                .all(|b| BEAM_RANGE.contains(b)),
            "{entry}: beam bindings leave 200-219"
        );
        assert_eq!(
            u32::try_from(storage).expect("few buffers"),
            STORAGE_BUFFERS,
            "{entry}"
        );
    }
}

// ---- the hardware twin ----

/// An unnormalised mirror-reflected camera field, affine in the image like a reflection leaf's.
struct Field {
    forward: DVec3,
    right: DVec3,
    up: DVec3,
    fov_tan: f64,
    mirror: DVec3,
}

impl Field {
    fn towards(camera: &Camera, target: Vec3) -> Self {
        let forward = camera.forward.as_dvec3();
        Self {
            forward,
            right: camera.right.as_dvec3(),
            up: camera.up.as_dvec3(),
            fov_tan: f64::from(camera.fov_tan),
            mirror: (forward - target.as_dvec3()).normalize(),
        }
    }

    fn direction(&self, p: DVec2) -> DVec3 {
        let size = f64::from(FRAME);
        let u = (p.x / size - 0.5) * 2.0 * self.fov_tan;
        let v = (0.5 - p.y / size) * 2.0 * self.fov_tan;
        let d = self.forward + self.right * u + self.up * v;
        d - self.mirror * (2.0 * d.dot(self.mirror))
    }
}

/// The weight of node `node` at `p`: a Fresnel-like level with a node slope and a spatial ramp.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the weight is deliberately rounded from f64 to the f32 the f16 packing starts from"
)]
fn weight(base: f64, slope: f64, node: usize, p: DVec2) -> f32 {
    let w = slope.mul_add(node as f64, base) + 0.01 * (p.x - 12.0) / 8.0;
    f16_bits_to_f32(f32_to_f16_bits(w as f32))
}

/// The CPU leaf and the same leaf in the GPU layout (weights rounded to f16 on both sides).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the GPU layout stores the f64 image points as f32 on purpose"
)]
fn build_leaf(field: &Field, base: f64, slope: f64, glare: bool) -> (Leaf, PackedLeaves) {
    let corners = [
        DVec2::new(7.3, 7.9),
        DVec2::new(16.6, 7.2),
        DVec2::new(16.1, 16.4),
        DVec2::new(8.0, 15.8),
    ];
    let image = ConvexPolygon::from_image_points(&corners).expect("a convex leaf");
    let mut dirs = Vec::new();
    let mut weights = Vec::new();
    let mut packed = PackedLeaves::default();
    for v in image.vertices() {
        let p = DVec2::new(v.x, v.y);
        let direction = field.direction(p).as_vec3();
        packed.vertices.push(GpuLeafVertex {
            image: [v.x as f32, v.y as f32],
            hero_dir: direction.to_array(),
            first_node: u32::try_from(packed.nodes.len()).expect("few nodes"),
        });
        for node in 0..NODES {
            let w = weight(base, slope, node, p);
            dirs.push(direction);
            weights.push(w);
            packed.nodes.push(GpuLeafNode {
                weight_dx: u32::from(f32_to_f16_bits(w)),
                dy_dz: 0,
            });
        }
    }
    let mut flags = LEAF_FLAG_REFLECTION;
    if glare {
        flags |= LEAF_FLAG_GLARE;
    }
    packed.headers.push(GpuLeafHeader {
        sig: [0, 0],
        first_vertex: 0,
        vertex_count: u32::try_from(image.len()).expect("few vertices"),
        first_node: 0,
        node_count: NODE_COUNT,
        flags,
        tau_omega: 1e-4,
    });
    packed.tiles = vec![0];
    packed.tile_offsets = vec![0, 1];
    let leaf = Leaf {
        tile: 0,
        image,
        dirs,
        weights,
        spectrum: SpectralInterval {
            first: 0,
            last: u16::try_from(NODES - 1).expect("few nodes"),
        },
        sig: [0, 0],
        prefix: BranchPrefix::EMPTY,
        glare,
        reflection: true,
        tau_omega: 1e-4,
        pre_clip: None,
    };
    (leaf, packed)
}

/// The corners of the leaf's image polygon.
fn image_corners(leaf: &Leaf) -> Vec<DVec2> {
    leaf.image
        .vertices()
        .iter()
        .map(|v| DVec2::new(v.x, v.y))
        .collect()
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

/// One GPU run of the leaf; the slots and the plan.
fn run_gpu(
    ctx: &GpuContext,
    camera: &Camera,
    environment: EnvironmentSource<'_>,
    packed: &PackedLeaves,
    max_levels: u32,
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
    let options = FragmentOptions {
        max_levels,
        n_lambda: NODE_COUNT,
        tolerance: 1e-3,
    };
    let slots = dispatch_fragments_with(ctx, &scene, &set, &plan, &options)
        .expect("the fragment kernels run");
    assert_eq!(slots.count, plan.total);
    let words =
        compute::readback::<[f32; 4]>(&ctx.device, &ctx.queue, &slots.slots, plan.total as usize);
    (words, plan)
}

/// The pixel with the largest non-glint error of a twin run.
struct WorstPixel {
    /// Pixel column.
    px: u32,
    /// Pixel row.
    py: u32,
    /// Index of the pixel in the readback slots.
    slot: usize,
    /// The CPU twin's XYZ.
    expected: Vec3,
    /// The GPU's XYZ.
    got: Vec3,
}

/// What the per-pixel comparison of a twin run found.
struct TwinStats {
    /// Largest relative error away from glints.
    worst: f32,
    /// Largest relative error on glint pixels.
    worst_glint: f32,
    /// Largest CPU Y.
    peak: f32,
    /// Deepest subdivision level of the CPU twin.
    deepest: u32,
    /// The CPU work of the worst non-glint pixel.
    worst_work: FragmentWork,
    /// The worst non-glint pixel.
    worst_pixel: Option<WorstPixel>,
}

/// One pixel of a twin run before its error is known.
struct PixelPair {
    /// Pixel column.
    px: u32,
    /// Pixel row.
    py: u32,
    /// Index of the pixel in the readback slots.
    slot: usize,
    /// The CPU twin's XYZ.
    expected: Vec3,
    /// The GPU's XYZ.
    got: Vec3,
    /// The CPU work of the pixel.
    work: FragmentWork,
}

/// Compares every pixel of the plan rectangle `rect` of `gpu` with the CPU twin of `leaf`.
fn compare_pixels(
    gpu: &[[f32; 4]],
    rect: [u32; 4],
    scene: &LightingScene<'_>,
    leaf: &Leaf,
    glint: bool,
) -> TwinStats {
    let width = rect[2] - rect[0];
    let corners = image_corners(leaf);
    let mut pixels = Vec::new();
    for py in rect[1]..rect[3] {
        for px in rect[0]..rect[2] {
            let cell = clip_cell(&corners, f64::from(px), f64::from(py));
            let slot = ((py - rect[1]) * width + (px - rect[0])) as usize;
            let Some(polygon) = ConvexPolygon::from_image_points(&cell) else {
                continue;
            };
            let (row, work) = integrate_fragment_counted(
                scene,
                &FragmentInput {
                    leaf,
                    polygon: &polygon,
                    group: 0,
                },
            );
            pixels.push(PixelPair {
                px,
                py,
                slot,
                expected: integrate_nodes_to_xyz(&row, 0, NODE_COUNT),
                got: Vec3::from_slice(&gpu[slot][..3]),
                work,
            });
        }
    }
    let peak = pixels.iter().map(|p| p.expected.y).fold(0.0, f32::max);
    let mut stats = TwinStats {
        worst: 0.0,
        worst_glint: 0.0,
        peak,
        deepest: 0,
        worst_work: FragmentWork::default(),
        worst_pixel: None,
    };
    for pair in pixels {
        let rel = (pair.got - pair.expected).abs().max_element() / error_scale(pair.expected, peak);
        if glint && pair.expected.y > 50.0 {
            stats.worst_glint = stats.worst_glint.max(rel);
        } else if rel > stats.worst {
            stats.worst = rel;
            stats.worst_work = pair.work;
            stats.worst_pixel = Some(WorstPixel {
                px: pair.px,
                py: pair.py,
                slot: pair.slot,
                expected: pair.expected,
                got: pair.got,
            });
        }
        stats.deepest = stats.deepest.max(pair.work.deepest);
    }
    stats
}

/// Prints the worst pixel with its sun and sky parts on both sides.
///
/// The CPU sun term is subtracted from the CPU total; the GPU is run again under `sky` (the same
/// sky without the disc, `DaylightDome`) and subtracted from the GPU total.
fn report_worst_pixel(
    ctx: &GpuContext,
    scene: &LightingScene<'_>,
    (leaf, packed): (&Leaf, &PackedLeaves),
    worst: &WorstPixel,
    sky: EnvironmentSource<'_>,
    label: &str,
) {
    let cell = clip_cell(
        &image_corners(leaf),
        f64::from(worst.px),
        f64::from(worst.py),
    );
    let polygon = ConvexPolygon::from_image_points(&cell).expect("the worst cell");
    let cpu_sun_row = sun_disc_term(
        scene,
        &FragmentInput {
            leaf,
            polygon: &polygon,
            group: 0,
        },
    );
    let cpu_sun = integrate_nodes_to_xyz(&cpu_sun_row, 0, NODE_COUNT);
    let (sky_gpu, _) = run_gpu(ctx, scene.camera, sky, packed, TWIN_MAX_LEVELS);
    let got_sky = Vec3::from_slice(&sky_gpu[worst.slot][..3]);
    println!(
        "{label}: worst non-glint pixel ({}, {}) cell {cell:?}: expected {:?} (CPU sun {cpu_sun:?}, CPU sky {:?}), got {:?} (GPU sky-only {got_sky:?}, GPU sun {:?})",
        worst.px,
        worst.py,
        worst.expected,
        worst.expected - cpu_sun,
        worst.got,
        worst.got - got_sky
    );
}

#[test]
fn gpu_fragments_equal_the_cpu_twin_on_reflection_leaves_under_three_lightings() {
    let name = "gpu_fragments_equal_the_cpu_twin_on_reflection_leaves_under_three_lightings";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let camera = twin_camera();
    let (yaw, pitch) = (0.4, 0.6);
    let rig = StudioRig::new(yaw, pitch);
    // (label, preset, mirror target, glint expected)
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
    for (light_label, preset, target, glint) in lightings {
        let environment = preset.studio(1.0, yaw, pitch);
        let field = Field::towards(&camera, target);
        for (stone_label, base, slope) in stones {
            let (leaf, packed) = build_leaf(&field, base, slope, true);
            let scene = LightingScene {
                environment,
                rig: Some(StudioRig::new(yaw, pitch)),
                camera: &camera,
                size: (FRAME, FRAME),
                tolerance: 1e-3,
                max_levels: TWIN_MAX_LEVELS,
                n_lambda: NODE_COUNT,
            };
            let (gpu, plan) = run_gpu(&ctx, &camera, environment, &packed, TWIN_MAX_LEVELS);
            let (gpu_again, _) = run_gpu(&ctx, &camera, environment, &packed, TWIN_MAX_LEVELS);
            assert_eq!(
                gpu, gpu_again,
                "{light_label}/{stone_label}: two runs differ"
            );
            let TwinStats {
                worst,
                worst_glint,
                peak,
                deepest,
                worst_work,
                worst_pixel,
            } = compare_pixels(&gpu, plan.pixel_rects[0], &scene, &leaf, glint);
            println!(
                "{light_label}/{stone_label}: worst rel {worst:.3e} (FRAGMENT_TWIN_REL {FRAGMENT_TWIN_REL}), worst glint rel {worst_glint:.3e} (GLINT_TWIN_REL {GLINT_TWIN_REL}), peak Y {peak}, deepest level {deepest}, work of the worst non-glint pixel {worst_work:?}"
            );
            if let (true, Some(worst_pixel)) = (glint, worst_pixel) {
                // The sun's own sky without the disc: `DaylightDome` is `daylight_sun_radiance` minus the
                // sun term (`Daylight` is the D65 studio rig and has nothing to do with it).
                let sky = LightingPreset::DaylightDome.studio(1.0, yaw, pitch);
                report_worst_pixel(
                    &ctx,
                    &scene,
                    (&leaf, &packed),
                    &worst_pixel,
                    sky,
                    &format!("{light_label}/{stone_label}"),
                );
            }
            assert!(
                worst <= FRAGMENT_TWIN_REL,
                "{light_label}/{stone_label}: {worst}"
            );
            assert!(
                worst_glint <= GLINT_TWIN_REL,
                "{light_label}/{stone_label}: glint {worst_glint}"
            );
        }
    }
}

/// A sky whose radiance changes with the azimuth everywhere, so any wrong turn shows.
fn turn_sky() -> EnvironmentMap {
    let (width, height) = (32_usize, 16_usize);
    let texels = (0..width * height)
        .map(|i| {
            let u = (i % width) as f32 / width as f32;
            let v = (i / width) as f32 / height as f32;
            [0.2 + u, 0.5f32.mul_add(v, 0.3), u.mul_add(v, 0.1)]
        })
        .collect();
    EnvironmentMap::from_rgb(width, height, texels).expect("self-consistent texels")
}

/// The fragment kernel relights a leaf under a turned HDR map as the CPU twin does (the lookups
/// go through `hdr_world_to_map` with the uniform's yaw built by
/// `GpuTransportParams::with_hdr_of`), and the turn changes the slots.
#[test]
fn gpu_fragments_equal_the_cpu_twin_under_a_turned_hdr_map() {
    let name = "gpu_fragments_equal_the_cpu_twin_under_a_turned_hdr_map";
    let Some(ctx) = context_or_skip(name) else {
        return;
    };
    let camera = twin_camera();
    let map = turn_sky();
    let field = Field::towards(&camera, Vec3::new(0.3, 0.8, 0.2).normalize());
    let (leaf, packed) = build_leaf(&field, 0.17, 0.0012, false);
    let mut frames = Vec::new();
    for yaw in [0.0f32, 1.3] {
        let environment =
            EnvironmentSource::hdr_at(&map, 1.5).with_hdr_rotation(EnvRotation::from_yaw(yaw));
        let scene = LightingScene {
            environment,
            rig: None,
            camera: &camera,
            size: (FRAME, FRAME),
            tolerance: 1e-3,
            max_levels: TWIN_MAX_LEVELS,
            n_lambda: NODE_COUNT,
        };
        let (gpu, plan) = run_gpu(&ctx, &camera, environment, &packed, TWIN_MAX_LEVELS);
        let stats = compare_pixels(&gpu, plan.pixel_rects[0], &scene, &leaf, false);
        println!(
            "turned hdr map, yaw {yaw}: worst rel {:.3e} (FRAGMENT_TWIN_REL \
             {FRAGMENT_TWIN_REL}), peak Y {}",
            stats.worst, stats.peak
        );
        assert!(
            stats.worst <= FRAGMENT_TWIN_REL,
            "yaw {yaw}: {}",
            stats.worst
        );
        frames.push(gpu);
    }
    assert!(
        frames[0] != frames[1],
        "a turned map must change the relit slots"
    );
}

#[test]
fn an_empty_plan_gives_no_slots_and_a_mismatched_plan_declines() {
    let Some(ctx) = context_or_skip("an_empty_plan_gives_no_slots_and_a_mismatched_plan_declines")
    else {
        return;
    };
    let camera = twin_camera();
    let field = Field::towards(&camera, Vec3::new(0.3, 0.8, 0.2).normalize());
    let (_, packed) = build_leaf(&field, 0.04, 0.0, false);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = GpuSceneRef {
        camera: &camera,
        width: FRAME,
        height: FRAME,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::LightTent.studio(1.0, 0.4, 0.6),
        estimator: None,
    };
    let set = LeafBufferSet::upload(&ctx, &packed).expect("upload");
    let mut plan = FragmentPlan::from_leaves(&packed, (FRAME, FRAME));
    let ok = dispatch_fragments(&ctx, &scene, &set, &plan, 6);
    assert!(ok.is_some());
    plan.pixel_rects.clear();
    assert!(dispatch_fragments(&ctx, &scene, &set, &plan, 6).is_none());
}
