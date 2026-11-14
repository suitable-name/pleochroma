//! The phone's path tracer: the split event-wavefront GPU kernels, never the megakernel.
//!
//! The desktop's one-shader megakernel is a single ~500 KB compute shader that mobile Vulkan
//! driver compilers reject (`VK_ERROR_UNKNOWN`, which wgpu reports as "Unexpected error variant"),
//! so mobile path tracing runs the same split kernels the browser does
//! (`EventWavefrontRenderer`; every kernel binds at most eight storage buffers). Like the
//! browser, a pass traces with the reduced compile-friendly kernel variant of the stone's class
//! (`browser_pass_class`), and a stone that needs a feature that variant compiled out is traced
//! on the CPU instead.
//!
//! A driver can still reject one of the split kernels, and wgpu then loses the whole device.
//! The stone's class stays on the CPU for the rest of the process, and a fresh device traces the
//! other classes at once.
use super::{BackendManager, TraceBackend, replace_due};
use glam::Vec3;
use pleochroma::optics::materials::GemMaterial;
#[cfg(feature = "device-gpu")]
use pleochroma::renderer::gpu::{
    GpuContext,
    event_wavefront::{EventWavefrontRenderer, EwError},
    frame::classify_material,
};
#[cfg(feature = "device-gpu")]
use pleochroma_web_core::gpu_admission::browser_pass_class;
use pleochroma_web_core::{
    render::{ChunkRange, trace_chunk_abortable},
    scene::OwnedScene,
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

/// The pause between two one-sample submissions. It leaves the phone compositor and other GPU
/// clients room and never reduces the requested samples.
#[cfg(feature = "device-gpu")]
const DUTY_GAP: std::time::Duration = std::time::Duration::from_millis(2);

/// The report when the stone needs a kernel feature the device variant does not compile.
#[cfg(feature = "device-gpu")]
const FEATURE_NOT_COMPILED: &str =
    "this stone needs a GPU kernel feature the device GPU does not compile";

/// The report for a stone whose class has a kernel the device's GPU driver could not build.
const KERNEL_REJECTED: &str = "the GPU driver cannot build the path tracer for this kind of stone";

/// Why a session is lost after its driver could not build a kernel. A stone of another class
/// that still holds the session reads it until a fresh device replaces the session.
#[cfg(feature = "device-gpu")]
const KERNEL_LOST: &str = "the GPU driver could not build a path tracing kernel";

/// The report of a build without device GPU support.
#[cfg(not(feature = "device-gpu"))]
const NOT_BUILT: &str = "device GPU support is not built into this app";

/// Why a pass produced sums the accumulator must never see.
const INVALID_SUMS: &str = "the GPU pass produced non-finite or negative sums";

/// The class value the event wavefront traces `material` with, or the reason the stone stays
/// on the CPU. A pure function of the material.
#[cfg(feature = "device-gpu")]
fn pass_class(material: &GemMaterial) -> Result<u32, &'static str> {
    browser_pass_class(classify_material(material)).ok_or(FEATURE_NOT_COMPILED)
}

/// The class value the event wavefront traces `material` with, or the reason the stone stays
/// on the CPU. A pure function of the material.
#[cfg(not(feature = "device-gpu"))]
const fn pass_class(_material: &GemMaterial) -> Result<u32, &'static str> {
    Err(NOT_BUILT)
}

/// The backend report of a stone traced on the device GPU through adapter `label`.
fn gpu_report(label: &str) -> String {
    format!("Device GPU · {label} (event wavefront)")
}

/// `true` when every value of `sums` is finite and non-negative, the browser's rule for a pass
/// the accumulator may merge.
fn sums_are_valid(sums: &[Vec3]) -> bool {
    sums.iter().all(|s| s.is_finite() && s.min_element() >= 0.0)
}

