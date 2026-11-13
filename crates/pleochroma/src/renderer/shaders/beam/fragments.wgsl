// Beam fragment kernel (beam guide 14.7 item 2, 11.4, 11.5).
//
// Appended at run time to the generated, zoned-patched prelude and bounce text of the megakernel
// (`gpu/beam/fragments.rs` cuts the text just ahead of `transport_main`), so the environment
// functions it calls are the tracer's own: `sample_environment_with_rig`, `studio_dispatch`,
// `hdr_env_radiance_at`, `studio_spectral_power`, `daylight_sun_factor`, `cie_1931_cmf`.
//
// One invocation per fragment slot (a covered pixel of one leaf). It clips the leaf polygon to the
// half-open pixel cell in `var<workgroup>` scratch (no function-scope arrays: AMD/FXC rule), tests
// the lighting for smoothness at the corners and the centroid, and either writes the centroid-rule
// XYZ into its slot or appends the slot to the refine list. `refine.wgsl` finishes those slots.
//
// Scene bindings (the megakernel's group 0, reduced to what the environment reaches): 0 camera,
// 1 params, 10 hdr_texels, 11 hdr_env_dims. Beam bindings 200-208 (the beam range is 200-219).
//
// Everything is prefixed `bm_` / `Bm` / `BM_` so it cannot collide with the prelude.

struct BmLeafHeader {
    sig0: u32,
    sig1: u32,
    first_vertex: u32,
    vertex_count: u32,
    first_node: u32,
    node_count: u32,
    flags: u32,
    tau_omega: f32,
}

struct BmLeafVertex {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    dz: f32,
    first_node: u32,
}

struct BmLeafNode {
    weight_dx: u32,
    dy_dz: u32,
}

// `FragParams` on the Rust side (32 bytes).
struct BmParams {
    total: u32,
    leaf_count: u32,
    n_lambda: u32,
    max_levels: u32,
    tolerance: f32,
    sigma: f32,
    grid_width: u32,
    slot_base: u32,
}

// Eight f32 lanes (two vec4): a tile of nodes' running sums or extremes.
struct BmLanes {
    a: vec4<f32>,
    b: vec4<f32>,
}

// One result of a piece: either "subdivide me" or the piece's XYZ.
struct BmTerm {
    needs_split: bool,
    xyz: vec3<f32>,
}

@group(0) @binding(200) var<storage, read> bm_headers: array<BmLeafHeader>;
@group(0) @binding(201) var<storage, read> bm_vertices: array<BmLeafVertex>;
@group(0) @binding(202) var<storage, read> bm_nodes: array<BmLeafNode>;
// Per leaf the first fragment slot; one extra entry holds the total.
@group(0) @binding(203) var<storage, read> bm_first_fragment: array<u32>;
// Per leaf its half-open pixel rectangle (x0, y0, x1, y1).
@group(0) @binding(204) var<storage, read> bm_pixel_rects: array<vec4<u32>>;
// The per-fragment output: XYZ and a spare lane.
@group(0) @binding(205) var<storage, read_write> bm_slots: array<vec4<f32>>;
// The slots whose fragment needs the refine pass; the count is `bm_refine_count`.
@group(0) @binding(206) var<storage, read_write> bm_refine_list: array<u32>;
@group(0) @binding(207) var<storage, read_write> bm_refine_count: atomic<u32>;
@group(0) @binding(208) var<uniform> bm_params: BmParams;

// Workgroup scratch: BM_SLOT_STRIDE vec2 slots per invocation, none shared between invocations, so
// no barrier is needed. Per invocation: 0..12 the fragment polygon, 12..24 spare (clip buffer, then
// the refine pieces), 24..32 the leaf polygon in pixel-local coordinates, 32..40 the Wachspress
// weights of the current point (one f32 each, in `.x`). 16 * 40 vec2 = 5120 bytes.
const BM_WG: u32 = 16u;
const BM_SLOT_STRIDE: u32 = 40u;
const BM_ROOT: u32 = 0u;
const BM_SPARE: u32 = 12u;
const BM_LEAF: u32 = 24u;
const BM_WEIGHTS: u32 = 32u;
const BM_MAX_POLY: u32 = 12u;
const BM_MAX_LEAF_VERTICES: u32 = 8u;
var<workgroup> bm_scratch: array<vec2<f32>, 640>;

// Nodes per tile of the smoothness test and the 7-point rule (the lanes of `BmLanes`); the
// smoothness test of a leaf with a per-node environment uses `BM_TILE_SOLO` so a rough piece
// stops after few environment calls.
const BM_TILE: u32 = 8u;
const BM_TILE_SOLO: u32 = 2u;
const BM_BIG: f32 = 3.0e38;

const BM_FLAG_GLARE: u32 = 1u;
const BM_FLAG_REFLECTION: u32 = 2u;
const BM_FLAG_DELTAS: u32 = 4u;

// Wachspress: a point within this fraction of the leaf extent of an edge is interpolated linearly
// along it (the CPU twin uses 1e-12 in f64).
const BM_EDGE_EPS: f32 = 2.0e-6;
const BM_EDGE_T: f32 = 1.0e-5;
// Slack on the sun early-out and the covered-area sanity bounds.
const BM_SUN_SLACK: f32 = 1.0e-6;
// The sky of a direction inside the sun disc is looked up at this multiple of the disc radius.
const BM_SKY_NUDGE: f32 = 1.05;
const BM_SKY_NUDGE_BAND: f32 = 1.0e-6;

// Radon's 7-point triangle rule (degree 5), the same constants as `RADON_7` on the CPU.
const BM_SQRT15: f32 = 3.872983346207417;
const BM_RADON_A1: f32 = (6.0 - BM_SQRT15) / 21.0;
const BM_RADON_A2: f32 = (6.0 + BM_SQRT15) / 21.0;
const BM_RADON_B1: f32 = 1.0 - 2.0 * BM_RADON_A1;
const BM_RADON_B2: f32 = 1.0 - 2.0 * BM_RADON_A2;
const BM_RADON_W0: f32 = 9.0 / 40.0;
const BM_RADON_W1: f32 = (155.0 - BM_SQRT15) / 1200.0;
const BM_RADON_W2: f32 = (155.0 + BM_SQRT15) / 1200.0;

