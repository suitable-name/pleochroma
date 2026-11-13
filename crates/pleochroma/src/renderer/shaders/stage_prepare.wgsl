// Between two stages of the staged megakernel (`spectral_transport_staged.wgsl`): turns the
// survivor count of the stage that just ran (`stage.out_counter`) into the indirect dispatch
// arguments of the next one, three words at `stage.out_counter * 3`. One thread. It is a unit of
// its own, so it does not compile the transport code.

// The same struct as in `spectral_transport_staged.wgsl` (`frame::staged::GpuStageParams`).
struct StageParams {
    mode: u32,
    first_bounce: u32,
    end_bounce: u32,
    in_base: u32,
    in_counter: u32,
    in_capacity: u32,
    out_base: u32,
    out_counter: u32,
    out_capacity: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(150) var<uniform> stage: StageParams;
@group(0) @binding(152) var<storage, read_write> stage_counters: array<atomic<u32>>;
@group(0) @binding(153) var<storage, read_write> stage_indirect: array<u32>;

@compute @workgroup_size(1)
fn transport_stage_prepare() {
    let survivors = min(atomicLoad(&stage_counters[stage.out_counter]), stage.out_capacity);
    let at = stage.out_counter * 3u;
    stage_indirect[at] = (survivors + 63u) / 64u;
    stage_indirect[at + 1u] = 1u;
    stage_indirect[at + 2u] = 1u;
}
