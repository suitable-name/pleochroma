//! Latest-request-wins background precomputation for the live tray.
//!
//! One named builder thread (`tray-shadow`) serves the newest request only: a newer request
//! cancels the running build through its token and replaces any pending one, and a result is
//! published only while its exact key is still the wanted one, so a stale completion is dropped.
//!
//! **Reuse.** While a new map builds, the previous map keeps lighting the picture, but
//! only for a light-only change: the reuse key ([`Planned::stone`]) is the stone, its material and
//! the bounce depth with the lighting reset, so a new cut, material or `max_bounces` never shows
//! the old map. A remote render needs the map of the exact scene, so with `exact_only` the old map
//! is withheld until the build of the exact key lands.
//!
//! **Cancellation.** Every job owns a token; replacing the request, turning the shadow off, an
//! unsupported light and dropping the worker all set it, and the build polls it.
//!
//! **Panics.** A panic inside a build is caught and published as [`Refusal::Failed`], so the view
//! carries on without the shadow (and shows the note) instead of waiting on a dead thread, and the
//! worker keeps serving later requests. The failed key is not retried until the request changes.
use pleochroma::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        materials::GemMaterial,
        raytracer::{
            LightingPreset,
            environment::tray_shadow::{
                TrayShadowInput, TrayShadowKey, TrayShadowMap, TrayShadowOff, TrayShadowQuality,
                tray_shadow_key,
            },
        },
    },
    render_setup::stone_shadow_for_with_threads,
    renderer::tonemap::live_cpu_budget,
};
use std::{
    num::NonZeroUsize,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

/// Why the live view has no stone shadow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The lighting or geometry cannot carry a shadow.
    Off(TrayShadowOff),
    /// The background build panicked; the view proceeds without the shadow.
    Failed,
}
impl Refusal {
    /// Sentence for the viewport note.
    pub(super) const fn message(self) -> &'static str {
        match self {
            Self::Off(reason) => reason.message(),
            Self::Failed => "Stone shadow could not be computed; the picture is drawn without it.",
        }
    }
}

/// Owned inputs for a background preview; camera and exposure do not affect its key.
#[derive(Clone)]
pub(super) struct Request {
    /// Shared facet planes.
    pub planes: Arc<Vec<GpuFacetPlane>>,
    /// Shared subtractive tools.
    pub tools: Arc<Vec<ToolPrimitive>>,
    /// Resolved optical material.
    pub material: GemMaterial,
    /// Analytic studio lighting.
    pub preset: LightingPreset,
    /// Light azimuth in radians.
    pub yaw: f32,
    /// Light elevation in radians.
    pub pitch: f32,
    /// Resolved tray radiance; zero declines the request.
    pub backdrop: f32,
    /// Maximum photon transport depth.
    pub max_bounces: u32,
}
impl Request {
    fn input(&self) -> TrayShadowInput<'_> {
        TrayShadowInput {
            geom: StoneGeometry {
                planes: &self.planes,
                tools: &self.tools,
            },
            material: &self.material,
            preset: self.preset,
            yaw: self.yaw,
            pitch: self.pitch,
            max_bounces: self.max_bounces,
        }
    }
    fn key(&self) -> TrayShadowKey {
        tray_shadow_key(&self.input(), TrayShadowQuality::Preview)
    }
    /// The key with the lighting reset: the same stone, material and bounce depth.
    fn stone_key(&self) -> TrayShadowKey {
        let mut input = self.input();
        input.preset = LightingPreset::IsoHemisphere;
        input.yaw = 0.0;
        input.pitch = 0.0;
        tray_shadow_key(&input, TrayShadowQuality::Preview)
    }
}

/// A [`Request`] with its two keys hashed once, so the render loop can keep one per scene
/// generation instead of hashing the geometry every frame.
pub(super) struct Planned {
    request: Request,
    key: TrayShadowKey,
    stone: TrayShadowKey,
}
impl Planned {
    /// Hashes `request`'s exact key and its reuse (stone) key.
    pub(super) fn new(request: Request) -> Self {
        let key = request.key();
        let stone = request.stone_key();
        Self {
            request,
            key,
            stone,
        }
    }
}