// Per-invocation state of the fragment being lit.
var<private> bm_base: u32 = 0u;
var<private> bm_pc: vec2<f32> = vec2<f32>(0.0, 0.0);
var<private> bm_vfirst: u32 = 0u;
var<private> bm_vcount: u32 = 0u;
var<private> bm_nfirst: u32 = 0u;
var<private> bm_ncount: u32 = 0u;
var<private> bm_flags: u32 = 0u;
var<private> bm_size: f32 = 1.0;
var<private> bm_key: vec3<f32> = vec3<f32>(0.0, 1.0, 0.0);
var<private> bm_fill: vec3<f32> = vec3<f32>(0.0, 1.0, 0.0);
var<private> bm_sin_lp: f32 = 0.0;
var<private> bm_sun_on: bool = false;
var<private> bm_sun_t1: vec3<f32> = vec3<f32>(1.0, 0.0, 0.0);
var<private> bm_sun_t2: vec3<f32> = vec3<f32>(0.0, 0.0, 1.0);
var<private> bm_tan_r: f32 = 1.0;
var<private> bm_radius: f32 = 1.0;
var<private> bm_chord: f32 = 1.0;
var<private> bm_nudge: vec2<f32> = vec2<f32>(0.0, 1.0);

fn bm_next(i: u32, n: u32) -> u32 {
    if (i + 1u == n) {
        return 0u;
    }
    return i + 1u;
}

fn bm_prev(i: u32, n: u32) -> u32 {
    if (i == 0u) {
        return n - 1u;
    }
    return i - 1u;
}

fn bm_cross(a: vec2<f32>, b: vec2<f32>) -> f32 {
    return a.x * b.y - a.y * b.x;
}

// Vertex `i` of the polygon stored at scratch offset `off`.
fn bm_sv(off: u32, i: u32) -> vec2<f32> {
    return bm_scratch[bm_base + off + i];
}

fn bm_set(off: u32, i: u32, p: vec2<f32>) {
    bm_scratch[bm_base + off + i] = p;
}

// ---- polygon helpers ----

fn bm_poly_twice_area(off: u32, n: u32) -> f32 {
    let o = bm_sv(off, 0u);
    var total = 0.0;
    for (var i = 1u; i + 1u < n; i = i + 1u) {
        total = total + bm_cross(bm_sv(off, i) - o, bm_sv(off, i + 1u) - o);
    }
    return total;
}

fn bm_poly_centroid(off: u32, n: u32) -> vec2<f32> {
    let o = bm_sv(off, 0u);
    var twice = 0.0;
    var weighted = vec2<f32>(0.0, 0.0);
    for (var i = 1u; i + 1u < n; i = i + 1u) {
        let a = bm_sv(off, i) - o;
        let b = bm_sv(off, i + 1u) - o;
        let cr = bm_cross(a, b);
        twice = twice + cr;
        weighted = weighted + (a + b) * cr;
    }
    if (abs(twice) > 0.0) {
        return o + weighted / (3.0 * twice);
    }
    var mean = vec2<f32>(0.0, 0.0);
    for (var i = 0u; i < n; i = i + 1u) {
        mean = mean + bm_sv(off, i);
    }
    return mean / f32(max(n, 1u));
}

fn bm_inside(p: vec2<f32>, axis: u32, bound: f32, keep_ge: bool) -> bool {
    let value = select(p.x, p.y, axis == 1u);
    if (keep_ge) {
        return value >= bound;
    }
    return value < bound;
}

// Sutherland-Hodgman against one axis-aligned half plane, from scratch offset `src` to `dst`.
// The pixel cell is half open: a lower bound keeps `>=`, an upper bound keeps `<`.
fn bm_clip(n: u32, src: u32, dst: u32, axis: u32, bound: f32, keep_ge: bool) -> u32 {
    if (n == 0u) {
        return 0u;
    }
    var count = 0u;
    var prev = bm_sv(src, n - 1u);
    var prev_in = bm_inside(prev, axis, bound, keep_ge);
    for (var i = 0u; i < n; i = i + 1u) {
        let cur = bm_sv(src, i);
        let cur_in = bm_inside(cur, axis, bound, keep_ge);
        if (cur_in != prev_in && count < BM_MAX_POLY) {
            let a = select(prev.x, prev.y, axis == 1u);
            let b = select(cur.x, cur.y, axis == 1u);
            let t = (bound - a) / (b - a);
            var q = prev + (cur - prev) * t;
            if (axis == 0u) {
                q.x = bound;
            } else {
                q.y = bound;
            }
            bm_set(dst, count, q);
            count = count + 1u;
        }
        if (cur_in && count < BM_MAX_POLY) {
            bm_set(dst, count, cur);
            count = count + 1u;
        }
        prev = cur;
        prev_in = cur_in;
    }
    return count;
}

// ---- the leaf field: Wachspress interpolation of the exit direction and weight ----

// Direction (xyz, unnormalised) and weight (w) of leaf vertex `i`, node `k` of the interval.
fn bm_vertex_field(i: u32, k: u32) -> vec4<f32> {
    let v = bm_vertices[bm_vfirst + i];
    let node = bm_nodes[v.first_node + k];
    let wd = unpack2x16float(node.weight_dx);
    var dir = vec3<f32>(v.dx, v.dy, v.dz);
    if ((bm_flags & BM_FLAG_DELTAS) != 0u) {
        let yz = unpack2x16float(node.dy_dz);
        dir = dir + vec3<f32>(wd.y, yz.x, yz.y);
    }
    return vec4<f32>(dir, wd.x);
}

fn bm_lv(i: u32) -> vec2<f32> {
    return bm_sv(BM_LEAF, i);
}

// Area of the triangle (p, v_i, v_i+1) of the leaf polygon.
fn bm_cap(i: u32, n: u32, p: vec2<f32>) -> f32 {
    return 0.5 * bm_cross(bm_lv(i) - p, bm_lv(bm_next(i, n)) - p);
}

// Area of the triangle (v_i-1, v_i, v_i+1).
fn bm_corner(i: u32, n: u32) -> f32 {
    let o = bm_lv(bm_prev(i, n));
    return 0.5 * bm_cross(bm_lv(i) - o, bm_lv(bm_next(i, n)) - o);
}

// The unnormalised Wachspress weight of vertex `i` at `p`.
fn bm_wach_raw(i: u32, n: u32, p: vec2<f32>) -> f32 {
    return bm_corner(i, n) / (bm_cap(bm_prev(i, n), n, p) * bm_cap(i, n, p));
}

