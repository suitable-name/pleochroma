const RING_LIGHT_COUNT: u32 = 16u;

fn studio_rig_key_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let cos_lp = cos(light_pitch);
    let sin_lp = sin(light_pitch);
    let cos_ly = cos(light_yaw);
    let sin_ly = sin(light_yaw);
    return normalize(vec3<f32>(cos_lp * sin_ly, sin_lp, cos_lp * cos_ly));
}

fn studio_rig_fill_dir(light_yaw: f32, light_pitch: f32) -> vec3<f32> {
    let fill_yaw = fma(PI, 0.78, light_yaw);
    let fill_pitch = clamp(light_pitch * 0.65, 0.15, 1.2);
    return normalize(vec3<f32>(cos(fill_pitch) * sin(fill_yaw), sin(fill_pitch), cos(fill_pitch) * cos(fill_yaw)));
}

fn studio_rig_ring_dir(i: u32, light_yaw: f32, sin_lp: f32) -> vec3<f32> {
    let angle = fma(f32(i), PI * 2.0 / f32(RING_LIGHT_COUNT), light_yaw);
    return normalize(vec3<f32>(sin(angle) * 0.75, sin_lp * 0.8, cos(angle) * 0.75));
}

// `optics::raytracer::environment::sample_light_tent`. It needs `studio_rig_ring_dir` for the
// black cards on ring slots 4, 8 and 12, so it lives here, not with the other lit models.
fn sample_light_tent(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
    shadow: vec2<f32>,
    tent: vec4<f32>,
    tent_flat: f32,
    tray: f32,
) -> f32 {
    // `tent` = (walls, cards, spark, ground) from `TentParams`; at the light tent's own values
    // the products are exact identities (x * 1.0, ground literal 0.02), see the CPU twin.
    // `tent_flat` blends the wall gradient towards its 30-degree value; 0 skips the blend.
    let horizon = horizon_blend(d);
    var walls = fma(0.08, max(d.y, 0.0), 0.14) * tent.x;
    if (tent_flat > 0.0) {
        walls = fma(0.18 * tent.x, tent_flat, walls * (1.0 - tent_flat));
    }
    var card: f32 = 0.0;
    for (var slot: u32 = 4u; slot < RING_LIGHT_COUNT; slot = slot + 4u) {
        let card_dir = studio_rig_ring_dir(slot, light_yaw, sin_lp);
        card = max(card, smoothstep_f32(CARD_OUTER_COS, CARD_INNER_COS, dot(d, card_dir)));
    }
    walls = walls * fma(card * tent.y, -0.9, 1.0);
    let key = smoothstep_f32(TENT_KEY_OUTER_COS, TENT_KEY_INNER_COS, dot(d, key_dir)) * (1.4 * spot_mult);
    let spark = smoothstep_f32(SPARK_OUTER_COS, SPARK_INNER_COS, dot(d, fill_dir)) * (5.0 * spot_mult) * tent.z;
    let vis = observer_visibility(d, observer, shadow);
    let above = ((walls + key) + spark) * (horizon * vis);
    var ground: f32 = tent.w * (1.0 - horizon);
    if (tray > 0.0) {
        // The lit tray replaces the tent's own ground term.
        ground = ((1.0 - horizon) * vis) * tray;
    }
    return (above + ground) * (spec_power * exposure);
}

