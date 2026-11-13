// Beam tracing (beam guide sections 5.5, 14.3, 14.7): the shared helpers of the WGSL reference
// variant of the megakernel and of the event-wavefront kernels.
//
// This text is APPENDED (after `branch_sig.wgsl`) to a generated transport unit by
// `renderer/gpu/beam/reference_source.rs`, which also applies the anchored one-line patches that
// call these helpers. The shipped sources are never edited. WGSL module-scope declarations are
// order-independent, so appending is enough. The unit-specific `ref_sig_for` (a private variable
// in the megakernel, a per-ray storage buffer in the event wavefront) is appended by the Rust side
// after this file. (This text names neither declaration: the tests check that the megakernel
// variant holds no trace of the event wavefront's buffer.)
//
// What the variant is: the exact-matching reference estimator with exit splitting off.
//   * a channel stays live at a refraction only if its refractive data are bitwise equal to the
//     hero's (`ref_bits_eq`, indices compared, never directions); the hero (channel 0) always
//     matches;
//   * a mismatched channel gets `stokes = 0`, `path_pdf = 0` and stages no split, so the split
//     machinery never fires and the plain spectral integration applies (`ref_integrate_plain`);
//   * the branch signature is accumulated at every facet event (`ref_sig_event`);
//   * `beam_kept` is called per live channel before the spectral integration and zeroes the
//     channels the beams cover.

// Bitwise equality of two f32 values: -0.0 differs from 0.0 and a NaN equals itself bit for bit.
fn ref_bits_eq(a: f32, b: f32) -> bool {
    return bitcast<u32>(a) == bitcast<u32>(b);
}

// One facet event of the branch signature (guide section 14.3 conventions): `facet` is the facet
// index of the hit, `is_ext` the mode AFTER the event, and the kind is derived from the inside
// flag: Transmit iff it toggled, otherwise the merged reflect code (TIR and partial reflection
// are one code).
fn ref_sig_event(
    sig: vec2<u32>,
    facet: u32,
    inside_before: bool,
    inside_after: bool,
    is_ext: bool,
) -> vec2<u32> {
    let kind = select(BRANCH_KIND_REFLECT, BRANCH_KIND_TRANSMIT, inside_before != inside_after);
    return branch_step(sig, facet, kind, is_ext);
}

// The sample's point on the image plane in pixel units, centre of pixel (x, y) at (x, y): the
// camera's own convention (`generate_camera_ray`, where the jitter lies in [-0.5, 0.5)). It
// recomputes the stratified jitter exactly as `transport_generate_ray` draws it, from `idx`
// alone, so it needs no stored state in either pipeline.
fn ref_image_point(idx: u32) -> vec2<f32> {
    // Written in a different operand order than `transport_generate_ray`'s lookup line on
    // purpose: `live_source::live_variant` requires that line to occur exactly once, so the
    // variant must not repeat it.
    let pixel = params.pixel_offset + idx / camera.num_samples;
    let sample_num = params.sample_offset + idx % camera.num_samples;
    let rot_jx = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM));
    let rot_jy = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM));
    let jx = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(sample_num, 3u), rot_jy) - 0.5;
    let width = u32(camera.width);
    return vec2<f32>(f32(pixel % width) + jx, f32(pixel / width) + jy);
}

// The plain spectral integration (`optics::raytracer::color::integrate_channels_to_xyz`): one
// shared MIS weight `N * p_hero / sum(p)`, radiance of every channel weighted by it. With exit
// splitting off a mismatched channel has `path_pdf = 0` and `radiance = 0`, so the sum holds the
// hero and its bitwise twins only and the hero's weight is N.
fn ref_integrate_plain(
    radiance: array<f32, 8>,
    lambdas: array<f32, 8>,
    path_pdf: array<f32, 8>,
) -> vec3<f32> {
    var sum_pdf: f32 = 0.0;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        sum_pdf = sum_pdf + path_pdf[k];
    }
    var mis_weight: f32 = 1.0;
    if (sum_pdf > 1e-12) {
        mis_weight = f32(NUM_CHANNELS) * path_pdf[0] / sum_pdf;
    }
    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let cmf = cie_1931_cmf(lambdas[k]);
        let weighted = radiance[k] * mis_weight;
        xyz = xyz + cmf * (weighted * NORM_FACTOR);
    }
    return xyz;
}

// The shipped `integrate_channels_to_xyz_family` is renamed `..._shipped` by the patcher; under
// the reference estimator every caller (the end-of-trace integration and the scattering-point
// NEE) takes the plain integration instead, so no call site changes and `compat` is unused.
fn integrate_channels_to_xyz_family(
    radiance: array<f32, 8>,
    lambdas: array<f32, 8>,
    path_pdf: array<f32, 8>,
    compat: array<u32, 8>,
) -> vec3<f32> {
    return ref_integrate_plain(radiance, lambdas, path_pdf);
}

// The kept hook, called per live channel (`path_pdf[k] > 0`) before the spectral integration: a
// kept channel is covered by a beam, so the residual sample drops its radiance.
// `idx` is the dispatch-local tuple index; `pixel` is derived from it the way
// `transport_generate_ray` derives it before the live-list lookup (so in a live variant it is the
// virtual pixel; the lane that combines both variants maps it through `live_pixels`).
fn ref_apply_kept(
    idx: u32,
    lambdas: array<f32, 8>,
    path_pdf: array<f32, 8>,
    radiance: ptr<function, array<f32, 8>>,
) {
    let sig = ref_sig_for(idx);
    let pixel = params.pixel_offset + idx / camera.num_samples;
    let image_point = ref_image_point(idx);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        if (path_pdf[k] > 0.0 && beam_kept(pixel, image_point, sig, lambdas[k])) {
            (*radiance)[k] = 0.0;
        }
    }
}

// The no-op stub of the plain reference variant. The kept variant (`reference_kept_variant`
// in `reference_source.rs`) swaps this line for the real body, which calls the kept unit's test of
// `kept.wgsl` (bindings 210-213, the bin rule of guide section 9.2), and appends that unit. This
// unit does not declare those bindings, so the plain variant keeps the stub. (This text does not
// name the kept unit's function: the tests check that the plain variant holds none of it.)
fn beam_kept(pixel: u32, image_point: vec2<f32>, sig: vec2<u32>, lambda: f32) -> bool { return false; }
