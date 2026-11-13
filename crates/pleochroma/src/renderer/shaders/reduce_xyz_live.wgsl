// Standalone kernel, not concatenated with other shader files, like `reduce_xyz.wgsl`.
// `renderer::gpu::frame` and `renderer::gpu::event_wavefront` include it via `include_str!`.
//
// # Why this exists
//
// An adaptive pass adds the samples `sample_offset..sample_offset + num_samples` to every pixel
// of the live tiles, and it also needs the sum of the ODD absolute sample indices for half-buffer
// B (see `renderer/adaptive/mod.rs`). This is `reduce_xyz_main` with that second sum: one thread
// per live pixel, six floats out, the XYZ sum of all samples and the XYZ sum of the odd ones.
//
// # Determinism
//
// Each thread sums in ascending sample-index order, as `reduce_xyz_main` does, so the first three
// floats are bit-identical to its output. The odd sum adds the same terms in the same order. No
// atomics and no dependence on thread order.
//
// # Parity
//
// Tuple `s` of a pixel is the sample with absolute index `sample_offset + s`, so a pass that starts
// at an odd `sample_offset` puts its first sample into half-buffer B.
//
// # Non-finite samples: dropped but still counted
//
// As in `reduce_xyz.wgsl`: a tuple with a NaN or infinite component adds nothing to either sum,
// and the caller still counts it.

struct GpuReduceLiveParams {
    num_pixels: u32,
    num_samples: u32,
    sample_offset: u32,
    _pad0: u32,
}

@group(0) @binding(0) var<uniform> reduce_params: GpuReduceLiveParams;
// The `f32` XYZ triples of `out_xyz`, read as raw bit patterns (see `reduce_xyz.wgsl`).
@group(0) @binding(1) var<storage, read> reduce_in_xyz: array<u32>;
// Six floats per live pixel: summed XYZ, then odd-sample XYZ.
@group(0) @binding(2) var<storage, read_write> out_live_xyz: array<f32>;

const F32_EXPONENT_MASK: u32 = 0x7f800000u;

fn valid_xyz_bits(bits: vec3<u32>) -> bool {
    let exp = bits & vec3<u32>(F32_EXPONENT_MASK);
    return all(exp != vec3<u32>(F32_EXPONENT_MASK));
}

@compute @workgroup_size(64)
fn reduce_xyz_live_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x;
    if (pixel >= reduce_params.num_pixels) {
        return;
    }

    var sum = vec3<f32>(0.0, 0.0, 0.0);
    var odd = vec3<f32>(0.0, 0.0, 0.0);
    let base_tuple = pixel * reduce_params.num_samples;
    for (var s: u32 = 0u; s < reduce_params.num_samples; s = s + 1u) {
        let base = (base_tuple + s) * 3u;
        let bits = vec3<u32>(
            reduce_in_xyz[base + 0u],
            reduce_in_xyz[base + 1u],
            reduce_in_xyz[base + 2u],
        );
        if (valid_xyz_bits(bits)) {
            let sample = bitcast<vec3<f32>>(bits);
            sum.x = sum.x + sample.x;
            sum.y = sum.y + sample.y;
            sum.z = sum.z + sample.z;
            if (((reduce_params.sample_offset + s) & 1u) == 1u) {
                odd.x = odd.x + sample.x;
                odd.y = odd.y + sample.y;
                odd.z = odd.z + sample.z;
            }
        }
    }

    let out_base = pixel * 6u;
    out_live_xyz[out_base + 0u] = sum.x;
    out_live_xyz[out_base + 1u] = sum.y;
    out_live_xyz[out_base + 2u] = sum.z;
    out_live_xyz[out_base + 3u] = odd.x;
    out_live_xyz[out_base + 4u] = odd.y;
    out_live_xyz[out_base + 5u] = odd.z;
}
