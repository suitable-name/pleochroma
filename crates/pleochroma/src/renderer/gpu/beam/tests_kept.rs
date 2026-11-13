//! Tests of the kept buffer and `beam_kept_test`.
//!
//! The upload layout, the CSR of the generated scenario and the naga check run everywhere; the
//! compute test skips cleanly without an adapter (`PLEOCHROMA_REQUIRE_GPU=1` turns the skip into
//! a failure, as in the other GPU tests). The compute test compares the kernel with a CPU twin
//! written here: the bin rule of guide §9.2 and the `contains_2d(q, 0.0)` convention (a point is
//! inside when `(b - a) x (q - a) >= 0` for every edge of the counter-clockwise polygon).

use std::mem::offset_of;

use bytemuck::{Pod, Zeroable};
use naga::{
    TypeInner,
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

use super::{
    kept::{
        KEPT_BINDING_INDICES, KEPT_BINDING_PARAMS, KEPT_BINDING_PIXEL_OFFSETS,
        KEPT_BINDING_REGIONS, KEPT_STORAGE_BUFFERS, KeptBufferSet, KeptParams, kept_params,
        kept_wgsl, max_kept_buffer_bytes,
    },
    reference_source::BRANCH_SIG_SRC,
};
use crate::renderer::{
    beam::beam::{GpuKeptRegion, KEPT_MAX_VERTICES, PackedKept, SpectralNode},
    gpu::{GpuContext, compute},
};

/// Environment variable that turns a missing adapter into a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// The lowest and highest binding the beam units may use (guide §14.7 item 5).
const BEAM_BINDINGS: std::ops::RangeInclusive<u32> = 200..=219;

/// The probe kernel's own bindings (inside the beam range, after the kept ones).
const PROBE_BINDING_IN: u32 = 214;
const PROBE_BINDING_OUT: u32 = 215;

/// The entry point of [`PROBE_KERNEL`].
const PROBE_ENTRY: &str = "kept_probe_main";

/// A tiny kernel that runs `beam_kept_test` on a list of probes.
const PROBE_KERNEL: &str = "
struct Probe {
    pixel: vec2<u32>,
    point: vec2<f32>,
    sig: vec2<u32>,
    lambda: f32,
    pad: u32,
}

@group(0) @binding(214) var<storage, read> probes: array<Probe>;
@group(0) @binding(215) var<storage, read_write> verdicts: array<u32>;

@compute @workgroup_size(64)
fn kept_probe_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&probes)) {
        return;
    }
    let p = probes[i];
    verdicts[i] = select(0u, 1u, beam_kept_test(p.pixel, p.point, p.sig, p.lambda));
}
";

/// The number of spectral nodes of the scenario.
const N_LAMBDA: u32 = 24;

/// A probe is "near an edge" when its distance to the polygon boundary is below this many pixels;
/// the GPU may contract the cross product into an `fma`, so only such probes may disagree.
const EDGE_TOL: f32 = 1e-4;

/// A wavelength is "near a bin edge" when its bin coordinate is within this of an integer; the
/// GPU division is not required to be correctly rounded.
const BIN_TOL: f32 = 1e-3;

/// The kernel text the tests compile: the signature unit, the kept unit and the probe kernel.
fn probe_source() -> String {
    format!("{BRANCH_SIG_SRC}\n{}\n{PROBE_KERNEL}", kept_wgsl())
}

/// One probe in the kernel's layout, 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct Probe {
    pixel: [u32; 2],
    point: [f32; 2],
    sig: [u32; 2],
    lambda: f32,
    pad: u32,
}

/// A deterministic `SplitMix64` generator.
struct Rng(u64);

