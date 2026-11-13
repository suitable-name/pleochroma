// Standalone kernel: not concatenated with the transport code. `renderer::gpu::frame` includes it
// via `include_str!` and assumes no symbol from another shader file is in scope.
//
// `transport_main` writes one XYZ triple per (pixel, sample) thread into `out_xyz`. Summing
// those per pixel on the host would read back about 200 MB per progressive pass at 1080p with
// 8 spp. `reduce_xyz_main` sums on the GPU, one thread per pixel, so only the per-pixel sums
// cross the bus.
//
// # Determinism
//
// Each thread sums its pixel's `num_samples` tuples in ascending sample-index order, the order
// `GpuFrameRenderer::drain_pending_chunk` uses on the CPU. Float addition is not associative,
// so the order must match for bit-identical results in `run_chunk_equivalence` and
// `estimator_check`.
//
// # Non-finite samples: dropped but still counted
//
// A tuple with any NaN or infinite component adds nothing to its pixel, but the caller still
// counts it in the sample count it divides by. Every backend uses this rule (CPU twin:
// `optics::raytracer::add_finite_sample`), so sums from different backends merge by plain
// addition. Keep the two in step.

struct GpuReduceParams {
    num_pixels: u32,
    num_samples: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> reduce_params: GpuReduceParams;
// The `f32` XYZ triples in `out_xyz`, read as raw bit patterns (`u32`, same 4-byte stride) so
// the finiteness test is integer work on the loaded bits. See `valid_xyz_bits`.
@group(0) @binding(1) var<storage, read> reduce_in_xyz: array<u32>;
@group(0) @binding(2) var<storage, read_write> out_pixel_xyz: array<f32>;

// IEEE-754 binary32 exponent field. A value is finite exactly when its exponent is not all ones.
const F32_EXPONENT_MASK: u32 = 0x7f800000u;

// GPU twin of `Vec3::is_finite` in `optics::raytracer::add_finite_sample`: true when all three
// components are finite. The test runs on the bit pattern, not with `x == x`, because WGSL lets
// the compiler assume no NaN or infinity, so a float-side test could be folded away.
fn valid_xyz_bits(bits: vec3<u32>) -> bool {
    let exp = bits & vec3<u32>(F32_EXPONENT_MASK);
    return all(exp != vec3<u32>(F32_EXPONENT_MASK));
}

@compute @workgroup_size(64)
fn reduce_xyz_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x;
    if (pixel >= reduce_params.num_pixels) {
        return;
    }

    // Ascending sample-index order, see Determinism above.
    var sum = vec3<f32>(0.0, 0.0, 0.0);
    let base_tuple = pixel * reduce_params.num_samples;
    for (var s: u32 = 0u; s < reduce_params.num_samples; s = s + 1u) {
        let base = (base_tuple + s) * 3u;
        let bits = vec3<u32>(
            reduce_in_xyz[base + 0u],
            reduce_in_xyz[base + 1u],
            reduce_in_xyz[base + 2u],
        );
        // Dropped but still counted, see above.
        if (valid_xyz_bits(bits)) {
            let sample = bitcast<vec3<f32>>(bits);
            sum.x = sum.x + sample.x;
            sum.y = sum.y + sample.y;
            sum.z = sum.z + sample.z;
        }
    }

    out_pixel_xyz[pixel * 3u + 0u] = sum.x;
    out_pixel_xyz[pixel * 3u + 1u] = sum.y;
    out_pixel_xyz[pixel * 3u + 2u] = sum.z;
}