/// What a build publishes: the exact key, the stone key and the map or the reason for none.
type Built = (
    TrayShadowKey,
    TrayShadowKey,
    Result<Option<Arc<TrayShadowMap>>, Refusal>,
);
struct Job {
    request: Request,
    key: TrayShadowKey,
    stone: TrayShadowKey,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
struct Slot {
    pending: Option<Job>,
    result: Option<Built>,
    stop: bool,
}

/// The threads a background build may use: half of the tracer's share, so the live passes keep
/// their cores (the same split as a still-view beam job).
fn build_threads() -> NonZeroUsize {
    NonZeroUsize::new((live_cpu_budget::partition().0 / 2).max(1)).unwrap_or(NonZeroUsize::MIN)
}

/// Builds one job's map, turning a panic into [`Refusal::Failed`].
fn build(job: &Job) -> Result<Option<Arc<TrayShadowMap>>, Refusal> {
    let r = &job.request;
    catch_unwind(AssertUnwindSafe(|| {
        let env = r
            .preset
            .studio(1.0, r.yaw, r.pitch)
            .with_backdrop(r.backdrop);
        stone_shadow_for_with_threads(
            true,
            env,
            r.input().geom,
            &r.material,
            r.max_bounces,
            TrayShadowQuality::Preview,
            &job.cancel,
            &mut |_| {},
            build_threads(),
        )
    }))
    .map_or(Err(Refusal::Failed), |built| built.map_err(Refusal::Off))
}

/// Manages one background builder and publishes only the latest requested result.
pub(super) struct TrayShadowWorker {
    shared: Arc<(Mutex<Slot>, Condvar)>,
    cancel: Option<Arc<AtomicBool>>,
    wanted: Option<TrayShadowKey>,
    ready: Option<(TrayShadowKey, TrayShadowKey, Arc<TrayShadowMap>)>,
    /// Current lighting or geometry restriction, if any.
    pub(super) refusal: Option<Refusal>,
}
impl TrayShadowWorker {
    /// Starts the background builder.
    pub(super) fn spawn() -> Self {
        let shared = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let worker = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("tray-shadow".into())
            .spawn(move || {
                loop {
                    let (lock, wake) = &*worker;
                    let mut slot = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    while slot.pending.is_none() && !slot.stop {
                        slot = wake.wait(slot).unwrap_or_else(PoisonError::into_inner);
                    }
                    if slot.stop {
                        return;
                    }
                    let job = slot.pending.take().expect("pending request");
                    drop(slot);
                    let result = build(&job);
                    let mut slot = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    if !job.cancel.load(Ordering::Acquire) {
                        slot.result = Some((job.key, job.stone, result));
                    }
                }
            })
            .expect("spawn the tray-shadow thread");
        Self {
            shared,
            cancel: None,
            wanted: None,
            ready: None,
            refusal: None,
        }
    }
    /// Returns a compatible map and whether a new result arrived.
    ///
    /// `allow_new` is false while tracing is suspended: a changed request then starts no build
    /// (the ready map stays, and a build already running still lands).
    pub(super) fn update(
        &mut self,
        request: Option<&Planned>,
        exact_only: bool,
        allow_new: bool,
    ) -> (Option<Arc<TrayShadowMap>>, bool) {
        let Some(planned) = request else {
            if self.wanted.is_some() {
                self.cancel();
            }
            self.ready = None;
            self.refusal = None;
            return (None, false);
        };
        let request = &planned.request;
        let decline = if request.preset == LightingPreset::Aset {
            Some(TrayShadowOff::Aset)
        } else if request.preset.is_uv_lamp() {
            Some(TrayShadowOff::UvLamp)
        } else if request.backdrop <= 0.0 {
            Some(TrayShadowOff::AsLit)
        } else {
            None
        };
        if let Some(reason) = decline {
            if self.wanted.is_some() {
                self.cancel();
            }
            self.refusal = Some(Refusal::Off(reason));
            return (None, false);
        }
        let (key, stone) = (planned.key, planned.stone);
        if allow_new && self.wanted != Some(key) {
            self.cancel();
            self.wanted = Some(key);
            self.refusal = None;
            let cancel = Arc::new(AtomicBool::new(false));
            self.cancel = Some(Arc::clone(&cancel));
            let (lock, wake) = &*self.shared;
            lock.lock().unwrap_or_else(PoisonError::into_inner).pending = Some(Job {
                request: request.clone(),
                key,
                stone,
                cancel,
            });
            wake.notify_one();
        }
        let result = self
            .shared
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .result
            .take();
        let mut changed = false;
        if let Some((result_key, result_stone, result)) = result
            && self.wanted == Some(result_key)
        {
            match result {
                Ok(Some(map)) => {
                    self.ready = Some((result_key, result_stone, map));
                    self.refusal = None;
                    changed = true;
                }
                Ok(None) => {}
                Err(reason) => {
                    self.ready = None;
                    self.refusal = Some(reason);
                    changed = true;
                }
            }
        }
        let map = compatible(self.ready.as_ref(), key, stone, exact_only).cloned();
        (map, changed)
    }
    fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.wanted = None;
        let mut slot = self.shared.0.lock().unwrap_or_else(PoisonError::into_inner);
        slot.pending = None;
        slot.result = None;
    }
}

