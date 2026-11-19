//! Adapter/device acquisition for `pleochroma`'s `gpu`-feature compute infrastructure.

use crate::diag::{self, Engine, PrepareStep, Step, StepKind};
use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// The one `wgpu::InstanceDescriptor` every native acquisition path uses.
///
/// Desktop targets get `Backends::PRIMARY` (Vulkan, Metal, DX12) rather than `wgpu`'s default
/// `Backends::all()`. GL is excluded because wgpu-hal 30.0.1 `unwrap`s inside the GL context
/// teardown, on whichever thread drops the last reference, so no probe can make it safe. GL
/// would add almost nothing: Windows has DX12 (and WARP), macOS has Metal, and on old Linux
/// hardware without a Vulkan driver or lavapipe the CPU tracer renders. Android keeps GLES and
/// wasm32 its browser backends. `WGPU_BACKEND=gl` is the explicit, at-your-own-risk opt-in.
pub(in crate::renderer::gpu) fn instance_descriptor() -> wgpu::InstanceDescriptor {
    #[cfg_attr(
        any(target_os = "android", target_arch = "wasm32"),
        allow(unused_mut, reason = "backends are only narrowed on desktop targets")
    )]
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
    {
        descriptor.backends = wgpu::Backends::PRIMARY;
    }
    descriptor.with_env()
}

/// Why [`GpuContext::acquire`] could not obtain a usable GPU.
///
/// Both variants carry the underlying `wgpu` error's `Debug` output as a `String`
/// (avoids exposing the raw error type in this crate's public API), so callers can
/// report a diagnostic and exit nonzero without panicking.
#[derive(Debug, Clone)]
pub enum GpuAcquireError {
    /// No backend/driver on this system produced an adapter at all.
    NoAdapter(String),
    /// An adapter was found but creating a logical device from it failed.
    RequestDevice(String),
}

impl fmt::Display for GpuAcquireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAdapter(msg) => write!(
                f,
                "no wgpu adapter is available on this system (backends tried: Vulkan, DX12, \
                 Metal; set WGPU_BACKEND=gl to try OpenGL at your own risk): {msg}"
            ),
            Self::RequestDevice(msg) => write!(f, "failed to acquire a wgpu device: {msg}"),
        }
    }
}

impl std::error::Error for GpuAcquireError {}

/// Storage buffers the transport megakernel (`transport_main`) binds in one stage.
///
/// Bindings 2-10 and 12-13 of `transport_bounce.wgsl`. Must track that binding list;
/// the layout check only sees per-binding shapes, not the count.
pub const MEGAKERNEL_STORAGE_BUFFERS: u32 = 11;

/// Storage buffers the wavefront `wavefront_bounce` kernel binds in one stage
/// (`wavefront_transport.wgsl`).
pub const WAVEFRONT_STORAGE_BUFFERS: u32 = 27;

/// Total uniform + storage buffer bindings `wavefront_bounce`'s auto-derived pipeline
/// layout reaches in one stage (`wavefront_transport.wgsl`).
///
/// Every scene binding (0-15, `camera` through `wf_params`; see
/// [`super::frame::wavefront_bounce_bind_group`] for why `camera` is included), the 14 per-ray
/// `SoA` arrays (16-29), `ray_pending_light_mis_dir` (34, the light-MIS interior-direction
/// hand-off) and `ray_nee_xyz` (35, the running Henyey-Greenstein scattering-point NEE total):
/// 32 in all. This is not [`WAVEFRONT_STORAGE_BUFFERS`]: `wgpu` counts uniform and storage
/// bindings alike against `max_buffers_and_acceleration_structures_per_shader_stage`, and under
/// the WebGPU baseline the implicit layout derivation fails with "Too many bindings" instead of
/// declining. [`Self::acquire_async`] requests this many (clamped), and
/// [`super::frame::GpuFrameRenderer::set_pipeline_kind`] checks the device granted them before
/// switching to [`super::frame::GpuPipelineKind::Wavefront`].
pub const WAVEFRONT_BOUNCE_TOTAL_BUFFERS: u32 = 32;