impl Rng {
    fn next_word(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform value in `[0, 1)` with 32 random bits of resolution (spread over two halves).
    fn unit(&mut self) -> f32 {
        let word = self.next_word();
        let high = u16::try_from((word >> 48) & 0xFFFF).unwrap_or(0);
        let low = u16::try_from((word >> 32) & 0xFFFF).unwrap_or(0);
        (f32::from(high) + f32::from(low) / 65536.0) / 65536.0
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        (high - low).mul_add(self.unit(), low)
    }

    /// A uniform integer in `0..n`.
    fn below(&mut self, n: u32) -> u32 {
        u32::try_from((self.next_word() >> 32) % u64::from(n)).unwrap_or(0)
    }
}

/// The branch signatures the scenario draws from.
const SIGS: [[u32; 2]; 4] = [[1, 2], [3, 4], [0xDEAD_BEEF, 7], [0, 0]];

/// The image is `IMAGE_W x IMAGE_H` pixels; the CSR covers the rectangle below it.
const IMAGE_W: u32 = 16;
const IMAGE_H: u32 = 12;
const CSR_ORIGIN: (u32, u32) = (3, 2);
const CSR_SIZE: (u32, u32) = (8, 6);
const REGION_COUNT: u32 = 40;

/// The polygon capacity of a region (`KEPT_MAX_VERTICES`, asserted equal in the layout test).
const MAX_VERTICES: u32 = 12;

/// A small unsigned value as `f32` (exact).
fn small(value: u32) -> f32 {
    f32::from(u16::try_from(value).expect("small value"))
}

/// A random convex counter-clockwise polygon inscribed in a circle, with 3 to 12 vertices.
fn random_region(rng: &mut Rng) -> GpuKeptRegion {
    let len = 3 + rng.below(MAX_VERTICES - 2);
    let (cx, cy) = (rng.range(2.0, 14.0), rng.range(1.0, 11.0));
    let radius = rng.range(0.8, 5.0);
    let mut angles: Vec<f32> = (0..len)
        .map(|_| rng.range(0.0, std::f32::consts::TAU))
        .collect();
    angles.sort_by(f32::total_cmp);
    let mut region = GpuKeptRegion::default();
    for (slot, angle) in region.polygon.iter_mut().zip(&angles) {
        *slot = [
            radius.mul_add(angle.cos(), cx),
            radius.mul_add(angle.sin(), cy),
        ];
    }
    region.len = len;
    region.sig = SIGS[rng.below(4) as usize];
    let (a, b) = (rng.below(N_LAMBDA), rng.below(N_LAMBDA));
    region.first_node = a.min(b);
    region.last_node = a.max(b);
    region
}

/// The bounding box `(min_x, min_y, max_x, max_y)` of a region's polygon.
fn region_box(region: &GpuKeptRegion) -> (f32, f32, f32, f32) {
    let used = &region.polygon[..region.len as usize];
    used.iter().fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(x0, y0, x1, y1), p| (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1])),
    )
}

/// The scenario: random regions and a conservative per-pixel CSR (a region is listed in every cell
/// whose one-pixel neighbourhood meets the region's bounding box), ascending region indices.
fn scenario(seed: u64) -> PackedKept {
    let mut rng = Rng(seed);
    let regions: Vec<GpuKeptRegion> = (0..REGION_COUNT).map(|_| random_region(&mut rng)).collect();
    let mut pixel_offsets = vec![0_u32];
    let mut indices = Vec::new();
    for cy in 0..CSR_SIZE.1 {
        for cx in 0..CSR_SIZE.0 {
            let px = small(CSR_ORIGIN.0 + cx);
            let py = small(CSR_ORIGIN.1 + cy);
            for (r, region) in regions.iter().enumerate() {
                let (x0, y0, x1, y1) = region_box(region);
                if x1 >= px - 1.0 && x0 <= px + 1.0 && y1 >= py - 1.0 && y0 <= py + 1.0 {
                    indices.push(u32::try_from(r).expect("small scenario"));
                }
            }
            pixel_offsets.push(u32::try_from(indices.len()).expect("small scenario"));
        }
    }
    PackedKept {
        regions,
        origin: CSR_ORIGIN,
        size: CSR_SIZE,
        pixel_offsets,
        indices,
        span_count: 0,
    }
}

/// The CPU twin of the bin rule (identical to `SpectralNode::of_wavelength`).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to [0, n_lambda - 1] first, and n_lambda is small"
)]
fn twin_node(lambda: f32, n_lambda: u32) -> u32 {
    let raw = ((lambda - 380.0) * (small(n_lambda) * 0.0025)).floor();
    if raw.is_nan() || raw < 0.0 {
        return 0;
    }
    raw.min(small(n_lambda.saturating_sub(1))) as u32
}

