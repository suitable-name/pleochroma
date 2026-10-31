// ---------------------------------------------------------------------------------
// Surface finish (lane RP-G2): the WGSL twin of `optics::raytracer::surface`.
//
// Pure functions only (no bindings), so the megakernel, the event wavefront and Tier 2's
// `transport_functions.wgsl` call the SAME text. Each section names the CPU file it mirrors;
// the arithmetic, the draw streams and the constants are copied from there, never
// re-derived:
//
// - word decoding         surface/facet_surface.rs (the packed `u32` of binding 8)
// - GGX lobe              surface/ggx.rs
// - total integrated scatter  surface/tis.rs
// - mirror/Snell/Fresnel at a micro-normal  surface/micro.rs
// - faceting machine sweep and scratch direction  surface/sweep.rs
// - energy compensation lookup (the table itself is the tail of the `dist_func` buffer; the
//   callers read it)  surface/compensation.rs
// - the ground (rough dielectric) event  surface/ground.rs
//
// A lobe is a `vec2<f32>` = (alpha across the scratches, alpha along them).
// ---------------------------------------------------------------------------------

// Draw streams of the surface events: `surface::SURFACE_*_STREAM`.
const SURFACE_HALO_STREAM: u32 = 0x85ebca6bu;
const SURFACE_VNDF_U_STREAM: u32 = 0xc2b2ae35u;
const SURFACE_VNDF_V_STREAM: u32 = 0xd6e8feb8u;
const SURFACE_SWEEP_STREAM: u32 = 0xa136aaadu;

// `ggx::MIN_ALPHA` / `ggx::MIN_COSINE`.
const SURF_MIN_ALPHA: f32 = 1e-3;
const SURF_MIN_COSINE: f32 = 1e-4;
// `R_UNPOL_PDF_MIN/MAX` (the technique densities of the halo) and `R_UNPOL_SELECT_MIN/MAX`
// (the ground arm's branch probability); the bounce text owns the same numbers under
// other names, Tier 2's text does not have them.
const SURF_PDF_MIN: f32 = 1e-4;
const SURF_PDF_MAX: f32 = 1.0 - 1e-4;
const SURF_R_SEL_MIN: f32 = 0.02;
const SURF_R_SEL_MAX: f32 = 0.98;
// `facet_surface::DEFAULT_GROUND_ALPHA`.
const SURF_DEFAULT_GROUND_ALPHA: f32 = 0.45;
// The word the surface phase stores in `BounceHit::finish` for a point inside the abraded
// edge band: kind 1 (ground) with the unused scratch-angle bit 29 set, so the ground arm
// can tell the band's lobe (the material's edge alpha) from a frosted facet's.
const SURF_EDGE_GROUND_WORD: u32 = 0x20000001u;
// Values of the energy-compensation table at the end of the `dist_func` buffer (10 alphas
// x 16 angles x 17 index ratios, `compensation.rs`).
const GROUND_TABLE_WORDS: u32 = 2720u;

// `surface::unit_draw`.
fn surface_unit_draw(rng_seed: u32, bounce: u32, stream: u32) -> f32 {
    return f32(hash_u32(rng_seed ^ hash_u32(bounce ^ stream))) / 4294967295.0;
}

// ---------------------------------------------------------------------------------
// The packed word (`FacetSurface`).
// ---------------------------------------------------------------------------------

fn surf_kind(word: u32) -> u32 {
    return word & 3u;
}

fn surf_level(word: u32) -> u32 {
    return (word >> 2u) & 0x3ffu;
}

// Kinds 1 (frosted) and 3 (ground) take the ground arm: `FacetSurface::is_ground`.
fn surf_is_ground(word: u32) -> bool {
    return (word & 1u) == 1u;
}

// `FacetSurface::has_roughness`.
fn surf_has_roughness(word: u32) -> bool {
    return (word & 3u) == 2u && surf_level(word) > 0u;
}

// `FacetSurface::rq_nm`.
fn surf_rq_nm(word: u32) -> f32 {
    if ((word & 3u) == 2u) {
        return f32(surf_level(word)) / 10.0;
    }
    return 0.0;
}