/// Upper bound on the per-stage storage-buffer limit requested from an adapter.
const MAX_REQUESTED_STORAGE_BUFFERS_PER_STAGE: u32 = 32;

/// Upper bound on the per-stage combined buffer limit requested from an adapter (see
/// [`WAVEFRONT_BOUNCE_TOTAL_BUFFERS`]), with a little headroom over that count.
const MAX_REQUESTED_BUFFERS_AND_ACCEL_PER_STAGE: u32 = 40;

/// Whether [`GpuContext::acquire_async`] asks for the adapter's own storage-binding and buffer
/// size limits instead of the WebGPU baseline (128 MiB binding, 256 MiB buffer).
///
/// The baseline would move any HDR map above about 8.4 M texels (16 bytes each) and any beam
/// reduction above 128 MiB to the CPU. The renderer's chunk sizing reads the device's real
/// limit (`cap_byte_budget_for_wavefront`, `chunk_pixels_for`), so a larger binding only widens
/// chunks that the byte budget still bounds.
const LIFT_STORAGE_BINDING_LIMIT: bool = true;

/// The `max_storage_buffer_binding_size` and `max_buffer_size` to request from an adapter.
///
/// With `lift` off these are the baseline defaults. With it on they are the adapter's own
/// limits, never below the baseline, and the binding limit never above the buffer limit (a
/// binding cannot be larger than the buffer behind it). The buffer limit is raised with it
/// because a storage binding that large needs a buffer that large.
fn requested_buffer_sizes(adapter: &wgpu::Limits, lift: bool) -> (u64, u64) {
    let baseline = wgpu::Limits::default();
    if !lift {
        return (
            baseline.max_storage_buffer_binding_size,
            baseline.max_buffer_size,
        );
    }
    let buffer = adapter.max_buffer_size.max(baseline.max_buffer_size);
    let binding = adapter
        .max_storage_buffer_binding_size
        .clamp(baseline.max_storage_buffer_binding_size, buffer);
    (binding, buffer)
}

/// A live `wgpu` adapter/device/queue, acquired once and reused by every self-test.
#[derive(Clone)]
pub struct GpuContext {
    /// Declared first, so it drops first: the last clone takes the lifecycle turn here and
    /// holds it while the handles below drop (`gpu::lifecycle`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::renderer::gpu) _teardown_begin: super::lifecycle::TeardownBegin,
    /// wgpu instance.
    pub instance: wgpu::Instance,
    /// Selected adapter.
    pub adapter: wgpu::Adapter,
    /// Logical device.
    pub device: wgpu::Device,
    /// Submission queue.
    pub queue: wgpu::Queue,
    /// Set by the uncaptured-error handler that [`Self::acquire_async`] installs.
    ///
    /// Without that handler `wgpu` panics on an uncaught validation error, possibly mid-dispatch
    /// with a staging buffer still mapped (see `GpuFrameRenderer::abandon_in_flight`). Reading
    /// this flag turns that panic into [`super::frame::GpuFrameError::DeviceLost`]; see
    /// `GpuFrameRenderer::accumulate_turn` and the event wavefront. `Arc` because the handler
    /// closure holds a clone for as long as `wgpu` keeps it registered.
    pub validation_error_seen: Arc<AtomicBool>,
    /// The first GPU failure, with a device-loss callback's details appended if it
    /// arrives later. Kept for this context's lifetime: a failed context must be
    /// replaced, and later rejected submissions must not overwrite the root cause.
    /// The field name is retained for existing diagnostic callers.
    pub last_uncaptured_error: Arc<Mutex<Option<String>>>,
    /// The beam kernels and uploads compiled for this device, shared by every clone of the
    /// context and dropped with the last one (see `gpu::beam::device_caches`).
    #[cfg(feature = "beam")]
    pub(crate) beam_caches: Arc<crate::renderer::gpu::beam::device_caches::DeviceCaches>,
    /// Declared last, so it drops last: gives back the turn `_teardown_begin` took.
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::renderer::gpu) _teardown_end: super::lifecycle::TeardownEnd,
}