fn bm_set_weight(i: u32, w: f32) {
    bm_scratch[bm_base + BM_WEIGHTS + i] = vec2<f32>(w, 0.0);
}

fn bm_weight(i: u32) -> f32 {
    return bm_scratch[bm_base + BM_WEIGHTS + i].x;
}

// The Wachspress weights of the leaf at the pixel-local point `p`, normalised to sum to one, into
// the weights scratch. A point on an edge gets the linear blend of its two ends (the edge rule);
// a degenerate sum falls back to vertex 0. The weights depend on the point only, so a point's
// nodes all read them through `bm_field_from_weights` / `bm_weight_at`.
fn bm_wachspress(p: vec2<f32>) {
    let n = bm_vcount;
    for (var i = 0u; i < n; i = i + 1u) {
        let j = bm_next(i, n);
        let a = bm_lv(i);
        let edge = bm_lv(j) - a;
        let len2 = dot(edge, edge);
        if (len2 > 0.0) {
            let rel = p - a;
            let h = abs(bm_cross(edge, rel)) / sqrt(len2);
            let t = dot(rel, edge) / len2;
            if (h <= BM_EDGE_EPS * bm_size && t >= -BM_EDGE_T && t <= 1.0 + BM_EDGE_T) {
                let tt = clamp(t, 0.0, 1.0);
                for (var m = 0u; m < n; m = m + 1u) {
                    bm_set_weight(m, 0.0);
                }
                bm_set_weight(i, 1.0 - tt);
                bm_set_weight(j, tt);
                return;
            }
        }
    }
    var total = 0.0;
    for (var i = 0u; i < n; i = i + 1u) {
        let raw = bm_wach_raw(i, n, p);
        bm_set_weight(i, raw);
        total = total + raw;
    }
    if (!(abs(total) > 1.0e-30 && abs(total) < 1.0e30)) {
        for (var m = 0u; m < n; m = m + 1u) {
            bm_set_weight(m, 0.0);
        }
        bm_set_weight(0u, 1.0);
        return;
    }
    for (var i = 0u; i < n; i = i + 1u) {
        bm_set_weight(i, bm_weight(i) / total);
    }
}

// Field of node `k` at the point whose weights are in the scratch: the interpolated direction
// (unnormalised) in xyz and the interpolated weight in w.
fn bm_field_from_weights(k: u32) -> vec4<f32> {
    var acc = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var i = 0u; i < bm_vcount; i = i + 1u) {
        let w = bm_weight(i);
        if (w != 0.0) {
            acc = acc + bm_vertex_field(i, k) * w;
        }
    }
    return acc;
}

// Only the interpolated weight of node `k` (the direction of a leaf without deltas is the same for
// every node).
fn bm_weight_at(k: u32) -> f32 {
    var acc = 0.0;
    for (var i = 0u; i < bm_vcount; i = i + 1u) {
        let w = bm_weight(i);
        if (w != 0.0) {
            let v = bm_vertices[bm_vfirst + i];
            let node = bm_nodes[v.first_node + k];
            acc = acc + unpack2x16float(node.weight_dx).x * w;
        }
    }
    return acc;
}

// Field of the leaf at the pixel-local point `p` for node `k`: the interpolated direction
// (unnormalised) in xyz and the interpolated weight in w.
fn bm_field(p: vec2<f32>, k: u32) -> vec4<f32> {
    bm_wachspress(p);
    return bm_field_from_weights(k);
}