// `FacetSurface::ground_alpha`.
fn surf_ground_alpha(word: u32) -> f32 {
    if (!surf_is_ground(word)) {
        return 0.0;
    }
    let level = surf_level(word);
    if (level == 0u) {
        return SURF_DEFAULT_GROUND_ALPHA;
    }
    return f32(level) / 1000.0;
}

// `FacetSurface::anisotropy`.
fn surf_anisotropy(word: u32) -> f32 {
    let code = (word >> 12u) & 0xffu;
    return max(f32(code) / 16.0, 1.0);
}

fn surf_has_custom_angle(word: u32) -> bool {
    return (word & 0x40000000u) != 0u;
}

fn surf_no_sweep(word: u32) -> bool {
    return (word & 0x80000000u) != 0u;
}

// `FacetSurface::custom_angle_deg` (only meaningful with the Custom Angle bit).
fn surf_custom_angle_deg(word: u32) -> f32 {
    return f32((word >> 20u) & 0x3ffu) * (180.0 / 1023.0);
}

// `FacetSurface::with_rq_at_least`'s quantiser: `(value * scale).round().clamp(0, max)`.
fn surf_quantise(value: f32, scale: f32, max_code: f32) -> u32 {
    return u32(clamp(floor(value * scale + 0.5), 0.0, max_code));
}

// `FacetSurface::with_rq_at_least`.
fn surf_with_rq_at_least(word: u32, rq_nm: f32) -> u32 {
    let level = surf_quantise(rq_nm, 10.0, 1023.0);
    if (level == 0u || surf_is_ground(word)) {
        return word;
    }
    if ((word & 3u) == 2u && surf_level(word) >= level) {
        return word;
    }
    let kept = word & ~(3u | (0x3ffu << 2u));
    return kept | 2u | (level << 2u);
}

// `facet_surface::halo_alpha`: the log-log interpolation of alpha = 2 Rq / lc between the
// four polish presets.
fn surf_halo_alpha(rq_nm: f32) -> f32 {
    if (rq_nm <= 0.0) {
        return 0.0;
    }
    if (rq_nm < 1.0) {
        return 0.010 * rq_nm / 1.0;
    }
    // The segment holding `rq_nm`; above the last anchor the last segment continues.
    var rq0: f32 = 6.0;
    var a0: f32 = 0.025;
    var rq1: f32 = 25.0;
    var a1: f32 = 0.040;
    if (rq_nm <= 2.5) {
        rq0 = 1.0;
        a0 = 0.010;
        rq1 = 2.5;
        a1 = 0.015152;
    } else if (rq_nm <= 6.0) {
        rq0 = 2.5;
        a0 = 0.015152;
        rq1 = 6.0;
        a1 = 0.025;
    }
    if (rq_nm == rq0) {
        return min(a0, 0.5);
    }
    let t = log(rq_nm / rq0) / log(rq1 / rq0);
    let alpha = exp(fma(t, log(a1) - log(a0), log(a0)));
    return min(alpha, 0.5);
}

// ---------------------------------------------------------------------------------
// The GGX lobe (`GgxLobe`).
// ---------------------------------------------------------------------------------

// `GgxLobe::isotropic`.
fn ggx_lobe_isotropic(alpha: f32) -> vec2<f32> {
    let a = max(alpha, SURF_MIN_ALPHA);
    return vec2<f32>(a, a);
}

// `GgxLobe::anisotropic`.
fn ggx_lobe_anisotropic(alpha: f32, ratio: f32) -> vec2<f32> {
    let a = max(alpha, SURF_MIN_ALPHA);
    let root = sqrt(max(ratio, 1.0));
    return vec2<f32>(a * root, max(a / root, SURF_MIN_ALPHA));
}

// `GgxLobe::alpha_mean`.
fn ggx_alpha_mean(lobe: vec2<f32>) -> f32 {
    return sqrt(lobe.x * lobe.y);
}

// `FacetSurface::halo_lobe` / `ground_lobe`.
fn surf_halo_lobe(word: u32) -> vec2<f32> {
    return ggx_lobe_anisotropic(surf_halo_alpha(surf_rq_nm(word)), surf_anisotropy(word));
}

fn surf_ground_lobe(word: u32) -> vec2<f32> {
    return ggx_lobe_anisotropic(surf_ground_alpha(word), surf_anisotropy(word));
}