/// How one traced range ended.
pub(in crate::render) enum EwPass {
    /// Every sample of the range is in the sum buffer.
    #[cfg_attr(
        not(feature = "device-gpu"),
        expect(dead_code, reason = "only the device GPU build traces a range")
    )]
    Done,
    /// The render was stopped; the sum buffer is to be discarded.
    Cancelled,
    /// The device is lost (or gave a bad pass); the session stays unusable until replaced.
    Lost(String),
    /// Only this range cannot run on the GPU; the device is fine.
    #[cfg_attr(
        not(feature = "device-gpu"),
        expect(dead_code, reason = "only the device GPU build traces a range")
    )]
    Refused(String),
    /// The driver could not build a kernel of the pass class, which lost the device. Only that
    /// class moves to the CPU.
    #[cfg_attr(
        not(feature = "device-gpu"),
        expect(dead_code, reason = "only the device GPU build traces a range")
    )]
    KernelRejected,
}

/// One acquired device GPU with its event-wavefront renderer. Only the process-wide probe
/// shares it between epochs and exports; the renderer needs `&mut self`, so it sits behind a
/// mutex, and a device loss marks the whole session lost until the probe replaces it.
pub(in crate::render) struct EwGpuSession {
    /// `None` for a session that never acquired a device or never had one in this build.
    #[cfg(feature = "device-gpu")]
    renderer: Option<Mutex<EventWavefrontRenderer>>,
    /// The adapter's name.
    label: String,
    /// Why the session is unusable, `None` while it is healthy. The first reason is kept.
    lost: Mutex<Option<String>>,
    /// Set when the session was lost because the driver could not build a kernel. That class
    /// stays on the CPU from then on, so a fresh device may replace this one at once.
    kernel_rejected: AtomicBool,
}

impl EwGpuSession {
    /// A session without a device, lost from the start for `reason`.
    pub(in crate::render) fn disabled(reason: &str) -> Self {
        Self {
            #[cfg(feature = "device-gpu")]
            renderer: None,
            label: String::new(),
            lost: Mutex::new(Some(reason.to_owned())),
            kernel_rejected: AtomicBool::new(false),
        }
    }

    /// Acquires the device GPU and builds its renderer, with the reason when that failed (the
    /// session is then [`Self::disabled`]). Slow; the probe calls it once per process.
    #[cfg(feature = "device-gpu")]
    pub(in crate::render) fn acquire_with_diagnostics() -> (Self, Option<String>) {
        match GpuContext::acquire() {
            Ok(ctx) => {
                let label = ctx.adapter.get_info().name;
                let session = Self {
                    renderer: Some(Mutex::new(EventWavefrontRenderer::new(ctx))),
                    label,
                    lost: Mutex::new(None),
                    kernel_rejected: AtomicBool::new(false),
                };
                (session, None)
            }
            Err(error) => {
                let reason = error.to_string();
                tracing::warn!(%reason, "no device GPU for path tracing");
                (Self::disabled(&reason), Some(reason))
            }
        }
    }

    /// Acquires the device GPU: a build without device GPU support has none.
    #[cfg(not(feature = "device-gpu"))]
    pub(in crate::render) fn acquire_with_diagnostics() -> (Self, Option<String>) {
        (Self::disabled(NOT_BUILT), Some(NOT_BUILT.to_owned()))
    }

    /// The adapter's name, empty for a session without a device.
    pub(in crate::render) fn label(&self) -> &str {
        &self.label
    }