impl GpuContext {
    /// The first recorded failure; failure state is sticky until this device is replaced.
    pub(crate) fn failure(&self) -> Option<String> {
        if !self.validation_error_seen.load(Ordering::Acquire) {
            return None;
        }
        Some(
            self.last_uncaptured_error
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| "wgpu reported a GPU error without diagnostic text".to_string()),
        )
    }

    /// Keeps the root error when a rejected submission produces a later polling error.
    pub(crate) fn failure_with_symptom(&self, symptom: &str) -> String {
        self.failure().map_or_else(
            || symptom.to_owned(),
            |first| format!("{first}; subsequent operation failed: {symptom}"),
        )
    }

    /// Synchronously acquires a `wgpu` adapter/device/queue via [`pollster::block_on`].
    ///
    /// Prefers a high-performance adapter but accepts whatever the system offers (no
    /// discrete-GPU requirement).
    ///
    /// # Errors
    ///
    /// [`GpuAcquireError::NoAdapter`] if no backend/driver can produce an adapter, or
    /// [`GpuAcquireError::RequestDevice`] if device creation failed. Callers MUST treat
    /// either as a clean, diagnosable condition to report and exit on -- never panic or
    /// `unwrap`, since "no GPU here" is expected on some machines, not a bug.
    ///
    /// Every acquisition, and the teardown of a context's last clone, takes the process-wide
    /// lifecycle turn (`gpu::lifecycle`): no two threads are inside instance or device creation
    /// or destruction at once, since concurrent calls deadlock in the driver. A test build adds
    /// the watchdog through `gpu::test_support::acquire_serialised`.
    pub fn acquire() -> Result<Self, GpuAcquireError> {
        #[cfg(all(test, not(target_arch = "wasm32")))]
        {
            super::test_support::acquire_serialised()
        }
        #[cfg(not(all(test, not(target_arch = "wasm32"))))]
        {
            pollster::block_on(Self::acquire_async())
        }
    }

    /// Asynchronously acquires a `wgpu` adapter/device/queue.
    ///
    /// # Errors
    ///
    /// [`GpuAcquireError::NoAdapter`] if no backend/driver can produce an adapter, or
    /// [`GpuAcquireError::RequestDevice`] if device creation failed.
    //
    // `pub`: the web GPU app (`pleochroma-web-gpu`) and the wasm32-only
    // `GpuFrameRenderer::new_async` await this directly instead of `pollster::block_on` (which
    // parks the OS thread, unavailable on wasm32).
    //
    // `clippy::future_not_send` fires only for wasm32: `wgpu`'s web backend future holds
    // browser-side `Rc` handles, never `Send` -- and there's no second thread on
    // wasm32-unknown-unknown to send it to anyway. `cfg_attr` scopes the allow to that
    // target so a native regression would still be caught by clippy.
    #[cfg_attr(
        target_arch = "wasm32",
        allow(
            clippy::future_not_send,
            reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                      handles); wasm32-unknown-unknown has no second thread regardless"
        )
    )]
    pub async fn acquire_async() -> Result<Self, GpuAcquireError> {
        let engine = Engine::PathTraced;
        // One thread at a time creates an instance, an adapter and a device; the handles of a
        // failed attempt drop inside the same turn (`lifecycle`).
        #[cfg(not(target_arch = "wasm32"))]
        let turn = super::lifecycle::enter();
        diag::set_prepare(engine, PrepareStep::StartingGpu, 0, 0);
        let began = diag::process_ms();
        let instance = wgpu::Instance::new(instance_descriptor());
        note_call_done(engine, "Instance::new", began);

        let adapter = request_adapter_logged(&instance, engine).await?;
        let (device, queue) = request_device_logged(
            &adapter,
            engine,
            &wgpu::DeviceDescriptor {
                label: Some("pleochroma gpu-feature compute device"),
                // Requested only where the adapter offers it (an empty set elsewhere), so
                // `compute::create_pipeline_cache` returns a cache on Vulkan and `None`
                // on every other backend.
                required_features: adapter.features() & wgpu::Features::PIPELINE_CACHE,
                required_limits: requested_limits(&adapter.limits()),
                ..Default::default()
            },
        )
        .await?;
        let (validation_error_seen, last_uncaptured_error) = install_failure_handlers(&device);

        // The turn ends before the teardown fields exist: dropping them takes the turn again.
        #[cfg(not(target_arch = "wasm32"))]
        let (teardown_begin, teardown_end) = {
            drop(turn);
            super::lifecycle::teardown_pair()
        };
        diag::clear_prepare(engine);
        Ok(Self {
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
}

/// The device limits [`GpuContext::acquire_async`] requests from an adapter.
///
/// wgpu's default limits are the WebGPU baseline, where a compute stage may bind only 8 storage
/// buffers. The transport megakernel binds `MEGAKERNEL_STORAGE_BUFFERS` and the wavefront
/// kernels more, so ask for whatever the adapter really offers (bounded, so a huge Vulkan
/// figure does not become a device requirement) and let `GpuFrameRenderer::new` decline cleanly
/// when even that is not enough.
fn requested_limits(adapter_limits: &wgpu::Limits) -> wgpu::Limits {
    let (max_storage_buffer_binding_size, max_buffer_size) =
        requested_buffer_sizes(adapter_limits, LIFT_STORAGE_BINDING_LIMIT);
    wgpu::Limits {
        max_storage_buffer_binding_size,
        max_buffer_size,
        max_storage_buffers_per_shader_stage: adapter_limits
            .max_storage_buffers_per_shader_stage
            .clamp(
                wgpu::Limits::default().max_storage_buffers_per_shader_stage,
                MAX_REQUESTED_STORAGE_BUFFERS_PER_STAGE,
            ),
        // Separate from `max_storage_buffers_per_shader_stage`: the wavefront pipeline's
        // implicit layout needs this one raised too (see `WAVEFRONT_BOUNCE_TOTAL_BUFFERS`).
        max_buffers_and_acceleration_structures_per_shader_stage: adapter_limits
            .max_buffers_and_acceleration_structures_per_shader_stage
            .clamp(
                wgpu::Limits::default().max_buffers_and_acceleration_structures_per_shader_stage,
                MAX_REQUESTED_BUFFERS_AND_ACCEL_PER_STAGE,
            ),
        ..Default::default()
    }
}

/// Registers the device-lost and uncaptured-error handlers of an acquired device.
///
/// Returns the sticky failure flag and the first failure text they fill.
fn install_failure_handlers(
    device: &wgpu::Device,
) -> (Arc<AtomicBool>, Arc<Mutex<Option<String>>>) {
    let validation_error_seen = Arc::new(AtomicBool::new(false));
    let last_uncaptured_error = Arc::new(Mutex::new(None));
    let lost_flag = Arc::clone(&validation_error_seen);
    let lost_detail = Arc::clone(&last_uncaptured_error);
    device.set_device_lost_callback(move |reason, message| {
        tracing::warn!(?reason, %message, "wgpu device lost");
        record_failure(
            &lost_flag,
            &lost_detail,
            format!("device lost ({reason:?}): {message}"),
            true,
        );
    });

    let error_flag = Arc::clone(&validation_error_seen);
    let error_detail = Arc::clone(&last_uncaptured_error);
    device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
        tracing::error!(%error, "wgpu uncaptured device error");
        record_failure(&error_flag, &error_detail, error.to_string(), false);
    }));
    (validation_error_seen, last_uncaptured_error)
}