fn sample_studio_rig(
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    tray: f32,
) -> f32 {
    let bg_val = max(fma(0.012, fma(d.y, 0.5, 0.5), 0.015), 0.005) * exposure;
    var radiance = bg_val * spec_power;

    let key_dot = max(dot(d, key_dir), 0.0);
    if (key_dot > 0.0) {
        let softbox = powi_u(key_dot, 28u) * 12.0 * spot_mult * exposure;
        radiance = fma(softbox, spec_power, radiance);
    }

    let fill_dot = max(dot(d, fill_dir), 0.0);
    if (fill_dot > 0.0) {
        let fill = powi_u(fill_dot, 18u) * 4.5 * exposure;
        radiance = fma(fill, spec_power, radiance);
    }

    for (var i: u32 = 0u; i < RING_LIGHT_COUNT; i = i + 1u) {
        let ring_dir = studio_rig_ring_dir(i, light_yaw, sin_lp);
        let ring_dot = max(dot(d, ring_dir), 0.0);
        if (ring_dot > 0.96) {
            let spark = (ring_dot - 0.96) / 0.04;
            let intensity = powi_u(spark, 6u) * 22.0 * spot_mult * exposure;
            radiance = fma(intensity, spec_power, radiance);
        }
    }

    if (tray > 0.0) {
        // The lit tray replaces everything below the horizon:
        // `fma(h, rig, ((g * tray) * exposure) * S)`.
        let horizon = horizon_blend(d);
        radiance = fma(horizon, radiance, (((1.0 - horizon) * tray) * exposure) * spec_power);
    }

    return radiance;
}

fn studio_dispatch(
    model: u32,
    d: vec3<f32>,
    spec_power: f32,
    spot_mult: f32,
    exposure: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    light_yaw: f32,
    observer: vec3<f32>,
    shadow: vec2<f32>,
    tent: vec4<f32>,
    tent_flat: f32,
    tray: f32,
) -> f32 {
    switch (model) {
        case 1u: {
            return sample_iso_hemisphere(d, spec_power, exposure, observer, shadow, tray);
        }
        case 2u: {
            return sample_light_tent(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw, observer, shadow, tent, tent_flat, tray);
        }
        case 3u: {
            return sample_daylight_dome(d, spec_power, exposure, key_dir, observer, shadow, tray);
        }
        case 4u: {
            var d65: f32 = 0.0;
            if (tray > 0.0) {
                d65 = d65_relative_spectral_power(spec_power);
            }
            return aset_radiance(d, spec_power, exposure, key_dir, observer, shadow, tray, d65);
        }
        case 5u: {
            return daylight_sun_radiance(d, spec_power, exposure, key_dir, observer, shadow, tray);
        }
        default: {
            return sample_studio_rig(d, spec_power, spot_mult, exposure, key_dir, fill_dir, sin_lp, light_yaw, tray);
        }
    }
}

// `LightingPreset::spectral_power`: the tabulated CIE D65 curve for the D65 presets,
// else a Planckian fit at `studio_temp_k`.
fn studio_spectral_power(lambda_nm: f32) -> f32 {
    if (params.studio_use_d65 != 0u) {
        return d65_relative_spectral_power(lambda_nm);
    }
    return blackbody_spectrum(lambda_nm, params.studio_temp_k);
}

// `key_dir`, `fill_dir` and `sin_lp` are constant for a whole ray. The caller computes them once
// per ray and passes them in, rather than once per channel.
fn sample_studio_environment_with_rig(
    dir_in: vec3<f32>,
    lambda_nm: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) -> f32 {
    let d = normalize(dir_in);
    var spec_power = studio_spectral_power(lambda_nm);
    if (params.studio_model == 4u) {
        // The ASET model reads the wavelength itself (`rig::aset_spec_input`).
        spec_power = lambda_nm;
    }

    return studio_dispatch(
        params.studio_model,
        d,
        spec_power,
        params.studio_spot_mult,
        params.studio_exposure,
        key_dir,
        fill_dir,
        sin_lp,
        params.studio_light_yaw,
        observer,
        vec2<f32>(params.head_shadow_outer_cos, params.head_shadow_inner_cos),
        vec4<f32>(params.tent_walls, params.tent_cards, params.tent_spark, params.tent_ground),
        params.tent_flat,
        params.backdrop,
    );
}

// `renderer::env_map::EnvironmentMap::{direction_to_uv, sample_bilinear, radiance_at}`, ported op
// for op, including the `fma` chains, to stay within the ULP budget of `environment_check`.

// `hdr_wrap_x`, `hdr_clamp_y` and `hdr_direction_to_uv` are in
// `transport_physics/05_nee_env_sampling.wgsl`.