/// The CPU twin of `beam_kept_test`: the verdict, and whether the probe is too close to a polygon
/// edge or a bin edge for the GPU's rounding to be trusted.
fn twin_kept(packed: &PackedKept, n_lambda: u32, probe: &Probe) -> (bool, bool) {
    let (width, height) = packed.size;
    let [px, py] = probe.pixel;
    let (ox, oy) = packed.origin;
    if px < ox || py < oy || px - ox >= width || py - oy >= height {
        return (false, false);
    }
    let cell = ((py - oy) * width + (px - ox)) as usize;
    let (first, last) = (
        packed.pixel_offsets[cell] as usize,
        packed.pixel_offsets[cell + 1] as usize,
    );
    let node = twin_node(probe.lambda, n_lambda);
    let bin_position = (probe.lambda - 380.0) * small(n_lambda) / 400.0;
    let mut near = (bin_position - bin_position.round()).abs() < BIN_TOL;
    let mut kept = false;
    for &index in &packed.indices[first..last] {
        let region = &packed.regions[index as usize];
        if region.sig != probe.sig || node < region.first_node || node > region.last_node {
            continue;
        }
        let n = region.len as usize;
        if n < 3 {
            continue;
        }
        let [qx, qy] = probe.point;
        let mut inside = true;
        let mut nearest = f32::MAX;
        for i in 0..n {
            let [ax, ay] = region.polygon[i];
            let [bx, by] = region.polygon[(i + 1) % n];
            let orient = (by - ay).mul_add(-(qx - ax), (bx - ax) * (qy - ay));
            inside &= orient >= 0.0;
            let edge = (bx - ax).hypot(by - ay);
            if edge > 0.0 {
                nearest = nearest.min((orient / edge).abs());
            }
        }
        near |= nearest < EDGE_TOL;
        kept |= inside;
    }
    (kept, near)
}

/// Random probes over and around the CSR rectangle, with signatures that mostly exist among the
/// regions and wavelengths that include both clamped ends.
fn random_probes(seed: u64, count: usize) -> Vec<Probe> {
    let mut rng = Rng(seed);
    (0..count)
        .map(|_| {
            let pixel = [rng.below(IMAGE_W), rng.below(IMAGE_H)];
            let point = if rng.below(8) == 0 {
                [
                    rng.range(-1.0, small(IMAGE_W) + 1.0),
                    rng.range(-1.0, small(IMAGE_H) + 1.0),
                ]
            } else {
                [
                    small(pixel[0]) + rng.range(-0.5, 0.5),
                    small(pixel[1]) + rng.range(-0.5, 0.5),
                ]
            };
            let sig = if rng.below(10) == 0 {
                [rng.below(1000), rng.below(1000)]
            } else {
                SIGS[rng.below(4) as usize]
            };
            Probe {
                pixel,
                point,
                sig,
                lambda: rng.range(370.0, 790.0),
                pad: 0,
            }
        })
        .collect()
}

fn parse_and_validate(source: &str) -> (naga::Module, naga::valid::ModuleInfo) {
    let module = wgsl::parse_str(source)
        .unwrap_or_else(|e| panic!("does not parse: {}", e.emit_to_string(source)));
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .unwrap_or_else(|e| panic!("does not validate: {}", e.emit_to_string(source)));
    (module, info)
}

#[test]
fn the_upload_layout_matches_the_rust_structs() {
    assert_eq!(KEPT_MAX_VERTICES, MAX_VERTICES as usize);
    assert_eq!(size_of::<GpuKeptRegion>(), 128);
    assert_eq!(size_of::<KeptParams>(), 32);
    assert_eq!(size_of::<Probe>(), 32);
    assert_eq!(KEPT_STORAGE_BUFFERS, 3);
    let (module, _) = parse_and_validate(kept_wgsl());
    let layout_of = |name: &str| -> (Vec<u32>, u32) {
        module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(n), TypeInner::Struct { members, span }) if n == name => {
                    Some((members.iter().map(|m| m.offset).collect(), *span))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("struct {name} exists"))
    };
    let region_offsets = [
        offset_of!(GpuKeptRegion, polygon),
        offset_of!(GpuKeptRegion, sig),
        offset_of!(GpuKeptRegion, len),
        offset_of!(GpuKeptRegion, first_node),
        offset_of!(GpuKeptRegion, last_node),
        offset_of!(GpuKeptRegion, flags),
        offset_of!(GpuKeptRegion, pad),
        offset_of!(GpuKeptRegion, pad) + 4,
    ]
    .map(|o| u32::try_from(o).expect("small offset"));
    assert_eq!(layout_of("KeptRegion"), (region_offsets.to_vec(), 128));
    let params_offsets = [
        offset_of!(KeptParams, origin),
        offset_of!(KeptParams, origin) + 4,
        offset_of!(KeptParams, size),
        offset_of!(KeptParams, size) + 4,
        offset_of!(KeptParams, n_lambda),
        offset_of!(KeptParams, region_count),
        offset_of!(KeptParams, spans),
        offset_of!(KeptParams, spans) + 4,
    ]
    .map(|o| u32::try_from(o).expect("small offset"));
    assert_eq!(layout_of("KeptParams"), (params_offsets.to_vec(), 32));
}

