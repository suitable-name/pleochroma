//! Disposable previews before independent final-quality accumulation.
//!
//! Each stationary scene starts with a quarter-resolution, one-sample bootstrap.
//! Camera movement uses the configured drag scale. Only a completed preview handed
//! to display allows the same scene to begin final accumulation; sample-target edits
//! keep that scene identity and continue its final samples. Remote epochs always
//! retain their configured dimensions and sample ranges.

use crate::settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale};

/// Preview radiance is disposable and never belongs to the final sample epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RenderPhase {
    Bootstrap,
    Moving,
    Final,
}

impl RenderPhase {
    pub(super) const fn is_preview(self) -> bool {
        !matches!(self, Self::Final)
    }

    /// Combined compute can show its first image without waiting for a shader.
    /// An explicit GPU choice continues to use the GPU for every phase.
    pub(super) const fn compute(self, configured: LocalComputeTarget) -> LocalComputeTarget {
        if self.is_preview() && matches!(configured, LocalComputeTarget::CpuGpu) {
            LocalComputeTarget::Cpu
        } else {
            configured
        }
    }
}

/// One cheap image per scene, followed by an independent final accumulation.
/// A target-only edit has the same generation and cannot restart this bootstrap.
#[derive(Default)]
pub(super) struct PreviewState {
    generation: Option<u64>,
    published: bool,
    previous_phase: Option<RenderPhase>,
}

impl PreviewState {
    pub(super) fn select(
        &mut self,
        generation: u64,
        moving: bool,
        remote_active: bool,
        target: LiveComputeTarget,
        display_ready: bool,
    ) -> (RenderPhase, bool) {
        let generation_changed = self.generation != Some(generation);
        if generation_changed {
            self.generation = Some(generation);
            self.published = false;
        }
        let phase = if remote_active || matches!(target, LiveComputeTarget::RemoteOnly) {
            RenderPhase::Final
        } else if moving {
            RenderPhase::Moving
        } else if self.published
            && (display_ready || self.previous_phase == Some(RenderPhase::Final))
        {
            RenderPhase::Final
        } else {
            RenderPhase::Bootstrap
        };
        // Explicitly reset even if floor-dividing a tiny frame kept its dimensions.
        let changed = generation_changed
            || self
                .previous_phase
                .is_some_and(|previous| previous != phase);
        self.previous_phase = Some(phase);
        (phase, changed)
    }

    /// Called only after a complete, uncancelled preview was handed to display.
    pub(super) const fn published(&mut self) {
        self.published = true;
    }
}

/// Whether a display cycle may run the denoiser. The first-hit guides (depth, normal,
/// facet id, path signature) are written only by local passes, so a cycle before any
/// local pass of the current accumulation -- a live epoch whose remote lane claimed
/// every sample index -- would filter against the reset (all-miss) guides. It shows the
/// plain tone-mapped average until a local pass has produced guides.
pub(super) const fn denoise_this_cycle(
    enabled: bool,
    phase: RenderPhase,
    local_samples: u32,
) -> bool {
    enabled && !phase.is_preview() && local_samples > 0
}

/// Decides whether a held mouse button counts as camera movement.
///
/// A plain press enters no motion: it would switch to [`RenderPhase::Moving`] and
/// back on release, and each switch resets the accumulation, so a click on the
/// viewport would throw away a converged image. A held drag therefore counts as
/// moving once the scene (pose or light) has changed since the press.
#[derive(Default)]
pub(super) struct DragGesture {
    press_generation: Option<u64>,
    moved: bool,
}

impl DragGesture {
    /// `handoff_moving` is the orchestrator's settled-motion flag
    /// (`RenderContext::camera_moving`); `drag_held` the raw button state.
    pub(super) fn moving(
        &mut self,
        handoff_moving: bool,
        drag_held: bool,
        generation: u64,
    ) -> bool {
        if !drag_held {
            *self = Self::default();
            return handoff_moving;
        }
        let press = *self.press_generation.get_or_insert(generation);
        self.moved |= generation != press;
        handoff_moving || self.moved
    }
}

