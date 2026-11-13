//! The tracer entry points of the `render` and `serve` paths, starting with [`trace_samples`].
//!
//! It traces an explicit `[first_sample, first_sample + samples)` range across a whole
//! frame and returns the summed (never averaged) per-pixel XYZ radiance -- exactly what
//! a caller accumulating contributions from multiple workers needs to add into its own
//! running total. See `pleochroma_net`'s crate docs for why sample-index partitioning
//! (not screen-space tiling) is what makes that additive.
//!
//! # Keeping the sample draws in sync with the viewer
//!
//! The per-sample seed and jitter draws come from `pleochroma`'s `sample_draws`, the one
//! formula the viewer's renderers use too. It is a function of `(global_pixel_idx,
//! sample_num)` alone, `sample_num` being the absolute sample index, never a batch-relative
//! offset: that is what makes a remote worker's range match the viewer's numbers for the same
//! range. A second copy of the formula would drift silently into a wrong but plausible image.
//!
//! # A process-wide permit pool bounds CPU-tracer threads across connections
//!
//! Every request spawns its own tracer threads, so concurrent requests would each ask for the
//! whole machine and fight over the same cores. [`THREAD_PERMITS`] caps the total: a pool
//! sized to `effective_thread_count(0)`, from which every [`trace_into`] call takes its thread
//! count before its `thread::scope` and returns it right after. A request asking for more than
//! the pool holds is clamped to the pool's capacity (see [`ThreadPermits::acquire`]), so it never
//! waits for permits that could not all be free at once. Permits are held per `trace_into` call
//! (one sub-batch), so a large request interleaves with others rather than starving them.
//!
//! The GPU submitter thread takes no permits: it is not a CPU tracer, and the core it needs is
//! already left out of the CPU request by [`hybrid::cpu_threads_beside_gpu`].

