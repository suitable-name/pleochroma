//! Deterministic worker tracing. Preparation is reused across a request's tap batches.
use crate::assets::TrayRef;
use glam::Vec3;
use pleochroma::{
    optics::raytracer::{
        Camera, DEFAULT_FOV_DEG, FacetFinish, add_finite_sample, build_plane_soa,
        environment::{
            TrayMirror,
            tray_shadow::{TrayShadowKey, TrayShadowMap},
        },
    },
    renderer::{
        frame_scene::FrameScene,
        gpu_backend::{
            GpuAccumulate,
            whitted::{WhittedGpuBackend, WhittedGpuPrepared},
        },
        whitted::{WhittedPrep, whitted_tap_xyz},
    },
    simd::PlanesSoA32,
};
use pleochroma_net::SceneState;
use std::{
    cell::RefCell,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

struct Prepared {
    /// The request's Stone Shadow map (a clone shares its texels), retained across tap batches
    /// and GPU declines; CPU preparation owns the same map values.
    shadow: Option<TrayShadowMap>,
    /// The request's Tray Reflection mirror (plain data), retained with the shadow.
    mirror: Option<TrayMirror>,
    camera: Camera,
    finishes: Vec<FacetFinish>,
    planes: PlanesSoA32,
    prep: WhittedPrep<'static>,
    /// Frozen uploads belong to this immutable scene, never an address-based key.
    gpu: Mutex<Option<WhittedGpuPrepared>>,
}
/// What a preparation was made for: the scene and the Stone Shadow map it was lit with. The map
/// is part of the key because the same scene prepares differently under the `Preview` and the
/// `Final` map (the request's intent picks the quality).
type PreparedKey = (SceneState, Option<TrayShadowKey>);

thread_local! {
    // A request has one tracer thread; no unbounded process-wide scene/HDR cache.
    static PREPARED: RefCell<Option<(PreparedKey, Arc<Prepared>)>> = const { RefCell::new(None) };
}
static GPU: LazyLock<WhittedGpuBackend> = LazyLock::new(WhittedGpuBackend::acquire);

/// Acquires the Ray-Traced GPU backend now (its kernel compiles inside the acquisition), so the
/// first Ray-Traced request does not wait for it. `true` when a device is usable. Called by the
/// shader warm-up (`crate::serve::warmup`) on its own thread; a request that arrives meanwhile
/// waits for the same one-time acquisition.
pub fn warm_up() -> bool {
    GPU.adapter_label().is_some()
}

fn frame<'a>(
    scene: &'a SceneState,
    camera: &'a Camera,
    finishes: &'a [FacetFinish],
    environment: pleochroma::optics::raytracer::EnvironmentSource<'a>,
) -> FrameScene<'a> {
    FrameScene {
        camera,
        width: scene.width,
        height: scene.height,
        planes: &scene.planes,
        facet_finishes: finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment,
    }
}
/// The preparation of `scene` lit with the request's resolved `tray`, reused while the scene and
/// the shadow map are the ones it was made for (the mirror is a function of the scene).
fn prepared(scene: &SceneState, tray: TrayRef<'_>) -> Option<Arc<Prepared>> {
    let shadow_key = tray.shadow.map(TrayShadowMap::key);
    PREPARED.with(|slot| {
        if let Some(((key, key_shadow), prep)) = slot.borrow().as_ref()
            && key == scene
            && *key_shadow == shadow_key
        {
            return Some(Arc::clone(prep));
        }
        let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
            .with_roll(scene.camera_roll)
            .with_pan(scene.camera_pan);
        let finishes = super::resolve_facet_finishes(scene);
        let hdr = crate::assets::resolved_hdr_map(scene);
        let environment = crate::assets::environment_for(scene, hdr.as_deref(), tray);
        let prep = WhittedPrep::new(
            &frame(scene, &camera, &finishes, environment),
            &scene.fluorescence,
            scene.engine.ray,
        )
        .ok()?
        .into_owned_with_hdr(hdr.clone());
        let value = Arc::new(Prepared {
            shadow: tray.shadow.cloned(),
            mirror: tray.mirror,
            camera,
            finishes,
            planes: build_plane_soa(&scene.planes),
            prep,
            gpu: Mutex::new(None),
        });
        *slot.borrow_mut() = Some(((scene.clone(), shadow_key), Arc::clone(&value)));
        Some(value)
    })
}

