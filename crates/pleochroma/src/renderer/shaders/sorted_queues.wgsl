// Queue layout of the sorted wavefront (`renderer::gpu::frame::sorted`). `build.rs` puts this file
// in front of both of its units: `sorted_wavefront.wgsl` (the transport kernels) and
// `sorted_prepare.wgsl` (the one-thread kernels). The host mirrors these constants in
// `frame::sorted`, and a test checks them against this text.
//
// A chunk traces `sw.tuples` (pixel, sample) tuples with a pool of `sw.pool` ray slots, in rounds.
// A queue is a region of `sw.pool` slot indices in `sw_queues` (queue q starts at q * sw.pool);
// its length is the counter `sw_counts[q]`. The queues come in two sides: round r reads side
// r & 1 and fills the other side for round r + 1.

// One chunk's parameters (`frame::sorted::GpuSortedParams`).
struct SwParams {
    // The tuples of the chunk: the megakernel's `num_pixels * num_samples`.
    tuples: u32,
    // The ray slots of the pool (at most `tuples`), also the length of every queue region.
    pool: u32,
    // Bit 0 (`SW_FLAG_CHECK`): count and checksum every finished path (the tests).
    flags: u32,
    pad0: u32,
}

const SW_WORKGROUP: u32 = 64u;
// The arms, numbered like `ARM_FROSTED` (1) to `ARM_GENERIC` (5) in the bounce code.
const SW_ARMS: u32 = 5u;
const SW_FLAG_CHECK: u32 = 1u;
// What `sw_arm_slot` returns past the end of its queue.
const SW_NO_SLOT: u32 = 0xFFFFFFFFu;

// The queues. Front: rays whose next step is the front half of a bounce (the new tuples of a
// round, and rays whose last bounce ended in the medium). Arm: rays whose arm is due, one queue
// per arm. Free: the slots of the paths that ended, which the next round gives to new tuples.
const SW_Q_FRONT: u32 = 0u;
const SW_Q_ARM: u32 = 2u;
const SW_Q_FREE: u32 = 12u;
const SW_QUEUES: u32 = 14u;

// Control words of `sw_counts`, after the queue lengths.
// The side the running round reads.
const SW_C_SIDE: u32 = 14u;
// Rounds opened so far.
const SW_C_ROUND: u32 = 15u;
// The next tuple to start.
const SW_C_NEXT: u32 = 16u;
// Entries of this round's front queue that the last round filed; the new tuples follow them.
const SW_C_KEEP: u32 = 17u;
// The first tuple this round starts.
const SW_C_GEN_BASE: u32 = 18u;
// 1 in round 0, where new tuple j takes slot j (no slot has been freed yet).
const SW_C_GEN_IDENTITY: u32 = 19u;
// Tuples the rounds started.
const SW_C_GENERATED: u32 = 20u;
// The drain: the side it reads, the end of the front leftovers, the end of each arm's leftovers
// (+ arm - 1, cumulative) and the number of paths; the paths after the queued ones are the
// tuples from `SW_C_NEXT` on.
const SW_C_DRAIN_SIDE: u32 = 21u;
const SW_C_DRAIN_FRONT_END: u32 = 22u;
const SW_C_DRAIN_ARM_END: u32 = 23u;
const SW_C_DRAIN_TOTAL: u32 = 28u;
// With `SW_FLAG_CHECK`: paths finalized, paths the drain finished, the wrapping sum of the
// finalized tuples and the xor of their hashes.
const SW_C_FINALIZED: u32 = 29u;
const SW_C_DRAINED: u32 = 30u;
const SW_C_SUM: u32 = 31u;
const SW_C_XOR: u32 = 32u;
// Pushes that found their queue full (none, by construction).
const SW_C_OVERFLOW: u32 = 33u;
const SW_COUNTERS: u32 = 34u;

// Indirect dispatch arguments in `sw_indirect`, three words each.
const SW_I_FRONT: u32 = 0u;
const SW_I_ARM: u32 = 1u;
const SW_I_DRAIN: u32 = 6u;
const SW_INDIRECTS: u32 = 7u;

fn sw_q_front(side: u32) -> u32 {
    return SW_Q_FRONT + side;
}

fn sw_q_arm(side: u32, arm: u32) -> u32 {
    return SW_Q_ARM + side * SW_ARMS + arm - 1u;
}

fn sw_q_free(side: u32) -> u32 {
    return SW_Q_FREE + side;
}