// `GgxLobe::d`.
fn ggx_d(lobe: vec2<f32>, m: vec3<f32>) -> f32 {
    if (m.z <= 0.0) {
        return 0.0;
    }
    let qx = m.x / lobe.x;
    let qy = m.y / lobe.y;
    let q = fma(qx, qx, fma(qy, qy, m.z * m.z));
    return 1.0 / (PI * lobe.x * lobe.y * q * q);
}

// `GgxLobe::lambda`.
fn ggx_lambda(lobe: vec2<f32>, w: vec3<f32>) -> f32 {
    let z2 = max(w.z * w.z, 1e-12);
    let ax = lobe.x * w.x;
    let ay = lobe.y * w.y;
    let a2 = fma(ax, ax, ay * ay) / z2;
    return 0.5 * (sqrt(1.0 + a2) - 1.0);
}

// `GgxLobe::g1`.
fn ggx_g1(lobe: vec2<f32>, w: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + ggx_lambda(lobe, w));
}

// `GgxLobe::g2`.
fn ggx_g2(lobe: vec2<f32>, wi: vec3<f32>, wo: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + ggx_lambda(lobe, wi) + ggx_lambda(lobe, wo));
}

// `GgxLobe::visible_density`.
fn ggx_visible_density(lobe: vec2<f32>, wi: vec3<f32>, m: vec3<f32>) -> f32 {
    let cos_im = max(dot(wi, m), 0.0);
    return ggx_g1(lobe, wi) * cos_im * ggx_d(lobe, m) / max(wi.z, SURF_MIN_COSINE);
}

// `GgxLobe::sample_visible` (Heitz 2018).
fn ggx_sample_visible(lobe: vec2<f32>, wi: vec3<f32>, u1: f32, u2: f32) -> vec3<f32> {
    let vh = normalize(vec3<f32>(lobe.x * wi.x, lobe.y * wi.y, wi.z));
    let len2 = fma(vh.x, vh.x, vh.y * vh.y);
    var t1 = vec3<f32>(1.0, 0.0, 0.0);
    if (len2 > 0.0) {
        t1 = vec3<f32>(-vh.y, vh.x, 0.0) / sqrt(len2);
    }
    let t2 = cross(vh, t1);
    let r = sqrt(u1);
    let phi = (2.0 * PI) * u2;
    let sin_phi = sin(phi);
    let cos_phi = cos(phi);
    let p1 = r * cos_phi;
    let p2_disk = r * sin_phi;
    let s = 0.5 * (1.0 + vh.z);
    let p2 = fma(1.0 - s, sqrt(max(1.0 - p1 * p1, 0.0)), s * p2_disk);
    let nh = t1 * p1 + t2 * p2 + vh * sqrt(max(1.0 - p1 * p1 - p2 * p2, 0.0));
    return normalize(vec3<f32>(lobe.x * nh.x, lobe.y * nh.y, max(nh.z, 0.0)));
}

// ---------------------------------------------------------------------------------
// Total integrated scatter (`tis.rs`).
// ---------------------------------------------------------------------------------

// `-expm1(-x)`: the series below 0.05 keeps the relative accuracy of the CPU's `exp_m1` for
// the 1e-5 scatter of a good polish, where `1 - exp(-x)` would lose it to cancellation.
fn surf_saturate(x: f32) -> f32 {
    if (x < 0.05) {
        return x * (1.0 - x * (0.5 - x * (1.0 / 6.0 - x * (1.0 / 24.0 - x * (1.0 / 120.0 - x * (1.0 / 720.0))))));
    }
    return 1.0 - exp(-x);
}

// `tis_reflection`.
fn surf_tis_reflection(rq_nm: f32, n1: f32, cos_i: f32, lambda_nm: f32) -> f32 {
    let phase = 4.0 * PI * n1 * rq_nm * cos_i / lambda_nm;
    return surf_saturate(phase * phase);
}