#[test]
fn the_scenario_csr_is_well_formed() {
    let packed = scenario(11);
    let cells = (CSR_SIZE.0 * CSR_SIZE.1) as usize;
    assert_eq!(packed.pixel_offsets.len(), cells + 1);
    assert_eq!(packed.pixel_offsets[0], 0);
    assert!(packed.pixel_offsets.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(
        packed.pixel_offsets.last().copied(),
        u32::try_from(packed.indices.len()).ok()
    );
    for cell in 0..cells {
        let row = &packed.indices
            [packed.pixel_offsets[cell] as usize..packed.pixel_offsets[cell + 1] as usize];
        assert!(row.windows(2).all(|w| w[0] < w[1]), "cell {cell} ascending");
        assert!(row.iter().all(|&r| r < REGION_COUNT));
    }
    assert_ne!(packed.indices.len(), 0);
    for region in &packed.regions {
        assert!((3..=MAX_VERTICES).contains(&region.len));
        assert!(region.first_node <= region.last_node && region.last_node < N_LAMBDA);
    }
    assert_eq!(
        kept_params(&packed, N_LAMBDA),
        Some(KeptParams {
            origin: CSR_ORIGIN.into(),
            size: CSR_SIZE.into(),
            n_lambda: N_LAMBDA,
            region_count: REGION_COUNT,
            spans: [0; 2],
        })
    );
    assert_eq!(
        max_kept_buffer_bytes(&packed),
        u64::from(REGION_COUNT) * 128
    );
}

#[test]
fn an_empty_batch_has_zero_params_and_one_element_buffers() {
    let empty = PackedKept::default();
    assert_eq!(
        kept_params(&empty, N_LAMBDA),
        Some(KeptParams {
            n_lambda: N_LAMBDA,
            ..KeptParams::default()
        })
    );
    assert_eq!(max_kept_buffer_bytes(&empty), 128);
}

#[test]
fn malformed_csr_tables_are_refused() {
    let mut packed = scenario(12);
    packed.pixel_offsets.pop();
    assert_eq!(kept_params(&packed, N_LAMBDA), None);
    let lopsided = PackedKept {
        size: (3, 0),
        ..PackedKept::default()
    };
    assert_eq!(kept_params(&lopsided, N_LAMBDA), None);
}

#[test]
fn the_twin_bin_rule_equals_the_cpu_node_rule() {
    for n_lambda in [1_u32, 2, 24, 48, 96] {
        for step in 0..=2000_u16 {
            let lambda = f32::from(step).mul_add(0.2, 360.0);
            assert_eq!(
                twin_node(lambda, n_lambda),
                u32::from(SpectralNode::of_wavelength(lambda, n_lambda).0),
                "lambda {lambda} n_lambda {n_lambda}"
            );
        }
    }
}

#[test]
fn the_kept_kernel_is_naga_clean_and_binds_only_the_beam_range() {
    let source = probe_source();
    let (module, info) = parse_and_validate(&source);
    let index = module
        .entry_points
        .iter()
        .position(|ep| ep.name == PROBE_ENTRY)
        .expect("the probe entry point exists");
    let uses = info.get_entry_point(index);
    let mut bindings: Vec<u32> = module
        .global_variables
        .iter()
        .filter(|(handle, _)| !uses[*handle].is_empty())
        .filter_map(|(_, var)| var.binding.as_ref().map(|b| b.binding))
        .collect();
    bindings.sort_unstable();
    assert_eq!(
        bindings,
        vec![
            KEPT_BINDING_REGIONS,
            KEPT_BINDING_PIXEL_OFFSETS,
            KEPT_BINDING_INDICES,
            KEPT_BINDING_PARAMS,
            PROBE_BINDING_IN,
            PROBE_BINDING_OUT,
        ]
    );
    assert!(bindings.iter().all(|b| BEAM_BINDINGS.contains(b)));
}

/// Runs `beam_kept_test` on `probes` against the uploaded `set`.
fn run_probes(ctx: &GpuContext, set: &KeptBufferSet, probes: &[Probe]) -> Vec<u32> {
    let device = &ctx.device;
    let pipeline =
        compute::create_compute_pipeline(device, "beam kept probe", &probe_source(), PROBE_ENTRY);
    let input = compute::upload(
        device,
        "beam kept probes",
        probes,
        wgpu::BufferUsages::STORAGE,
    );
    let output = compute::zeroed_buffer::<u32>(
        device,
        "beam kept verdicts",
        probes.len(),
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let [regions, offsets, indices, params] = set.bindings();
    let bind_group = compute::bind_buffers(
        device,
        "beam kept probe bind group",
        &pipeline,
        &[
            regions,
            offsets,
            indices,
            params,
            (PROBE_BINDING_IN, &input),
            (PROBE_BINDING_OUT, &output),
        ],
    );
    let groups = u32::try_from(probes.len().div_ceil(64)).expect("small probe list");
    compute::dispatch_and_wait(device, &ctx.queue, &pipeline, &bind_group, (groups, 1, 1));
    compute::readback::<u32>(device, &ctx.queue, &output, probes.len())
}

#[test]
fn the_gpu_kept_test_equals_the_cpu_twin_away_from_edges() {
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                std::env::var(REQUIRE_GPU_ENV).is_err(),
                "{REQUIRE_GPU_ENV} is set but no adapter is available: {error:?}"
            );
            eprintln!("skipping the GPU kept test: no adapter ({error:?})");
            return;
        }
    };
    let packed = scenario(2026);
    let set = KeptBufferSet::upload(&ctx, &packed, N_LAMBDA).expect("the scenario fits");
    assert_eq!(set.region_count, REGION_COUNT);
    let probes = random_probes(99, 20_000);
    let gpu = run_probes(&ctx, &set, &probes);
    assert_eq!(gpu.len(), probes.len());
    let (mut kept, mut near, mut mismatched, mut near_mismatches) = (0_usize, 0_usize, 0_usize, 0);
    for (index, (probe, &verdict)) in probes.iter().zip(&gpu).enumerate() {
        let (expected, ambiguous) = twin_kept(&packed, N_LAMBDA, probe);
        kept += usize::from(expected);
        near += usize::from(ambiguous);
        if (verdict == 1) != expected {
            if ambiguous {
                near_mismatches += 1;
            } else {
                mismatched += 1;
                eprintln!("probe {index}: gpu {verdict} cpu {expected}: {probe:?}");
            }
        }
    }
    eprintln!(
        "kept probes: {} total, {kept} kept, {near} near an edge, {near_mismatches} disagreeing \
         near an edge, {mismatched} disagreeing elsewhere",
        probes.len()
    );
    assert_eq!(mismatched, 0);
    assert!(kept > 0, "the scenario keeps some probes");
    assert!(kept < probes.len(), "the scenario drops some probes");
}

