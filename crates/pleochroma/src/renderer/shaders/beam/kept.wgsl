// Beam tracing (beam guide sections 9.2, 12.4, 14.7 item 3): the kept buffer and its membership
// test, the WGSL twin of the CPU kept index.
//
// A batch's kept regions are image-space convex polygons (counter-clockwise, at most 12
// vertices), each with a branch signature and an inclusive node range. A residual sample is
// "kept" (covered by a beam, so the residual drops it) when the region of its signature, node and
// image point exists. A per-pixel CSR lists, for every pixel of the batch (the bounding rectangle
// of its tiles, or for a sparse batch just its own pixels through a row table), the regions whose
// polygon overlaps it (ascending region index).
//
// This unit has no entry point and depends on no other unit: the signature is compared lane by
// lane. It is appended to a transport unit (or a test kernel) by the Rust side
// (`gpu/beam/kept.rs::kept_wgsl`). Bindings 210-213 (the beam range 200-219; 200 is the event
// wavefront's `ray_sig`, 200-204 the reduction kernel's own pipeline).
//
// Conventions matched with the CPU:
//   * bin rule (guide 9.2), identical to `SpectralNode::of_wavelength`:
//       node = min(floor((lambda - 380) * N / 400), N - 1); lambdas below 380 nm give node 0;
//   * polygon inclusion is `ConvexPolygon::contains_2d(q, 0.0)`: q is inside when
//       (b - a) x (q - a) >= 0 for every edge a -> b of the counter-clockwise polygon, so edges
//     and vertices are inside. Fewer than 3 vertices contain nothing.
//
// Layouts (must equal `GpuKeptRegion` and `KeptParams` in Rust):
//   KeptRegion, 128 bytes: polygon 12 x vec2<f32> (96), sig (8), len, first_node, last_node,
//   flags, two pad words.
//   KeptParams, uniform, 32 bytes: CSR origin x y, CSR size x y, n_lambda, region_count,
//   span_count, span_first (the tile-slot form's row table; both 0 for the dense form).

const KEPT_MAX_VERTICES: u32 = 12u;
const KEPT_LAMBDA_MIN_NM: f32 = 380.0;
const KEPT_LAMBDA_SPAN_NM: f32 = 400.0;
// 1 / KEPT_LAMBDA_SPAN_NM as a literal both languages parse to the same f32.
const KEPT_NODES_PER_NM_PER_NODE: f32 = 0.0025;

struct KeptRegion {
    polygon: array<vec2<f32>, 12>,
    sig: vec2<u32>,
    len: u32,
    first_node: u32,
    last_node: u32,
    flags: u32,
    pad_a: u32,
    pad_b: u32,
}

struct KeptParams {
    origin_x: u32,
    origin_y: u32,
    size_x: u32,
    size_y: u32,
    n_lambda: u32,
    region_count: u32,
    span_count: u32,
    span_first: u32,
}

@group(0) @binding(210) var<storage, read> kept_regions: array<KeptRegion>;
@group(0) @binding(211) var<storage, read> kept_pixel_offsets: array<u32>;
@group(0) @binding(212) var<storage, read> kept_indices: array<u32>;
@group(0) @binding(213) var<uniform> kept_params: KeptParams;

// The spectral node of `lambda` (nm) among `n_lambda` nodes: the bin rule of guide section 9.2.
fn kept_node_of(lambda: f32, n_lambda: u32) -> u32 {
    if (n_lambda == 0u) {
        return 0u;
    }
    // Two correctly rounded operations, no division (WGSL allows 2.5 ULP there): the same bits
    // as `SpectralNode::of_wavelength` on the CPU. 0.0025 = 1 / 400 nm.
    let raw = floor((lambda - KEPT_LAMBDA_MIN_NM) * (f32(n_lambda) * KEPT_NODES_PER_NM_PER_NODE));
    // Written so a NaN falls to node 0 like the Rust `as` cast.
    if (!(raw >= 0.0)) {
        return 0u;
    }
    return u32(min(raw, f32(n_lambda - 1u)));
}