use glam::Vec3;
use pleochroma::optics::{
    materials::GemMaterial,
    raytracer::{
        Camera, DEFAULT_FOV_DEG, EnvironmentSource, FacetFinish, add_finite_sample,
        build_plane_soa, pixel_rotations, sample_draws,
        surface::{SurfaceScene, merged_facet_finishes},
        transport::{PreparedMaterial, trace_spectral_ray_prepared},
    },
};
use pleochroma_dispatch::SampleRange;
use pleochroma_net::SceneState;
use std::{
    sync::{
        Condvar, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use pleochroma::{
    geometry::StoneGeometry,
    renderer::gpu_backend::{
        GpuAccumulate, GpuBackend, GpuRouteDecline, GpuSceneRef, scene_routes_to_gpu_with_tools,
    },
};

use crate::assets::TrayRef;

#[cfg(feature = "beam")]
pub(crate) mod beam;
pub(crate) mod hybrid;
#[cfg(feature = "adaptive-sampling")]
pub(crate) mod live;
#[cfg(feature = "adaptive-sampling")]
pub(crate) mod live_cache;
pub(crate) mod whitted;

/// The stone `scene` describes: its planes with its concave tools subtracted.
///
/// A borrow, so every tracer entry point below builds it for free per call.
#[must_use]
pub(crate) fn stone_geometry(scene: &SceneState) -> StoneGeometry<'_> {
    StoneGeometry {
        planes: &scene.planes,
        tools: &scene.tools,
    }
}

/// Whether `scene` may be offered to the GPU at all (`scene_routes_to_gpu_with_tools`): the
/// material must be GPU-supported, the stone have at most `GPU_MAX_TOOL_PRIMITIVES` concave
/// tools, the material not be fluorescent and the lighting not a UV lamp.
///
/// Concave tools are passed to the GPU with the scene. A scene the GPU cannot trace is traced
/// entirely on the CPU, never by a hybrid split, which would hand part of the samples to a GPU
/// that cannot trace them. An HDR panorama has no lamp, so the preset only counts for the
/// analytic rig.
///
/// A zoned material routes through `GemMaterial::gpu_supported` (inside
/// `scene_routes_to_gpu_with_tools`). A zone the GPU kernels do not support (a mesh-shell zone,
/// a scattering zoned stone, an oversized band set) sends the whole scene to the CPU, never a
/// silent fallback to its base zone.
///
/// The surface finish (`scene.effects`) rides with each GPU request; inclusions and
/// play-of-color ride in the traced material.
///
/// A scene with the Stone Shadow goes to the GPU only when `gpu` can sample the map
/// (`GpuBackend::supports_tray_shadow`), and one with the Tray Reflection only when `gpu` can
/// trace the mirror (`GpuBackend::supports_tray_mirror`). Otherwise it is traced on the CPU,
/// which [`note_tray_route`] reports.
#[must_use]
pub(crate) fn scene_uses_gpu(scene: &SceneState, gpu: &GpuBackend) -> bool {
    // Ray uses its independent small kernel, never spectral hybrid calibration.
    if scene.engine.is_ray() {
        return false;
    }
    if scene.stone_shadow && !gpu.supports_tray_shadow() {
        return false;
    }
    if scene.tray_reflection > 0.0 && !gpu.supports_tray_mirror() {
        return false;
    }
    let lighting = match scene.environment {
        pleochroma_net::scene::SceneEnvironment::Studio => scene.lighting_preset,
        pleochroma_net::scene::SceneEnvironment::Hdr(_) => {
            pleochroma::optics::raytracer::LightingPreset::default()
        }
    };
    scene_routes_to_gpu_with_tools(
        &scene.traced_material(),
        stone_geometry(scene),
        &scene.fluorescence,
        lighting,
    )
}

/// Logs, once per request, a Stone Shadow or Tray Reflection scene that `gpu` cannot trace
/// (`GpuRouteDecline`). The log is the only report: there is no protocol message for it.
/// Silent for a CPU-only worker and for a scene with neither.
pub(crate) fn note_tray_route(scene: &SceneState, gpu: &GpuBackend) {
    if scene.stone_shadow
        && !scene.engine.is_ray()
        && gpu.adapter_label().is_some()
        && !gpu.supports_tray_shadow()
    {
        tracing::info!(
            reason = GpuRouteDecline::STONE_SHADOW_MESSAGE,
            "worker: GPU route declined"
        );
    }
    if scene.tray_reflection > 0.0
        && !scene.engine.is_ray()
        && gpu.adapter_label().is_some()
        && !gpu.supports_tray_mirror()
    {
        tracing::info!(
            reason = GpuRouteDecline::TRAY_REFLECTION_MESSAGE,
            "worker: GPU route declined"
        );
    }
}

/// Resolves the per-facet finish `scene.girdle_frosted` and `scene.frosted_planes` imply.
///
/// `pleochroma_net::SceneState` carries no `Vec<FacetFinish>`: the wire encoding is the
/// `girdle_frosted` bool plus the frosted plane indices. The worker re-derives the same list the
/// viewer does, via `pleochroma::geometry::facet_finishes_with_frosted` or
/// `pleochroma::geometry::girdle_facet_finishes`, which are pure functions of the scene. An
/// empty list means every facet is `FacetFinish::default()`, Polished.
#[must_use]
pub(crate) fn resolve_facet_finishes(scene: &SceneState) -> Vec<FacetFinish> {
    if !scene.frosted_planes.is_empty() {
        pleochroma::geometry::facet_finishes_with_frosted(
            &scene.planes,
            scene.girdle_frosted,
            &scene.frosted_planes,
        )
    } else if scene.girdle_frosted {
        pleochroma::geometry::girdle_facet_finishes(&scene.planes)
    } else {
        Vec::new()
    }
}

/// Resolves a `--threads`-style argument (`0` meaning "let the OS decide") to an actual
/// thread count.
///
/// Shared by [`trace_samples`]'s own chunking and by `serve`'s `Welcome` message (which
/// reports the thread count it actually renders with, not the literal `0` sentinel). The
/// one definition lives in `pleochroma::renderer::tonemap`, shared with the tone-mapper
/// and the denoiser.
pub use pleochroma::renderer::tonemap::effective_thread_count;

/// A process-wide counting semaphore over CPU tracer threads (see the module docs).
struct ThreadPermits {
    /// Total capacity, fixed at construction (`effective_thread_count(0)`); only
    /// [`Self::available`] moves.
    capacity: usize,
    /// How many permits are currently unclaimed, guarded by the `Mutex` the [`Condvar`]
    /// waits on.
    available: Mutex<usize>,
    /// Notified whenever [`Self::release`] returns permits, so a blocked [`Self::acquire`]
    /// re-checks instead of polling.
    freed: Condvar,
}

/// The wait slice of [`ThreadPermits::acquire`] between re-checks of its cancel flag.
const ACQUIRE_POLL: Duration = Duration::from_millis(100);

impl ThreadPermits {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            available: Mutex::new(capacity.max(1)),
            freed: Condvar::new(),
        }
    }

    /// Blocks until `wanted` permits are free, then claims them and returns how many were
    /// taken (`wanted.clamp(1, self.capacity)`); `None` if `cancel` is observed set first,
    /// with nothing claimed.
    ///
    /// The count is clamped to `1..=self.capacity`: waiting for more permits than the pool
    /// holds would never end, and zero permits would skip the accounting.
    ///
    /// Waits in [`ACQUIRE_POLL`] slices and re-checks `cancel` between them, so a request
    /// queued behind a wedged tracer can still be cancelled.
    fn acquire(&self, wanted: usize, cancel: &AtomicBool) -> Option<usize> {
        let wanted = wanted.clamp(1, self.capacity);
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *available < wanted {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            available = self
                .freed
                .wait_timeout(available, ACQUIRE_POLL)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        *available -= wanted;
        drop(available);
        Some(wanted)
    }

    /// Returns `held` permits (from a matching [`Self::acquire`]) to the pool and wakes every
    /// waiter: `notify_all`, since more than one blocked request may now fit.
    fn release(&self, held: usize) {
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *available += held;
        drop(available);
        self.freed.notify_all();
    }
}