/// Two empty-kept tiles at opposite corners of a 7680 x 4320 frame.
fn corner_tiles() -> Vec<(
    crate::renderer::beam::primary::TileRect,
    crate::renderer::beam::residual::KeptIndex,
)> {
    use crate::renderer::beam::{primary::TileRect, residual::KeptIndex};
    [
        TileRect {
            x0: 0,
            y0: 0,
            x1: 64,
            y1: 64,
        },
        TileRect {
            x0: 7616,
            y0: 4256,
            x1: 7680,
            y1: 4320,
        },
    ]
    .into_iter()
    .map(|rect| (rect, KeptIndex::from_leaves(rect, &[])))
    .collect()
}

#[test]
fn an_empty_kept_set_at_opposite_corners_packs_to_nothing() {
    use crate::renderer::beam::residual::{kept_footprint, pack_kept, try_pack_kept};
    let tiles = corner_tiles();
    let footprint = kept_footprint(&tiles);
    // The batch's own slots are two tiles; the bounding rectangle is the whole frame.
    assert_eq!(footprint.tile_pixels, 2 * 64 * 64);
    assert_eq!(footprint.bounding_pixels(), 7680 * 4320);
    assert!(footprint.is_empty());
    assert_eq!(footprint.bytes(), 0);
    // Nothing is kept: no table over the bounding rectangle, even with a budget of zero bytes.
    let packed = pack_kept(&tiles);
    assert_eq!(packed.pixel_offsets, vec![0_u32]);
    assert_eq!(packed.size, (0, 0));
    assert_eq!(packed.regions.len(), 0);
    assert_eq!(packed.indices.len(), 0);
    assert_eq!(try_pack_kept(&tiles, 0), Some(packed));
}

