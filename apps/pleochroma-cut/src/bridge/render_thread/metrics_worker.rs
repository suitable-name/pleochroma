//! The gemological-metrics worker: evaluates the optical metrics and the angular profile
//! off the render thread, so a light, cut or material change never stalls tracing.
//!
//! One evaluation is an 18x18 grid of multi-bounce traces (see
//! `pleochroma::color::metrics`), and a change of light, facets or material needs twenty
//! of them (the pose plus the 19-point profile). Run inline, that held the render loop for
//! seconds. Here the loop only *requests* an evaluation when its inputs differ from the
//! last request, and keeps using the last published result until a newer one lands.
//!
//! # Scheduling
//!
//! Requests are debounced: the worker starts once [`DEBOUNCE`] has passed without a newer
//! request, so an orbit or a slider drag costs one evaluation after the gesture settles,
//! not one per intermediate pose. [`MAX_DEFER`] bounds that wait, so a gesture that never
//! settles still refreshes the numbers about twice a second. The very first request runs
//! immediately. The thread is an ordinary OS thread, not a rayon task, and does not touch
//! the trace thread pool.

use super::{
    display_thread::FrameMetricsSnapshot, frame_helpers::push_metrics_to_ui, metrics::MetricsCache,
};
use pleochroma::{
    color::metrics::{
        FinishedStone, GemOpticalMetrics, compute_or_reuse_metrics_finished_cancellable,
        finishes_hash,
    },
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, FacetFinish, LightingPreset},
    },
    render_setup::hash_geometry,
    renderer::env_map::EnvironmentMap,
};
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    thread,
    time::{Duration, Instant},
};

/// Quiet time after the latest request before the worker starts evaluating it.
pub(super) const DEBOUNCE: Duration = Duration::from_millis(100);

/// Longest a pending request may wait while newer requests keep replacing it.
pub(super) const MAX_DEFER: Duration = Duration::from_millis(500);

/// One finished evaluation: the pose metrics and the three 19-point profile graphs.
#[derive(Clone, Copy)]
pub(super) struct EvaluatedMetrics {
    pub(super) metrics: GemOpticalMetrics,
    pub(super) graph_brilliance: [f32; 19],
    pub(super) graph_extinction: [f32; 19],
    pub(super) graph_windowing: [f32; 19],
}

impl EvaluatedMetrics {
    /// The display-cycle payload for these values at the current camera pitch.
    pub(super) const fn snapshot(self, cam_pitch_deg: f32) -> FrameMetricsSnapshot {
        FrameMetricsSnapshot {
            metrics: self.metrics,
            graph_brilliance: self.graph_brilliance,
            graph_extinction: self.graph_extinction,
            graph_windowing: self.graph_windowing,
            cam_pitch_deg,
        }
    }
}

/// The values shown before the first evaluation lands, equal to the UI's own initial
/// state.
const NOT_YET_EVALUATED: EvaluatedMetrics = EvaluatedMetrics {
    metrics: GemOpticalMetrics {
        brilliance_pct: 0.0,
        fire_index: 0.0,
        scintillation_pct: 0.0,
        windowing_pct: 0.0,
        extinction_pct: 0.0,
        doubling_arcmin: 0.0,
    },
    graph_brilliance: [0.0; 19],
    graph_extinction: [0.0; 19],
    graph_windowing: [0.0; 19],
};

/// Everything one evaluation reads.
struct Request {
    planes: Arc<Vec<GpuFacetPlane>>,
    /// `hash_geometry` of `planes` and `tools`, computed once at submission so
    /// [`Request::same_profile_inputs`] never rehashes the stone.
    planes_hash: u64,
    /// The concave tools cut out of `planes`; empty for a planar stone.
    tools: Arc<Vec<ToolPrimitive>>,
    /// The finish of each facet, as the viewport renders it (authored frosting and the frosted
    /// girdle); empty for a polished stone.
    finishes: Arc<Vec<FacetFinish>>,
    /// `finishes_hash` of `finishes` (`0` when nothing is frosted), computed once at
    /// submission so [`Request::same_profile_inputs`] never walks the list.
    finishes_hash: u64,
    material: GemMaterial,
    /// `[yaw, pitch, light_yaw, light_pitch]`.
    pose: [f32; 4],
    /// The selected lighting preset: the metrics are scored under the radiance the viewport
    /// is lit with.
    lighting_preset: LightingPreset,
    /// The loaded HDR panorama, which replaces the preset's rig as the lighting.
    env_map: Option<Arc<EnvironmentMap>>,
    head_shadow_deg: f32,
}

