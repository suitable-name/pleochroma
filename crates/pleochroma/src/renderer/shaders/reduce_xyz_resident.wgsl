// Standalone kernel, not concatenated with other shader files, like `reduce_xyz_live.wgsl`.
// `renderer::gpu::frame::resident` includes it via `include_str!`.
//
// # Why this exists
//
// A GPU-resident adaptive run keeps the frame's summed XYZ and half-buffer B on the GPU, so a
// pass reads nothing back. This is `reduce_xyz_live_main` with the last step changed: the pass
// totals are added into the resident buffers at each pixel's real index, instead of being written
// to a compact output.
//
// # Determinism
//
// Each thread sums in the same ascending sample-index order as `reduce_xyz_live_main`, then adds
// the total into the resident value, as the host's `sum[p] += total` does. Each live pixel appears
// once in the list and belongs to one thread, so there is no race.
//
// # Parity and non-finite samples
//
// As in `reduce_xyz_live.wgsl`: tuple `s` has absolute index `sample_offset + s`, the odd ones also
// feed half-buffer B, and a tuple with a NaN or infinite component adds nothing.

struct GpuReduceResidentParams {
    // Live pixels in this chunk.
    num_pixels: u32,
    // Samples per pixel in this chunk.
    num_samples: u32,
    // Absolute index of the pass's first sample.
    sample_offset: u32,
    // Virtual index (position in the live pixel list) of this chunk's first pixel.
    first_virtual: u32,
}

@group(0) @binding(0) var<uniform> reduce_params: GpuReduceResidentParams;
// The `f32` XYZ triples of the transport kernel's `out_xyz`, read as raw bit patterns so the
// finiteness test below is pure integer work (see `reduce_xyz.wgsl`).
@group(0) @binding(1) var<storage, read> reduce_in_xyz: array<u32>;
// Real pixel index of every virtual pixel of the pass (the transport kernel's `live_pixels`).
@group(0) @binding(2) var<storage, read> live_pixel_list: array<u32>;
// Three floats per frame pixel: the run's summed XYZ.
@group(0) @binding(3) var<storage, read_write> resident_sum: array<f32>;
// Three floats per frame pixel: the run's odd-sample XYZ (half-buffer B).
@group(0) @binding(4) var<storage, read_write> resident_half_b: array<f32>;

const F32_EXPONENT_MASK: u32 = 0x7f800000u;

fn valid_xyz_bits(bits: vec3<u32>) -> bool {
    let exp = bits & vec3<u32>(F32_EXPONENT_MASK);
    return all(exp != vec3<u32>(F32_EXPONENT_MASK));
}

@compute @workgroup_size(64)
fn reduce_xyz_resident_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let local_pixel = gid.x;
    if (local_pixel >= reduce_params.num_pixels) {
        return;
    }

    var sum = vec3<f32>(0.0, 0.0, 0.0);
    var odd = vec3<f32>(0.0, 0.0, 0.0);
    let base_tuple = local_pixel * reduce_params.num_samples;
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

    let real_pixel = live_pixel_list[reduce_params.first_virtual + local_pixel];
    let out_base = real_pixel * 3u;
    resident_sum[out_base + 0u] = resident_sum[out_base + 0u] + sum.x;
    resident_sum[out_base + 1u] = resident_sum[out_base + 1u] + sum.y;
    resident_sum[out_base + 2u] = resident_sum[out_base + 2u] + sum.z;
    resident_half_b[out_base + 0u] = resident_half_b[out_base + 0u] + odd.x;
    resident_half_b[out_base + 1u] = resident_half_b[out_base + 1u] + odd.y;
    resident_half_b[out_base + 2u] = resident_half_b[out_base + 2u] + odd.z;
}