/// The process-wide pool, sized once at first use to `effective_thread_count(0)`. Every
/// [`trace_into`] call takes its permits from this one instance.
static THREAD_PERMITS: LazyLock<ThreadPermits> =
    LazyLock::new(|| ThreadPermits::new(effective_thread_count(0)));

/// RAII guard releasing `n` permits to [`THREAD_PERMITS`] on drop, including on unwind, so
/// a panicking tracer never leaks permits.
struct PermitGuard(usize);

impl Drop for PermitGuard {
    fn drop(&mut self) {
        THREAD_PERMITS.release(self.0);
    }
}

/// Traces samples `[first_sample, first_sample + samples)` across the whole frame.
///
/// Parallel across `threads` CPU threads (`0` for "all available cores" -- see
/// [`effective_thread_count`]), over `scene.width x scene.height`.
///
/// Returns the summed (not averaged) per-pixel XYZ radiance, one [`Vec3`] per pixel in
/// row-major order. Callers that want a displayable image divide by their own total
/// sample count when tone-mapping; callers accumulating contributions from multiple
/// workers add these sums directly into their own running total -- this function itself
/// never divides.
///
/// Returns an all-[`Vec3::ZERO`] buffer (still correctly sized) if `samples`, `width`,
/// or `height` is zero -- callers are expected to have rejected those via
/// [`crate::validate`] if invalid for their use case; this function has no opinion.
///
/// A scene with the Stone Shadow is traced with the `Final` map, and one with the Tray Reflection
/// with its mirror, both built here.
///
/// # Panics
///
/// When `scene.stone_shadow` or `scene.tray_reflection` is set and the map or the mirror cannot
/// be built (a stone that is not closed; the lighting cases are refused by
/// [`crate::validate::validate_scene`]). This convenience entry has no way to refuse a request;
/// the request paths resolve them first ([`crate::assets::resolve_tray`]) and answer with the
/// refusal instead.
#[must_use]
pub fn trace_samples(
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
) -> Vec<Vec3> {
    let width = scene.width;
    let height = scene.height;
    let mut buffer = vec![Vec3::ZERO; width as usize * height as usize];
    if samples == 0 || width == 0 || height == 0 {
        return buffer;
    }

    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll);
    // The studio rig, or the HDR map the request path resolved -- see
    // `crate::assets::resolved_hdr_map`.
    let cancel = &AtomicBool::new(false);
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let tray = crate::assets::resolve_tray(scene, None, cancel)
        .expect("the shadow and the mirror of this scene can be built (see `# Panics`)");
    let environment = crate::assets::environment_for(scene, hdr_map.as_deref(), tray.view());

    // This path has no cancellation, so a permit wait cannot be cut short and `trace_into`
    // returns `true`.
    let never_cancel = AtomicBool::new(false);
    let _ = trace_into(
        scene,
        SampleRange::new(first_sample, samples),
        threads,
        &camera,
        environment,
        &mut buffer,
        &never_cancel,
    );
    buffer
}