impl Request {
    /// The environment this request is scored under: the loaded panorama if any, else the
    /// preset's rig at the request's light pose.
    fn environment(&self) -> EnvironmentSource<'_> {
        let [_, _, light_yaw, light_pitch] = self.pose;
        self.env_map.as_deref().map_or_else(
            || {
                self.lighting_preset
                    .studio(1.0, light_yaw, light_pitch)
                    .with_head_shadow(self.head_shadow_deg)
            },
            EnvironmentSource::HdrMap,
        )
    }

    /// Whether the 19-point angular profile for `self` equals the one for `other`: the
    /// profile does not depend on the camera, only on the stone, material and lighting.
    ///
    /// Deliberately conservative -- it also compares what the cache key ignores for some
    /// presets (the head-shadow size under a studio rig): a spurious "different" only
    /// abandons a profile that the newer request then recomputes, never serves a wrong one.
    fn same_profile_inputs(&self, other: &Self) -> bool {
        let [_, _, light_yaw, light_pitch] = self.pose;
        let [_, _, other_yaw, other_pitch] = other.pose;
        self.planes_hash == other.planes_hash
            && self.finishes_hash == other.finishes_hash
            && self.material == other.material
            && self.lighting_preset == other.lighting_preset
            && same_env_map(self.env_map.as_ref(), other.env_map.as_ref())
            && self.head_shadow_deg.to_bits() == other.head_shadow_deg.to_bits()
            && light_yaw.to_bits() == other_yaw.to_bits()
            && light_pitch.to_bits() == other_pitch.to_bits()
    }
}

/// Whether two optional panoramas are the same loaded map (or both absent).
fn same_env_map(a: Option<&Arc<EnvironmentMap>>, b: Option<&Arc<EnvironmentMap>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// The shared facet planes and concave tools of one stone, as the render loop holds them:
/// a queued request clones the `Arc`s instead of copying the data. `finishes` is the frame's
/// finish list (empty when nothing is frosted); only a changed stone copies it.
#[derive(Clone, Copy)]
pub(super) struct SharedGeometry<'a> {
    pub(super) planes: &'a Arc<Vec<GpuFacetPlane>>,
    pub(super) tools: &'a Arc<Vec<ToolPrimitive>>,
    pub(super) finishes: &'a [FacetFinish],
}

/// A request waiting out its debounce.
struct Pending {
    request: Request,
    first_queued: Instant,
    last_changed: Instant,
}

impl Pending {
    /// When the worker may start: [`DEBOUNCE`] after the last change, but never later
    /// than [`MAX_DEFER`] after the request first queued, and at once while nothing has
    /// been published yet.
    fn due_at(&self, have_result: bool) -> Instant {
        if have_result {
            (self.last_changed + DEBOUNCE).min(self.first_queued + MAX_DEFER)
        } else {
            self.first_queued
        }
    }
}

#[derive(Default)]
struct State {
    pending: Option<Pending>,
    published: Option<EvaluatedMetrics>,
    /// Bumped by every publication; `0` until the first.
    version: u64,
    closed: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Replaces the pending request, keeping the time the first one queued.
    fn submit(&self, request: Request) {
        let now = Instant::now();
        let mut state = self.lock();
        let first_queued = state
            .pending
            .as_ref()
            .map_or(now, |pending| pending.first_queued);
        state.pending = Some(Pending {
            request,
            first_queued,
            last_changed: now,
        });
        drop(state);
        self.wake.notify_one();
    }