#[test]
fn the_planned_kept_buffers_are_sized_by_the_footprint_before_packing() {
    use crate::renderer::beam::residual::KeptFootprint;
    let footprint = KeptFootprint {
        origin: (0, 0),
        size: (7680, 4320),
        tile_pixels: 2 * 64 * 64,
        rows: 2 * 64,
        regions: 2,
        memberships: 10,
    };
    // Two tiles at opposite 8K corners are the tile-slot form: the offsets table (one word per
    // batch pixel plus one) is the largest buffer, not a table over the bounding rectangle.
    assert!(footprint.uses_tile_slots());
    let offsets = (2_u64 * 64 * 64 + 1) * 4;
    assert_eq!(super::kept::planned_kept_buffer_bytes(&footprint), offsets);
    assert!(super::kept::kept_footprint_fits(&footprint, 64 << 10));
    assert!(!super::kept::kept_footprint_fits(&footprint, 16 << 10));
    // The same tiles filling their bounding rectangle stay dense: one word per bounding pixel.
    let filled = KeptFootprint {
        size: (128, 64),
        ..footprint
    };
    assert!(!filled.uses_tile_slots());
    assert_eq!(
        super::kept::planned_kept_buffer_bytes(&filled),
        (128_u64 * 64 + 1) * 4
    );
    // An empty kept set is the single zero element of each buffer, which is what uploads pad to.
    let empty = KeptFootprint::default();
    assert_eq!(
        super::kept::planned_kept_buffer_bytes(&empty),
        max_kept_buffer_bytes(&PackedKept::default())
    );
}

#[test]
fn an_empty_batch_keeps_nothing_on_the_gpu() {
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                std::env::var(REQUIRE_GPU_ENV).is_err(),
                "{REQUIRE_GPU_ENV} is set but no adapter is available: {error:?}"
            );
            eprintln!("skipping the empty GPU kept test: no adapter ({error:?})");
            return;
        }
    };
    let set = KeptBufferSet::upload(&ctx, &PackedKept::default(), N_LAMBDA).expect("fits");
    assert_eq!(set.region_count, 0);
    let probes = random_probes(5, 256);
    assert_eq!(run_probes(&ctx, &set, &probes), vec![0_u32; probes.len()]);
}

// ---- the tile-slot CSR ----