/// Traces samples `[first_sample, first_sample + samples)` across the whole frame,
/// preferring `gpu`.
///
/// Falls back to the CPU tracer (via [`trace_into`], the same code [`trace_samples`]
/// runs) whenever `gpu` declines -- a decline is normal, not an error.
///
/// A thin wrapper over [`trace_samples_with_gpu_cancellable`] with a cancellation flag
/// that is never set, used by `render_cmd`, which has no cancellation.
///
/// Same contract as [`trace_samples`]: summed, never averaged, and disjoint sample ranges add
/// up, so a GPU worker's ranges merge with a CPU viewer's.
///
/// A scene with the Stone Shadow is traced with the `Final` map, and one with the Tray Reflection
/// with its mirror, both built here.
///
/// # Panics
///
/// Does not panic for a scene with neither: the dispatch cannot observe a cancellation, since
/// `never_cancel` is never set.
///
/// With `scene.stone_shadow` or `scene.tray_reflection` set it panics when the map or the mirror
/// cannot be built (a stone that is not closed; the lighting cases are refused by
/// [`crate::validate::validate_scene`]). This convenience entry has no way to refuse a request;
/// the request paths resolve them first ([`crate::assets::resolve_tray`]) and answer with the
/// refusal instead.
#[must_use]
pub fn trace_samples_with_gpu(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
) -> Vec<Vec3> {
    let never_cancel = AtomicBool::new(false);
    let tray = crate::assets::resolve_tray(scene, None, &never_cancel)
        .expect("the shadow and the mirror of this scene can be built (see `# Panics`)");
    trace_samples_with_gpu_cancellable(
        gpu,
        scene,
        (first_sample, samples),
        threads,
        &never_cancel,
        tray.view(),
    )
    .expect("cancel is never set, so this dispatch can never observe GpuAccumulate::Cancelled")
}

/// Cancellable counterpart of [`trace_samples_with_gpu`], used by
/// `stream_emit::tracer::run_tracer` for single-engine sub-batches.
///
/// A GPU sub-batch can be interrupted mid-dispatch rather than only between sub-batches,
/// which bounds the cancellation latency.
///
/// Returns `None` if `cancel` was observed set before the dispatch finished; nothing was
/// traced into the result then, so the caller has nothing to fold in.
/// A `Declined` GPU still falls back to the CPU tracer for the full `samples` exactly as
/// [`trace_samples_with_gpu`] does; `cancel` plays no further role once declined.
///
/// `tray` is the request's resolved Stone Shadow and Tray Reflection
/// ([`crate::assets::resolve_tray`]): `shadow` is `Some` exactly when `scene.stone_shadow` is set
/// and `mirror` exactly when `scene.tray_reflection > 0.0`.
#[must_use]
pub fn trace_samples_with_gpu_cancellable(
    gpu: &GpuBackend,
    scene: &SceneState,
    (first_sample, samples): (u32, u32),
    threads: usize,
    cancel: &AtomicBool,
    tray: TrayRef<'_>,
) -> Option<Vec<Vec3>> {
    let mut buffer = vec![Vec3::ZERO; scene.width as usize * scene.height as usize];
    trace_samples_with_gpu_into(
        gpu,
        scene,
        (first_sample, samples),
        threads,
        cancel,
        &mut buffer,
        tray,
    )
    .then_some(buffer)
}