    /// Blocks until a request is due and takes it; `None` once the handle is gone.
    fn next_due(&self) -> Option<Request> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            let have_result = state.published.is_some();
            let due = state
                .pending
                .as_ref()
                .map(|pending| pending.due_at(have_result));
            state = match due {
                None => self
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(due) => {
                    let now = Instant::now();
                    if now >= due {
                        return state.pending.take().map(|pending| pending.request);
                    }
                    self.wake
                        .wait_timeout(state, due - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
    }

    /// Whether the evaluation of `current` should be abandoned: the handle is gone, or a
    /// newer request is pending whose profile inputs differ. A pending request that only
    /// moved the camera does not count -- the profile being computed is still the one it
    /// needs.
    fn profile_superseded(&self, current: &Request) -> bool {
        let state = self.lock();
        state.closed
            || state
                .pending
                .as_ref()
                .is_some_and(|pending| !pending.request.same_profile_inputs(current))
    }

    fn publish(&self, evaluated: EvaluatedMetrics) {
        let mut state = self.lock();
        state.published = Some(evaluated);
        state.version += 1;
    }
}

/// The worker thread's body: evaluate what is due, publish, repeat until closed.
fn run(shared: &Shared) {
    // Kept across requests: the cache key covers the stone, material, lighting preset, light
    // pose, HDR map and the effective head-shadow cone, so an orbit stop reuses the 19-point
    // angular profile and only a change of those inputs recomputes it.
    let mut cache: Option<MetricsCache> = None;
    while let Some(request) = shared.next_due() {
        let [yaw, pitch, ..] = request.pose;
        // Asked before each of the profile's 19 poses. An abandoned evaluation publishes
        // and caches nothing; the loop then takes the (newer) pending request.
        let Some((metrics, graph_brilliance, graph_extinction, graph_windowing)) =
            compute_or_reuse_metrics_finished_cancellable(
                &mut cache,
                FinishedStone::new(
                    StoneGeometry {
                        planes: &request.planes,
                        tools: &request.tools,
                    },
                    &request.finishes,
                ),
                &request.material,
                yaw,
                pitch,
                request.environment(),
                &mut || !shared.profile_superseded(&request),
            )
        else {
            continue;
        };
        shared.publish(EvaluatedMetrics {
            metrics,
            graph_brilliance,
            graph_extinction,
            graph_windowing,
        });
    }
}

/// What the last queued request was built from, to tell whether the next inputs differ.
struct Submitted {
    /// `hash_geometry` of the planes and tools: the planes' own `hash_planes` for a
    /// planar stone, so nothing keyed on it moves.
    planes_hash: u64,
    /// `finishes_hash` of the finish list: `0` for a polished stone.
    finishes_hash: u64,
    material: GemMaterial,
    pose: [f32; 4],
    lighting_preset: LightingPreset,
    env_map: Option<Arc<EnvironmentMap>>,
    head_shadow_deg: f32,
}

/// The render thread's handle on the metrics worker.
pub(super) struct MetricsWorker {
    shared: Arc<Shared>,
    submitted: Option<Submitted>,
}

impl MetricsWorker {
    /// Starts the worker thread. If the OS refuses the thread, no metrics are ever
    /// published and the HUD keeps its initial values; rendering is unaffected.
    pub(super) fn spawn() -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("pleochroma-metrics".to_string())
            .spawn(move || run(&worker_shared));
        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the metrics worker; HUD metrics stay at their defaults");
        }
        Self {
            shared,
            submitted: None,
        }
    }