/// A leaf whose image polygon is the square `[lo, hi]^2`, kept for every node of `sig`.
fn square_leaf(lo: f64, hi: f64, sig: [u32; 2]) -> crate::renderer::beam::beam::Leaf {
    use glam::{DVec2, Vec3};

    use crate::renderer::beam::{
        beam::{BranchPrefix, Leaf, SpectralInterval},
        geom::polygon::ConvexPolygon,
    };
    let points = [
        DVec2::new(lo, lo),
        DVec2::new(hi, lo),
        DVec2::new(hi, hi),
        DVec2::new(lo, hi),
    ];
    let spectrum = SpectralInterval { first: 0, last: 47 };
    let count = points.len() * spectrum.len();
    Leaf {
        tile: 0,
        image: ConvexPolygon::from_image_points(&points).expect("a polygon"),
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

/// The CPU twin of `kept_cell` for the tile-slot form: the cell of pixel `(x, y)`.
fn tile_slot_cell(packed: &PackedKept, x: u32, y: u32) -> Option<u32> {
    let rows: &[[u32; 4]] = packed.row_table().as_chunks::<4>().0;
    let after = rows.partition_point(|row| (row[0], row[1]) <= (y, x));
    let row = rows.get(after.checked_sub(1)?)?;
    (row[0] == y && x < row[2]).then(|| row[3] + (x - row[1]))
}

#[test]
fn a_sparse_batch_packs_to_the_tile_slot_csr_and_keeps_every_pixel_list() {
    use crate::renderer::beam::{
        primary::TileRect,
        residual::{KeptIndex, kept_footprint, pack_kept},
    };
    let near = TileRect {
        x0: 0,
        y0: 0,
        x1: 4,
        y1: 4,
    };
    let far = TileRect {
        x0: 20,
        y0: 20,
        x1: 24,
        y1: 24,
    };
    let tiles = vec![
        (
            near,
            KeptIndex::from_leaves(near, &[square_leaf(1.0, 3.0, [1, 2])]),
        ),
        (
            far,
            KeptIndex::from_leaves(
                far,
                &[
                    square_leaf(21.0, 23.0, [3, 4]),
                    square_leaf(22.0, 24.0, [5, 6]),
                ],
            ),
        ),
    ];
    let footprint = kept_footprint(&tiles);
    assert_eq!((footprint.tile_pixels, footprint.rows), (32, 8));
    assert_eq!(footprint.bounding_pixels(), 24 * 24);
    assert!(footprint.uses_tile_slots());
    let packed = pack_kept(&tiles);
    // One cell per batch pixel plus the end entry, and a four-word row per tile row.
    assert_eq!(packed.span_count, 8);
    assert_eq!(packed.pixel_offsets.len(), 33);
    assert_eq!(packed.row_table().len(), 32);
    assert_eq!(packed.region_lists().len() as u64, footprint.memberships);
    assert_eq!(
        footprint.bytes(),
        packed.regions.len() as u64 * 128
            + packed.pixel_offsets.len() as u64 * 4
            + packed.indices.len() as u64 * 4
    );
    // The bounding rectangle is still reported; a dense table over it would hold 576 cells.
    assert_eq!((packed.origin, packed.size), ((0, 0), (24, 24)));
    let rows: &[[u32; 4]] = packed.row_table().as_chunks::<4>().0;
    assert!(
        rows.windows(2)
            .all(|w| (w[0][0], w[0][1]) < (w[1][0], w[1][1]))
    );
    // Every pixel of every tile has its own list, and nothing else has a cell.
    let mut base = 0_u32;
    for (rect, index) in &tiles {
        for y in rect.y0..rect.y1 {
            for x in rect.x0..rect.x1 {
                let cell =
                    tile_slot_cell(&packed, x, y).expect("a batch pixel has a cell") as usize;
                let list = &packed.region_lists()
                    [packed.pixel_offsets[cell] as usize..packed.pixel_offsets[cell + 1] as usize];
                let expected: Vec<u32> = index
                    .pixel_regions((x, y))
                    .iter()
                    .map(|region| region + base)
                    .collect();
                assert_eq!(list, expected, "pixel ({x}, {y})");
            }
        }
        base += u32::try_from(index.regions().len()).expect("small");
    }
    for (x, y) in [(10, 10), (4, 0), (0, 4), (24, 24), (19, 20), (24, 20)] {
        assert_eq!(tile_slot_cell(&packed, x, y), None, "pixel ({x}, {y})");
    }
    assert_eq!(
        kept_params(&packed, N_LAMBDA),
        Some(KeptParams {
            origin: [0, 0],
            size: [0, 0],
            n_lambda: N_LAMBDA,
            region_count: 3,
            spans: [
                8,
                u32::try_from(packed.region_lists().len()).expect("small")
            ],
        })
    );
    // A row that points past the offsets table is refused.
    let mut broken = packed.clone();
    let last = broken.indices.len() - 1;
    broken.indices[last] = 1000;
    assert_eq!(kept_params(&broken, N_LAMBDA), None);
}

/// `dense` (a bounding-rectangle CSR) rebuilt as the tile-slot form of `tiles`, each
/// `(x0, y0, x1, y1)` inside the dense rectangle.
fn to_tile_slots(dense: &PackedKept, tiles: &[(u32, u32, u32, u32)]) -> PackedKept {
    let (origin_x, origin_y) = dense.origin;
    let mut offsets = vec![0_u32];
    let mut lists: Vec<u32> = Vec::new();
    let mut rows: Vec<[u32; 4]> = Vec::new();
    for &(x0, y0, x1, y1) in tiles {
        for y in y0..y1 {
            let first = u32::try_from(offsets.len() - 1).expect("small");
            rows.push([y, x0, x1, first]);
            for x in x0..x1 {
                let cell = ((y - origin_y) * dense.size.0 + (x - origin_x)) as usize;
                let (start, end) = (
                    dense.pixel_offsets[cell] as usize,
                    dense.pixel_offsets[cell + 1] as usize,
                );
                lists.extend_from_slice(&dense.indices[start..end]);
                offsets.push(u32::try_from(lists.len()).expect("small"));
            }
        }
    }
    rows.sort_unstable();
    let span_count = u32::try_from(rows.len()).expect("small");
    lists.extend(rows.into_iter().flatten());
    PackedKept {
        regions: dense.regions.clone(),
        origin: dense.origin,
        size: dense.size,
        pixel_offsets: offsets,
        indices: lists,
        span_count,
    }
}

#[test]
fn the_gpu_tile_slot_kept_test_equals_the_dense_one_inside_the_tiles() {
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                std::env::var(REQUIRE_GPU_ENV).is_err(),
                "{REQUIRE_GPU_ENV} is set but no adapter is available: {error:?}"
            );
            eprintln!("skipping the tile-slot GPU kept test: no adapter ({error:?})");
            return;
        }
    };
    let dense = scenario(2026);
    let tiles = [(3, 2, 6, 5), (8, 4, 11, 8)];
    let sparse = to_tile_slots(&dense, &tiles);
    let set = KeptBufferSet::upload(&ctx, &sparse, N_LAMBDA).expect("the sparse scenario fits");
    let probes = random_probes(77, 20_000);
    let gpu = run_probes(&ctx, &set, &probes);
    let (mut kept, mut mismatched) = (0_usize, 0_usize);
    for (index, (probe, &verdict)) in probes.iter().zip(&gpu).enumerate() {
        let [px, py] = probe.pixel;
        let inside = tiles
            .iter()
            .any(|&(x0, y0, x1, y1)| (x0..x1).contains(&px) && (y0..y1).contains(&py));
        let (dense_kept, ambiguous) = twin_kept(&dense, N_LAMBDA, probe);
        let expected = dense_kept && inside;
        kept += usize::from(expected);
        if (verdict == 1) != expected && !ambiguous {
            mismatched += 1;
            eprintln!("probe {index}: gpu {verdict} cpu {expected}: {probe:?}");
        }
    }
    assert_eq!(mismatched, 0);
    assert!(kept > 0, "the tiles keep some probes");
}

