//! The two off-thread background generations the orchestrator keeps in flight.
//!
//! They are the guide-buffer prepass ([`PendingGuideGeneration`]/[`spawn_guide_generation`]/
//! [`adopt_ready_guides`]) and the full denoise-and-tonemap pass
//! ([`PendingDenoiseGeneration`]/[`spawn_denoise_generation`]/[`adopt_ready_denoise`]).
//! See this group's own `mod.rs` doc comment.

use super::compositing::{AccumSnapshot, PoseAndGeometry, render_merged_frame};
use crate::bridge::{
    frame_cache::guide_pass::{
        GuideBuffers, GuideCache, GuideKey, generate_guide_buffers_cancellable_geom_with_index,
    },
    render_thread::DenoiseScratch,
};
use glam::Vec3;
use pleochroma::{
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::raytracer::Camera,
    renderer::denoise::AtrousDenoiser,
};
use std::sync::{Arc, Mutex, PoisonError, atomic::AtomicBool};

/// A background primary-ray guide-buffer prepass for one dispatched `RenderRequest`. It
/// starts at dispatch (`tick::dispatch::start_remote_render`), so it overlaps the network
/// round trip and the remote render.
///
/// Cancellation is cooperative: setting `cancel` makes the worker's per-row check stop
/// early (the same `Arc<AtomicBool>` pattern as `bridge::export_thread::ExportHandle`).
///
/// A superseded generation cannot overwrite a newer result, because each
/// [`spawn_guide_generation`] allocates a fresh `result` slot. A late write lands in a slot
/// that nothing reads any more.
pub(super) struct PendingGuideGeneration {
    /// The pose/geometry/resolution this generation is for -- the same [`GuideKey`]
    /// identity `GuideCache` keys its own cache on. Compared against the current
    /// desired key on every redraw (`adopt_ready_guides`) so a result is only ever
    /// adopted when it matches the image currently being displayed.
    pub(super) key: GuideKey,
    /// Cooperative cancellation flag: set when the pose changes again (the render this
    /// generation was for gets cancelled/superseded), so the background thread abandons
    /// a now-pointless computation instead of running it to completion.
    pub(super) cancel: Arc<AtomicBool>,
    /// Filled in by the background thread once, and only once, if it finishes without
    /// observing `cancel`. `None` while generation is still in flight (or was
    /// abandoned) -- callers must treat a `None` read as "not ready yet", never block
    /// waiting for it.
    result: Arc<Mutex<Option<GuideBuffers>>>,
}

/// Kicks off the guide-buffer prepass for `key` on a background thread. Does NOT cancel
/// or replace any previous [`PendingGuideGeneration`] itself -- the caller
/// (`super::tick::start_remote_render`) does that first, since only it knows whether a
/// previous generation exists at all.
pub(super) fn spawn_guide_generation(
    key: GuideKey,
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    tools: Vec<ToolPrimitive>,
    width: u32,
    height: u32,
    n_d: f32,
) -> PendingGuideGeneration {
    let cancel = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    let cancel_worker = Arc::clone(&cancel);
    let result_worker = Arc::clone(&result);

    std::thread::spawn(move || {
        if let Some(buffers) = generate_guide_buffers_cancellable_geom_with_index(
            width,
            height,
            &camera,
            StoneGeometry {
                planes: &planes,
                tools: &tools,
            },
            n_d,
            &cancel_worker,
        ) {
            *result_worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(buffers);
        }
        // A `None` (cancelled) result is simply dropped: `pending_guide_gen` has
        // already moved on to a different generation by the time cancellation is
        // observed, so nothing would read this result even if it were written.
    });

    PendingGuideGeneration {
        key,
        cancel,
        result,
    }
}

/// Adopts a background generation's guides into `guide_cache` when they match
/// `desired_key` (the pose and geometry of the image about to be drawn). Returns `false`
/// when they are not ready, so the caller falls back to a plain tonemap instead of
/// blocking the UI thread to regenerate guides.
///
/// Apart from one lock on the result slot it is pure, so the tests use a hand-built
/// [`PendingGuideGeneration`] with no background thread.
pub(super) fn adopt_ready_guides(
    desired_key: &GuideKey,
    guide_cache: &mut GuideCache,
    pending: Option<&PendingGuideGeneration>,
) -> bool {
    if guide_cache.matches_key(desired_key) {
        // Already installed, or `ensure` was called for this exact key before.
        return true;
    }

    let Some(pending) = pending else {
        return false;
    };
    if pending.key != *desired_key {
        // In-flight (or finished) generation is for a different pose -- reject it.
        return false;
    }

    let mut slot = pending
        .result
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(buffers) = slot.take() else {
        // Still in flight.
        return false;
    };
    drop(slot);
    guide_cache.adopt(desired_key.clone(), buffers);
    true
}