    /// Queues an evaluation of these inputs unless they equal the last ones queued.
    ///
    /// `lighting_preset` and `env_map` are the lighting the viewport renders with (the
    /// panorama, when one is loaded, replaces the preset's rig).
    ///
    /// The frosted facets and girdle (`geometry.finishes`) are part of the stone: changing them
    /// re-queues the evaluation, and the figures are scored on the frosted stone.
    ///
    /// Costs one hash of the facet planes (and tools), one of the frosted facets and a material
    /// comparison on the calling thread; the material is cloned only when it actually changed,
    /// and the finish list only when a request is queued.
    #[expect(
        clippy::float_cmp,
        reason = "pose is an exact change detector: any bit difference must resubmit"
    )]
    pub(super) fn request_with_head_shadow(
        &mut self,
        geometry: SharedGeometry<'_>,
        material: &GemMaterial,
        pose: [f32; 4],
        lighting_preset: LightingPreset,
        env_map: Option<&Arc<EnvironmentMap>>,
        head_shadow_deg: f32,
    ) {
        let SharedGeometry {
            planes,
            tools,
            finishes,
        } = geometry;
        let planes_hash = hash_geometry(StoneGeometry { planes, tools });
        let frosting_hash = finishes_hash(finishes);
        if self.submitted.as_ref().is_some_and(|last| {
            last.planes_hash == planes_hash
                && last.finishes_hash == frosting_hash
                && last.pose == pose
                && last.material == *material
                && last.lighting_preset == lighting_preset
                && last.head_shadow_deg.to_bits() == head_shadow_deg.to_bits()
                && same_env_map(last.env_map.as_ref(), env_map)
        }) {
            return;
        }
        let last = self.submitted.get_or_insert_with(|| Submitted {
            planes_hash,
            finishes_hash: frosting_hash,
            material: material.clone(),
            pose,
            lighting_preset,
            env_map: env_map.cloned(),
            head_shadow_deg,
        });
        last.planes_hash = planes_hash;
        last.finishes_hash = frosting_hash;
        last.pose = pose;
        last.lighting_preset = lighting_preset;
        last.head_shadow_deg = head_shadow_deg;
        last.env_map = env_map.cloned();
        if last.material != *material {
            last.material.clone_from(material);
        }
        self.shared.submit(Request {
            planes: Arc::clone(planes),
            planes_hash,
            tools: Arc::clone(tools),
            finishes: Arc::new(finishes.to_vec()),
            finishes_hash: frosting_hash,
            material: material.clone(),
            pose,
            lighting_preset,
            env_map: env_map.cloned(),
            head_shadow_deg,
        });
    }

    #[cfg(test)]
    fn request(
        &mut self,
        planes: &Arc<Vec<GpuFacetPlane>>,
        tools: &Arc<Vec<ToolPrimitive>>,
        material: &GemMaterial,
        pose: [f32; 4],
        preset: LightingPreset,
        hdr: Option<&Arc<EnvironmentMap>>,
    ) {
        self.request_finished(
            SharedGeometry {
                planes,
                tools,
                finishes: &[],
            },
            material,
            pose,
            preset,
            hdr,
        );
    }

    #[cfg(test)]
    fn request_finished(
        &mut self,
        geometry: SharedGeometry<'_>,
        material: &GemMaterial,
        pose: [f32; 4],
        preset: LightingPreset,
        hdr: Option<&Arc<EnvironmentMap>>,
    ) {
        self.request_with_head_shadow(
            geometry,
            material,
            pose,
            preset,
            hdr,
            pleochroma::optics::raytracer::DEFAULT_HEAD_SHADOW_DEG,
        );
    }
    /// The newest published values and their version (`0` while still the defaults).
    ///
    /// Possibly for inputs older than the last [`Self::request`]: the loop keeps showing
    /// these until the debounced evaluation replaces them.
    pub(super) fn latest(&self) -> (u64, EvaluatedMetrics) {
        let state = self.shared.lock();
        (state.version, state.published.unwrap_or(NOT_YET_EVALUATED))
    }
}

impl Drop for MetricsWorker {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.wake.notify_all();
    }
}