pub(super) fn phase_dimensions(
    width: u32,
    height: u32,
    scale: LocalPreviewScale,
    phase: RenderPhase,
) -> (u32, u32) {
    match phase {
        // `Off` opts out of drag scaling; a new stationary scene still gets a
        // bounded bootstrap before paying for the requested final resolution.
        RenderPhase::Bootstrap => {
            effective_dimensions(width, height, LocalPreviewScale::Quarter, true)
        }
        RenderPhase::Moving => effective_dimensions(width, height, scale, true),
        RenderPhase::Final => (width, height),
    }
}

/// Resolves the `width x height` the render loop should trace THIS frame, from the
/// configured `width x height`, the [`LocalPreviewScale`] choice, and whether the
/// camera is currently `moving`.
///
/// Returns `(width, height)` unchanged whenever `scale` is [`LocalPreviewScale::Off`]
/// or `moving` is `false`. Only `moving && scale != Off` reduces the dimensions,
/// floored at `1x1` so a tiny configured resolution never resolves to a zero-area
/// frame -- same floor `PreviewScale::resolve` applies for the remote `PREVIEW` case.
#[must_use]
pub fn effective_dimensions(
    width: u32,
    height: u32,
    scale: LocalPreviewScale,
    moving: bool,
) -> (u32, u32) {
    if !moving {
        return (width, height);
    }
    let divisor = scale.divisor();
    if divisor <= 1 {
        return (width, height);
    }
    ((width / divisor).max(1), (height / divisor).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_phase(
        state: &mut PreviewState,
        generation: u64,
        display_ready: bool,
    ) -> (RenderPhase, bool) {
        state.select(
            generation,
            false,
            false,
            LiveComputeTarget::LocalOnly,
            display_ready,
        )
    }

    #[test]
    fn bootstrap_finishes_display_before_final_and_never_restarts_for_a_target_edit() {
        let mut state = PreviewState::default();
        assert_eq!(
            local_phase(&mut state, 7, true),
            (RenderPhase::Bootstrap, true)
        );
        state.published();
        assert_eq!(
            local_phase(&mut state, 7, false),
            (RenderPhase::Bootstrap, false)
        );
        assert_eq!(local_phase(&mut state, 7, true), (RenderPhase::Final, true));
        assert_eq!(
            local_phase(&mut state, 7, false),
            (RenderPhase::Final, false)
        );
        // Raising/lowering the sample target keeps the same scene generation.
        assert_eq!(
            local_phase(&mut state, 7, true),
            (RenderPhase::Final, false)
        );
        assert_eq!(
            local_phase(&mut state, 8, true),
            (RenderPhase::Bootstrap, true)
        );
    }

    #[test]
    fn cancellation_retries_bootstrap_and_remote_ownership_never_claims_preview_samples() {
        let mut state = PreviewState::default();
        local_phase(&mut state, 1, true);
        // An interrupted pass never calls published(), including pause/resume.
        assert_eq!(
            local_phase(&mut state, 1, true),
            (RenderPhase::Bootstrap, false)
        );
        assert_eq!(
            state.select(1, false, true, LiveComputeTarget::Both, true),
            (RenderPhase::Final, true),
        );
        assert_eq!(
            state.select(2, false, false, LiveComputeTarget::RemoteOnly, true),
            (RenderPhase::Final, true),
        );
    }

    #[test]
    fn moving_preview_settles_once_and_respects_explicit_compute_modes() {
        let mut state = PreviewState::default();
        assert_eq!(
            state
                .select(3, true, false, LiveComputeTarget::LocalOnly, true)
                .0,
            RenderPhase::Moving,
        );
        state.published();
        assert_eq!(local_phase(&mut state, 3, true), (RenderPhase::Final, true));
        assert_eq!(
            RenderPhase::Bootstrap.compute(LocalComputeTarget::CpuGpu),
            LocalComputeTarget::Cpu
        );
        assert_eq!(
            RenderPhase::Bootstrap.compute(LocalComputeTarget::Gpu),
            LocalComputeTarget::Gpu
        );
        assert_eq!(
            RenderPhase::Final.compute(LocalComputeTarget::CpuGpu),
            LocalComputeTarget::CpuGpu
        );
        assert_eq!(
            phase_dimensions(1280, 720, LocalPreviewScale::Off, RenderPhase::Bootstrap),
            (320, 180)
        );
    }

    #[test]
    fn denoising_waits_for_guides_from_a_local_pass() {
        assert!(denoise_this_cycle(true, RenderPhase::Final, 1));
        assert!(
            !denoise_this_cycle(true, RenderPhase::Final, 0),
            "remote-only samples have no local guides"
        );
        assert!(!denoise_this_cycle(true, RenderPhase::Moving, 8));
        assert!(!denoise_this_cycle(true, RenderPhase::Bootstrap, 8));
        assert!(!denoise_this_cycle(false, RenderPhase::Final, 8));
    }

    /// A click (press and release without moving) must keep the converged Final image:
    /// no phase change, so no accumulation reset.
    #[test]
    fn a_plain_press_does_not_leave_final_or_reset_the_accumulation() {
        let mut state = PreviewState::default();
        let mut drag = DragGesture::default();
        local_phase(&mut state, 5, true);
        state.published();
        assert_eq!(local_phase(&mut state, 5, true), (RenderPhase::Final, true));
        for held in [false, true, true, false] {
            let moving = drag.moving(false, held, 5);
            assert!(!moving, "held={held}");
            let (phase, changed) =
                state.select(5, moving, false, LiveComputeTarget::LocalOnly, true);
            assert_eq!((phase, changed), (RenderPhase::Final, false), "held={held}");
        }
    }

    /// Once the pose changes under the held button the drag is real motion, and it
    /// stays so for the rest of the press even while the pointer rests.
    #[test]
    fn a_held_drag_counts_as_moving_once_the_scene_changed_since_the_press() {
        let mut drag = DragGesture::default();
        assert!(!drag.moving(false, true, 9));
        assert!(drag.moving(false, true, 10));
        assert!(drag.moving(false, true, 10), "latched until release");
        assert!(!drag.moving(false, false, 10));
        // The next press starts from its own generation.
        assert!(!drag.moving(false, true, 10));
        // The orchestrator's own motion flag always wins.
        assert!(drag.moving(true, false, 10));
    }

    #[test]
    fn off_never_reduces_regardless_of_movement() {
        assert_eq!(
            effective_dimensions(1920, 1080, LocalPreviewScale::Off, true),
            (1920, 1080)
        );
        assert_eq!(
            effective_dimensions(1920, 1080, LocalPreviewScale::Off, false),
            (1920, 1080)
        );
    }

    #[test]
    fn settled_always_returns_the_full_configured_resolution() {
        for scale in [
            LocalPreviewScale::Off,
            LocalPreviewScale::Half,
            LocalPreviewScale::Quarter,
        ] {
            assert_eq!(
                effective_dimensions(1280, 720, scale, false),
                (1280, 720),
                "scale={scale:?}"
            );
        }
    }

    #[test]
    fn half_scale_halves_only_while_moving() {
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Half, true),
            (640, 360)
        );
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Half, false),
            (1280, 720)
        );
    }

    #[test]
    fn quarter_scale_quarters_only_while_moving() {
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Quarter, true),
            (320, 180)
        );
        assert_eq!(
            effective_dimensions(1280, 720, LocalPreviewScale::Quarter, false),
            (1280, 720)
        );
    }

    /// A tiny configured resolution must never resolve to a zero-area preview frame.
    #[test]
    fn tiny_configured_dimensions_floor_at_one_by_one_while_moving() {
        assert_eq!(
            effective_dimensions(2, 2, LocalPreviewScale::Quarter, true),
            (1, 1)
        );
        assert_eq!(
            effective_dimensions(1, 1, LocalPreviewScale::Half, true),
            (1, 1)
        );
    }

    /// Non-power-of-two configured dimensions must floor-divide, not panic or round up.
    #[test]
    fn odd_configured_dimensions_floor_divide() {
        assert_eq!(
            effective_dimensions(801, 601, LocalPreviewScale::Half, true),
            (400, 300)
        );
    }
}