    /// Why the session is unusable, `None` while it is healthy.
    pub(in crate::render) fn lost_reason(&self) -> Option<String> {
        self.lost
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether the session lost its device.
    pub(in crate::render) fn is_lost(&self) -> bool {
        self.lost_reason().is_some()
    }

    /// Whether the session was lost because the driver could not build a kernel.
    pub(in crate::render) fn lost_to_rejected_kernel(&self) -> bool {
        self.kernel_rejected.load(Ordering::Acquire)
    }

    /// Records `reason` as the loss of this session, keeping an earlier reason.
    #[cfg(feature = "device-gpu")]
    fn mark_lost(&self, reason: String) {
        self.lost
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(reason);
    }

    /// Traces the samples of `range` into `sum`, one submission per sample, in `class` (the
    /// stone's pass class).
    ///
    /// `sum` holds one value per pixel and is written only into; a result other than
    /// [`EwPass::Done`] leaves it to be discarded.
    pub(in crate::render) fn accumulate(
        &self,
        scene: &OwnedScene,
        class: u32,
        range: ChunkRange,
        sum: &mut [Vec3],
        stopped: &dyn Fn() -> bool,
    ) -> EwPass {
        if stopped() {
            return EwPass::Cancelled;
        }
        if let Some(reason) = self.lost_reason() {
            return EwPass::Lost(reason);
        }
        let pass = self.run(scene, class, range, sum, stopped);
        if matches!(pass, EwPass::Done) && !sums_are_valid(sum) {
            return self.lose(INVALID_SUMS.to_owned());
        }
        pass
    }

    /// Marks the session lost for `reason`, logs the reason in full (the status line may cut
    /// it short) and returns it as the pass result.
    #[cfg(feature = "device-gpu")]
    fn lose(&self, reason: String) -> EwPass {
        tracing::warn!(
            adapter = %self.label,
            %reason,
            "device GPU path tracing lost; the CPU traces instead"
        );
        self.mark_lost(reason.clone());
        EwPass::Lost(reason)
    }

    /// Marks the session lost because the driver could not build `kernel` for pass class
    /// `class`, logging the device's failure in full.
    #[cfg(feature = "device-gpu")]
    fn reject_kernel(&self, kernel: &str, class: u32, detail: &str) -> EwPass {
        tracing::warn!(
            adapter = %self.label,
            kernel,
            class,
            %detail,
            "the GPU driver could not build a path tracing kernel; its class is traced on the CPU"
        );
        self.kernel_rejected.store(true, Ordering::Release);
        self.mark_lost(KERNEL_LOST.to_owned());
        EwPass::KernelRejected
    }

    /// Marks the session lost for `reason` and returns that as the pass result.
    #[cfg(not(feature = "device-gpu"))]
    fn lose(&self, reason: String) -> EwPass {
        EwPass::Lost(reason)
    }

    /// The sample loop of [`Self::accumulate`] on the device.
    #[cfg(feature = "device-gpu")]
    fn run(
        &self,
        scene: &OwnedScene,
        class: u32,
        range: ChunkRange,
        sum: &mut [Vec3],
        stopped: &dyn Fn() -> bool,
    ) -> EwPass {
        let Some(renderer) = &self.renderer else {
            return EwPass::Lost("no device GPU renderer".into());
        };
        let Ok(mut renderer) = renderer.lock() else {
            return self.lose("GPU renderer mutex poisoned (a previous turn panicked)".into());
        };
        let Some(end) = range.sample_offset.checked_add(range.spp) else {
            return EwPass::Refused("the sample range overflows".into());
        };
        renderer.set_surface_scene(&scene.surface_scene());
        let frame = scene.frame_scene();
        for sample in range.sample_offset..end {
            if stopped() {
                return EwPass::Cancelled;
            }
            // The stone's concave tools travel with the call; without them the stone would be
            // drawn flat.
            let result = pollster::block_on(renderer.accumulate_via_class_with_tools(
                &frame,
                scene.tools(),
                class,
                sample,
                1,
                sum,
            ));
            match result {
                Ok(()) => {}
                Err(EwError::KernelCompile { kernel, detail, .. }) => {
                    return self.reject_kernel(kernel.entry_point(), class, &detail);
                }
                Err(
                    error @ (EwError::DeviceLost(_)
                    | EwError::ChunkTimeout
                    | EwError::BufferMapping(_)),
                ) => {
                    return self.lose(format!("event wavefront accumulation failed: {error}"));
                }
                Err(error) => {
                    return EwPass::Refused(format!("event wavefront pass refused: {error}"));
                }
            }
            std::thread::sleep(DUTY_GAP);
        }
        EwPass::Done
    }

    /// A build without device GPU support never has a device to trace on.
    #[cfg(not(feature = "device-gpu"))]
    fn run(
        &self,
        _scene: &OwnedScene,
        _class: u32,
        _range: ChunkRange,
        _sum: &mut [Vec3],
        _stopped: &dyn Fn() -> bool,
    ) -> EwPass {
        EwPass::Lost(NOT_BUILT.into())
    }
}

impl BackendManager {
    /// Whether the lost path-tracing `session` may be replaced now; records the replacement
    /// when it may. A session lost to a kernel the driver rejected is replaced at once: its
    /// class now stays on the CPU, so the fresh device is never asked for that pipeline. Any
    /// other loss waits for the cool-down, so a device that is lost on every use is not
    /// re-acquired on every epoch.
    fn replace_lost_spectral(&self, session: &EwGpuSession) -> bool {
        let mut last = self
            .gpu
            .spectral_replaced
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let due = session.lost_to_rejected_kernel() || replace_due(*last, now);
        if due {
            *last = Some(now);
        }
        due
    }