/// Writes the tracer line of a graphics-card start call that begins.
pub(in crate::renderer::gpu) fn note_call_start(engine: Engine, call: &str) {
    tracing::info!(
        target: diag::TRACER_TARGET,
        engine = engine.name(),
        call,
        state = "start",
        "GPU start"
    );
}

/// Writes the tracer line of a graphics-card start call that ended, with its time since `began`.
pub(in crate::renderer::gpu) fn note_call_done(engine: Engine, call: &str, began: u64) {
    tracing::info!(
        target: diag::TRACER_TARGET,
        engine = engine.name(),
        call,
        state = "done",
        ms = diag::process_ms().saturating_sub(began),
        "GPU start"
    );
}

/// Ends the preparation step, writes the tracer line of a failed start call, and passes the
/// failure on.
pub(in crate::renderer::gpu) fn start_failed(
    engine: Engine,
    call: &str,
    failure: GpuAcquireError,
) -> GpuAcquireError {
    let (GpuAcquireError::NoAdapter(text) | GpuAcquireError::RequestDevice(text)) = &failure;
    diag::clear_prepare(engine);
    tracing::warn!(
        target: diag::TRACER_TARGET,
        engine = engine.name(),
        call,
        state = "failed",
        error = %text,
        "GPU start"
    );
    failure
}

