//! The stall reproduction run by `scripts/retest.sh` (`--gate-repro`, `--hang-repro`).
//!
//! The stall happened while several test threads created and dropped GPU instances and devices
//! at once, and finished test threads exited. These tests are that pattern and nothing else. A
//! GPU test acquires a context of its own, compiles one tiny kernel, runs it, reads the result
//! back and drops everything. A CPU test works for a few milliseconds, so test threads end while
//! others are inside the driver and libtest's main thread joins them. All are ignored: the
//! script runs the module with `--include-ignored`, with the lifecycle turn on or off.

use super::{GpuContext, compute};

/// `1` makes a missing adapter a failure.
const REQUIRE_GPU_ENV: &str = "PLEOCHROMA_REQUIRE_GPU";

/// Adds its index to every cell.
const KERNEL: &str = "
@group(0) @binding(0) var<storage, read_write> cells: array<u32>;

@compute @workgroup_size(64)
fn repro_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x < arrayLength(&cells)) {
        cells[gid.x] = cells[gid.x] + gid.x;
    }
}
";

/// The device of one lifecycle, acquired on the calling thread; `None` (the test skips) when no
/// adapter is available and `PLEOCHROMA_REQUIRE_GPU` is not `1`.
pub(super) fn acquire_or_skip(name: &str) -> Option<GpuContext> {
    match GpuContext::acquire() {
        Ok(ctx) => Some(ctx),
        Err(error) => {
            assert!(
                !std::env::var(REQUIRE_GPU_ENV).is_ok_and(|value| value == "1"),
                "{name}: {REQUIRE_GPU_ENV}=1 but no GPU is available: {error}"
            );
            eprintln!("skipping {name}: no adapter ({error})");
            None
        }
    }
}

/// The tiny kernel, compiled on the calling thread.
pub(super) fn compile(ctx: &GpuContext, name: &str) -> wgpu::ComputePipeline {
    compute::create_compute_pipeline(&ctx.device, name, KERNEL, "repro_main")
}

/// Uploads `cells` ones, runs `pipeline` over them, reads them back and checks them, on the
/// calling thread; the buffer and the bind group drop before it returns.
pub(super) fn dispatch_and_check(
    ctx: &GpuContext,
    pipeline: &wgpu::ComputePipeline,
    name: &str,
    cells: u32,
) {
    let start = vec![1u32; cells as usize];
    let buffer = compute::upload(
        &ctx.device,
        name,
        &start,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    );
    let bind_group = compute::bind_buffers(&ctx.device, name, pipeline, &[(0, &buffer)]);
    compute::dispatch_and_wait_for(
        &ctx.device,
        &ctx.queue,
        pipeline,
        &bind_group,
        (cells.div_ceil(64), 1, 1),
        compute::SELF_TEST_WAIT_TIMEOUT,
    );
    let out: Vec<u32> = compute::readback(&ctx.device, &ctx.queue, &buffer, cells as usize);
    assert_eq!(out.len(), cells as usize, "{name}");
    for (expected, value) in (1u32..).zip(&out) {
        assert_eq!(*value, expected, "{name}: cell {}", expected - 1);
    }
}

/// One device lifecycle on the calling thread: acquire, compile, dispatch, read back, drop.
pub(super) fn gpu_lifecycle(name: &str, cells: u32) {
    let Some(ctx) = acquire_or_skip(name) else {
        return;
    };
    let pipeline = compile(&ctx, name);
    dispatch_and_check(&ctx, &pipeline, name, cells);
}

/// A few milliseconds of work without a GPU.
fn cpu_filler(seed: u64) {
    let mut state = seed | 1;
    for _ in 0..2_000_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
    }
    assert_ne!(std::hint::black_box(state), 0);
}

macro_rules! repro_tests {
    ($($name:ident => $body:expr;)*) => {
        $(
            #[test]
            #[ignore = "stall reproduction: run by scripts/retest.sh (--gate-repro, --hang-repro)"]
            fn $name() {
                $body;
            }
        )*
    };
}

repro_tests! {
    m00_gpu => gpu_lifecycle("m00_gpu", 64);
    m01_gpu => gpu_lifecycle("m01_gpu", 256);
    m02_gpu => gpu_lifecycle("m02_gpu", 4096);
    m03_cpu => cpu_filler(3);
    m04_gpu => gpu_lifecycle("m04_gpu", 65_536);
    m05_cpu => cpu_filler(5);
    m06_gpu => gpu_lifecycle("m06_gpu", 64);
    m07_gpu => gpu_lifecycle("m07_gpu", 256);
    m08_gpu => gpu_lifecycle("m08_gpu", 4096);
    m09_cpu => cpu_filler(9);
    m10_gpu => gpu_lifecycle("m10_gpu", 65_536);
    m11_cpu => cpu_filler(11);
    m12_gpu => gpu_lifecycle("m12_gpu", 64);
    m13_gpu => gpu_lifecycle("m13_gpu", 256);
    m14_gpu => gpu_lifecycle("m14_gpu", 4096);
    m15_cpu => cpu_filler(15);
    m16_gpu => gpu_lifecycle("m16_gpu", 65_536);
    m17_cpu => cpu_filler(17);
    m18_gpu => gpu_lifecycle("m18_gpu", 64);
    m19_gpu => gpu_lifecycle("m19_gpu", 256);
    m20_gpu => gpu_lifecycle("m20_gpu", 4096);
    m21_cpu => cpu_filler(21);
    m22_gpu => gpu_lifecycle("m22_gpu", 65_536);
    m23_cpu => cpu_filler(23);
    m24_gpu => gpu_lifecycle("m24_gpu", 64);
    m25_gpu => gpu_lifecycle("m25_gpu", 256);
    m26_gpu => gpu_lifecycle("m26_gpu", 4096);
    m27_cpu => cpu_filler(27);
    m28_gpu => gpu_lifecycle("m28_gpu", 65_536);
    m29_cpu => cpu_filler(29);
    m30_gpu => gpu_lifecycle("m30_gpu", 64);
    m31_gpu => gpu_lifecycle("m31_gpu", 256);
    m32_gpu => gpu_lifecycle("m32_gpu", 4096);
    m33_cpu => cpu_filler(33);
    m34_gpu => gpu_lifecycle("m34_gpu", 65_536);
    m35_cpu => cpu_filler(35);
}