// `tis_transmission`.
fn surf_tis_transmission(rq_nm: f32, n1: f32, n2: f32, cos_i: f32, lambda_nm: f32) -> f32 {
    let eta = n1 / n2;
    let sin2_t = eta * eta * fma(-cos_i, cos_i, 1.0);
    if (sin2_t >= 1.0) {
        return surf_tis_reflection(rq_nm, n1, cos_i, lambda_nm);
    }
    let cos_t = sqrt(1.0 - sin2_t);
    let phase = 2.0 * PI * rq_nm * abs(fma(n1, cos_i, -(n2 * cos_t))) / lambda_nm;
    return surf_saturate(phase * phase);
}

// ---------------------------------------------------------------------------------
// Interface optics at a micro-normal (`micro.rs`). Directions point AWAY from the surface;
// a refracted direction is the propagation direction on the far side.
// ---------------------------------------------------------------------------------

// `fresnel_unpolarized`.
fn surf_fresnel(n1: f32, n2: f32, cos_i: f32) -> f32 {
    let eta = n1 / n2;
    let sin2_t = eta * eta * fma(-cos_i, cos_i, 1.0);
    if (sin2_t >= 1.0) {
        return 1.0;
    }
    let cos_t = sqrt(1.0 - sin2_t);
    let r_s = fma(n1, cos_i, -(n2 * cos_t)) / fma(n1, cos_i, n2 * cos_t);
    let r_p = fma(n2, cos_i, -(n1 * cos_t)) / fma(n2, cos_i, n1 * cos_t);
    return 0.5 * fma(r_p, r_p, r_s * r_s);
}

// `reflect_about`.
fn surf_reflect_about(i: vec3<f32>, m: vec3<f32>) -> vec3<f32> {
    return 2.0 * dot(i, m) * m - i;
}

struct SurfRefract {
    ok: bool,
    dir: vec3<f32>,
}

// `refract_about`; `ok == false` past the critical angle.
fn surf_refract_about(i: vec3<f32>, m: vec3<f32>, n1: f32, n2: f32) -> SurfRefract {
    var out: SurfRefract;
    out.ok = false;
    out.dir = vec3<f32>(0.0, 0.0, 0.0);
    let eta = n1 / n2;
    let c = dot(i, m);
    let k = 1.0 - eta * eta * fma(-c, c, 1.0);
    if (k < 0.0) {
        return out;
    }
    out.ok = true;
    out.dir = fma(eta, c, -sqrt(k)) * m - eta * i;
    return out;
}

// ---------------------------------------------------------------------------------
// The faceting machine sweep (`sweep.rs`).
//
// The machine rides in the material block as `(arm_offset_deg, a, b)` with
// `a = inner_mm / mast_distance_mm` and `b = (outer_mm - inner_mm) / mast_distance_mm`
// (both signed like the mast distance; both zero for an arm through the lap centre), so the
// contact radius of the draw `u` over the mast distance is `x = a + u b` and the scratch angle
// is `arm_offset + asin(1 / x)` (clamped: the contact point cannot be closer than the arm).
// ---------------------------------------------------------------------------------

// `FacetingMachine::angle_at_radius_deg` at the sweep position `u` in [0, 1].
fn surf_machine_angle_deg(machine: vec3<f32>, u: f32) -> f32 {
    let a = machine.y;
    let b = machine.z;
    if (a == 0.0 && b == 0.0) {
        return machine.x;
    }
    let x = fma(u, b, a);
    var v: f32 = 0.0;
    if (abs(x) <= 1.0) {
        v = 1.0;
        if (x < 0.0 || (x == 0.0 && (a < 0.0 || b < 0.0))) {
            v = -1.0;
        }
    } else {
        v = 1.0 / x;
    }
    return machine.x + degrees(asin(clamp(v, -1.0, 1.0)));
}

// `scratch_angle_deg`: a Custom Angle fixes it, "No Sweep" takes the middle of the sweep,
// otherwise the sweep draws it from `u`.
fn surf_scratch_angle_deg(word: u32, machine: vec3<f32>, u: f32) -> f32 {
    if (surf_has_custom_angle(word)) {
        return surf_custom_angle_deg(word);
    }
    if (surf_no_sweep(word)) {
        return surf_machine_angle_deg(machine, 0.5);
    }
    return surf_machine_angle_deg(machine, u);
}

