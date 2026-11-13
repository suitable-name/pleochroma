// The stone shadow on the tray (optics::raytracer::environment::tray_shadow): T(x, z, lambda)
// in a buffer of f32 words (header in words 0..16), looked up where an escaping ray meets the tray.
@group(0) @binding(190) var<storage, read> tray_shadow: array<f32>;

const TS_HEADER_WORDS: u32 = 16u;
const TS_BANDS: u32 = 20u;

fn ts_reach() -> f32 {
    return tray_shadow[0];
}

// (u, v, 1) where (origin, dir) meets the plane inside the map; (0, 0, 0) otherwise.
fn ts_plane_uv(origin: vec3<f32>, dir: vec3<f32>) -> vec3<f32> {
    let plane_y = tray_shadow[4];
    if (!(dir.y < 0.0) || !(origin.y > plane_y)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let t = (plane_y - origin.y) / dir.y;
    let x = origin.x + dir.x * t;
    let z = origin.z + dir.z * t;
    let u = (x - tray_shadow[5]) / tray_shadow[7];
    let v = (z - tray_shadow[6]) / tray_shadow[7];
    if (!(abs(u) <= 1.0) || !(abs(v) <= 1.0)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    return vec3<f32>(u, v, 1.0);
}

fn ts_value(i: u32, j: u32, b: u32, n: u32) -> f32 {
    return tray_shadow[TS_HEADER_WORDS + (j * n + i) * TS_BANDS + b];
}

fn ts_bilinear(i0: u32, i1: u32, j0: u32, j1: u32, ax: f32, ay: f32, b: u32, n: u32) -> f32 {
    let a = ts_value(i0, j0, b, n);
    let bb = ts_value(i1, j0, b, n);
    let c = ts_value(i0, j1, b, n);
    let d = ts_value(i1, j1, b, n);
    let top = a + (bb - a) * ax;
    let bottom = c + (d - c) * ax;
    return top + (bottom - top) * ay;
}

fn ts_factor(uv: vec2<f32>, lambda_nm: f32) -> f32 {
    let n = u32(tray_shadow[2]);
    let half = 0.5 * f32(n);
    let last = f32(n - 1u);
    let fx = clamp((uv.x + 1.0) * half - 0.5, 0.0, last);
    let fy = clamp((uv.y + 1.0) * half - 0.5, 0.0, last);
    let i0 = u32(floor(fx));
    let j0 = u32(floor(fy));
    let i1 = min(i0 + 1u, n - 1u);
    let j1 = min(j0 + 1u, n - 1u);
    let ax = fx - f32(i0);
    let ay = fy - f32(j0);
    let fb = clamp((lambda_nm - tray_shadow[8]) / tray_shadow[9], 0.0, f32(TS_BANDS - 1u));
    let b0 = u32(floor(fb));
    let b1 = min(b0 + 1u, TS_BANDS - 1u);
    let ab = fb - f32(b0);
    let t0 = ts_bilinear(i0, i1, j0, j1, ax, ay, b0, n);
    let t1 = ts_bilinear(i0, i1, j0, j1, ax, ay, b1, n);
    return t0 + (t1 - t0) * ab;
}

// The Tray Reflection (optics::raytracer::environment::tray_mirror::TrayMirror), header words
// 10..12 of the same buffer: word 10 = mirror on (1.0 / 0.0), word 11 = head-on reflectance R0,
// word 12 = plane height. A mirror-only scene uploads the 16 header words with word 0 = 0.
//
// Twin of `TrayMirror::hit`, with the same operations in the same order. `fresnel` is Schlick,
// R0 + (1 - R0) (1 - cos)^5 with cos = -dir.y. The reflected direction is (dir.x, -dir.y, dir.z).
struct TmFloor {
    hit: bool,
    point: vec3<f32>,
    fresnel: f32,
}

fn tm_floor(origin: vec3<f32>, dir: vec3<f32>) -> TmFloor {
    var f: TmFloor;
    f.hit = false;
    f.point = vec3<f32>(0.0, 0.0, 0.0);
    f.fresnel = 0.0;
    let r0 = tray_shadow[11];
    let plane_y = tray_shadow[12];
    if (!(dir.y < 0.0) || !(origin.y > plane_y)) {
        return f;
    }
    let t = (plane_y - origin.y) / dir.y;
    f.point = vec3<f32>(origin.x + dir.x * t, plane_y, origin.z + dir.z * t);
    let c = clamp(-dir.y, 0.0, 1.0);
    let m = 1.0 - c;
    let m2 = m * m;
    let m5 = m2 * m2 * m;
    f.fresnel = r0 + (1.0 - r0) * m5;
    f.hit = true;
    return f;
}

fn ts_environment(origin: vec3<f32>, dir: vec3<f32>, lambda_nm: f32, key_dir: vec3<f32>,
                  fill_dir: vec3<f32>, sin_lp: f32, observer: vec3<f32>) -> f32 {
    if (params.env_mode == 1u && params.backdrop > 0.0 && tray_shadow[0] != 0.0) {
        let hit = ts_plane_uv(origin, dir);
        if (hit.z != 0.0) {
            let tray = max(params.backdrop * ts_factor(hit.xy, lambda_nm), 1.17549435e-38);
            return ts_studio_environment_tray(dir, lambda_nm, key_dir, fill_dir, sin_lp, observer, tray);
        }
    }
    return sample_environment_with_rig(dir, lambda_nm, key_dir, fill_dir, sin_lp, observer);
}

// True when a Tray Reflection applies: a studio rig with a backdrop, and the header's mirror flag.
fn tm_on() -> bool {
    return params.env_mode == 1u && params.backdrop > 0.0 && tray_shadow[10] != 0.0;
}

// Chromatic termination check for one mismatched channel k at an exit event. True when the
// hero's exit direction (`hero_dir`) or channel k's own direction meets the floor from
// `hit_point + dir * 1e-4`. A probe cannot follow a floor bounce, so the channel is terminated
// (`path_pdf[k] = 0`, no probe) and the hero path carries the full weight. Twin of the CPU's
// `apply_channel_chromatic_termination`. The check runs before the channel's exit split is
// staged, in the generic arm and the uniaxial internal exit.
fn tm_split_blocked(hit_point: vec3<f32>, hero_dir: vec3<f32>, dir_k: vec3<f32>) -> bool {
    if (!tm_on()) {
        return false;
    }
    if (tm_floor(hit_point + hero_dir * 1e-4, hero_dir).hit) {
        return true;
    }
    return tm_floor(hit_point + dir_k * 1e-4, dir_k).hit;
}

// Twin of the CPU's floor bounce at a miss (`handle_miss`). When the ray meets the floor:
// the MIS weight as the ordinary escape; the (1 - F) tray share is deposited through
// `ts_environment` (the shadow included); every deposit so far (radiance plus the staged split
// radiance) is integrated at THIS moment's `path_pdf` / `compat` into `nee_xyz` and zeroed; the
// Stokes vectors are depolarized and scaled by F; the ray restarts along the reflection,
// `point + reflected * 1e-4`. `path_escaped` stays false and the MIS carry stays consumed (0), so
// the next escape takes MIS weight 1. Returns false, touching nothing, when the mirror is off or
// the ray does not meet the floor.
//
// The flush above also adds the studio floor deposits to `nee_xyz`. The finalize step still
// adds `nee_xyz` ahead of the white balance.
fn tm_floor_bounce(
    s: ptr<function, BounceRay>,
    rc: RayConstants,
    observer: vec3<f32>,
    phase_pdf_this_check: f32,
    phase_dir_this_check: vec3<f32>,
) -> bool {
    if (!tm_on()) {
        return false;
    }
    let fh = tm_floor((*s).origin, (*s).dir);
    if (!fh.hit) {
        return false;
    }
    var tm_mis: f32 = 1.0;
    if (phase_pdf_this_check > 0.0 && nee_light_available()) {
        let tm_light_pdf = nee_light_pdf(phase_dir_this_check, rc.studio_key_dir);
        tm_mis = balance_heuristic(phase_pdf_this_check, tm_light_pdf);
    }
    let tray_weight = tm_mis * (1.0 - fh.fresnel);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let env = ts_environment((*s).origin, (*s).dir, (*s).lambdas[k], rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp, observer);
        (*s).radiance[k] = fma(max((*s).stokes[k].x, 0.0) * tray_weight, env, (*s).radiance[k]);
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*s).radiance[k] = (*s).radiance[k] + (*s).split_radiance[k];
    }
    (*s).nee_xyz = (*s).nee_xyz
        + integrate_channels_to_xyz_family((*s).radiance, (*s).lambdas, (*s).path_pdf, (*s).compat);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*s).radiance[k] = 0.0;
        (*s).split_radiance[k] = 0.0;
    }
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*s).stokes[k] = vec4<f32>(max((*s).stokes[k].x, 0.0) * fh.fresnel, 0.0, 0.0, 0.0);
    }
    (*s).have_prev_plane_normal = false;
    let reflected = vec3<f32>((*s).dir.x, -(*s).dir.y, (*s).dir.z);
    (*s).origin = fh.point + reflected * 1e-4;
    (*s).dir = reflected;
    (*s).k = reflected;
    return true;
}