#[test]
fn two_tiles_with_regions_at_opposite_8k_corners_pack_a_table_of_tile_pixels() {
    use crate::renderer::beam::{
        primary::TileRect,
        residual::{KeptIndex, kept_footprint, pack_kept, try_pack_kept},
    };
    let first = TileRect {
        x0: 0,
        y0: 0,
        x1: 64,
        y1: 64,
    };
    let second = TileRect {
        x0: 7616,
        y0: 4256,
        x1: 7680,
        y1: 4320,
    };
    let tiles = vec![
        (
            first,
            KeptIndex::from_leaves(first, &[square_leaf(10.0, 20.0, [1, 2])]),
        ),
        (
            second,
            KeptIndex::from_leaves(second, &[square_leaf(7620.0, 7630.0, [3, 4])]),
        ),
    ];
    let footprint = kept_footprint(&tiles);
    assert_eq!(footprint.tile_pixels, 2 * 64 * 64);
    assert_eq!(footprint.bounding_pixels(), 7680 * 4320);
    let packed = pack_kept(&tiles);
    // The offsets table follows the batch's pixels, not the frame's 33 million.
    assert_eq!(packed.pixel_offsets.len(), 2 * 64 * 64 + 1);
    assert_eq!(packed.span_count, 128);
    // The plan made before packing equals the packed buffers, and a budget below the footprint
    // refuses the batch before any table is built.
    assert_eq!(
        super::kept::planned_kept_buffer_bytes(&footprint),
        max_kept_buffer_bytes(&packed)
    );
    assert_eq!(try_pack_kept(&tiles, footprint.bytes()), Some(packed));
    assert_eq!(try_pack_kept(&tiles, footprint.bytes() - 1), None);
    assert!(footprint.bytes() < 1 << 20);
}
