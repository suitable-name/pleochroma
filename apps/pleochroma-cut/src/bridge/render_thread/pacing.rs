//! Display pacing constants, shared unchanged by both image engines.
use std::time::Duration;

/// Minimum real-time gap between two display cycles sent while samples still accumulate.
///
/// The loop body's `denoise_due` computation lists the cycles that always send regardless
/// (first frame, `dirty`, a dimension change, denoising disabled, reaching `target_samples`).
/// The À-Trous denoiser dominates every live frame (267 ms to 1072 ms per call at 800x600 to
/// 1920x1080), so running it on every ~16 ms iteration while converging capped the viewport
/// at a few FPS. 120 ms still feels live while converging.
///
/// The denoise, tone-map and push work runs on [`super::display_thread`], so the render
/// thread never blocks on a cycle's cost.
pub(super) const DENOISE_MIN_INTERVAL: Duration = Duration::from_millis(120);

/// Timeout for the wait on `display.busy()`. À-Trous cycles run in the 100 ms to 1 s range,
/// so 5 s is generous for a healthy display thread.
pub(super) const DISPLAY_BUSY_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Target length of one loop iteration while the picture is being shown or the camera is
/// moving (~60 FPS).
///
/// # Pacing rule
///
/// The loop sleeps only the *remainder* of this interval, measured from the start of the
/// iteration, and only when the iteration either handled a moving camera or had a display
/// cycle due (`denoise_due` in the loop body). An iteration that is merely adding samples
/// to a still picture and has nothing to show does not sleep at all: the GPU is fed the
/// next chunk immediately instead of idling through readback, hand-off and a fixed sleep.
/// The idle states keep their own sleeps (converged, waiting on the remote lane,
/// suspended), so the loop never spins when there is nothing to trace.
pub(super) const FRAME_PACE: Duration = Duration::from_millis(16);
