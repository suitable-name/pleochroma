fn wt_studio_g(dir: vec3<f32>, observer: vec3<f32>) -> f32 {
    return wt_studio_dispatch(u32(wt.studio.w), normalize(dir), 1.0, wt.studio.x,
        wt.values.w, wt.key_dir.xyz, wt.fill_dir.xyz, wt.studio.z, wt.studio.y,
        observer, wt.shadow.xy, wt.tent, wt.extra.x, wt.values.z);
}
fn wt_studio_g_tray(dir: vec3<f32>, observer: vec3<f32>, tray: f32) -> f32 {
    return wt_studio_dispatch(u32(wt.studio.w), normalize(dir), 1.0, wt.studio.x,
        wt.values.w, wt.key_dir.xyz, wt.fill_dir.xyz, wt.studio.z, wt.studio.y,
        observer, wt.shadow.xy, wt.tent, wt.extra.x, tray);
}
// `w` is 1 where the tray-shadow map applies and 0 where the unchanged path is used.
fn wt_tray_shadow_rgb(origin: vec3<f32>, dir: vec3<f32>) -> vec4<f32> {
    if (wt.tray_shadow_layout.x == 0u || !(dir.y < 0.0) || !(origin.y > wt.tray_shadow.x)) {
        return vec4<f32>(0.0);
    }
    let t = (wt.tray_shadow.x - origin.y) / dir.y;
    let x = origin.x + dir.x * t;
    let z = origin.z + dir.z * t;
    let u = (x - wt.tray_shadow.y) / wt.tray_shadow.w;
    let v = (z - wt.tray_shadow.z) / wt.tray_shadow.w;
    if (!(abs(u) <= 1.0) || !(abs(v) <= 1.0)) { return vec4<f32>(0.0); }
    let n = wt.tray_shadow_layout.y;
    let last = f32(n - 1u);
    let fx = clamp((u + 1.0) * (0.5 * f32(n)) - 0.5, 0.0, last);
    let fy = clamp((v + 1.0) * (0.5 * f32(n)) - 0.5, 0.0, last);
    let i = u32(floor(fx)); let j = u32(floor(fy));
    let i1 = min(i + 1u, n - 1u); let j1 = min(j + 1u, n - 1u);
    let ax = fx - f32(i); let ay = fy - f32(j);
    let base = wt.tray_shadow_layout.z;
    let a = wt_env[base + j * n + i].xyz;
    let b = wt_env[base + j * n + i1].xyz;
    let c = wt_env[base + j1 * n + i].xyz;
    let d = wt_env[base + j1 * n + i1].xyz;
    let top = a + (b - a) * ax;
    let bottom = c + (d - c) * ax;
    return vec4<f32>(top + (bottom - top) * ay, 1.0);
}
// Where a downward ray meets the glossy tray: `TrayMirror::hit` twin, plain multiplies and
// adds in the same order. `wt.tray_mirror` is (on 0/1, plane y, head-on reflectance R0, 0);
// a ray that does not meet the plane is `valid == false`. The on flag is tested by the caller
// (`wt_floor_escape`), which never calls this with the mirror off.
struct WtFloor { point: vec3<f32>, reflected: vec3<f32>, fresnel: f32, valid: bool }
fn wt_floor(origin: vec3<f32>, dir: vec3<f32>) -> WtFloor {
    let none = WtFloor(vec3<f32>(0.0), vec3<f32>(0.0), 0.0, false);
    let plane_y = wt.tray_mirror.y;
    let reflectance = wt.tray_mirror.z;
    if (!(dir.y < 0.0) || !(origin.y > plane_y)) { return none; }
    let t = (plane_y - origin.y) / dir.y;
    let point = vec3<f32>(origin.x + dir.x * t, plane_y, origin.z + dir.z * t);
    let c = clamp(-dir.y, 0.0, 1.0);
    let m = 1.0 - c;
    let m2 = m * m;
    let m5 = m2 * m2 * m;
    let fresnel = reflectance + (1.0 - reflectance) * m5;
    return WtFloor(point, vec3<f32>(dir.x, -dir.y, dir.z), fresnel, true);
}
fn wt_leaf_at(origin: vec3<f32>, dir: vec3<f32>, observer: vec3<f32>) -> vec3<f32> {
    if (wt.tray_shadow_layout.x == 0u) { return wt_leaf(dir, observer); }
    let factor = wt_tray_shadow_rgb(origin, dir);
    if (factor.w == 0.0) { return wt_leaf(dir, observer); }
    let tray = max(vec3<f32>(wt.values.z) * factor.xyz, vec3<f32>(1.17549435e-38));
    return wt.env_rgb.xyz * vec3<f32>(wt_studio_g_tray(dir, observer, tray.x),
        wt_studio_g_tray(dir, observer, tray.y), wt_studio_g_tray(dir, observer, tray.z));
}
fn wt_hdr_texel(x: i32, y: i32) -> vec3<f32> {
    let width = i32(wt_dims.x);
    let xx = u32(((x % width) + width) % width);
    let yy = u32(clamp(y, 0, i32(wt_dims.y) - 1));
    return wt_env[yy * wt_dims.x + xx].xyz;
}
// renderer::env_map::EnvRotation::to_map: a world direction into the HDR map's frame (a turn by
// -yaw about +Y). `rot` = (cos, sin) of the map's yaw, computed on the CPU; exactly (1, 0) returns
// `dir` untouched, so an unrotated map keeps its bits.
fn hdr_world_to_map(dir: vec3<f32>, rot: vec2<f32>) -> vec3<f32> {
    if (rot.x == 1.0 && rot.y == 0.0) {
        return dir;
    }
    return vec3<f32>(fma(rot.x, dir.x, -(rot.y * dir.z)), dir.y, fma(rot.x, dir.z, rot.y * dir.x));
}
// The map's yaw (cosine, sine) rides in `key_dir.xy` in HDR mode (mode 2) only.
fn wt_hdr(dir: vec3<f32>) -> vec3<f32> {
    let d = normalize(hdr_world_to_map(dir, wt.key_dir.xy));
    let v = acos(clamp(d.y, -1.0, 1.0)) / wt_PI;
    let u = fract(atan2(d.x, d.z) / (2.0 * wt_PI));
    let fx = fma(u, f32(wt_dims.x), -0.5);
    let fy = fma(clamp(v, 0.0, 1.0), f32(wt_dims.y), -0.5);
    let ix = i32(floor(fx)); let iy = i32(floor(fy));
    let tx = fx - floor(fx); let ty = fy - floor(fy);
    let a = wt_hdr_texel(ix, iy); let b = wt_hdr_texel(ix + 1, iy);
    let c = wt_hdr_texel(ix, iy + 1); let d2 = wt_hdr_texel(ix + 1, iy + 1);
    let top = fma(b, vec3<f32>(tx), a * (1.0 - tx));
    let bottom = fma(d2, vec3<f32>(tx), c * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}
// optics::raytracer::environment::hdr_rgb_at_exposure: the HDR map's exposure (`values.w`,
// the slot the studio rig keeps its exposure in) scales the texel. Exactly 1.0 skips the
// multiply, so an unexposed map keeps its bits.
fn wt_hdr_at_exposure(rgb: vec3<f32>) -> vec3<f32> {
    let exposure = wt.values.w;
    if (exposure == 1.0) {
        return rgb;
    }
    return rgb * exposure;
}
fn wt_leaf(dir: vec3<f32>, observer: vec3<f32>) -> vec3<f32> {
    switch (wt.modes.y) {
        case 1u: {
            let pos = (clamp(dir.y, -1.0, 1.0) + 1.0) * 511.5;
            let lo = u32(pos); let f = pos - f32(lo);
            return wt_env[lo].xyz + (wt_env[min(lo + 1u, 1023u)].xyz - wt_env[lo].xyz) * f;
        }
        case 2u: { return wt_hdr_at_exposure(wt_hdr(dir)); }
        case 3u: { return wt.env_rgb.xyz; }
        default: { return wt.env_rgb.xyz * wt_studio_g(dir, observer); }
    }
}
