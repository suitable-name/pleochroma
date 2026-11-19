//! A Path-Traced kernel is built, persisted and loaded again through the pipeline cache folder.
//!
//! This is its own test binary because the cache folder is process-wide and the first call
//! wins (`pipeline_cache::set_pipeline_cache_dir`). Run it by hand on a machine with a GPU:
//! `cargo test -p pleochroma --features gpu --test pipeline_cache_ew -- --ignored`.
#![cfg(feature = "gpu")]

use pleochroma::renderer::gpu::{
    GpuContext,
    event_wavefront::{EventWavefrontRenderer, EwKernel},
    frame::material_class,
    pipeline_cache,
};

/// The isotropic pass class, as the phone traces it.
const ISOTROPIC: u32 = material_class::ISOTROPIC
    | material_class::NO_ORIENTED
    | material_class::NO_ZONED_ORIENTED
    | material_class::NO_EXTRAORDINARY_DISPERSION;

/// The shard files stored for one adapter, `*.bin` under its folder.
fn shard_files(adapter_dir: &std::path::Path) -> usize {
    std::fs::read_dir(adapter_dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "bin"))
            .count()
    })
}

#[test]
#[ignore = "needs a GPU"]
fn a_path_traced_kernel_is_persisted_and_loaded_again() {
    let dir = std::env::temp_dir().join(format!(
        "pleochroma-pipeline-cache-ew-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        pipeline_cache::set_pipeline_cache_dir(dir.clone()),
        "this binary must be the first to set a cache folder"
    );

    let ctx = GpuContext::acquire().expect("a GPU device");
    if !ctx
        .device
        .features()
        .contains(wgpu::Features::PIPELINE_CACHE)
    {
        eprintln!("skipped: the device does not offer PIPELINE_CACHE");
        return;
    }
    let info = ctx.adapter.get_info();
    let Some(key) = wgpu::util::pipeline_cache_key(&info) else {
        eprintln!("skipped: the adapter has no pipeline cache key");
        return;
    };
    let adapter_dir = dir.join("wgpu30-v2").join(key);

    // First start: the reduce kernel and the generate kernel compile and are saved.
    let fresh_before = pipeline_cache::fresh_compiles_on_this_thread();
    let mut renderer = EventWavefrontRenderer::new(ctx);
    let _ = renderer.ensure_pipeline(EwKernel::Generate, ISOTROPIC);
    assert!(
        pipeline_cache::fresh_compiles_on_this_thread() > fresh_before,
        "the first start compiles from source"
    );
    assert!(
        shard_files(&adapter_dir) >= 2,
        "the reduce kernel and the generate kernel each stored a shard in {adapter_dir:?}"
    );
    drop(renderer);

    // Second start, on a fresh device: the blobs load ("blob loaded: n bytes") and the same
    // kernels build without a fresh compile.
    let ctx = GpuContext::acquire().expect("a second GPU device");
    let fresh_before = pipeline_cache::fresh_compiles_on_this_thread();
    let mut renderer = EventWavefrontRenderer::new(ctx);
    let _ = renderer.ensure_pipeline(EwKernel::Generate, ISOTROPIC);
    assert_eq!(
        pipeline_cache::fresh_compiles_on_this_thread(),
        fresh_before,
        "the second start loads both shards instead of compiling"
    );
    drop(renderer);
    let _ = std::fs::remove_dir_all(&dir);
}