/// Requests the adapter as a breadcrumb step and records which adapter it is.
///
/// Shared by the Path-Traced and the Ray-Traced acquisition.
///
/// # Errors
///
/// [`GpuAcquireError::NoAdapter`] when no backend produces an adapter.
#[cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                  handles); wasm32-unknown-unknown has no second thread regardless"
    )
)]
pub(in crate::renderer::gpu) async fn request_adapter_logged(
    instance: &wgpu::Instance,
    engine: Engine,
) -> Result<wgpu::Adapter, GpuAcquireError> {
    note_call_start(engine, "request_adapter");
    let began = diag::process_ms();
    let step = diag::enter(&Step {
        kind: StepKind::AdapterRequest,
        engine: Some(engine),
        label: "",
        class: "",
    });
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        })
        .await
        .map_err(|error| {
            start_failed(
                engine,
                "request_adapter",
                GpuAcquireError::NoAdapter(format!("{error:?}")),
            )
        })?;
    drop(step);
    note_call_done(engine, "request_adapter", began);
    // Remember which driver this is, so the app can offer the RADV scheduler workaround
    // (`renderer::gpu_driver`) without touching wgpu itself.
    let info = adapter.get_info();
    crate::renderer::gpu_driver::record_adapter(&info.name, &info.driver, &info.driver_info);
    diag::record_adapter(&diag::adapter_facts(&adapter));
    Ok(adapter)
}

/// Requests the device as a breadcrumb step.
///
/// Shared by the Path-Traced and the Ray-Traced acquisition.
///
/// # Errors
///
/// [`GpuAcquireError::RequestDevice`] when the device cannot be created.
#[cfg_attr(
    target_arch = "wasm32",
    allow(
        clippy::future_not_send,
        reason = "wgpu's web backend futures are inherently !Send (browser-side Rc \
                  handles); wasm32-unknown-unknown has no second thread regardless"
    )
)]
pub(in crate::renderer::gpu) async fn request_device_logged(
    adapter: &wgpu::Adapter,
    engine: Engine,
    descriptor: &wgpu::DeviceDescriptor<'_>,
) -> Result<(wgpu::Device, wgpu::Queue), GpuAcquireError> {
    note_call_start(engine, "request_device");
    let began = diag::process_ms();
    let step = diag::enter(&Step {
        kind: StepKind::DeviceRequest,
        engine: Some(engine),
        label: "",
        class: "",
    });
    let granted = adapter.request_device(descriptor).await.map_err(|error| {
        start_failed(
            engine,
            "request_device",
            GpuAcquireError::RequestDevice(format!("{error:?}")),
        )
    })?;
    drop(step);
    note_call_done(engine, "request_device", began);
    Ok(granted)
}

/// Whether the lifecycle turn stayed taken for the whole `wait`.
///
/// A hung acquisition holds the turn, so a later start that waits this long reports it instead of
/// hanging too. It does not take the turn.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn lifecycle_busy_for(wait: std::time::Duration) -> bool {
    super::lifecycle::busy_for(wait)
}