// `scratch_direction`.
fn surf_scratch_direction(normal: vec3<f32>, angle_deg: f32) -> vec3<f32> {
    var girdle = cross(normal, vec3<f32>(0.0, 1.0, 0.0));
    if (dot(girdle, girdle) < 1e-8) {
        let x_axis = vec3<f32>(1.0, 0.0, 0.0);
        girdle = normalize_or_zero(x_axis - normal * dot(normal, x_axis));
    } else {
        girdle = normalize(girdle);
    }
    let angle = angle_deg * 0.017453292519943295;
    let sin_a = sin(angle);
    let cos_a = cos(angle);
    return normalize_or_zero(girdle * cos_a + cross(normal, girdle) * sin_a);
}

// `dispatch::across_scratches`: the direction across the scratches of a sample on a facet
// with outward normal `normal_out`.
fn surf_across_scratches(word: u32, machine: vec3<f32>, normal_out: vec3<f32>, rng_seed: u32, bounce: u32) -> vec3<f32> {
    let angle = surf_scratch_angle_deg(word, machine, surface_unit_draw(rng_seed, bounce, SURFACE_SWEEP_STREAM));
    return cross(normal_out, surf_scratch_direction(normal_out, angle));
}

// ---------------------------------------------------------------------------------
// Energy compensation lookup (`compensation::ground_albedo`): the cell of the trilinear
// interpolation. The caller reads the eight taps from wherever it keeps the table
// (`ground_cell_index` is the word offset into it) and blends them.
// ---------------------------------------------------------------------------------

struct GroundCell {
    a0: u32,
    c0: u32,
    e0: u32,
    ta: f32,
    tc: f32,
    te: f32,
}

// `compensation::ALPHA_NODES`.
fn ground_alpha_node(i: u32) -> f32 {
    var v: f32 = 1.0;
    if (i == 0u) {
        v = 0.05;
    } else if (i == 1u) {
        v = 0.1;
    } else if (i == 2u) {
        v = 0.2;
    } else if (i == 3u) {
        v = 0.3;
    } else if (i == 4u) {
        v = 0.4;
    } else if (i == 5u) {
        v = 0.45;
    } else if (i == 6u) {
        v = 0.5;
    } else if (i == 7u) {
        v = 0.6;
    } else if (i == 8u) {
        v = 0.8;
    }
    return v;
}

// The cell of `ground_albedo(alpha, cos_i, eta_rel)`.
fn ground_albedo_cell(alpha: f32, cos_i: f32, eta_rel: f32) -> GroundCell {
    var cell: GroundCell;
    // `bracket` on the alpha nodes.
    cell.a0 = 0u;
    cell.ta = 0.0;
    if (alpha >= 1.0) {
        cell.a0 = 8u;
        cell.ta = 1.0;
    } else if (alpha > 0.05) {
        var upper: u32 = 0u;
        for (var j: u32 = 0u; j < 10u; j = j + 1u) {
            if (ground_alpha_node(j) <= alpha) {
                upper = upper + 1u;
            }
        }
        upper = min(upper, 9u);
        let lower = upper - 1u;
        cell.a0 = lower;
        let lo = ground_alpha_node(lower);
        cell.ta = (alpha - lo) / (ground_alpha_node(upper) - lo);
    }
    // The angle and index axes are uniform in their own coordinates.
    var angle = degrees(acos(clamp(cos_i, 0.0, 1.0))) / 86.0 * 15.0;
    angle = clamp(angle, 0.0, 15.0);
    cell.c0 = min(u32(floor(angle)), 14u);
    cell.tc = angle - f32(cell.c0);
    var eta = (log(max(eta_rel, 1e-3)) / log(3.0) + 1.0) * 0.5 * 16.0;
    eta = clamp(eta, 0.0, 16.0);
    cell.e0 = min(u32(floor(eta)), 15u);
    cell.te = eta - f32(cell.e0);
    return cell;
}

// The word offset of the tap `(a0 + da, c0 + dc, e0 + de)` in the table.
fn ground_cell_index(cell: GroundCell, da: u32, dc: u32, de: u32) -> u32 {
    return ((cell.a0 + da) * 16u + (cell.c0 + dc)) * 17u + (cell.e0 + de);
}