/// [`trace_samples_with_gpu_cancellable`] into a caller-owned `buffer`.
///
/// A loop of sub-batches reuses one frame-sized buffer instead of allocating a fresh one for
/// each.
///
/// `buffer` must be `width * height` long and all `Vec3::ZERO` on entry -- both backends
/// add into it, so the result is bit-identical to a freshly allocated zero buffer.
/// Returns `false` if `cancel` fired first, with `buffer` untouched (still all-zero).
///
/// `tray` is the request's resolved Stone Shadow and Tray Reflection, held by the caller for
/// every sub-batch (`shadow` `Some` exactly when `scene.stone_shadow` is set, `mirror` exactly
/// when `scene.tray_reflection > 0.0`); neither is rebuilt here.
pub fn trace_samples_with_gpu_into(
    gpu: &GpuBackend,
    scene: &SceneState,
    (first_sample, samples): (u32, u32),
    threads: usize,
    cancel: &AtomicBool,
    buffer: &mut [Vec3],
    tray: TrayRef<'_>,
) -> bool {
    if scene.engine.is_ray() {
        return whitted::trace(
            scene,
            tray,
            first_sample,
            samples,
            threads,
            gpu.adapter_label().is_some(),
            cancel,
            buffer,
        );
    }
    let width = scene.width;
    let height = scene.height;
    if samples == 0 || width == 0 || height == 0 {
        return true;
    }

    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll);
    // The studio rig, or the HDR map the request path resolved -- see
    // `crate::assets::resolved_hdr_map`.
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_for(scene, hdr_map.as_deref(), tray);

    // The frosting as `resolve_facet_finishes` expands it. The CPU fallback resolves the same
    // finishes, so a GPU decline cannot change the polish.
    let facet_finishes = resolve_facet_finishes(scene);
    // Scenes the GPU cannot trace (fluorescence, a UV lamp, too many tools) go to the CPU
    // below. Concave tools go to the GPU with the scene (`scene.tools`).
    let traced_material = scene.traced_material();
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width,
        height,
        planes: &scene.planes,
        facet_finishes: &facet_finishes,
        material: &traced_material,
        max_bounces: scene.max_bounces,
        environment,
        estimator: Some(scene.estimator),
    };
    // The facet surfaces, machine and abraded edge travel with the GPU request.
    let surfaces = scene.effects.surfaces();
    let outcome = if scene_uses_gpu(scene, gpu) {
        gpu.try_accumulate_geom_surface_cancellable(
            &gpu_scene,
            &scene.tools,
            &scene.effects.surface_scene(&surfaces),
            first_sample,
            samples,
            buffer,
            cancel,
        )
    } else {
        GpuAccumulate::Declined
    };
    match outcome {
        GpuAccumulate::Done => return true,
        GpuAccumulate::Cancelled => return false,
        GpuAccumulate::Declined => {}
    }

    // The GPU declined (no adapter, `ComputeMode::OnlyCpu`, an unsupported material, or a
    // device lost mid-request). `buffer` is still all-zero: the GPU traces into its own
    // scratch buffer and adds it only on `Done`, so the fallback starts from zero too.
    // `trace_into` returns `false` only when `cancel` fires while queued for CPU permits,
    // before any tracing, so `buffer` is still zero then as well.
    trace_into(
        scene,
        SampleRange::new(first_sample, samples),
        threads,
        &camera,
        environment,
        buffer,
        cancel,
    )
}