    /// Whether the GPU driver could not build a kernel of pass class `class` on this device.
    fn class_rejected(&self, class: u32) -> bool {
        self.gpu
            .spectral_rejected
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&class)
    }

    /// Records that the GPU driver could not build a kernel of pass class `class`.
    fn reject_class(&self, class: u32) {
        let mut rejected = self
            .gpu
            .spectral_rejected
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !rejected.contains(&class) {
            rejected.push(class);
        }
    }

    /// Routes a stone that passed the shared scene gates to the event wavefront, or to the CPU
    /// with the reason in the report. Called only by worker-owned scene preparation.
    pub(super) fn prepare_event_wavefront(&self, scene: &OwnedScene, epoch: u64) -> TraceBackend {
        let class = match pass_class(scene.material()) {
            Ok(class) => class,
            Err(reason) => {
                self.cpu_fallback(epoch, reason);
                return TraceBackend::Cpu;
            }
        };
        // The driver could not build a kernel of this class: no device is acquired for it.
        if self.class_rejected(class) {
            self.cpu_fallback(epoch, KERNEL_REJECTED);
            return TraceBackend::Cpu;
        }
        // One cached probe per process, including failed probes. The lock covers only the
        // (slow, once-per-process) probe itself; the answer is cloned out so that routing,
        // recovery and reporting never hold it. A session that lost its device is replaced by
        // a fresh one, but not more often than the cool-down allows unless a rejected kernel
        // lost it; a failed probe is remembered and never retried.
        let (session, decline) = {
            let mut slot = self
                .gpu
                .spectral
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if slot.as_ref().is_some_and(|(session, decline)| {
                decline.is_none() && session.is_lost() && self.replace_lost_spectral(session)
            }) {
                *slot = None;
            }
            slot.get_or_insert_with(|| {
                let (session, decline) = EwGpuSession::acquire_with_diagnostics();
                (Arc::new(session), decline)
            })
            .clone()
        };
        if let Some(reason) = &decline {
            self.cpu_fallback(epoch, reason);
            return TraceBackend::Cpu;
        }
        if let Some(reason) = session.lost_reason() {
            self.cpu_fallback(epoch, &reason);
            return TraceBackend::Cpu;
        }
        self.set_report(epoch, gpu_report(session.label()));
        TraceBackend::Gpu(session)
    }

