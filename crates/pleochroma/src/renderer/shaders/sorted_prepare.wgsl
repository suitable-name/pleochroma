// The two one-thread kernels of the sorted wavefront (`renderer::gpu::frame::sorted`). They are a
// unit of their own, so they do not compile the transport code.
// `sw_prepare` opens a round: it gives the slots freed by the last round to new tuples, writes the
// indirect arguments of the front kernel and of every arm kernel, and empties the queues of the
// other side, which the round fills.
// `sw_prepare_drain` lays out what the rounds left for `sw_drain_main`: the queued rays of the side
// the next round would read, then the tuples not yet started.

@group(0) @binding(160) var<uniform> sw: SwParams;
@group(0) @binding(162) var<storage, read_write> sw_counts: array<atomic<u32>>;
@group(0) @binding(163) var<storage, read_write> sw_indirect: array<u32>;

// Writes the indirect arguments at `at`: workgroups of `SW_WORKGROUP` covering `items` invocations.
fn sw_set_groups(at: u32, items: u32) {
    sw_indirect[at * 3u] = (items + SW_WORKGROUP - 1u) / SW_WORKGROUP;
    sw_indirect[at * 3u + 1u] = 1u;
    sw_indirect[at * 3u + 2u] = 1u;
}

@compute @workgroup_size(1)
fn sw_prepare() {
    let round = atomicLoad(&sw_counts[SW_C_ROUND]);
    let side = round & 1u;
    let other = 1u - side;
    let keep = min(atomicLoad(&sw_counts[sw_q_front(side)]), sw.pool);
    var free_slots: u32 = min(atomicLoad(&sw_counts[sw_q_free(side)]), sw.pool);
    if (round == 0u) {
        free_slots = sw.pool;
    }
    let next = atomicLoad(&sw_counts[SW_C_NEXT]);
    let started = min(free_slots, sw.tuples - next);
    atomicStore(&sw_counts[SW_C_SIDE], side);
    atomicStore(&sw_counts[SW_C_KEEP], keep);
    atomicStore(&sw_counts[SW_C_GEN_BASE], next);
    atomicStore(&sw_counts[SW_C_GEN_IDENTITY], select(0u, 1u, round == 0u));
    atomicStore(&sw_counts[SW_C_NEXT], next + started);
    atomicStore(&sw_counts[SW_C_GENERATED], atomicLoad(&sw_counts[SW_C_GENERATED]) + started);
    atomicStore(&sw_counts[sw_q_front(side)], keep + started);
    sw_set_groups(SW_I_FRONT, keep + started);
    for (var arm: u32 = 1u; arm <= SW_ARMS; arm = arm + 1u) {
        sw_set_groups(SW_I_ARM + arm - 1u, min(atomicLoad(&sw_counts[sw_q_arm(side, arm)]), sw.pool));
        atomicStore(&sw_counts[sw_q_arm(other, arm)], 0u);
    }
    atomicStore(&sw_counts[sw_q_front(other)], 0u);
    atomicStore(&sw_counts[sw_q_free(other)], 0u);
    atomicStore(&sw_counts[SW_C_ROUND], round + 1u);
}

@compute @workgroup_size(1)
fn sw_prepare_drain() {
    let side = atomicLoad(&sw_counts[SW_C_ROUND]) & 1u;
    var end: u32 = min(atomicLoad(&sw_counts[sw_q_front(side)]), sw.pool);
    atomicStore(&sw_counts[SW_C_DRAIN_SIDE], side);
    atomicStore(&sw_counts[SW_C_DRAIN_FRONT_END], end);
    for (var arm: u32 = 1u; arm <= SW_ARMS; arm = arm + 1u) {
        end = end + min(atomicLoad(&sw_counts[sw_q_arm(side, arm)]), sw.pool);
        atomicStore(&sw_counts[SW_C_DRAIN_ARM_END + arm - 1u], end);
    }
    let total = end + sw.tuples - atomicLoad(&sw_counts[SW_C_NEXT]);
    atomicStore(&sw_counts[SW_C_DRAIN_TOTAL], total);
    sw_set_groups(SW_I_DRAIN, total);
}