/// The CPU tracing loop, adding into an already-sized `buffer` (`width x height` entries,
/// row-major). Shared by [`trace_samples`] and the CPU fallback of
/// [`trace_samples_with_gpu_into`], so both use the same draws and thread split.
///
/// # Work distribution: a shared atomic row counter, not contiguous row bands
///
/// Rows through the stone cost far more than background rows, so threads claim rows from a
/// shared `AtomicUsize` counter rather than taking fixed bands. `buffer` is pre-split into
/// per-row slices behind a `Mutex`; a thread takes its row's slice under the lock and traces
/// it without the lock held. Each row goes to exactly one thread, so per-pixel sums do not
/// depend on the split.
fn trace_into(
    scene: &SceneState,
    range: SampleRange,
    threads: usize,
    camera: &Camera,
    environment: EnvironmentSource<'_>,
    buffer: &mut [Vec3],
    cancel: &AtomicBool,
) -> bool {
    if scene.engine.is_ray() {
        return whitted::trace(
            scene,
            TrayRef {
                shadow: environment.tray_shadow(),
                mirror: environment.tray_mirror(),
            },
            range.first_sample,
            range.samples,
            threads,
            false,
            cancel,
            buffer,
        );
    }
    // Destructured here so the uses below read as plain locals; the bundle keeps
    // `trace_into`'s argument count within clippy's limit.
    let SampleRange {
        first_sample,
        samples,
    } = range;
    let width = scene.width;
    let height = scene.height;
    let width_usize = width as usize;

    let requested_threads = effective_thread_count(threads).max(1);
    // Take the process-wide CPU permits before spawning (see the module docs and
    // `THREAD_PERMITS`). `acquire` clamps to the pool's capacity and returns how many it
    // granted, which is exactly the number of threads spawned below. `_permits` releases
    // them when the call ends, even on panic. `false` (with `buffer` untouched) means
    // `cancel` fired while waiting for permits.
    let Some(num_threads) = THREAD_PERMITS.acquire(requested_threads, cancel) else {
        return false;
    };
    let _permits = PermitGuard(num_threads);

    let geom = stone_geometry(scene);
    // `merged_facet_finishes` folds the surface facets into the finish list once, not per sample.
    let surfaces = scene.effects.surfaces();
    let resolved_finishes = resolve_facet_finishes(scene);
    let facet_finishes = merged_facet_finishes(&resolved_finishes, &surfaces);
    let traced_material = scene.traced_material();
    let material: &GemMaterial = &traced_material;
    // The zone kernels are built once per call, not per sample. The scene's own estimator is
    // used, so a frame split between the viewer and workers runs one estimator.
    let prepared = PreparedMaterial::new(material).with_estimator(scene.estimator);
    // Built once per call rather than per traced sample, which would allocate on every
    // sample. See `trace_spectral_ray_with_finish_soa`'s doc comment for the arena contract.
    let plane_soa = build_plane_soa(geom.planes);
    let tracer = RowTracer {
        scene,
        camera,
        geom,
        plane_soa: &plane_soa,
        facet_finishes: &facet_finishes,
        prepared: &prepared,
        surface: scene.effects.surface_scene(&surfaces),
        environment,
        first_sample,
        samples,
    };

    let rows: Vec<Option<&mut [Vec3]>> = buffer.chunks_mut(width_usize).map(Some).collect();
    let rows = Mutex::new(rows);
    let next_row = AtomicUsize::new(0);

    thread::scope(|s| {
        for _ in 0..num_threads {
            let rows = &rows;
            let next_row = &next_row;
            let tracer = &tracer;

            s.spawn(move || {
                loop {
                    let y = next_row.fetch_add(1, Ordering::Relaxed);
                    if y >= height as usize {
                        break;
                    }

                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("each row index is claimed by exactly one thread via fetch_add");

                    tracer.trace_row(row, y);
                }
            });
        }
    });

    true
}

/// What every worker thread of one [`trace_into`] call reads while tracing a row, bundled
/// so the per-row loop can be its own method.
struct RowTracer<'a> {
    scene: &'a SceneState,
    camera: &'a Camera,
    geom: StoneGeometry<'a>,
    /// Built once per `trace_into` call from `geom.planes`, as the tracer requires.
    plane_soa: &'a pleochroma::simd::PlanesSoA32,
    facet_finishes: &'a [FacetFinish],
    /// The scene's material with its inclusions and play-of-color applied, prepared once per
    /// `trace_into` call (its zone kernels are shared by every traced sample).
    prepared: &'a PreparedMaterial<'a>,
    /// The facet surfaces, faceting machine and abraded edge (`SurfaceScene::IDEAL` without).
    surface: SurfaceScene<'a>,
    environment: EnvironmentSource<'a>,
    first_sample: u32,
    samples: u32,
}