/// The full À-Trous denoise-and-tonemap pass running on a background thread for one
/// pose and buffer snapshot. Like [`PendingGuideGeneration`], each dispatch gets a fresh
/// `result` slot, so a stale write is never read.
///
/// Unlike guide generation (once per `RenderRequest`), a denoise generation is redispatched
/// each time the previous one is adopted, since the accumulation buffer keeps growing.
/// There is no cooperative `cancel`: `AtrousDenoiser` has no early exit, so an abandoned
/// generation runs to completion and touches nothing else. Correctness comes from the key
/// check in [`adopt_ready_denoise`], not from timing.
pub(super) struct PendingDenoiseGeneration {
    /// The pose/geometry/resolution this generation's output is valid for; a result is
    /// only adopted once [`adopt_ready_denoise`] confirms this still equals the pose
    /// currently on screen. The buffer's sample count is deliberately not part of the
    /// key: a denoised frame a few samples behind the latest accumulation is still a
    /// valid (if slightly stale) image of the same pose.
    pub(super) key: GuideKey,
    /// Filled in by the background thread once, and only once, with the finished RGBA
    /// bytes. `None` while still in flight; callers must treat that as "not ready yet",
    /// never block waiting for it.
    result: Arc<Mutex<Option<Vec<u8>>>>,
}

/// Everything [`spawn_denoise_generation`] needs on its background thread, owned rather
/// than borrowed since that thread outlives this function's own call frame.
pub(super) struct DenoiseGenerationJob {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) samples_done: u32,
    pub(super) buffer: Vec<Vec3>,
    pub(super) guides: GuideBuffers,
    pub(super) yaw: f32,
    pub(super) camera_roll: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) planes: Vec<GpuFacetPlane>,
    /// The concave tools cut out of `planes`; empty for a planar stone.
    pub(super) tools: Vec<ToolPrimitive>,
    /// The material's `n_d` (part of `guides`' key).
    pub(super) n_d: f32,
}

/// Starts one denoise-and-tonemap pass on a background thread for `key`'s pose, over the
/// given buffer snapshot and `guides`. The caller has already confirmed the guides match
/// `key` with [`adopt_ready_guides`], and clones them out of `guide_cache`, which only the
/// UI thread touches.
///
/// It runs through [`render_merged_frame`] with a throwaway [`GuideCache`] pre-seeded with
/// `guides`, so the internal `ensure` is a cache hit. The throwaway denoiser and scratch
/// buffers are owned by this thread alone, and their allocation is negligible next to the pass.
pub(super) fn spawn_denoise_generation(
    key: GuideKey,
    job: DenoiseGenerationJob,
) -> PendingDenoiseGeneration {
    let result = Arc::new(Mutex::new(None));
    let result_worker = Arc::clone(&result);
    let key_for_thread = key.clone();

    std::thread::spawn(move || {
        let mut guide_cache = GuideCache::new();
        guide_cache.adopt(key_for_thread, job.guides);
        let mut denoiser = AtrousDenoiser::new();
        let mut avg_color_buf = Vec::new();
        let mut filtered_buf = Vec::new();
        let bytes = render_merged_frame(
            AccumSnapshot {
                width: job.width,
                height: job.height,
                samples_done: job.samples_done,
                buffer: &job.buffer,
            },
            true,
            PoseAndGeometry {
                yaw: job.yaw,
                camera_roll: job.camera_roll,
                pitch: job.pitch,
                distance: job.distance,
                planes: &job.planes,
                tools: &job.tools,
                n_d: job.n_d,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg_color_buf,
                filtered_buf: &mut filtered_buf,
            },
        );
        *result_worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(bytes);
    });

    PendingDenoiseGeneration { key, result }
}