// The trilinear blend of the eight taps `vXYZ` (X: alpha, Y: angle, Z: index ratio), summed in
// the CPU's order, floored at `MIN_ALBEDO`.
fn ground_albedo_blend(cell: GroundCell, v000: f32, v001: f32, v010: f32, v011: f32, v100: f32, v101: f32, v110: f32, v111: f32) -> f32 {
    let wa0 = 1.0 - cell.ta;
    let wc0 = 1.0 - cell.tc;
    let we0 = 1.0 - cell.te;
    var total: f32 = 0.0;
    total = total + wa0 * wc0 * we0 * v000;
    total = total + wa0 * wc0 * cell.te * v001;
    total = total + wa0 * cell.tc * we0 * v010;
    total = total + wa0 * cell.tc * cell.te * v011;
    total = total + cell.ta * wc0 * we0 * v100;
    total = total + cell.ta * wc0 * cell.te * v101;
    total = total + cell.ta * cell.tc * we0 * v110;
    total = total + cell.ta * cell.tc * cell.te * v111;
    return max(total, 0.05);
}

// ---------------------------------------------------------------------------------
// The ground (rough dielectric) event (`ground.rs`).
// ---------------------------------------------------------------------------------

// `GroundFrame`: `t` across the scratches, `b = n x t` along them, `n` the macro normal on
// the incident side.
struct GroundFrame {
    t: vec3<f32>,
    b: vec3<f32>,
    n: vec3<f32>,
}

// `GroundFrame::new`.
fn ground_frame_new(normal: vec3<f32>, across: vec3<f32>, has_across: bool) -> GroundFrame {
    var t = vec3<f32>(0.0, 0.0, 0.0);
    var found = false;
    if (has_across) {
        let projected = normalize_or_zero(across - normal * dot(normal, across));
        if (dot(projected, projected) > 0.0) {
            t = projected;
            found = true;
        }
    }
    if (!found) {
        t = frosted_orthonormal_basis(normal).t;
    }
    var frame: GroundFrame;
    frame.t = t;
    frame.b = cross(normal, t);
    frame.n = normal;
    return frame;
}

fn ground_to_local(frame: GroundFrame, v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(v, frame.t), dot(v, frame.b), dot(v, frame.n));
}

fn ground_to_world(frame: GroundFrame, v: vec3<f32>) -> vec3<f32> {
    return frame.t * v.x + frame.b * v.y + frame.n * v.z;
}

// `GroundCtx`. `compensation` starts at 1: the caller looks `ground_albedo` up for
// `(alpha_mean, i.z, n2 / n1)` and stores its inverse.
struct GroundCtx {
    frame: GroundFrame,
    lobe: vec2<f32>,
    i: vec3<f32>,
    n1: f32,
    n2: f32,
    r_sel: f32,
    compensation: f32,
}

// `GroundCtx::new` without the albedo lookup.
fn ground_ctx_new(
    normal: vec3<f32>,
    k_in: vec3<f32>,
    lobe: vec2<f32>,
    across: vec3<f32>,
    has_across: bool,
    n1: f32,
    n2: f32,
    cos_i: f32,
    sin2_t: f32,
) -> GroundCtx {
    var ctx: GroundCtx;
    ctx.frame = ground_frame_new(normal, across, has_across);
    var i = ground_to_local(ctx.frame, -k_in);
    i.z = max(i.z, SURF_MIN_COSINE);
    ctx.i = normalize(i);
    ctx.lobe = lobe;
    ctx.n1 = n1;
    ctx.n2 = n2;
    ctx.r_sel = SURF_R_SEL_MAX;
    if (sin2_t <= 1.0) {
        ctx.r_sel = clamp(surf_fresnel(n1, n2, cos_i), SURF_R_SEL_MIN, SURF_R_SEL_MAX);
    }
    ctx.compensation = 1.0;
    return ctx;
}

// `GroundCtx::density`.
fn ground_density(ctx: GroundCtx, o: vec3<f32>, h: vec3<f32>, c: f32, is_refl: bool) -> f32 {
    let base = ggx_g1(ctx.lobe, ctx.i) * ggx_d(ctx.lobe, h) / ctx.i.z;
    if (is_refl) {
        return 0.25 * base;
    }
    let o_dot_h = dot(o, h);
    let denom = fma(ctx.n1, c, ctx.n2 * o_dot_h);
    if (abs(denom) < 1e-12) {
        return 0.0;
    }
    return base * c * ctx.n2 * ctx.n2 * abs(o_dot_h) / (denom * denom);
}