impl RowTracer<'_> {
    /// Traces every pixel of row `y` over this call's sample range, adding each pixel's
    /// summed samples into `row`. Rows are independent, so which thread traces which row
    /// never changes a pixel's sum.
    fn trace_row(&self, row: &mut [Vec3], y: usize) {
        let scene = self.scene;
        let width = scene.width;
        let height = scene.height;
        let width_usize = width as usize;
        for (x, pixel) in row.iter_mut().enumerate() {
            let global_pixel_idx = u32::try_from(y * width_usize + x)
                .expect("a pixel index within one frame fits in u32");
            let mut sample_sum = Vec3::ZERO;

            // Per-pixel Cranley-Patterson rotations for the jitter and hero draws below,
            // hoisted out of the sample loop. The three draws use different prime bases
            // (2, 3, 5), not one base rotated three ways.
            let rot = pixel_rotations(global_pixel_idx);

            for s_idx in 0..self.samples {
                let abs_sample = self.first_sample.wrapping_add(s_idx);
                // A pure function of the absolute sample index, so disjoint
                // sample ranges compose.
                let draws = sample_draws(global_pixel_idx, abs_sample, &rot);

                let ray = self.camera.generate_ray(
                    x as f32,
                    y as f32,
                    width as f32,
                    height as f32,
                    draws.jitter_x,
                    draws.jitter_y,
                );

                let sample = trace_spectral_ray_prepared(
                    ray,
                    self.geom,
                    self.plane_soa,
                    self.facet_finishes,
                    self.prepared,
                    &scene.fluorescence,
                    scene.max_bounces,
                    self.environment,
                    draws.seed,
                    draws.hero_rand,
                    None,
                    self.surface,
                );
                // A non-finite sample is dropped but still counted, the rule every backend
                // applies, so a client can merge this buffer with its own. No NaN or Inf
                // source is known on this path, but one would poison the accumulator for good.
                add_finite_sample(&mut sample_sum, sample);
            }

            *pixel += sample_sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    pub(super) fn tiny_scene() -> SceneState {
        SceneState {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 8,
            height: 8,
            yaw: 0.4,
            camera_roll: 0.0,
            stone_shadow: false,
            tray_reflection: 0.0,
            hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
            estimator: pleochroma::optics::raytracer::Estimator::Branching,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: pleochroma_net::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
            effects: pleochroma_net::scene::effects::SceneEffects::default(),
        }
    }

    #[test]
    #[ignore = "manual probe: reports whether this machine has a usable wgpu adapter"]
    fn probe_gpu_adapter() {
        let backend = GpuBackend::acquire();
        match backend.adapter_label() {
            Some(label) => eprintln!("GPU ADAPTER FOUND: {label}"),
            None => eprintln!("NO GPU ADAPTER (declined)"),
        }
    }

    #[test]
    fn returns_a_buffer_sized_to_the_scene() {
        let scene = tiny_scene();
        let buf = trace_samples(&scene, 0, 4, 1);
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn zero_samples_returns_an_all_zero_buffer_of_the_right_size() {
        let scene = tiny_scene();
        let buf = trace_samples(&scene, 0, 0, 1);
        assert_eq!(buf.len(), 64);
        assert!(buf.iter().all(|v| *v == Vec3::ZERO));
    }

    #[test]
    fn single_and_multi_threaded_traces_agree() {
        let scene = tiny_scene();
        let single = trace_samples(&scene, 0, 4, 1);
        let multi = trace_samples(&scene, 0, 4, 4);
        for (a, b) in single.iter().zip(multi.iter()) {
            // Thread count only changes chunking, never which (pixel, sample) pairs
            // are traced -- must be bit-exact.
            assert_eq!(a, b);
        }
    }

    #[test]
    fn splitting_a_sample_range_across_two_calls_sums_to_the_same_result_as_one_call() {
        // The additivity property this crate exists to preserve. Relative tolerance,
        // not bit-exact: float addition isn't associative, so a different grouping can
        // differ in the last bit or two of an f32 even when correct.
        let scene = tiny_scene();
        let whole = trace_samples(&scene, 0, 8, 2);
        let first_half = trace_samples(&scene, 0, 4, 2);
        let second_half = trace_samples(&scene, 4, 4, 2);

        for i in 0..whole.len() {
            let split_sum = first_half[i] + second_half[i];
            let diff = (whole[i] - split_sum).abs();
            let scale = whole[i].abs().max(split_sum.abs()).max(Vec3::splat(1e-6));
            let rel = diff / scale;
            assert!(
                rel.max_element() < 1e-3,
                "pixel {i}: whole={:?} split_sum={:?} rel={:?}",
                whole[i],
                split_sum,
                rel
            );
        }
    }
}

#[cfg(test)]
mod effects_tests;
#[cfg(test)]
mod estimator_tests;
#[cfg(test)]
mod shadow_tests;
#[cfg(test)]
mod whitted_tests;