/// A prior light may be shown only for the same stone; mixed remote work requires the exact key.
fn compatible<T>(
    ready: Option<&(TrayShadowKey, TrayShadowKey, T)>,
    key: TrayShadowKey,
    stone: TrayShadowKey,
    exact_only: bool,
) -> Option<&T> {
    ready
        .filter(|(k, s, _)| *s == stone && (!exact_only || *k == key))
        .map(|(_, _, map)| map)
}
impl Drop for TrayShadowWorker {
    fn drop(&mut self) {
        self.cancel();
        let (lock, wake) = &*self.shared;
        lock.lock().unwrap_or_else(PoisonError::into_inner).stop = true;
        wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(preset: LightingPreset, yaw: f32) -> Request {
        Request {
            planes: Arc::new(Vec::new()),
            tools: Arc::new(Vec::new()),
            material: GemMaterial::diamond(),
            preset,
            yaw,
            pitch: 0.5,
            backdrop: 0.5,
            max_bounces: 8,
        }
    }

    fn planned(preset: LightingPreset, yaw: f32) -> Planned {
        Planned::new(request(preset, yaw))
    }

    // A worker without a thread allows deterministic injected completions without building a map.
    fn synthetic_worker() -> TrayShadowWorker {
        TrayShadowWorker {
            shared: Arc::new((Mutex::new(Slot::default()), Condvar::new())),
            cancel: None,
            wanted: None,
            ready: None,
            refusal: None,
        }
    }

    /// A prior light shows only for the same stone, and a remote render needs the exact key.
    #[test]
    fn compatible_map_keeps_only_the_same_stone_and_exact_remote_key() {
        let ready = (TrayShadowKey(1), TrayShadowKey(2), 42);
        assert_eq!(
            compatible(Some(&ready), TrayShadowKey(3), TrayShadowKey(2), false),
            Some(&42)
        );
        assert_eq!(
            compatible(Some(&ready), TrayShadowKey(3), TrayShadowKey(2), true),
            None
        );
        assert_eq!(
            compatible(Some(&ready), TrayShadowKey(1), TrayShadowKey(2), true),
            Some(&42)
        );
        assert_eq!(
            compatible(Some(&ready), TrayShadowKey(1), TrayShadowKey(4), false),
            None
        );
    }

    /// A newer request cancels the older job, and the older job's completion is dropped.
    #[test]
    fn newest_request_cancels_the_previous_and_rejects_stale_completion() {
        let mut worker = synthetic_worker();
        let old = planned(LightingPreset::LightTent, 0.0);
        let (old_key, stone) = (old.key, old.stone);
        worker.update(Some(&old), false, true);
        let old_cancel = Arc::clone(worker.cancel.as_ref().expect("old job"));
        let new = planned(LightingPreset::LightTent, 0.3);
        let new_key = new.key;
        worker.update(Some(&new), false, true);
        assert!(old_cancel.load(Ordering::Acquire));
        let failed = Refusal::Off(TrayShadowOff::DegenerateStone);
        worker.shared.0.lock().unwrap().result = Some((old_key, stone, Err(failed)));
        assert!(!worker.update(Some(&new), false, true).1);
        assert!(worker.refusal.is_none());
        worker.shared.0.lock().unwrap().result = Some((new_key, stone, Err(failed)));
        assert!(worker.update(Some(&new), false, true).1);
        assert_eq!(worker.refusal, Some(failed));
    }

    /// An unsupported light or turning the shadow off cancels the job and starts no retry.
    #[test]
    fn unsupported_lighting_and_disable_cancel_without_retrying() {
        let mut worker = synthetic_worker();
        worker.update(Some(&planned(LightingPreset::LightTent, 0.0)), false, true);
        let token = Arc::clone(worker.cancel.as_ref().unwrap());
        worker.update(Some(&planned(LightingPreset::Aset, 0.0)), false, true);
        assert!(token.load(Ordering::Acquire));
        assert_eq!(worker.refusal, Some(Refusal::Off(TrayShadowOff::Aset)));
        assert!(worker.shared.0.lock().unwrap().pending.is_none());
        worker.update(None, false, true);
        assert!(worker.refusal.is_none());
        assert!(worker.wanted.is_none());
    }

    /// Dropping the worker cancels its job and releases the shared slot.
    #[test]
    fn drop_releases_the_worker_slot_and_cancels_pending_work() {
        let mut worker = synthetic_worker();
        worker.update(Some(&planned(LightingPreset::LightTent, 0.0)), false, true);
        let token = Arc::clone(worker.cancel.as_ref().unwrap());
        let shared = Arc::downgrade(&worker.shared);
        drop(worker);
        assert!(token.load(Ordering::Acquire));
        assert!(shared.upgrade().is_none());
    }

    /// While tracing is suspended, a changed request starts no build.
    #[test]
    fn a_suspended_view_starts_no_new_build() {
        let mut worker = synthetic_worker();
        worker.update(Some(&planned(LightingPreset::LightTent, 0.0)), false, false);
        assert!(worker.wanted.is_none());
        assert!(worker.shared.0.lock().unwrap().pending.is_none());
        worker.update(Some(&planned(LightingPreset::LightTent, 0.0)), false, true);
        assert!(worker.wanted.is_some());
    }

    /// The reuse key follows the bounce depth but not the light.
    #[test]
    fn the_stone_key_follows_the_bounce_depth_and_not_the_light() {
        let base = planned(LightingPreset::LightTent, 0.0);
        let moved = planned(LightingPreset::LightTent, 0.4);
        assert_ne!(base.key, moved.key);
        assert_eq!(base.stone, moved.stone);
        let mut deeper = request(LightingPreset::LightTent, 0.0);
        deeper.max_bounces = 16;
        assert_ne!(base.stone, Planned::new(deeper).stone);
    }

    /// A panicking build is published as a failure instead of killing the thread silently.
    #[test]
    fn a_panic_in_a_build_becomes_a_failure() {
        let result: Result<Option<Arc<TrayShadowMap>>, Refusal> =
            catch_unwind(AssertUnwindSafe(|| -> Result<_, TrayShadowOff> {
                panic!("injected build failure")
            }))
            .map_or(Err(Refusal::Failed), |built| built.map_err(Refusal::Off));
        assert_eq!(result.err(), Some(Refusal::Failed));
        assert_ne!(Refusal::Failed.message(), "");
    }
}