/// Pushes the newest published metrics to the UI without an image, if `pushed_version` has
/// not seen them yet. This is what updates the HUD once the picture has converged and no
/// display cycle is left to carry them.
///
/// `ui_idle` must be `false` while a display cycle is in flight: that cycle carries its
/// own copy of the metrics, and pushing around it could let the older copy land last.
pub(super) fn push_if_fresh<T, M>(
    worker: &MetricsWorker,
    pushed_version: &mut u64,
    ui_idle: bool,
    ui_weak: &Weak<T>,
    update_metrics: &M,
    cam_pitch_deg: f32,
) where
    T: ComponentHandle + 'static,
    M: Fn(&T, f32, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
{
    let (version, evaluated) = worker.latest();
    if ui_idle && version > *pushed_version {
        *pushed_version = version;
        push_metrics_to_ui(ui_weak, update_metrics, evaluated.snapshot(cam_pitch_deg));
    }
}

/// Whether the suspended-tab loop has anything new for the UI: a newer evaluation than
/// the last push, or a moved camera-pitch marker (compared bitwise; `None` = nothing
/// pushed yet).
fn metrics_push_due(
    version: u64,
    pushed_version: u64,
    cam_pitch_deg: f32,
    pushed_pitch_deg: Option<f32>,
) -> bool {
    version > pushed_version
        || pushed_pitch_deg.is_none_or(|pushed| pushed.to_bits() != cam_pitch_deg.to_bits())
}

/// Hands `snapshot` to `push` and records what was pushed, only when
/// [`metrics_push_due`]; returns whether it did. The decision half of
/// [`push_if_changed`], separate so it can be tested without a UI.
fn push_snapshot_if_due(
    version: u64,
    pushed_version: &mut u64,
    pushed_pitch_deg: &mut Option<f32>,
    snapshot: FrameMetricsSnapshot,
    push: impl FnOnce(FrameMetricsSnapshot),
) -> bool {
    let cam_pitch_deg = snapshot.cam_pitch_deg;
    if !metrics_push_due(version, *pushed_version, cam_pitch_deg, *pushed_pitch_deg) {
        return false;
    }
    *pushed_version = (*pushed_version).max(version);
    *pushed_pitch_deg = Some(cam_pitch_deg);
    push(snapshot);
    true
}

/// The suspended-tab counterpart of [`push_if_fresh`]: pushes the newest published metrics
/// only when they are newer than the last push or the camera-pitch marker moved, so an
/// idle hidden tab no longer re-sends identical values every 100 ms tick. No `ui_idle`
/// gate: a suspended loop has no display cycle in flight.
pub(super) fn push_if_changed<T, M>(
    worker: &MetricsWorker,
    pushed_version: &mut u64,
    pushed_pitch_deg: &mut Option<f32>,
    ui_weak: &Weak<T>,
    update_metrics: &M,
    cam_pitch_deg: f32,
) where
    T: ComponentHandle + 'static,
    M: Fn(&T, f32, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
{
    let (version, evaluated) = worker.latest();
    push_snapshot_if_due(
        version,
        pushed_version,
        pushed_pitch_deg,
        evaluated.snapshot(cam_pitch_deg),
        |snapshot| push_metrics_to_ui(ui_weak, update_metrics, snapshot),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(first_queued: Instant, last_changed: Instant) -> Pending {
        Pending {
            request: Request {
                planes: Arc::new(Vec::new()),
                planes_hash: 0,
                tools: Arc::new(Vec::new()),
                finishes: Arc::new(Vec::new()),
                finishes_hash: 0,
                material: GemMaterial::diamond(),
                pose: [0.0; 4],
                lighting_preset: LightingPreset::RingLights,
                env_map: None,
                head_shadow_deg: pleochroma::optics::raytracer::DEFAULT_HEAD_SHADOW_DEG,
            },
            first_queued,
            last_changed,
        }
    }

    fn empty_shared() -> Shared {
        Shared {
            state: Mutex::new(State::default()),
            wake: Condvar::new(),
        }
    }

    fn shared_with_pending(request: Request) -> Shared {
        let now = Instant::now();
        let shared = empty_shared();
        shared.lock().pending = Some(Pending {
            request,
            first_queued: now,
            last_changed: now,
        });
        shared
    }

    fn request_with(material: GemMaterial, pose: [f32; 4]) -> Request {
        Request {
            planes: Arc::new(Vec::new()),
            planes_hash: 7,
            tools: Arc::new(Vec::new()),
            finishes: Arc::new(Vec::new()),
            finishes_hash: 0,
            material,
            pose,
            lighting_preset: LightingPreset::RingLights,
            env_map: None,
            head_shadow_deg: pleochroma::optics::raytracer::DEFAULT_HEAD_SHADOW_DEG,
        }
    }

    #[test]
    fn a_pending_request_with_other_profile_inputs_supersedes_the_running_one() {
        let running = request_with(GemMaterial::diamond(), [0.1, 0.2, 0.3, 0.4]);
        let other_material = request_with(GemMaterial::ruby(), running.pose);
        assert!(other_material.material != running.material, "test premise");
        assert!(shared_with_pending(other_material).profile_superseded(&running));

        let moved_light = request_with(GemMaterial::diamond(), [0.1, 0.2, 0.9, 0.4]);
        assert!(shared_with_pending(moved_light).profile_superseded(&running));
    }

    #[test]
    fn a_pending_request_with_other_finishes_supersedes_the_running_one() {
        let running = request_with(GemMaterial::diamond(), [0.1, 0.2, 0.3, 0.4]);
        let mut frosted = request_with(GemMaterial::diamond(), running.pose);
        frosted.finishes = Arc::new(vec![FacetFinish::Frosted]);
        frosted.finishes_hash = finishes_hash(&frosted.finishes);
        assert_ne!(
            frosted.finishes_hash, 0,
            "test premise: a frosted facet hashes"
        );
        assert!(!running.same_profile_inputs(&frosted));
        assert!(shared_with_pending(frosted).profile_superseded(&running));
    }

    #[test]
    fn a_pose_only_change_or_no_pending_request_does_not_supersede() {
        let running = request_with(GemMaterial::diamond(), [0.1, 0.2, 0.3, 0.4]);
        let orbited = request_with(GemMaterial::diamond(), [0.7, 0.5, 0.3, 0.4]);
        assert!(!shared_with_pending(orbited).profile_superseded(&running));
        assert!(!empty_shared().profile_superseded(&running));
    }

    #[test]
    fn a_closed_worker_supersedes_everything() {
        let running = request_with(GemMaterial::diamond(), [0.1, 0.2, 0.3, 0.4]);
        let shared = empty_shared();
        shared.lock().closed = true;
        assert!(shared.profile_superseded(&running));
    }

    /// The suspended loop routes through this guard: an unchanged version and pitch
    /// pushes nothing, a moved pitch or a newer version pushes exactly once.
    #[test]
    fn the_suspended_push_guard_sends_only_changes() {
        let mut pushed_version = 3;
        let mut pushed_pitch = Some(30.0_f32);
        let mut pushes = 0;
        let mut call =
            |version: u64, pitch: f32, version_seen: &mut u64, pitch_seen: &mut Option<f32>| {
                push_snapshot_if_due(
                    version,
                    version_seen,
                    pitch_seen,
                    NOT_YET_EVALUATED.snapshot(pitch),
                    |_| pushes += 1,
                )
            };

        assert!(!call(3, 30.0, &mut pushed_version, &mut pushed_pitch));
        assert!(call(3, 31.0, &mut pushed_version, &mut pushed_pitch));
        assert!(!call(3, 31.0, &mut pushed_version, &mut pushed_pitch));
        assert!(call(4, 31.0, &mut pushed_version, &mut pushed_pitch));
        assert!(!call(4, 31.0, &mut pushed_version, &mut pushed_pitch));
        assert_eq!(pushed_version, 4);
        assert_eq!(pushed_pitch.map(f32::to_bits), Some(31.0_f32.to_bits()));
        assert_eq!(pushes, 2);
    }

    #[test]
    fn nothing_pushed_yet_pushes_once() {
        assert!(metrics_push_due(0, 0, 0.0, None));
        assert!(!metrics_push_due(0, 0, 0.0, Some(0.0)));
    }

    /// The Doubling figure reaches the display cycle's payload, which hands it to the UI
    /// callback (`frame_helpers::push_metrics_to_ui`) and so to the readout.
    #[test]
    fn the_snapshot_carries_the_doubling_figure_to_the_ui_callback() {
        let mut evaluated = NOT_YET_EVALUATED;
        assert_eq!(
            evaluated.snapshot(0.0).metrics.doubling_arcmin.to_bits(),
            0.0f32.to_bits(),
            "an isotropic or not-yet-evaluated stone reads zero"
        );
        evaluated.metrics.doubling_arcmin = 3.5;
        let snapshot = evaluated.snapshot(12.0);
        assert_eq!(snapshot.metrics.doubling_arcmin.to_bits(), 3.5f32.to_bits());
        assert_eq!(snapshot.cam_pitch_deg.to_bits(), 12.0f32.to_bits());
    }

    #[test]
    fn a_settled_request_starts_after_the_debounce() {
        let t0 = Instant::now();
        assert_eq!(pending(t0, t0).due_at(true), t0 + DEBOUNCE);
    }

    #[test]
    fn a_request_that_keeps_changing_starts_no_later_than_the_max_defer() {
        let t0 = Instant::now();
        let last_changed = t0 + MAX_DEFER;
        assert_eq!(pending(t0, last_changed).due_at(true), t0 + MAX_DEFER);
    }

    #[test]
    fn the_first_request_starts_immediately() {
        let t0 = Instant::now();
        assert_eq!(pending(t0, t0 + DEBOUNCE).due_at(false), t0);
    }

    #[test]
    fn an_unchanged_request_is_not_queued_twice() {
        let mut worker = MetricsWorker {
            shared: Arc::new(Shared {
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
            }),
            submitted: None,
        };
        let planes = Arc::new(Vec::new());
        let tools = Arc::new(Vec::new());
        let material = GemMaterial::diamond();
        let pose = [0.1, 0.2, 0.3, 0.4];
        let preset = LightingPreset::RingLights;

        worker.request(&planes, &tools, &material, pose, preset, None);
        let first = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert!(first.is_some(), "the first request must queue");

        worker.request(&planes, &tools, &material, pose, preset, None);
        let second = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_eq!(first, second, "identical inputs must not re-queue");

        thread::sleep(Duration::from_millis(2));
        worker.request(
            &planes,
            &tools,
            &material,
            [0.1, 0.2, 0.3, 0.5],
            preset,
            None,
        );
        let third = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_ne!(second, third, "a changed light pose must re-queue");

        thread::sleep(Duration::from_millis(2));
        worker.request(
            &planes,
            &tools,
            &material,
            [0.1, 0.2, 0.3, 0.5],
            LightingPreset::LightTent,
            None,
        );
        let fourth = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_ne!(third, fourth, "a changed lighting preset must re-queue");

        thread::sleep(Duration::from_millis(2));
        let map = Arc::new(EnvironmentMap::uniform(4, 2, [1.0, 1.0, 1.0]));
        worker.request(
            &planes,
            &tools,
            &material,
            [0.1, 0.2, 0.3, 0.5],
            LightingPreset::LightTent,
            Some(&map),
        );
        let fifth = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_ne!(fourth, fifth, "a loaded HDR map must re-queue");

        thread::sleep(Duration::from_millis(2));
        let cut = Arc::new(vec![ToolPrimitive::ball(
            glam::Vec3::new(0.0, 0.3, 0.0),
            0.1,
        )]);
        worker.request(
            &planes,
            &cut,
            &material,
            [0.1, 0.2, 0.3, 0.5],
            LightingPreset::LightTent,
            Some(&map),
        );
        let sixth = worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed);
        assert_ne!(
            fifth, sixth,
            "adding a concave tool changes the stone, so the metrics must be re-evaluated"
        );
    }

    fn worker_for_tests() -> MetricsWorker {
        MetricsWorker {
            shared: Arc::new(empty_shared()),
            submitted: None,
        }
    }

    fn last_queued(worker: &MetricsWorker) -> Option<Instant> {
        worker
            .shared
            .lock()
            .pending
            .as_ref()
            .map(|pending| pending.last_changed)
    }

    /// The frosted facets and the frosted girdle are part of the stone the figures describe:
    /// editing them re-queues the evaluation, an unchanged list does not.
    #[test]
    fn a_frosting_change_requeues_the_metrics() {
        let mut worker = worker_for_tests();
        let planes = Arc::new(Vec::new());
        let tools = Arc::new(Vec::new());
        let material = GemMaterial::diamond();
        let pose = [0.1, 0.2, 0.3, 0.4];
        let preset = LightingPreset::RingLights;
        let ask = |worker: &mut MetricsWorker, finishes: &[FacetFinish]| {
            thread::sleep(Duration::from_millis(2));
            worker.request_finished(
                SharedGeometry {
                    planes: &planes,
                    tools: &tools,
                    finishes,
                },
                &material,
                pose,
                preset,
                None,
            );
            last_queued(worker)
        };

        let polished = ask(&mut worker, &[]);
        assert!(polished.is_some(), "the first request must queue");
        assert_eq!(
            ask(&mut worker, &[]),
            polished,
            "the same polished stone must not re-queue"
        );
        assert_eq!(
            ask(&mut worker, &[FacetFinish::Polished, FacetFinish::Polished]),
            polished,
            "an all-polished list is the polished stone"
        );

        let frosted = ask(&mut worker, &[FacetFinish::Frosted]);
        assert_ne!(polished, frosted, "frosting a facet must re-queue");
        assert_eq!(
            ask(&mut worker, &[FacetFinish::Frosted]),
            frosted,
            "the same frosting must not re-queue"
        );
        assert_eq!(
            worker
                .shared
                .lock()
                .pending
                .as_ref()
                .map(|pending| pending.request.finishes.to_vec()),
            Some(vec![FacetFinish::Frosted]),
            "the queued request carries the finish list the numbers are scored on"
        );

        let moved = ask(&mut worker, &[FacetFinish::Polished, FacetFinish::Frosted]);
        assert_ne!(frosted, moved, "frosting another facet must re-queue");

        let polished_again = ask(&mut worker, &[]);
        assert_ne!(moved, polished_again, "removing the frosting must re-queue");
    }
}
