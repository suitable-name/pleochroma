// Path cache completion on the GPU: the WGSL twin of
// `renderer::path_cache::shade::Shader::complete_residual` and `shade::is_covered`.
//
// Not a standalone module. `renderer::gpu::path_cache::source::completion_variant` patches the
// megakernel unit and appends `shaders/path_cache/phase_b.wgsl`, `shaders/beam/branch_sig.wgsl` and
// this file. The patched `transport_main` then traces one fired sample of phase B's list per
// invocation, with the megakernel's ray generation, bounce loop and finalize:
//   - the dispatch covers the fired entries of the chunk (`pcc_fired_total`);
//   - the camera ray is the one of the fired position (`pcc_jitter_index`); the pixel, seed and
//     hero wavelengths are the fired sample's (`pcc_pixel`, `pcc_sample`);
//   - after every facet event, `pcc_facet` folds the event into the branch signature;
//   - `pcc_finalize` replaces `transport_finalize_ray`: it finalizes the fired sample's tuple slot,
//     applies the cover test and writes phase B's kept value plus `traced / p` into that slot, so
//     `reduce_xyz_live_main` sums it into the pixel (and into half-buffer B for an odd sample).
//
// Own bindings: none; it reads phase B's (180-184) and the megakernel's.
// No function-scope arrays: the per-channel arrays are `transport_main`'s own, passed through.

// Branch signature of the traced path (`PathEvents::signature`). One fired sample per invocation.
var<private> pcc_sig: vec2<u32> = BRANCH_SIG_SEED;

// Fired entries of the chunk. The host dispatches the tuple count, an upper bound, so invocations
// past this count return right after the shared plane fill.
fn pcc_fired_total() -> u32 {
    return min(atomicLoad(&pcb_fired.count), pcb.fired_capacity);
}

// The real pixel of fired entry `idx`.
fn pcc_pixel(idx: u32) -> u32 {
    return pcb_pixels[pcb_fired.items[idx].x];
}

// The absolute sample index of fired entry `idx`: its seed and hero wavelengths.
fn pcc_sample(idx: u32) -> u32 {
    return pcb_fired.items[idx].y;
}

// Sample index whose jitter gives the camera ray of fired entry `idx` (phase A built position `j`
// from the jitter of sample index `j`).
fn pcc_jitter_index(idx: u32) -> u32 {
    return pcb_fired.items[idx].z;
}

// Signature step of a facet event, as `PathEvents::facet`: Transmit iff the inside flag toggled,
// otherwise the merged reflect code. `is_ext` is the eigenmode after the event.
fn pcc_facet(facet: u32, inside_before: bool, inside_after: bool, is_ext: bool) {
    let kind = select(BRANCH_KIND_REFLECT, BRANCH_KIND_TRANSMIT, inside_before != inside_after);
    pcc_sig = branch_step(pcc_sig, facet, kind, is_ext);
}

// `signature::fold_sig`.
fn pcc_fold_sig(sig: vec2<u32>) -> u32 {
    return sig.x ^ ((sig.y << 16u) | (sig.y >> 16u));
}

// `shade::is_covered` for the fired position. A record covers the traced path when it has the
// folded branch signature and an absorption-free weight above zero at the hero wavelength
// (`pcb_weight_at`; the transmittance does not decide the cover). A path that never escaped
// carries no light, so it counts as covered.
fn pcc_covered(px: PcbPixel, position: u32, lambda_hero: f32, escaped: bool) -> bool {
    if (!escaped) {
        return true;
    }
    let signature = pcc_fold_sig(pcc_sig);
    let first_record = pcb_words[px.position_offsets + position];
    let last_record = pcb_words[px.position_offsets + position + 1u];
    for (var r: u32 = first_record; r < last_record; r = r + 1u) {
        let rec = px.records + r * PCB_RECORD_WORDS;
        if (pcb_words[rec + 7u] == signature && pcb_weight_at(rec, lambda_hero) > 0.0) {
            return true;
        }
    }
    return false;
}

// Replaces `transport_finalize_ray` in this variant's `transport_main`: finalizes the traced
// sample into the fired tuple's slot, applies the cover test, and writes phase B's kept value
// plus `traced / p`.
fn pcc_finalize(
    idx: u32,
    lambdas: array<f32, 8>,
    radiance: ptr<function, array<f32, 8>>,
    split_radiance: array<f32, 8>,
    path_pdf: array<f32, 8>,
    compat: array<u32, 8>,
    path_escaped: bool,
    nee_xyz: vec3<f32>,
    escaped_dir: vec3<f32>,
) {
    let item = pcb_fired.items[idx];
    let slot = item.w * 3u;
    let kept = vec3<f32>(out_xyz[slot], out_xyz[slot + 1u], out_xyz[slot + 2u]);
    transport_finalize_ray(item.w, lambdas, radiance, split_radiance, path_pdf, compat, path_escaped, nee_xyz);
    let traced = vec3<f32>(out_xyz[slot], out_xyz[slot + 1u], out_xyz[slot + 2u]);
    let px = pcb_locate(pcb_pixels[item.x]);
    let position = px.first_position + item.z;
    let probability = min(pcb_position_residual(px.residuals, position) * PCB_RESIDUAL_FIRE_GAIN, 1.0);
    var completion = vec3<f32>(0.0, 0.0, 0.0);
    if (probability > 0.0 && !pcc_covered(px, position, lambdas[0], path_escaped)) {
        completion = traced / probability;
    }
    let total = kept + completion;
    out_xyz[slot] = total.x;
    out_xyz[slot + 1u] = total.y;
    out_xyz[slot + 2u] = total.z;
}
