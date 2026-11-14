// ---------------------------------------------------------------------------------
// Exit-event spectral splitting: the pure per-channel helpers of optics::raytracer::refraction
// (`compute_channel_transmission`, `compute_uniaxial_exit_transmission`, `narrow_compat`).
// Each is a direct transcription of its CPU function (`f32::mul_add` -> `fma`, same clamp
// bounds, same operation order). Each function's comment names its CPU counterpart.
//
// The two `compute_*` functions are private to `refraction.rs`, and `narrow_compat` is
// `pub(super)`, so code outside `optics::raytracer` cannot call them. The CPU reference in
// `renderer::gpu::transport_check::p6_exit_splitting` therefore holds verbatim copies of them
// instead of calling the CPU code.
// ---------------------------------------------------------------------------------

// optics::raytracer::refraction::compute_channel_transmission. WGSL has no `Option`, so
// `azimuth_valid == false` stands in for the CPU's `None`, as the `valid` field does in
// `entry_eigenmode_selection`. Callers pass `false`/`0.0`/`0.0` when `entering_anisotropic`
// is false or the entry mode-selection draw was invalid.
struct ChannelTransmissionW {
    transmitted: vec4<f32>,
    r_unpol_k: f32,
}

fn compute_channel_transmission(
    n1k: f32,
    n2k: f32,
    cos_i: f32,
    cos_t_k: f32,
    r_unpol: f32,
    entering_anisotropic: bool,
    azimuth_valid: bool,
    cos_2psi_x: f32,
    sin_2psi_x: f32,
    incident_stokes_k: vec4<f32>,
) -> ChannelTransmissionW {
    let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
    let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
    let trans_matrix_k = mueller_fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    var incident_k = incident_stokes_k;
    if (entering_anisotropic && azimuth_valid) {
        let i_k = incident_stokes_k.x;
        incident_k = vec4<f32>(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0);
    }
    let transmitted = (trans_matrix_k * incident_k) * (1.0 / (1.0 - r_unpol));
    let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);
    let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);
    let r_unpol_k = clamp(0.5 * fma(r_p_k, r_p_k, r_s_k * r_s_k), 1e-4, 1.0 - 1e-4);
    var result: ChannelTransmissionW;
    result.transmitted = transmitted;
    result.r_unpol_k = r_unpol_k;
    return result;
}

// optics::raytracer::refraction::compute_uniaxial_exit_transmission.
struct UniaxialExitTransmissionW {
    transmitted: vec4<f32>,
    i_unit: f32,
}

fn compute_uniaxial_exit_transmission(
    sol: InternalSolW,
    r_branch: f32,
    incident_i: f32,
) -> UniaxialExitTransmissionW {
    let flux_inc = max(sol.flux_inc, 1e-12);
    let ts_n = cplx_scale(sol.t_s, sqrt(sol.flux_ts / flux_inc));
    let tp_n = cplx_scale(sol.t_p, sqrt(sol.flux_tp / flux_inc));
    let i_unit = cplx_norm_sqr(ts_n) + cplx_norm_sqr(tp_n);
    let q_unit = cplx_norm_sqr(ts_n) - cplx_norm_sqr(tp_n);
    let cross_st = cplx_mul(ts_n, cplx_conj(tp_n));
    let u_unit = 2.0 * cross_st.re;
    let v_unit = -2.0 * cross_st.im;
    let transmitted = vec4<f32>(
        incident_i * i_unit, incident_i * q_unit, incident_i * u_unit, incident_i * v_unit,
    ) * (1.0 / (1.0 - r_branch));
    var result: UniaxialExitTransmissionW;
    result.transmitted = transmitted;
    result.i_unit = i_unit;
    return result;
}

// optics::raytracer::refraction::{DIRECTION_MATCH_DISTANCE_SQ, directions_match}: whether two
// unit directions are within the direction-match tolerance, an angle of 1.414 mrad (cosine
// 1 - 1e-6). Tested as the squared distance (`|a - b|^2 = 2 (1 - cos)`), not as the cosine:
// next to 1 an f32 cosine has steps of 6e-8, 17 of them down to 1 - 1e-6, so `dot >= 1 - 1e-6`
// flipped with a rounding difference of one ULP; the difference vector keeps full precision.
const DIRECTION_MATCH_DISTANCE_SQ: f32 = 2.0e-6;

fn directions_match(a: vec3<f32>, b: vec3<f32>) -> bool {
    let d = a - b;
    return dot(d, d) <= DIRECTION_MATCH_DISTANCE_SQ;
}

// optics::raytracer::refraction::narrow_compat. `compat[c]` bit `j` set means channels
// `c`/`j` have refracted within the direction-match tolerance of each other at every
// interior dispersive event so far -- see `ExitSplitCtx::compat`'s own CPU-side doc
// comment. Hero is always channel 0 in this kernel's convention (`path_pdf[0]`
// throughout `spectral_transport.wgsl`), so this is specialized to that fixed hero
// index rather than taking one as a parameter, unlike the CPU function's `hero: usize`.
// `dirs_valid` and `hero_match` are channel bit masks: bit `k` of `dirs_valid` clear stands in
// for the CPU `dirs[k]: Option<Vec3> == None` case, and bit `k` of `hero_match` is the CPU
// `hero_match[k]`. Masks, not `array<bool, 8>` values, so no bool array is copied into function
// memory at the call.
fn narrow_compat(
    compat: ptr<function, array<u32, 8>>,
    dirs: array<vec3<f32>, 8>,
    dirs_valid: u32,
    hero_match: u32,
) {
    for (var a: u32 = 0u; a < 8u; a = a + 1u) {
        for (var b: u32 = a + 1u; b < 8u; b = b + 1u) {
            var matches_ab: bool = true;
            if (a == 0u) {
                matches_ab = ((hero_match >> b) & 1u) != 0u;
            } else if (b == 0u) {
                matches_ab = ((hero_match >> a) & 1u) != 0u;
            } else if (((dirs_valid >> a) & (dirs_valid >> b) & 1u) != 0u) {
                matches_ab = directions_match(dirs[a], dirs[b]);
            } else {
                matches_ab = true;
            }
            if (!matches_ab) {
                (*compat)[a] = (*compat)[a] & ~(1u << b);
                (*compat)[b] = (*compat)[b] & ~(1u << a);
            }
        }
    }
}

// optics::raytracer::refraction::exit_split::leaf_mis_weight with the hero in channel 0: the
// spectral-MIS weight of a collected leaf of a channel whose family is `compat_c`.
// `8 * path_pdf[0] / sum of path_pdf over (compat_c & producers)`; zero when the hero is not in
// that family or the family carries no density. `producers` is the bit mask of the techniques
// that produce the leaf: the channels that are not totally reflected at an interior event,
// 0xFF at an entry.
fn collect_leaf_weight(compat_c: u32, path_pdf: array<f32, 8>, producers: u32) -> f32 {
    let family = compat_c & producers;
    if ((family & 1u) == 0u) {
        return 0.0;
    }
    var family_pdf: f32 = 0.0;
    for (var j: u32 = 0u; j < 8u; j = j + 1u) {
        if ((family & (1u << j)) != 0u) {
            family_pdf = family_pdf + path_pdf[j];
        }
    }
    if (family_pdf <= 1e-12) {
        return 0.0;
    }
    return 8.0 * path_pdf[0] / family_pdf;
}