    /// Traces a whole-frame `range` on the event wavefront, or retraces all of it on the CPU
    /// when the device is lost, refuses the pass or returns bad sums.
    ///
    /// The GPU sums are private until the range completes, so GPU and CPU samples are never
    /// mixed or counted twice.
    pub(super) fn trace_event_wavefront(
        &self,
        gpu: &EwGpuSession,
        scene: &OwnedScene,
        range: ChunkRange,
        cancel: &AtomicBool,
        aborted: &dyn Fn() -> bool,
        epoch: u64,
    ) -> Option<Vec<Vec3>> {
        if range.first_pixel != 0 || range.stride != 1 {
            return None;
        }
        // The class was admitted when the backend was prepared; a stone that no longer maps
        // to one is traced exactly on the CPU.
        let class = match pass_class(scene.material()) {
            Ok(class) => class,
            Err(reason) => {
                self.cpu_fallback(epoch, reason);
                return trace_chunk_abortable(scene, range, aborted);
            }
        };
        let mut sum =
            vec![Vec3::ZERO; (scene.width() as usize).checked_mul(scene.height() as usize)?];
        let stopped = || cancel.load(Ordering::Acquire) || aborted();
        match gpu.accumulate(scene, class, range, &mut sum, &stopped) {
            EwPass::Done => Some(sum),
            EwPass::Cancelled => None,
            EwPass::KernelRejected => {
                self.reject_class(class);
                self.cpu_fallback(epoch, KERNEL_REJECTED);
                trace_chunk_abortable(scene, range, aborted)
            }
            EwPass::Lost(reason) | EwPass::Refused(reason) => {
                self.cpu_fallback(epoch, &reason);
                trace_chunk_abortable(scene, range, aborted)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::raytracer::LightingPreset;
    use pleochroma_web_core::scene::{
        CameraSpec, FinishSpec, LightingSpec, MaterialSpec, SceneSpec, planes_to_data,
    };

    fn scene(material: &str, width: u32, height: u32) -> OwnedScene {
        let spec = SceneSpec {
            width,
            height,
            planes: planes_to_data(
                &pleochroma::geometry::StandardGemCuts::standard_round_brilliant(),
            ),
            finishes: FinishSpec::AllPolished,
            material: MaterialSpec::catalogue(material),
            camera: CameraSpec {
                yaw: 0.6,
                pitch: 0.45,
                distance: 3.0,
            },
            lighting: LightingSpec {
                preset_index: LightingPreset::Daylight.index(),
                exposure: 1.0,
                light_yaw: 0.7,
                light_pitch: 0.8,
                backdrop_index: crate::design_props::backdrop_index(
                    crate::settings::LightingSettings::default().backdrop,
                ),
                head_shadow_deg: 16.0,
                surface_glare: 1.0,
                hdr_yaw_deg: 0.0,
                hdr_follows_camera: false,
            },
            max_bounces: 12,
            hdr_id: None,
        };
        OwnedScene::build(&spec, None).expect("scene")
    }

    fn range() -> ChunkRange {
        ChunkRange {
            first_pixel: 0,
            stride: 1,
            sample_offset: 1,
            spp: 3,
        }
    }

    #[test]
    fn the_report_names_the_adapter_and_the_event_wavefront() {
        assert_eq!(
            gpu_report("Adreno (TM) 740"),
            "Device GPU · Adreno (TM) 740 (event wavefront)"
        );
    }

    #[test]
    fn only_finite_non_negative_sums_are_valid() {
        assert!(sums_are_valid(&[Vec3::ZERO, Vec3::new(1.0, 2.0, 0.5)]));
        assert!(!sums_are_valid(&[Vec3::new(f32::NAN, 0.0, 0.0)]));
        assert!(!sums_are_valid(&[Vec3::new(0.0, f32::INFINITY, 0.0)]));
        assert!(!sums_are_valid(&[Vec3::new(0.0, 0.0, -0.001)]));
    }

    #[test]
    fn a_lost_session_retraces_the_entire_range_on_the_cpu() {
        let scene = scene("Diamond", 16, 12);
        let manager = BackendManager::new();
        manager.begin(1);
        let session = EwGpuSession::disabled("GPU device lost or unresponsive: test");
        assert!(session.is_lost());
        let expected = trace_chunk_abortable(&scene, range(), &|| false).expect("CPU reference");
        let actual = manager
            .trace_event_wavefront(
                &session,
                &scene,
                range(),
                &AtomicBool::new(false),
                &|| false,
                1,
            )
            .expect("CPU fallback");
        assert_eq!(actual, expected);
        let report = manager.report();
        assert!(report.starts_with("Device CPU"), "{report}");
        #[cfg(feature = "device-gpu")]
        assert!(report.contains("device lost"), "{report}");
    }

    #[cfg(feature = "device-gpu")]
    #[test]
    fn stopped_work_is_discarded_not_retraced() {
        let scene = scene("Diamond", 16, 12);
        let manager = BackendManager::new();
        manager.begin(1);
        let session = EwGpuSession::disabled("lost");
        assert!(
            manager
                .trace_event_wavefront(
                    &session,
                    &scene,
                    range(),
                    &AtomicBool::new(true),
                    &|| false,
                    1,
                )
                .is_none()
        );
        assert!(
            manager
                .trace_event_wavefront(
                    &session,
                    &scene,
                    range(),
                    &AtomicBool::new(false),
                    &|| true,
                    1,
                )
                .is_none()
        );
    }

    #[test]
    fn a_partitioned_range_is_not_a_whole_frame_and_is_declined() {
        let scene = scene("Diamond", 16, 12);
        let manager = BackendManager::new();
        let session = EwGpuSession::disabled("lost");
        let strided = ChunkRange {
            stride: 2,
            ..range()
        };
        assert!(
            manager
                .trace_event_wavefront(
                    &session,
                    &scene,
                    strided,
                    &AtomicBool::new(false),
                    &|| false,
                    1,
                )
                .is_none()
        );
    }

    #[test]
    fn a_lost_session_is_replaced_at_most_once_per_cool_down() {
        let manager = BackendManager::new();
        let session = EwGpuSession::disabled("GPU device lost or unresponsive: test");
        assert!(manager.replace_lost_spectral(&session));
        assert!(!manager.replace_lost_spectral(&session));
    }

    #[test]
    fn a_session_lost_to_a_rejected_kernel_is_replaced_inside_the_cool_down() {
        let manager = BackendManager::new();
        *manager.gpu.spectral_replaced.lock().expect("stamp") = Some(Instant::now());
        let rejected =
            EwGpuSession::disabled("the GPU driver could not build a path tracing kernel");
        rejected.kernel_rejected.store(true, Ordering::Release);
        assert!(rejected.lost_to_rejected_kernel());
        assert!(manager.replace_lost_spectral(&rejected));
        let lost = EwGpuSession::disabled("GPU device lost or unresponsive: test");
        assert!(!manager.replace_lost_spectral(&lost));
    }

    #[test]
    fn a_rejected_class_is_recorded_once_and_alone() {
        let manager = BackendManager::new();
        assert!(!manager.class_rejected(14));
        manager.reject_class(14);
        manager.reject_class(14);
        assert!(manager.class_rejected(14));
        assert!(!manager.class_rejected(45));
        assert_eq!(
            *manager.gpu.spectral_rejected.lock().expect("rejected"),
            vec![14]
        );
    }

    #[test]
    fn live_and_export_share_the_one_path_tracing_probe() {
        let live = BackendManager::new();
        let export = BackendManager::sharing(&live.probe());
        assert!(Arc::ptr_eq(&live.gpu.spectral, &export.gpu.spectral));
        assert!(Arc::ptr_eq(
            &live.gpu.spectral_replaced,
            &export.gpu.spectral_replaced
        ));
        assert!(Arc::ptr_eq(
            &live.gpu.spectral_rejected,
            &export.gpu.spectral_rejected
        ));
    }

    #[cfg(feature = "device-gpu")]
    #[test]
    fn a_stone_the_device_variant_cannot_trace_goes_to_the_cpu_with_its_reason() {
        // A class value that names no kind is refused by the browser variant; a plain stone
        // is not.
        assert_eq!(browser_pass_class(0), None);
        assert!(pass_class(&GemMaterial::diamond()).is_ok());
        let manager = BackendManager::new();
        manager.begin(1);
        manager.cpu_fallback(1, FEATURE_NOT_COMPILED);
        assert_eq!(
            manager.report(),
            "Device CPU · this stone needs a GPU kernel feature the device GPU does not compile"
        );
    }

    #[cfg(feature = "device-gpu")]
    #[test]
    fn a_lost_session_inside_the_cool_down_keeps_the_stone_on_the_cpu() {
        let manager = BackendManager::new();
        manager.begin(1);
        *manager.gpu.spectral_replaced.lock().expect("stamp") = Some(Instant::now());
        *manager.gpu.spectral.lock().expect("probe") = Some((
            Arc::new(EwGpuSession::disabled(
                "GPU device lost or unresponsive: test",
            )),
            None,
        ));
        let backend = manager.prepare_event_wavefront(&scene("Diamond", 16, 12), 1);
        assert!(matches!(backend, TraceBackend::Cpu));
        assert!(manager.report().contains("device lost"));
    }

    #[cfg(feature = "device-gpu")]
    #[test]
    fn a_stone_of_a_rejected_class_stays_on_the_cpu_without_a_device() {
        let manager = BackendManager::new();
        manager.begin(1);
        let sapphire = scene("Sapphire", 16, 12);
        let class = pass_class(sapphire.material()).expect("a plain sapphire has a pass class");
        assert_ne!(Ok(class), pass_class(&GemMaterial::diamond()));
        manager.reject_class(class);
        let backend = manager.prepare_event_wavefront(&sapphire, 1);
        assert!(matches!(backend, TraceBackend::Cpu));
        assert_eq!(manager.report(), format!("Device CPU · {KERNEL_REJECTED}"));
        assert!(manager.gpu.spectral.lock().expect("probe").is_none());
    }

    /// Traces a small stone of each kind (isotropic, uniaxial, biaxial: one pass class each)
    /// through the real event wavefront and compares its mean luminance with the CPU's.
    #[cfg(feature = "device-gpu")]
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn the_event_wavefront_agrees_with_the_cpu_on_mean_luminance() {
        let manager = BackendManager::new();
        for (epoch, material) in (1_u64..).zip(["Diamond", "Sapphire", "Tanzanite"]) {
            let scene = scene(material, 32, 24);
            manager.begin(epoch);
            let backend = manager.prepare_event_wavefront(&scene, epoch);
            assert!(
                matches!(backend, TraceBackend::Gpu(..)),
                "{material}: {}",
                manager.report()
            );
            let range = ChunkRange {
                sample_offset: 0,
                spp: 16,
                ..range()
            };
            let gpu = manager
                .trace(
                    &backend,
                    &scene,
                    range,
                    &AtomicBool::new(false),
                    &|| false,
                    epoch,
                )
                .expect("GPU frame");
            assert!(
                manager.report().starts_with("Device GPU"),
                "{material}: {}",
                manager.report()
            );
            let cpu = trace_chunk_abortable(&scene, range, &|| false).expect("CPU frame");
            let mean = |sums: &[Vec3]| sums.iter().map(|s| s.y).sum::<f32>() / sums.len() as f32;
            let (gpu, cpu) = (mean(&gpu), mean(&cpu));
            println!("{material}: mean luminance GPU {gpu} CPU {cpu}");
            assert!(
                (gpu - cpu).abs() <= 0.25 * cpu.max(1e-6),
                "{material}: GPU {gpu} vs CPU {cpu}"
            );
        }
    }
}