// `GroundCtx::branch_energy`.
fn ground_branch_energy(ctx: GroundCtx, fresnel: f32, is_refl: bool) -> f32 {
    if (is_refl) {
        return fresnel / ctx.r_sel;
    }
    return (1.0 - fresnel) / (1.0 - ctx.r_sel);
}

// `GroundSample`.
struct GroundSample {
    dir: vec3<f32>,
    crossed: bool,
    reflect_branch: bool,
    weight: f32,
    branch_prob: f32,
    pdf: f32,
    valid: bool,
}

// `GroundCtx::sample`.
fn ground_sample(ctx: GroundCtx, u1: f32, u2: f32, u_branch: f32) -> GroundSample {
    let i = ctx.i;
    let is_refl = u_branch < ctx.r_sel;
    var out: GroundSample;
    out.branch_prob = 1.0 - ctx.r_sel;
    if (is_refl) {
        out.branch_prob = ctx.r_sel;
    }
    let m = ggx_sample_visible(ctx.lobe, i, u1, u2);
    let c = dot(i, m);
    var o = vec3<f32>(0.0, 0.0, 0.0);
    var have = false;
    if (is_refl) {
        o = surf_reflect_about(i, m);
        have = true;
    } else {
        let refracted = surf_refract_about(i, m, ctx.n1, ctx.n2);
        o = refracted.dir;
        have = refracted.ok;
    }
    var side_ok = o.z < 0.0;
    if (is_refl) {
        side_ok = o.z > 0.0;
    }
    out.reflect_branch = is_refl;
    if (have && c > 0.0 && side_ok) {
        let on = normalize(o);
        let fresnel = surf_fresnel(ctx.n1, ctx.n2, c);
        let masking = ggx_g2(ctx.lobe, i, on) / ggx_g1(ctx.lobe, i);
        out.dir = normalize(ground_to_world(ctx.frame, on));
        out.crossed = !is_refl;
        out.weight = ground_branch_energy(ctx, fresnel, is_refl) * masking * ctx.compensation;
        out.pdf = ground_density(ctx, on, m, c, is_refl);
        out.valid = true;
    } else {
        out.dir = normalize(ground_to_world(ctx.frame, vec3<f32>(-i.x, -i.y, i.z)));
        out.crossed = false;
        out.weight = 0.0;
        out.pdf = 0.0;
        out.valid = false;
    }
    return out;
}

// `GroundEval`: `ok == false` is the CPU's `None`.
struct GroundEval {
    ok: bool,
    weight: f32,
    pdf: f32,
}

// `GroundCtx::eval`.
fn ground_eval(ctx: GroundCtx, o_world: vec3<f32>, is_refl: bool) -> GroundEval {
    var out: GroundEval;
    out.ok = false;
    out.weight = 0.0;
    out.pdf = 0.0;
    let i = ctx.i;
    let o = ground_to_local(ctx.frame, o_world);
    if (is_refl != (o.z > 0.0) || o.z == 0.0) {
        return out;
    }
    var half_vec = i + o;
    if (!is_refl) {
        half_vec = ctx.n1 * i + ctx.n2 * o;
    }
    let len = length(half_vec);
    if (len < 1e-6) {
        return out;
    }
    var h = half_vec / len;
    if (h.z < 0.0) {
        h = -h;
    }
    let c = dot(i, h);
    if (c <= 0.0 || (!is_refl && dot(o, h) >= 0.0)) {
        return out;
    }
    let eta = ctx.n1 / ctx.n2;
    if (!is_refl && eta * eta * fma(-c, c, 1.0) >= 1.0) {
        return out;
    }
    let fresnel = surf_fresnel(ctx.n1, ctx.n2, c);
    let masking = ggx_g2(ctx.lobe, i, o) / ggx_g1(ctx.lobe, i);
    let pdf = ground_density(ctx, o, h, c, is_refl);
    if (pdf > 0.0) {
        out.ok = true;
        out.weight = ground_branch_energy(ctx, fresnel, is_refl) * masking * ctx.compensation;
        out.pdf = pdf;
    }
    return out;
}