/// Traces taps `first..first + samples` of `scene` into `out`: on the GPU when `prefer_gpu`
/// is set and the backend takes it, else on the CPU. Cancellation never exposes partial sums.
///
/// `tray` is the request's resolved Stone Shadow and Tray Reflection (`shadow` is `Some` exactly
/// when `scene.stone_shadow` is set, `mirror` exactly when `scene.tray_reflection > 0.0`). The
/// preparation is reused only while the map is the same.
#[expect(
    clippy::redundant_pub_crate,
    reason = "called from jobs::run and render_cmd outside render_core, so it cannot narrow to pub(super)"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "the range, engine preference, cancellation, output and the request's resolved tray are independent inputs of one tap batch"
)]
pub(crate) fn trace(
    scene: &SceneState,
    tray: TrayRef<'_>,
    first: u32,
    samples: u32,
    threads: usize,
    prefer_gpu: bool,
    cancel: &AtomicBool,
    out: &mut [Vec3],
) -> bool {
    if cancel.load(Ordering::Acquire) {
        return false;
    }
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    if pixels == 0
        || pixels != out.len() as u64
        || !pleochroma::renderer::engine::AA_TAP_CHOICES.contains(&scene.engine.ray.aa_taps)
    {
        return false;
    }
    let Some(end) = first
        .checked_add(samples)
        .filter(|end| *end <= u32::from(scene.engine.ray.aa_taps))
    else {
        return false;
    };
    if samples == 0 {
        return true;
    }
    let Some(prep) = prepared(scene, tray) else {
        return false;
    };
    let hdr = crate::assets::resolved_hdr_map(scene);
    let frame = frame(
        scene,
        &prep.camera,
        &prep.finishes,
        crate::assets::environment_for(
            scene,
            hdr.as_deref(),
            TrayRef {
                shadow: prep.shadow.as_ref(),
                mirror: prep.mirror,
            },
        ),
    );
    if prefer_gpu {
        match trace_prepared_gpu(&prep, &frame, scene, (first, samples), out, cancel) {
            GpuAccumulate::Done => return true,
            GpuAccumulate::Cancelled => return false,
            GpuAccumulate::Declined => {}
        }
    }
    let Some(count) =
        super::THREAD_PERMITS.acquire(super::effective_thread_count(threads).max(1), cancel)
    else {
        return false;
    };
    let _permits = super::PermitGuard(count);
    let mut scratch = vec![Vec3::ZERO; out.len()];
    trace_rows_cpu(
        scene,
        &frame,
        &prep,
        count,
        (first, end),
        cancel,
        &mut scratch,
    );
    if cancel.load(Ordering::Acquire) {
        return false;
    }
    for (dst, src) in out.iter_mut().zip(scratch) {
        *dst += src;
    }
    true
}

/// Traces taps `first..end` of every row on `count` threads into `scratch`. Each thread
/// claims whole rows; a fired `cancel` stops it between rows and between pixels.
fn trace_rows_cpu(
    scene: &SceneState,
    frame: &FrameScene<'_>,
    prep: &Prepared,
    count: usize,
    (first, end): (u32, u32),
    cancel: &AtomicBool,
    scratch: &mut [Vec3],
) {
    let rows = Mutex::new(
        scratch
            .chunks_mut(scene.width as usize)
            .map(Some)
            .collect::<Vec<_>>(),
    );
    let next = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..count {
            let (rows, next) = (&rows, &next);
            scope.spawn(move || {
                loop {
                    if cancel.load(Ordering::Acquire) {
                        break;
                    }
                    let y = next.fetch_add(1, Ordering::Relaxed);
                    if y >= scene.height as usize {
                        break;
                    }
                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("a row is claimed once");
                    for (x, sum) in row.iter_mut().enumerate() {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        for tap in first..end {
                            add_finite_sample(
                                sum,
                                whitted_tap_xyz(
                                    frame,
                                    &scene.tools,
                                    &prep.planes,
                                    &prep.prep,
                                    u32::try_from(y * scene.width as usize + x)
                                        .expect("validated frame fits u32"),
                                    tap,
                                    None,
                                ),
                            );
                        }
                    }
                }
            });
        }
    });
}