// Whether `q` lies in the counter-clockwise convex polygon of region `r`, edges inclusive.
fn kept_contains(r: u32, q: vec2<f32>) -> bool {
    let n = min(kept_regions[r].len, KEPT_MAX_VERTICES);
    if (n < 3u) {
        return false;
    }
    for (var i: u32 = 0u; i < n; i = i + 1u) {
        var j: u32 = i + 1u;
        if (j == n) {
            j = 0u;
        }
        let a = kept_regions[r].polygon[i];
        let b = kept_regions[r].polygon[j];
        let orient = (b.x - a.x) * (q.y - a.y) - (b.y - a.y) * (q.x - a.x);
        if (orient < 0.0) {
            return false;
        }
    }
    return true;
}

// "No cell": the pixel is outside the batch.
const KEPT_NO_CELL: u32 = 0xFFFFFFFFu;

// The CSR cell of `pixel`, or KEPT_NO_CELL outside the batch.
//
// Dense form (span_count == 0): the cell is the pixel's place in the rectangle origin/size,
// row-major.
// Tile-slot form (span_count > 0): the cell is the pixel's slot in the batch's own pixels. The
// row table at kept_indices[span_first ..] has span_count entries of four words [y, x0, x1,
// first cell], one per tile row, sorted by (y, x0); the pixel's row is the last entry whose
// (y, x0) is not after (pixel.y, pixel.x), found by binary search, and the cell is
// `first cell + (pixel.x - x0)` when pixel.x < x1.
fn kept_cell(pixel: vec2<u32>) -> u32 {
    if (kept_params.span_count == 0u) {
        if (pixel.x < kept_params.origin_x || pixel.y < kept_params.origin_y) {
            return KEPT_NO_CELL;
        }
        let local_x = pixel.x - kept_params.origin_x;
        let local_y = pixel.y - kept_params.origin_y;
        if (local_x >= kept_params.size_x || local_y >= kept_params.size_y) {
            return KEPT_NO_CELL;
        }
        return local_y * kept_params.size_x + local_x;
    }
    // Binary search for the first entry whose (y, x0) is after (pixel.y, pixel.x).
    var low: u32 = 0u;
    var high: u32 = kept_params.span_count;
    while (low < high) {
        let mid = low + (high - low) / 2u;
        let at = kept_params.span_first + mid * 4u;
        let row_y = kept_indices[at];
        let row_x0 = kept_indices[at + 1u];
        if (row_y < pixel.y || (row_y == pixel.y && row_x0 <= pixel.x)) {
            low = mid + 1u;
        } else {
            high = mid;
        }
    }
    if (low == 0u) {
        return KEPT_NO_CELL;
    }
    let found = kept_params.span_first + (low - 1u) * 4u;
    if (kept_indices[found] != pixel.y || pixel.x >= kept_indices[found + 2u]) {
        return KEPT_NO_CELL;
    }
    return kept_indices[found + 3u] + (pixel.x - kept_indices[found + 1u]);
}

// The kept test of the reference variant: whether a sample of pixel `pixel` (x, y), at
// `image_point` (pixel units, the pixel centre at (x, y)), with branch signature `sig` and
// wavelength `lambda` is covered by a kept region. Pixels outside the CSR are never kept.
fn beam_kept_test(pixel: vec2<u32>, image_point: vec2<f32>, sig: vec2<u32>, lambda: f32) -> bool {
    let cell = kept_cell(pixel);
    if (cell == KEPT_NO_CELL) {
        return false;
    }
    let first = kept_pixel_offsets[cell];
    let last = kept_pixel_offsets[cell + 1u];
    let node = kept_node_of(lambda, kept_params.n_lambda);
    for (var k: u32 = first; k < last; k = k + 1u) {
        let r = kept_indices[k];
        if (r >= kept_params.region_count) {
            continue;
        }
        let region_sig = kept_regions[r].sig;
        if (region_sig.x != sig.x || region_sig.y != sig.y) {
            continue;
        }
        if (node < kept_regions[r].first_node || node > kept_regions[r].last_node) {
            continue;
        }
        if (kept_contains(r, image_point)) {
            return true;
        }
    }
    return false;
}