/// Publish the diagnostic before the flag. Never replace the first error with a
/// cascading submission rejection; a genuine device-loss callback adds its reason.
fn record_failure(
    flag: &AtomicBool,
    detail: &Mutex<Option<String>>,
    message: String,
    device_lost: bool,
) {
    let mut first = detail
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match first.as_mut() {
        Some(first) if device_lost => {
            first.push_str("; ");
            first.push_str(&message);
        }
        None => *first = Some(message),
        Some(_) => {}
    }
    drop(first);
    flag.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::{GpuAcquireError, record_failure, requested_buffer_sizes};
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    fn adapter_with(binding: u64, buffer: u64) -> wgpu::Limits {
        wgpu::Limits {
            max_storage_buffer_binding_size: binding,
            max_buffer_size: buffer,
            ..wgpu::Limits::default()
        }
    }

    /// A Chrome-like adapter (2 GiB binding, 4 GiB buffer) gets both lifted; with the lift off
    /// the baseline is requested whatever the adapter offers.
    #[test]
    fn the_adapters_own_binding_and_buffer_limits_are_requested_when_lifted() {
        let baseline = wgpu::Limits::default();
        let chrome = adapter_with(2_147_483_644, 4_294_967_296);
        assert_eq!(
            requested_buffer_sizes(&chrome, true),
            (2_147_483_644, 4_294_967_296)
        );
        assert_eq!(
            requested_buffer_sizes(&chrome, false),
            (
                baseline.max_storage_buffer_binding_size,
                baseline.max_buffer_size
            )
        );
    }

    /// Never below the baseline, and the binding never above the buffer behind it.
    #[test]
    fn the_lifted_request_stays_between_the_baseline_and_the_buffer_limit() {
        let baseline = wgpu::Limits::default();
        let small = adapter_with(1024, 2048);
        assert_eq!(
            requested_buffer_sizes(&small, true),
            (
                baseline.max_storage_buffer_binding_size,
                baseline.max_buffer_size
            )
        );
        let odd = adapter_with(u64::MAX, 1 << 30);
        assert_eq!(requested_buffer_sizes(&odd, true), (1 << 30, 1 << 30));
    }

    #[test]
    fn cascading_submit_errors_keep_the_first_failure_and_device_loss_reason() {
        let flag = AtomicBool::new(false);
        let detail = Mutex::new(None);
        record_failure(&flag, &detail, "invalid bind group".to_string(), false);
        record_failure(&flag, &detail, "submission was rejected".to_string(), false);
        record_failure(&flag, &detail, "device lost: reset".to_string(), true);
        assert!(flag.load(Ordering::Acquire));
        assert_eq!(
            detail.lock().expect("not poisoned").as_deref(),
            Some("invalid bind group; device lost: reset"),
        );
    }

    /// Desktop targets never create a GL instance unless `WGPU_BACKEND` asks for one: the GL
    /// teardown in wgpu-hal 30.0.1 can abort the process (see [`instance_descriptor`]).
    #[cfg(not(any(target_os = "android", target_arch = "wasm32")))]
    #[test]
    fn instance_descriptor_excludes_gl_on_desktop() {
        if std::env::var_os("WGPU_BACKEND").is_some() {
            return;
        }
        let descriptor = super::instance_descriptor();
        assert!(!descriptor.backends.contains(wgpu::Backends::GL));
        assert!(descriptor.backends.contains(wgpu::Backends::PRIMARY));
    }

    /// The no-adapter message names the backends tried and the GL opt-in.
    #[test]
    fn the_no_adapter_message_names_the_backends_and_the_opt_in() {
        let text = GpuAcquireError::NoAdapter("none".to_owned()).to_string();
        assert!(text.contains("Vulkan, DX12"), "{text}");
        assert!(text.contains("WGPU_BACKEND=gl"), "{text}");
    }
}
