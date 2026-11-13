// Per-pixel reduction of the beam fragment slots (beam guide 14.7 item 2, "Output").
//
// One invocation per pixel of the batch's bounding rectangle. It sums the pixel's fragment slots
// in the CPU-built CSR order (leaf order, then the leaf's row-major pixels), with plain f32 adds
// and no atomics, so the sum is deterministic and equals the CPU sum in the same order bit for
// bit. Bindings 200-204 (the beam range, 200-219).
//
// Layout (`ReduceParams` in `gpu/beam/buffers.rs`, uniform, 16 bytes): x = pixel count of the
// rectangle, yzw unused.

// The fragment slots from the fragment kernel: XYZ and a spare lane, in the plan's order.
@group(0) @binding(200) var<storage, read> beam_reduce_slots: array<vec4<f32>>;
// CSR offsets: pixel `i` owns `beam_reduce_slot_ids[offsets[i] .. offsets[i + 1]]`.
@group(0) @binding(201) var<storage, read> beam_reduce_offsets: array<u32>;
// The slot indices of every pixel, in summation order.
@group(0) @binding(202) var<storage, read> beam_reduce_slot_ids: array<u32>;
// The per-pixel sums: XYZ and zero.
@group(0) @binding(203) var<storage, read_write> beam_reduce_out: array<vec4<f32>>;
@group(0) @binding(204) var<uniform> beam_reduce_params: vec4<u32>;

@compute @workgroup_size(64)
fn beam_reduce_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
) {
    let index = gid.y * (groups.x * 64u) + gid.x;
    if (index >= beam_reduce_params.x) {
        return;
    }
    let first = beam_reduce_offsets[index];
    let last = beam_reduce_offsets[index + 1u];
    var sum = vec3<f32>(0.0, 0.0, 0.0);
    for (var k = first; k < last; k = k + 1u) {
        sum = sum + beam_reduce_slots[beam_reduce_slot_ids[k]].xyz;
    }
    beam_reduce_out[index] = vec4<f32>(sum, 0.0);
}