fn hdr_texel(x: u32, y: u32, width: u32) -> vec3<f32> {
    return hdr_texels[y * width + x].xyz;
}

// `EnvironmentMap::sample_bilinear`: same half-texel offset, wrap/clamp neighbour selection and
// interpolation order.
fn hdr_env_sample_bilinear(u_in: f32, v_in: f32) -> vec3<f32> {
    let width = hdr_env_dims.width;
    let height = hdr_env_dims.height;
    let width_i = i32(width);
    let height_i = i32(height);

    let u_wrapped = fract(u_in);
    let v_clamped = clamp(v_in, 0.0, 1.0);
    let fx = fma(u_wrapped, f32(width), -0.5);
    let fy = fma(v_clamped, f32(height), -0.5);

    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;

    let x0i = hdr_wrap_x(i32(x0), width_i);
    let x1i = hdr_wrap_x(i32(x0) + 1, width_i);
    let y0i = hdr_clamp_y(i32(y0), height_i);
    let y1i = hdr_clamp_y(i32(y0) + 1, height_i);

    let p00 = hdr_texel(x0i, y0i, width);
    let p10 = hdr_texel(x1i, y0i, width);
    let p01 = hdr_texel(x0i, y1i, width);
    let p11 = hdr_texel(x1i, y1i, width);

    let top = fma(p10, vec3<f32>(tx), p00 * (1.0 - tx));
    let bottom = fma(p11, vec3<f32>(tx), p01 * (1.0 - tx));
    return fma(bottom, vec3<f32>(ty), top * (1.0 - ty));
}

// An HDR texel's RGB at the map's exposure (`params.hdr_exposure`). An exposure of exactly 1.0
// returns the texel untouched. Callers lift the result with `rgb_to_spectral_radiance`. It applies
// to every HDR radiance read: the miss lookup below and the three NEE draws in
// `06_nee_sampling.wgsl`. The pdfs are unchanged.
fn hdr_rgb_at_exposure(rgb: vec3<f32>) -> vec3<f32> {
    let exposure = params.hdr_exposure;
    if (exposure == 1.0) {
        return rgb;
    }
    return rgb * exposure;
}

// The HDR map's yaw as (cos, sin), from `params.hdr_rot_cos` and `hdr_rot_sin`. World-direction
// lookups go through `hdr_world_to_map(.., hdr_rot())`, and directions drawn from the map through
// `hdr_map_to_world(.., hdr_rot())`. The identity (1.0, 0.0) is skipped.
fn hdr_rot() -> vec2<f32> {
    return vec2<f32>(params.hdr_rot_cos, params.hdr_rot_sin);
}

// The HDR map arm of the environment lookup: world direction into the map frame, bilinear RGB,
// the exposure, then the spectral lift with `rgb_to_spectral_radiance`.
fn hdr_env_radiance_at(dir: vec3<f32>, lambda_nm: f32) -> f32 {
    let uv = hdr_direction_to_uv(hdr_world_to_map(dir, hdr_rot()));
    let rgb = hdr_rgb_at_exposure(hdr_env_sample_bilinear(uv.x, uv.y));
    return rgb_to_spectral_radiance(rgb.x, rgb.y, rgb.z, lambda_nm);
}

fn sample_environment_with_rig(
    dir: vec3<f32>,
    lambda_nm: f32,
    key_dir: vec3<f32>,
    fill_dir: vec3<f32>,
    sin_lp: f32,
    observer: vec3<f32>,
) -> f32 {
    if (params.env_mode == 0u) {
        return rgb_to_spectral_radiance(params.l0, params.l0, params.l0, lambda_nm);
    } else if (params.env_mode == 2u) {
        return hdr_env_radiance_at(dir, lambda_nm);
    }
    return sample_studio_environment_with_rig(dir, lambda_nm, key_dir, fill_dir, sin_lp, observer);
}

// The dispersion, absorption and polarization helpers (`spectral_absorption`,
// `electric_field_direction`, `pleochroic_channel_alpha`, ...) are in `transport_physics/`, which
// `build.rs` concatenates ahead of this file.