/// Reuses the GPU upload for every tap batch of the scene. A declined dispatch drops the
/// upload, so the next call prepares again, which may succeed after backend recovery.
fn trace_prepared_gpu(
    prepared: &Prepared,
    frame: &FrameScene<'_>,
    scene: &SceneState,
    (first, samples): (u32, u32),
    out: &mut [Vec3],
    cancel: &AtomicBool,
) -> GpuAccumulate {
    // A panic while preparing/executing may leave GPU resources mapped. Retire this
    // scene's GPU cache on poison instead of reusing it through into_inner().
    let Ok(mut uploaded) = prepared.gpu.lock() else {
        return GpuAccumulate::Declined;
    };
    let cancelled = || cancel.load(Ordering::Acquire);
    if uploaded.is_none() {
        match GPU.prepare_cancellable(frame, &scene.tools, &prepared.prep, &cancelled) {
            Ok(snapshot) => *uploaded = Some(snapshot),
            Err(outcome) => return outcome,
        }
    }
    let outcome = GPU.try_accumulate_prepared_cancellable(
        uploaded
            .as_ref()
            .expect("successful GPU preparation was cached"),
        first,
        samples,
        out,
        &cancelled,
    );
    if outcome == GpuAccumulate::Declined {
        *uploaded = None;
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::renderer::engine::RenderEngine;

    fn scene() -> SceneState {
        let mut scene = super::super::tests::tiny_scene();
        scene.engine.engine = RenderEngine::RayTraced;
        scene.engine.ray.aa_taps = 4;
        scene
    }

    #[test]
    fn preparation_reuses_equal_scenes_and_replaces_changed_contents() {
        let mut scene = scene();
        let none = TrayRef::default();
        let original = prepared(&scene, none).unwrap();
        assert!(Arc::ptr_eq(&original, &prepared(&scene, none).unwrap()));
        // Reuse the SceneState allocation but change a frozen input.
        scene.exposure += 0.5;
        let changed = prepared(&scene, none).unwrap();
        assert!(!Arc::ptr_eq(&original, &changed));
        assert!(Arc::ptr_eq(&changed, &prepared(&scene, none).unwrap()));
        scene.camera_roll = std::f32::consts::FRAC_PI_2;
        let rolled = prepared(&scene, none).unwrap();
        assert!(
            !Arc::ptr_eq(&changed, &rolled),
            "roll invalidates frozen CPU/GPU preparation"
        );
    }

    #[test]
    fn a_poisoned_upload_cache_retries_the_entire_range_on_cpu_without_gpu_access() {
        let scene = scene();
        let cancel = AtomicBool::new(false);
        let mut expected = vec![Vec3::ONE; (scene.width * scene.height) as usize];
        let none = TrayRef::default();
        assert!(trace(&scene, none, 0, 4, 1, false, &cancel, &mut expected));
        let cached = prepared(&scene, none).unwrap();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _upload = cached.gpu.lock().unwrap();
            panic!("simulate a GPU call unwinding while holding its scene cache");
        }));
        assert!(poisoned.is_err());
        assert!(cached.gpu.is_poisoned());
        let mut actual = vec![Vec3::ONE; expected.len()];
        // The poison check must precede touching the lazy GPU backend.
        assert!(trace(&scene, none, 0, 4, 2, true, &cancel, &mut actual));
        assert_eq!(actual, expected);
        assert!(cached.gpu.is_poisoned());
    }
}