fn bm_unit_dir(p: vec2<f32>, k: u32) -> vec3<f32> {
    let f = bm_field(p, k).xyz;
    let l2 = dot(f, f);
    if (!(l2 > 0.0)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    return f * inverseSqrt(l2);
}

// ---- lighting ----

// `-camera_dir(x)` at the image point `s` (the tracer's `generate_camera_ray`, continuous).
fn bm_observer(s: vec2<f32>) -> vec3<f32> {
    let aspect = camera.width / camera.height;
    let u = (s.x / camera.width - 0.5) * 2.0 * aspect * camera.fov_tan;
    let v = (0.5 - s.y / camera.height) * 2.0 * camera.fov_tan;
    return -normalize(camera.forward + camera.right * u + camera.up * v);
}

// The node centre in nm: `380 + (first + k + 1/2) * 400 / n_lambda`.
fn bm_lambda(k: u32) -> f32 {
    return fma(f32(bm_nfirst + k) + 0.5, 400.0 / f32(max(bm_params.n_lambda, 1u)), 380.0);
}

fn bm_cmf(k: u32) -> vec3<f32> {
    return cie_1931_cmf(bm_lambda(k));
}

// `(400 / N_lambda) / 106.856`, the node width over the CIE normalisation.
fn bm_norm() -> f32 {
    return (400.0 / f32(max(bm_params.n_lambda, 1u))) / 106.856;
}

// glam's `Vec3::any_orthonormal_vector` of the key (the CPU twin's fallback tangent of the nudge).
fn bm_key_orthonormal() -> vec3<f32> {
    let sgn = select(1.0, -1.0, bm_key.z < 0.0);
    let a = -1.0 / (sgn + bm_key.z);
    let b = bm_key.x * bm_key.y * a;
    return vec3<f32>(b, sgn + bm_key.y * bm_key.y * a, -bm_key.y);
}

// `v` normalised, or zero for a zero vector.
fn bm_safe_unit(v: vec3<f32>) -> vec3<f32> {
    let l2 = dot(v, v);
    if (!(l2 > 0.0)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    return v * inverseSqrt(l2);
}

// A direction inside the sun disc is moved radially to just outside it, so the tracer's function
// returns the sky without the disc (the disc is integrated exactly by `bm_sun_piece`).
fn bm_sky_dir(d: vec3<f32>) -> vec3<f32> {
    if (!bm_sun_on) {
        return d;
    }
    let c = dot(d, bm_key);
    if (c < SUN_DISC_COS - BM_SKY_NUDGE_BAND) {
        return d;
    }
    let off = d - bm_key * c;
    let off_len = length(off);
    var tangent = bm_key_orthonormal();
    if (off_len > 1.0e-7) {
        tangent = off / off_len;
    }
    return normalize(bm_key * bm_nudge.y + tangent * bm_nudge.x);
}

fn bm_glare() -> f32 {
    if ((bm_flags & BM_FLAG_GLARE) != 0u) {
        return params.surface_glare;
    }
    return 1.0;
}

// `I * glare * L` for the unit direction `dir` and weight `weight` at the local point `p`.
fn bm_integrand_dir(p: vec2<f32>, dir: vec3<f32>, weight: f32, k: u32) -> f32 {
    let observer = bm_observer(bm_pc + p);
    let radiance = sample_environment_with_rig(
        bm_sky_dir(dir), bm_lambda(k), bm_key, bm_fill, bm_sin_lp, observer,
    );
    return max(weight, 0.0) * bm_glare() * radiance;
}

// Whether the environment of this leaf is one radiance row per point: the nodes share one exit
// direction (no dispersion) and the studio rig is linear in the spectral power. Not the ASET model
// (it reads the wavelength itself), the flat environment or the HDR map (both per node).
fn bm_row_shared() -> bool {
    return (bm_flags & BM_FLAG_DELTAS) == 0u && params.env_mode == 1u && params.studio_model != 4u;
}

// The studio radiance along `d` at unit spectral power (the CPU `radiance_row`'s shared part).
fn bm_radiance_row(d: vec3<f32>, observer: vec3<f32>) -> f32 {
    return studio_dispatch(
        params.studio_model,
        normalize(d),
        1.0,
        params.studio_spot_mult,
        params.studio_exposure,
        bm_key,
        bm_fill,
        bm_sin_lp,
        params.studio_light_yaw,
        observer,
        vec2<f32>(params.head_shadow_outer_cos, params.head_shadow_inner_cos),
        vec4<f32>(params.tent_walls, params.tent_cards, params.tent_spark, params.tent_ground),
        params.tent_flat,
        params.backdrop,
    );
}

// The shared radiance row at the local point `p` (its weights are in the scratch): zero when the
// exit direction is degenerate, as the integrand is then zero for every node.
fn bm_point_row(p: vec2<f32>) -> f32 {
    let f = bm_field_from_weights(0u);
    let l2 = dot(f.xyz, f.xyz);
    if (!(l2 > 0.0)) {
        return 0.0;
    }
    return bm_radiance_row(bm_sky_dir(f.xyz * inverseSqrt(l2)), bm_observer(bm_pc + p));
}

// The integrand `I(x, k) * glare * L(omega(x, k), k)` of node `k` at the local point `p`, whose
// Wachspress weights are in the scratch (`bm_wachspress`). With `shared_row` the radiance is
// `row * spec` (`bm_point_row`, the node's spectral power); otherwise it is looked up per node.
fn bm_value(p: vec2<f32>, k: u32, row: f32, spec: f32, shared_row: bool) -> f32 {
    if (shared_row) {
        return max(bm_weight_at(k), 0.0) * bm_glare() * (row * spec);
    }
    let f = bm_field_from_weights(k);
    let l2 = dot(f.xyz, f.xyz);
    if (!(l2 > 0.0)) {
        return 0.0;
    }
    return bm_integrand_dir(p, f.xyz * inverseSqrt(l2), f.w, k);
}

// The studio spectral power of the nodes `first..first + BM_TILE` (lane `m` is node `first + m`).
fn bm_spec_lanes(first: u32) -> BmLanes {
    var res = BmLanes(vec4<f32>(0.0), vec4<f32>(0.0));
    for (var m = 0u; m < BM_TILE; m = m + 1u) {
        let k = first + m;
        if (k < bm_ncount) {
            res = bm_lanes_set(res, m, studio_spectral_power(bm_lambda(k)));
        }
    }
    return res;
}

// ---- eight lanes in two vec4 (no function-scope arrays) ----

fn bm_lane_mask(j: u32) -> vec4<bool> {
    return vec4<u32>(0u, 1u, 2u, 3u) == vec4<u32>(j);
}

fn bm_lane(v: vec4<f32>, j: u32) -> f32 {
    var res = v.x;
    if (j == 1u) {
        res = v.y;
    } else if (j == 2u) {
        res = v.z;
    } else if (j == 3u) {
        res = v.w;
    }
    return res;
}

fn bm_lanes_get(l: BmLanes, m: u32) -> f32 {
    if (m < 4u) {
        return bm_lane(l.a, m);
    }
    return bm_lane(l.b, m - 4u);
}

fn bm_lanes_set(l: BmLanes, m: u32, v: f32) -> BmLanes {
    if (m < 4u) {
        return BmLanes(select(l.a, vec4<f32>(v), bm_lane_mask(m)), l.b);
    }
    return BmLanes(l.a, select(l.b, vec4<f32>(v), bm_lane_mask(m - 4u)));
}

fn bm_lanes_add(l: BmLanes, m: u32, v: f32) -> BmLanes {
    if (m < 4u) {
        return BmLanes(select(l.a, l.a + vec4<f32>(v), bm_lane_mask(m)), l.b);
    }
    return BmLanes(l.a, select(l.b, l.b + vec4<f32>(v), bm_lane_mask(m - 4u)));
}

fn bm_lanes_min(l: BmLanes, m: u32, v: f32) -> BmLanes {
    if (m < 4u) {
        return BmLanes(select(l.a, min(l.a, vec4<f32>(v)), bm_lane_mask(m)), l.b);
    }
    return BmLanes(l.a, select(l.b, min(l.b, vec4<f32>(v)), bm_lane_mask(m - 4u)));
}

fn bm_lanes_max(l: BmLanes, m: u32, v: f32) -> BmLanes {
    if (m < 4u) {
        return BmLanes(select(l.a, max(l.a, vec4<f32>(v)), bm_lane_mask(m)), l.b);
    }
    return BmLanes(l.a, select(l.b, max(l.b, vec4<f32>(v)), bm_lane_mask(m - 4u)));
}

// ---- the sun disc, exactly (guide 11.4 step 3) ----

// The signed area of the triangle (0, a + d t0, a + d t1) intersected with the unit disc.
fn bm_unit_piece(a: vec2<f32>, d: vec2<f32>, t0: f32, t1: f32) -> f32 {
    let p = a + d * t0;
    let q = a + d * t1;
    let mid = (p + q) * 0.5;
    let cr = bm_cross(p, q);
    if (dot(mid, mid) <= 1.0) {
        return 0.5 * cr;
    }
    let dp = dot(p, q);
    // atan2(0, 0) is undefined (a vertex on the ellipse centre): no sector there.
    if (cr == 0.0 && dp == 0.0) {
        return 0.0;
    }
    return 0.5 * atan2(cr, dp);
}

// The signed area of the triangle (0, a, b) intersected with the unit disc.
fn bm_unit_segment(a: vec2<f32>, b: vec2<f32>) -> f32 {
    let d = b - a;
    let dd = dot(d, d);
    var ta = 2.0;
    var tb = 2.0;
    if (dd > 0.0) {
        let ad = dot(a, d);
        let disc = ad * ad - dd * (dot(a, a) - 1.0);
        if (disc > 0.0) {
            let root = sqrt(disc);
            let r0 = (-ad - root) / dd;
            let r1 = (-ad + root) / dd;
            if (r0 > 0.0 && r0 < 1.0) {
                ta = r0;
            }
            if (r1 > 0.0 && r1 < 1.0) {
                if (ta > 1.5) {
                    ta = r1;
                } else {
                    tb = r1;
                }
            }
        }
    }
    var prev_t = 0.0;
    var total = 0.0;
    if (ta < 1.5) {
        total = total + bm_unit_piece(a, d, prev_t, ta);
        prev_t = ta;
    }
    if (tb < 1.5) {
        total = total + bm_unit_piece(a, d, prev_t, tb);
        prev_t = tb;
    }
    return total + bm_unit_piece(a, d, prev_t, 1.0);
}

// The three vertices (indices) of the largest triangle of the `n` points at scratch offset `off`.
fn bm_best_triple(off: u32, n: u32) -> vec3<u32> {
    var best = vec3<u32>(0u, 1u, 2u);
    var best_area = -1.0;
    for (var i = 0u; i < n; i = i + 1u) {
        for (var j = i + 1u; j < n; j = j + 1u) {
            for (var m = j + 1u; m < n; m = m + 1u) {
                let a = bm_sv(off, i);
                let twice = abs(bm_cross(bm_sv(off, j) - a, bm_sv(off, m) - a));
                if (twice > best_area) {
                    best_area = twice;
                    best = vec3<u32>(i, j, m);
                }
            }
        }
    }
    return best;
}

// The piece's area times the share of its sample directions (the centroid's unit direction
// `unit_c` and the `n` vertices) inside the sun disc, for node `k`. The fallback of
// `bm_sun_cover` when the affine fit degenerates: all inside = fully covered, none = zero.
fn bm_sun_share(off: u32, n: u32, unit_c: vec3<f32>, k: u32, area: f32) -> f32 {
    var inside = 0.0;
    if (length(unit_c - bm_key) <= bm_chord) {
        inside = inside + 1.0;
    }
    for (var j = 0u; j < n; j = j + 1u) {
        let fj = bm_field(bm_sv(off, j), k).xyz;
        let lj = length(fj);
        if (lj > 0.0 && length(fj / lj - bm_key) <= bm_chord) {
            inside = inside + 1.0;
        }
    }
    return area * inside / f32(n + 1u);
}

// How the piece (scratch offset `off`, `n` vertices) lies against the disc, in the unit-disc frame
// set by `c`, `centre`, `l11`, `l21`, `l22` and `inv_root` (as the sector sum uses them): 1 when
// every vertex is inside (a convex piece is then wholly covered), 2 when no edge reaches the disc
// and the disc centre is outside the piece (nothing is covered), 0 when the rim crosses the piece.
// The exact cases skip the sector sum, whose f32 cancellation is about 1e-7 of the ellipse area,
// enough to light empty pixels of a sun scene.
fn bm_unit_relation(off: u32, n: u32, c: vec2<f32>, centre: vec2<f32>, l11: f32, l21: f32, l22: f32, inv_root: f32) -> u32 {
    var all_inside = true;
    var rim_clear = true;
    var left = false;
    var right = false;
    for (var j = 0u; j < n; j = j + 1u) {
        let d0 = bm_sv(off, j) - c - centre;
        let d1 = bm_sv(off, bm_next(j, n)) - c - centre;
        let u0 = vec2<f32>(l11 * d0.x + l21 * d0.y, l22 * d0.y) * inv_root;
        let u1 = vec2<f32>(l11 * d1.x + l21 * d1.y, l22 * d1.y) * inv_root;
        if (dot(u0, u0) > 1.0) {
            all_inside = false;
        }
        // The closest point of the edge to the disc centre.
        let e = u1 - u0;
        let ee = dot(e, e);
        var t = 0.0;
        if (ee > 0.0) {
            t = clamp(-dot(u0, e) / ee, 0.0, 1.0);
        }
        let q = u0 + e * t;
        if (dot(q, q) < 1.0) {
            rim_clear = false;
        }
        // The side of the edge the centre is on; mixed sides put it outside the convex piece.
        let side = bm_cross(e, -u0);
        if (side > 0.0) {
            left = true;
        }
        if (side < 0.0) {
            right = true;
        }
    }
    if (all_inside) {
        return 1u;
    }
    if (rim_clear && left && right) {
        return 2u;
    }
    return 0u;
}

// The area of the sun disc's preimage inside the piece (scratch offset `off`, `n` vertices,
// centroid `c`) for node `k`, as `(area, needs_split)`.
//
// The exit direction is fitted by an affine field about the centroid: exactly, through the leaf's
// largest vertex triangle, for a reflection leaf (the unnormalised direction is affine in the
// image); through the piece's own largest triangle for a refracted leaf, which then asks for a
// split when the fit misses the field by more than `tau_L` of the disc radius.
fn bm_sun_cover(off: u32, n: u32, c: vec2<f32>, area: f32, k: u32, level: u32) -> vec2<f32> {
    let none = vec2<f32>(0.0, 0.0);
    let fc = bm_field(c, k);
    let len_c = length(fc.xyz);
    if (!(len_c > 0.0)) {
        return none;
    }
    let unit_c = fc.xyz / len_c;
    var spread = 0.0;
    for (var j = 0u; j < n; j = j + 1u) {
        let fj = bm_field(bm_sv(off, j), k).xyz;
        let lj = length(fj);
        if (lj > 0.0) {
            spread = max(spread, length(fj / lj - unit_c));
        }
    }
    if (length(unit_c - bm_key) > fma(2.0, spread, bm_chord) + BM_SUN_SLACK) {
        return none;
    }
    let reflection = (bm_flags & BM_FLAG_REFLECTION) != 0u;
    var tri = vec3<u32>(0u, 1u, 2u);
    var pa = vec2<f32>(0.0, 0.0);
    var pb = vec2<f32>(0.0, 0.0);
    var pcc = vec2<f32>(0.0, 0.0);
    var da = vec3<f32>(0.0, 0.0, 0.0);
    var db = vec3<f32>(0.0, 0.0, 0.0);
    var dc3 = vec3<f32>(0.0, 0.0, 0.0);
    if (reflection) {
        tri = bm_best_triple(BM_LEAF, bm_vcount);
        pa = bm_lv(tri.x);
        pb = bm_lv(tri.y);
        pcc = bm_lv(tri.z);
        da = bm_vertex_field(tri.x, k).xyz;
        db = bm_vertex_field(tri.y, k).xyz;
        dc3 = bm_vertex_field(tri.z, k).xyz;
    } else {
        tri = bm_best_triple(off, n);
        pa = bm_sv(off, tri.x);
        pb = bm_sv(off, tri.y);
        pcc = bm_sv(off, tri.z);
        da = bm_field(pa, k).xyz;
        db = bm_field(pb, k).xyz;
        dc3 = bm_field(pcc, k).xyz;
    }
    let e1 = pb - pa;
    let e2 = pcc - pa;
    let det0 = bm_cross(e1, e2);
    // Scale-invariant degeneracy test: twice the triangle's area against its longest squared edge,
    // as in the CPU twin's normalised fit. A flat triangle takes the fallback: the share of the
    // sample directions inside the disc (all inside = fully covered), never zero for a fragment
    // that points into the disc.
    let longest = max(dot(e1, e1), dot(e2, e2));
    if (!(abs(det0) > 1.0e-6 * longest)) {
        return vec2<f32>(bm_sun_share(off, n, unit_c, k, area), 0.0);
    }
    let dd1 = db - da;
    let dd2 = dc3 - da;
    let gx = (dd1 * e2.y - dd2 * e1.y) / det0;
    let gy = (dd2 * e1.x - dd1 * e2.x) / det0;
    var d_c = fc.xyz;
    if (reflection) {
        d_c = da + gx * (c.x - pa.x) + gy * (c.y - pa.y);
    } else {
        var residual = 0.0;
        for (var j = 0u; j < n; j = j + 1u) {
            let pj = bm_sv(off, j);
            let model = d_c + gx * (pj.x - c.x) + gy * (pj.y - c.y);
            // Residual on unit directions, as the CPU twin fits them.
            residual = max(residual, length(bm_safe_unit(model) - bm_safe_unit(bm_field(pj, k).xyz)));
        }
        if (residual > bm_params.tolerance * bm_radius && level < bm_params.max_levels) {
            return vec2<f32>(0.0, 1.0);
        }
    }
    // The cone |t|^2 <= tan^2 r * (s . d)^2 with d affine in the local coordinates, scaled by
    // 1 / tan r so that every coefficient is of order one.
    let it = 1.0 / bm_tan_r;
    let c1 = dot(bm_sun_t1, d_c) * it;
    let c2 = dot(bm_sun_t2, d_c) * it;
    let cs = dot(bm_key, d_c);
    let gx1 = dot(bm_sun_t1, gx) * it;
    let gx2 = dot(bm_sun_t2, gx) * it;
    let gxs = dot(bm_key, gx);
    let gy1 = dot(bm_sun_t1, gy) * it;
    let gy2 = dot(bm_sun_t2, gy) * it;
    let gys = dot(bm_key, gy);
    let qa = gx1 * gx1 + gx2 * gx2 - gxs * gxs;
    let qb = gx1 * gy1 + gx2 * gy2 - gxs * gys;
    let qc = gy1 * gy1 + gy2 * gy2 - gys * gys;
    let qd = gx1 * c1 + gx2 * c2 - gxs * cs;
    let qe = gy1 * c1 + gy2 * c2 - gys * cs;
    let qf = c1 * c1 + c2 * c2 - cs * cs;
    let det = qa * qc - qb * qb;
    if (!(qa > 0.0 && det > 0.0)) {
        // A rank-deficient map (a constant direction, say) has no preimage ellipse: the sample
        // share, as the CPU twin's `sample_share_area`.
        return vec2<f32>(bm_sun_share(off, n, unit_c, k, area), 0.0);
    }
    let centre = vec2<f32>(-(qc * qd - qb * qe) / det, -(qa * qe - qb * qd) / det);
    let kappa = -(qd * centre.x + qe * centre.y) - qf;
    if (!(kappa > 0.0)) {
        return none;
    }
    if (cs + gxs * centre.x + gys * centre.y <= 0.0) {
        return none;
    }
    let l11 = sqrt(qa);
    let l21 = qb / l11;
    let l22 = sqrt(det / qa);
    let inv_root = inverseSqrt(kappa);
    let relation = bm_unit_relation(off, n, c, centre, l11, l21, l22, inv_root);
    if (relation == 1u) {
        return vec2<f32>(area, 0.0);
    }
    if (relation == 2u) {
        return none;
    }
    var signed_total = 0.0;
    for (var j = 0u; j < n; j = j + 1u) {
        let d0 = bm_sv(off, j) - c - centre;
        let d1 = bm_sv(off, bm_next(j, n)) - c - centre;
        let u0 = vec2<f32>(l11 * d0.x + l21 * d0.y, l22 * d0.y) * inv_root;
        let u1 = vec2<f32>(l11 * d1.x + l21 * d1.y, l22 * d1.y) * inv_root;
        signed_total = signed_total + bm_unit_segment(u0, u1);
    }
    let covered = abs(signed_total) * kappa / sqrt(det);
    // A non-finite total would be turned into `area` (a fully covered piece) by the min below.
    if (!(covered == covered)) {
        return none;
    }
    return vec2<f32>(min(covered, area), 0.0);
}

// The sun disc's contribution to the piece, added to the sky: `glare * sun radiance * I *
// covered area` per node, in XYZ.
fn bm_sun_piece(off: u32, n: u32, c: vec2<f32>, area: f32, level: u32) -> BmTerm {
    var term = BmTerm(false, vec3<f32>(0.0, 0.0, 0.0));
    if (!bm_sun_on) {
        return term;
    }
    var glare = 1.0;
    if ((bm_flags & BM_FLAG_GLARE) != 0u) {
        glare = params.surface_glare;
    }
    let factor = daylight_sun_factor(bm_key);
    let norm = bm_norm();
    let dispersive = (bm_flags & BM_FLAG_DELTAS) != 0u;
    var cover = vec2<f32>(0.0, 0.0);
    for (var k = 0u; k < bm_ncount; k = k + 1u) {
        if (k == 0u || dispersive) {
            cover = bm_sun_cover(off, n, c, area, k, level);
            if (cover.y > 0.5) {
                return BmTerm(true, vec3<f32>(0.0, 0.0, 0.0));
            }
            // The covered area just computed clobbered the weights scratch: the centroid's weights
            // are set again, once for all the nodes that reuse this cover.
            if (cover.x > 0.0) {
                bm_wachspress(c);
            }
        }
        if (cover.x > 0.0) {
            let weight = max(bm_weight_at(k), 0.0);
            let radiance = factor * (studio_spectral_power(bm_lambda(k)) * params.studio_exposure);
            term.xyz = term.xyz + bm_cmf(k) * (((glare * radiance) * weight * cover.x) * norm);
        }
    }
    return term;
}

// ---- the sky: smoothness test, centroid rule, 7-point rule ----

fn bm_radon(r: u32) -> vec4<f32> {
    switch (r) {
        case 0u: {
            return vec4<f32>(1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0, BM_RADON_W0);
        }
        case 1u: {
            return vec4<f32>(BM_RADON_A1, BM_RADON_A1, BM_RADON_B1, BM_RADON_W1);
        }
        case 2u: {
            return vec4<f32>(BM_RADON_A1, BM_RADON_B1, BM_RADON_A1, BM_RADON_W1);
        }
        case 3u: {
            return vec4<f32>(BM_RADON_B1, BM_RADON_A1, BM_RADON_A1, BM_RADON_W1);
        }
        case 4u: {
            return vec4<f32>(BM_RADON_A2, BM_RADON_A2, BM_RADON_B2, BM_RADON_W2);
        }
        case 5u: {
            return vec4<f32>(BM_RADON_A2, BM_RADON_B2, BM_RADON_A2, BM_RADON_W2);
        }
        default: {
            return vec4<f32>(BM_RADON_B2, BM_RADON_A2, BM_RADON_A2, BM_RADON_W2);
        }
    }
}

// The 7-point rule over the fan of triangles from `c` to the polygon's edges, as XYZ.
fn bm_rule7(off: u32, n: u32, c: vec2<f32>) -> vec3<f32> {
    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    let norm = bm_norm();
    let shared_row = bm_row_shared();
    // The points are the outer loop so the Wachspress weights (and, for a shared row, the
    // environment) are computed once per point for a tile of up to `BM_TILE` nodes.
    for (var first = 0u; first < bm_ncount; first = first + BM_TILE) {
        var total = BmLanes(vec4<f32>(0.0), vec4<f32>(0.0));
        var spec = BmLanes(vec4<f32>(0.0), vec4<f32>(0.0));
        if (shared_row) {
            spec = bm_spec_lanes(first);
        }
        for (var e = 0u; e < n; e = e + 1u) {
            let a = bm_sv(off, e);
            let b = bm_sv(off, bm_next(e, n));
            let tri_area = 0.5 * abs(bm_cross(a - c, b - c));
            for (var r = 0u; r < 7u; r = r + 1u) {
                let q = bm_radon(r);
                let pt = c * q.x + a * q.y + b * q.z;
                bm_wachspress(pt);
                var row = 0.0;
                if (shared_row) {
                    row = bm_point_row(pt);
                }
                for (var m = 0u; m < BM_TILE; m = m + 1u) {
                    let k = first + m;
                    if (k < bm_ncount) {
                        let v = bm_value(pt, k, row, bm_lanes_get(spec, m), shared_row);
                        total = bm_lanes_add(total, m, (q.w * tri_area) * v);
                    }
                }
            }
        }
        for (var m = 0u; m < BM_TILE; m = m + 1u) {
            let k = first + m;
            if (k < bm_ncount) {
                xyz = xyz + bm_cmf(k) * (bm_lanes_get(total, m) * norm);
            }
        }
    }
    return xyz;
}

// HDR maps: whether the piece's angular extent (twice the largest corner-to-centroid chord of
// node 0) is within half a texel.
fn bm_within_texel(off: u32, n: u32, c: vec2<f32>) -> bool {
    let centre_dir = bm_unit_dir(c, 0u);
    var widest = 0.0;
    for (var j = 0u; j < n; j = j + 1u) {
        widest = max(widest, length(bm_unit_dir(bm_sv(off, j), 0u) - centre_dir));
    }
    return 2.0 * widest <= bm_params.sigma;
}

// Lights one piece (the fragment itself at level 0, or a refine piece): its XYZ, or the request to
// subdivide it. Mirrors the CPU twin's `piece`: the smoothness test over the corners and the
// centroid (every node), the centroid rule when smooth, a split while `level < max_levels`, the
// 7-point rule below that; the sun disc is added on top.
fn bm_piece(off: u32, n: u32, level: u32) -> BmTerm {
    let none = BmTerm(false, vec3<f32>(0.0, 0.0, 0.0));
    let area = 0.5 * abs(bm_poly_twice_area(off, n));
    if (!(area > 0.0)) {
        return none;
    }
    let c = bm_poly_centroid(off, n);
    let sun = bm_sun_piece(off, n, c, area, level);
    if (sun.needs_split) {
        return BmTerm(true, vec3<f32>(0.0, 0.0, 0.0));
    }
    let hdr = params.env_mode == 2u;
    let norm = bm_norm();
    var flat_sky = true;
    var sky = vec3<f32>(0.0, 0.0, 0.0);
    // Nodes go in tiles; per tile each sample point (the centroid, then the corners) has its
    // Wachspress weights, and for a shared row its environment, computed once. The first tile that
    // is not smooth ends the test. The sky of a smooth piece is the centroid value of every node.
    let shared_row = bm_row_shared();
    let tile_len = select(BM_TILE_SOLO, BM_TILE, shared_row);
    for (var first = 0u; first < bm_ncount && flat_sky; first = first + tile_len) {
        var lo = BmLanes(vec4<f32>(BM_BIG), vec4<f32>(BM_BIG));
        var hi = BmLanes(vec4<f32>(-BM_BIG), vec4<f32>(-BM_BIG));
        var spec = BmLanes(vec4<f32>(0.0), vec4<f32>(0.0));
        if (shared_row) {
            spec = bm_spec_lanes(first);
        }
        for (var j = 0u; j <= n; j = j + 1u) {
            var p = c;
            if (j > 0u) {
                p = bm_sv(off, j - 1u);
            }
            bm_wachspress(p);
            var row = 0.0;
            if (shared_row) {
                row = bm_point_row(p);
            }
            for (var m = 0u; m < tile_len; m = m + 1u) {
                let k = first + m;
                if (k < bm_ncount) {
                    let v = bm_value(p, k, row, bm_lanes_get(spec, m), shared_row);
                    lo = bm_lanes_min(lo, m, v);
                    hi = bm_lanes_max(hi, m, v);
                    if (j == 0u) {
                        sky = sky + bm_cmf(k) * ((v * area) * norm);
                    }
                }
            }
        }
        for (var m = 0u; m < tile_len; m = m + 1u) {
            if (first + m < bm_ncount) {
                let low = bm_lanes_get(lo, m);
                let high = bm_lanes_get(hi, m);
                if (!(high - low <= bm_params.tolerance * high)) {
                    flat_sky = false;
                    break;
                }
            }
        }
    }
    var within = false;
    if (hdr) {
        within = bm_within_texel(off, n, c);
    }
    if (flat_sky && (!hdr || within)) {
        return BmTerm(false, sky + sun.xyz);
    }
    if (!within && level < bm_params.max_levels) {
        return BmTerm(true, vec3<f32>(0.0, 0.0, 0.0));
    }
    return BmTerm(false, bm_rule7(off, n, c) + sun.xyz);
}

// ---- one fragment ----

fn bm_setup_lighting() {
    bm_key = studio_rig_key_dir(params.studio_light_yaw, params.studio_light_pitch);
    bm_fill = studio_rig_fill_dir(params.studio_light_yaw, params.studio_light_pitch);
    bm_sin_lp = sin(params.studio_light_pitch);
    bm_sun_on = params.env_mode == 1u && params.studio_model == 5u
        && daylight_sun_factor(bm_key) > 0.0;
    if (bm_sun_on) {
        let sin_r = sqrt(SUN_ONE_MINUS_COS * (2.0 - SUN_ONE_MINUS_COS));
        bm_radius = atan2(sin_r, SUN_DISC_COS);
        bm_tan_r = sin_r / SUN_DISC_COS;
        bm_chord = 2.0 * sin(0.5 * bm_radius);
        bm_nudge = vec2<f32>(sin(BM_SKY_NUDGE * bm_radius), cos(BM_SKY_NUDGE * bm_radius));
        var helper = vec3<f32>(1.0, 0.0, 0.0);
        if (abs(bm_key.x) > 0.9) {
            helper = vec3<f32>(0.0, 1.0, 0.0);
        }
        bm_sun_t1 = normalize(cross(bm_key, helper));
        bm_sun_t2 = cross(bm_key, bm_sun_t1);
    }
}

// The leaf of fragment slot `slot`: the last leaf whose first slot is `<= slot`.
fn bm_find_leaf(slot: u32) -> u32 {
    var lo = 0u;
    var hi = bm_params.leaf_count;
    for (var round = 0u; round < 32u && lo < hi; round = round + 1u) {
        let mid = lo + (hi - lo) / 2u;
        if (bm_first_fragment[mid] > slot) {
            hi = mid;
        } else {
            lo = mid + 1u;
        }
    }
    return max(lo, 1u) - 1u;
}

// Loads fragment `slot`, clips its leaf to the pixel cell into the scratch polygon (offset
// `BM_ROOT`) and returns the polygon's vertex count (0: nothing to light).
fn bm_setup(slot: u32, local_index: u32) -> u32 {
    bm_base = local_index * BM_SLOT_STRIDE;
    let leaf = bm_find_leaf(slot);
    let header = bm_headers[leaf];
    let rect = bm_pixel_rects[leaf];
    let width = rect.z - rect.x;
    if (width == 0u || rect.w <= rect.y) {
        return 0u;
    }
    let local = slot - bm_first_fragment[leaf];
    bm_pc = vec2<f32>(f32(rect.x + local % width), f32(rect.y + local / width));
    bm_vfirst = header.first_vertex;
    bm_vcount = min(header.vertex_count, BM_MAX_LEAF_VERTICES);
    bm_nfirst = header.first_node;
    bm_ncount = header.node_count;
    bm_flags = header.flags;
    if (bm_vcount < 3u || bm_ncount == 0u) {
        return 0u;
    }
    bm_setup_lighting();
    var lo = vec2<f32>(1.0e30, 1.0e30);
    var hi = vec2<f32>(-1.0e30, -1.0e30);
    for (var i = 0u; i < bm_vcount; i = i + 1u) {
        let v = bm_vertices[bm_vfirst + i];
        let p = vec2<f32>(v.x, v.y) - bm_pc;
        bm_set(BM_LEAF, i, p);
        bm_set(BM_ROOT, i, p);
        lo = min(lo, p);
        hi = max(hi, p);
    }
    let extent = hi - lo;
    bm_size = max(max(extent.x, extent.y), 1.0e-30);
    var n = bm_vcount;
    n = bm_clip(n, BM_ROOT, BM_SPARE, 0u, -0.5, true);
    n = bm_clip(n, BM_SPARE, BM_ROOT, 0u, 0.5, false);
    n = bm_clip(n, BM_ROOT, BM_SPARE, 1u, -0.5, true);
    n = bm_clip(n, BM_SPARE, BM_ROOT, 1u, 0.5, false);
    return n;
}

@compute @workgroup_size(16)
fn bm_fragments_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // `total` is the slot count of this dispatch (one chunk), `slot_base` its first slot: the
    // grid's overshoot past the chunk must not light the next chunk's slots.
    let chunk_index = gid.y * bm_params.grid_width + gid.x;
    if (chunk_index >= bm_params.total) {
        return;
    }
    let slot = bm_params.slot_base + chunk_index;
    let n = bm_setup(slot, local_index);
    if (n < 3u) {
        bm_slots[slot] = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        return;
    }
    let result = bm_piece(BM_ROOT, n, 0u);
    if (result.needs_split) {
        bm_slots[slot] = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        let index = atomicAdd(&bm_refine_count, 1u);
        bm_refine_list[index] = slot;
        return;
    }
    bm_slots[slot] = vec4<f32>(result.xyz, 0.0);
}