/// Checks whether a background denoise pass is finished and still valid for
/// `desired_key`, taking (and returning) its bytes if so. `None` covers "nothing in
/// flight", "in flight for a different pose", and "in flight but not done yet" -- the
/// caller doesn't need to distinguish, since every fallback (show the last-adopted
/// frame, or a plain tonemap) is the same regardless.
///
/// Pure aside from the one `Mutex` lock on `pending`'s result slot, so it's directly
/// unit-testable with a hand-built [`PendingDenoiseGeneration`] and no real background
/// thread.
pub(super) fn adopt_ready_denoise(
    desired_key: &GuideKey,
    pending: Option<&PendingDenoiseGeneration>,
) -> Option<Vec<u8>> {
    let pending = pending?;
    if pending.key != *desired_key {
        return None;
    }
    let mut slot = pending
        .result
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    slot.take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::frame_cache::guide_pass::generate_guide_buffers;
    use pleochroma::{geometry::cuts::StandardGemCuts, optics::raytracer::DEFAULT_FOV_DEG};

    /// Builds a [`PendingGuideGeneration`] whose result is already sitting in its slot
    /// (as if the background thread had already finished), for `key`.
    fn ready_pending(key: GuideKey, buffers: GuideBuffers) -> PendingGuideGeneration {
        PendingGuideGeneration {
            key,
            cancel: Arc::new(AtomicBool::new(false)),
            result: Arc::new(Mutex::new(Some(buffers))),
        }
    }

    /// Builds a [`PendingDenoiseGeneration`] whose result is already sitting in its slot
    /// (as if the background thread had already finished), for `key`.
    fn ready_denoise_pending(key: GuideKey, bytes: Vec<u8>) -> PendingDenoiseGeneration {
        PendingDenoiseGeneration {
            key,
            result: Arc::new(Mutex::new(Some(bytes))),
        }
    }

    /// Builds a [`PendingDenoiseGeneration`] that is still in flight -- no result yet --
    /// for `key`.
    fn in_flight_denoise_pending(key: GuideKey) -> PendingDenoiseGeneration {
        PendingDenoiseGeneration {
            key,
            result: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn adopt_ready_denoise_returns_none_when_nothing_is_pending() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);
        assert_eq!(adopt_ready_denoise(&key, None), None);
    }

    #[test]
    fn adopt_ready_denoise_returns_none_while_still_in_flight() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);
        let pending = in_flight_denoise_pending(key.clone());

        assert_eq!(
            adopt_ready_denoise(&key, Some(&pending)),
            None,
            "a generation that hasn't produced a result yet is 'not ready', not an error"
        );
    }

    #[test]
    fn adopt_ready_denoise_takes_the_result_once_ready_and_matching() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);
        let bytes = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let pending = ready_denoise_pending(key.clone(), bytes.clone());

        assert_eq!(
            adopt_ready_denoise(&key, Some(&pending)),
            Some(bytes),
            "a finished generation for the exact pose on screen must be adopted"
        );
        assert_eq!(
            adopt_ready_denoise(&key, Some(&pending)),
            None,
            "the result is taken, not cloned -- a second read must not resurrect it"
        );
    }

    #[test]
    fn adopt_ready_denoise_rejects_a_result_for_a_different_pose() {
        let planes = StandardGemCuts::standard_round_brilliant();
        // Finished, but for a different yaw than the pose being rendered -- e.g. it
        // settled on the previous pose right as a new drag started.
        let stale_key = GuideCache::key_for(6, 5, 0.10, 0.45, 2.4, &planes);
        let pending = ready_denoise_pending(stale_key, vec![9u8, 9, 9, 9]);

        let current_key = GuideCache::key_for(6, 5, 0.60, 0.45, 2.4, &planes);

        assert_eq!(
            adopt_ready_denoise(&current_key, Some(&pending)),
            None,
            "a background result for a pose other than the one displayed must never be \
             adopted -- rejection is structural (a key comparison), not timing-dependent"
        );
    }

    #[test]
    fn a_superseded_generations_late_result_does_not_overwrite_newer_guides() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);

        // The cache already holds the current (newer) pose's guides, as if an earlier
        // redraw had already adopted them.
        let current_key = GuideCache::key_for(width, height, 0.90, 0.45, 2.4, &planes);
        let current_camera = Camera::new(0.90, 0.45, 2.4, DEFAULT_FOV_DEG);
        let current_buffers = generate_guide_buffers(width, height, &current_camera, &planes);
        let mut guide_cache = GuideCache::new();
        guide_cache.adopt(current_key.clone(), current_buffers.clone());

        // A generation for an older pose finally finishes late, after being superseded
        // (its cancel flag was set, but it raced past the last check anyway).
        let old_key = GuideCache::key_for(width, height, 0.10, 0.45, 2.4, &planes);
        let old_camera = Camera::new(0.10, 0.45, 2.4, DEFAULT_FOV_DEG);
        let old_buffers = generate_guide_buffers(width, height, &old_camera, &planes);
        let superseded = ready_pending(old_key, old_buffers);

        let adopted = adopt_ready_guides(&current_key, &mut guide_cache, Some(&superseded));

        assert!(
            adopted,
            "the cache already had the correct guides for the current pose"
        );
        assert!(
            guide_cache.matches_key(&current_key),
            "a superseded generation's late result for an older pose must never \
             overwrite the current pose's already-adopted guides"
        );
        assert_eq!(
            guide_cache
                .ensure(width, height, 0.90, 0.45, 2.4, &planes)
                .depth,
            current_buffers.depth,
            "the buffers in the cache must still be the current pose's, not the stale \
             generation's"
        );
    }
}
