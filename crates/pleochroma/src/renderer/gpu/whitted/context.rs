//! Acquisition for the small kernel. Never requests megakernel/wavefront limits.
use crate::{
    diag::{self, Engine, PrepareStep},
    renderer::gpu::{
        GpuAcquireError, GpuContext,
        context::{note_call_done, request_adapter_logged, request_device_logged, start_failed},
    },
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// Request an adapter and a device limited to the four-storage-buffer kernel.
///
/// # Errors
///
/// Returns [`GpuAcquireError`] when no adapter is available, the adapter lacks
/// four storage and seven total buffer bindings, or device creation fails.
#[cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                  handles); wasm32-unknown-unknown has no second thread regardless"
    )
)]
pub(in crate::renderer::gpu) async fn acquire() -> Result<GpuContext, GpuAcquireError> {
    let engine = Engine::RayTraced;
    // One thread at a time creates an instance, an adapter and a device (`gpu::lifecycle`).
    #[cfg(not(target_arch = "wasm32"))]
    let turn = crate::renderer::gpu::lifecycle::enter();
    diag::set_prepare(engine, PrepareStep::StartingGpu, 0, 0);
    let began = diag::process_ms();
    let instance = wgpu::Instance::new(crate::renderer::gpu::instance_descriptor());
    note_call_done(engine, "Instance::new", began);
    let adapter = request_adapter_logged(&instance, engine).await?;
    let available = adapter.limits();
    if available.max_storage_buffers_per_shader_stage < 4
        || available.max_buffers_and_acceleration_structures_per_shader_stage < 7
    {
        return Err(start_failed(
            engine,
            "limits",
            GpuAcquireError::RequestDevice(
                "Ray-Traced requires four storage and seven total buffer bindings".into(),
            ),
        ));
    }
    let limits = wgpu::Limits {
        max_storage_buffers_per_shader_stage: 4,
        max_buffers_and_acceleration_structures_per_shader_stage: 7,
        ..wgpu::Limits::downlevel_defaults().using_resolution(available)
    };
    let (device, queue) = request_device_logged(
        &adapter,
        engine,
        &wgpu::DeviceDescriptor {
            label: Some("pleochroma Ray-Traced four-storage-buffer device"),
            required_limits: limits,
            required_features: adapter.features() & wgpu::Features::PIPELINE_CACHE,
            ..Default::default()
        },
    )
    .await?;
    let validation_error_seen = Arc::new(AtomicBool::new(false));
    let last_uncaptured_error = Arc::new(Mutex::new(None));
    let flag = Arc::clone(&validation_error_seen);
    let text = Arc::clone(&last_uncaptured_error);
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        tracing::error!(target: diag::TRACER_TARGET, engine = "Ray-Traced", %error, "uncaptured device error");
        flag.store(true, Ordering::Relaxed);
        *text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error.to_string());
    }));
    let flag = Arc::clone(&validation_error_seen);
    let text = Arc::clone(&last_uncaptured_error);
    device.set_device_lost_callback(move |reason, message| {
        tracing::warn!(target: diag::TRACER_TARGET, engine = "Ray-Traced", ?reason, %message, "device lost");
        flag.store(true, Ordering::Relaxed);
        *text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(format!("device lost ({reason:?}): {message}"));
    });
    // The turn ends before the teardown fields exist: dropping them takes the turn again.
    #[cfg(not(target_arch = "wasm32"))]
    let (teardown_begin, teardown_end) = {
        drop(turn);
        crate::renderer::gpu::lifecycle::teardown_pair()
    };
    diag::clear_prepare(engine);
    Ok(GpuContext {
        #[cfg(not(target_arch = "wasm32"))]
        _teardown_begin: teardown_begin,
        instance,
        adapter,
        device,
        queue,
        validation_error_seen,
        last_uncaptured_error,
        #[cfg(feature = "beam")]
        beam_caches: Arc::default(),
        #[cfg(not(target_arch = "wasm32"))]
        _teardown_end: teardown_end,
    })
}
