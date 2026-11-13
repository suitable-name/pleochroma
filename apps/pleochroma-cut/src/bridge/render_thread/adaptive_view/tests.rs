//! Tests of the adaptive viewport view: the rules that decide when it applies, the run's life
//! (start, reset, restart), and a real CPU run over a small stone.

use super::*;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, surface::SurfaceScene},
    },
};

/// A frame of 3 x 3 tiles: the stone fills the middle, the corners are background.
const SIZE: (u32, u32) = (48, 48);

/// Runs `check` on a frame of the standard brilliant in a daylight studio.
fn with_frame<R>(check: impl FnOnce(&BackendFrame<'_>) -> R) -> R {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in material");
    let camera = Camera::new(0.3, 0.4, 5.0, 18.0);
    let frame = BackendFrame {
        width: SIZE.0,
        height: SIZE.1,
        yaw: 0.3,
        pitch: 0.4,
        distance: 5.0,
        camera: &camera,
        planes: &planes,
        tools: &[],
        fluorescence: Fluorescence::none(),
        facet_finishes: &[],
        surface: SurfaceScene::IDEAL,
        material: &material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        spp: 1,
        sample_offset: 0,
    };
    check(&frame)
}

fn want(reset: bool, first_pass_done: bool, cap: u32) -> Wanted {
    Wanted {
        applies: true,
        reset,
        first_pass_done,
        size: SIZE,
        cap,
    }
}

fn samples_done(view: &AdaptiveView) -> u32 {
    view.engaged
        .as_ref()
        .expect("a run is engaged")
        .run
        .samples_done()
}

#[test]
fn the_viewport_settings_are_valid_and_use_the_standard_preset() {
    for cap in 1..=130 {
        let settings = viewport_settings(cap, standard_target_noise());
        assert_eq!(settings.validate(), Ok(()), "cap {cap}");
        assert_eq!(settings.max_spp, cap);
    }
    for cap in [MIN_ADAPTIVE_CAP, 100, 128, 512, 4096] {
        let settings = viewport_settings(cap, standard_target_noise());
        assert_eq!(settings.min_spp, VIEWPORT_MIN_SPP, "cap {cap}");
        assert_eq!(settings.pass_spp, 16);
        assert_eq!(settings.tile, 16);
    }
    assert!(
        (standard_target_noise() - TARGET_NOISE_PRESETS[DEFAULT_TARGET_NOISE_PRESET].1).abs()
            < f32::EPSILON
    );
    assert_eq!(
        TARGET_NOISE_PRESETS[DEFAULT_TARGET_NOISE_PRESET].0,
        "Standard"
    );
}

#[test]
fn the_adaptive_path_applies_only_to_a_still_local_path_traced_view_with_a_big_cap() {
    let still = RenderPhase::Final;
    assert!(applies(true, false, false, still, MIN_ADAPTIVE_CAP));
    assert!(applies(true, false, false, still, 4096));
    assert!(!applies(false, false, false, still, 128), "preference off");
    assert!(!applies(true, true, false, still, 128), "Ray-Traced");
    assert!(!applies(true, false, true, still, 128), "a remote lane");
    assert!(!applies(true, false, false, RenderPhase::Moving, 128));
    assert!(!applies(true, false, false, RenderPhase::Bootstrap, 128));
    assert!(!applies(true, false, false, still, MIN_ADAPTIVE_CAP - 1));
    assert!(!applies(true, false, false, still, 0));
}

#[test]
fn a_view_the_adaptive_path_does_not_apply_to_never_engages() {
    let mut view = AdaptiveView::default();
    for reset in [true, false, false] {
        let off = Wanted {
            applies: false,
            ..want(reset, true, 128)
        };
        assert!(!view.sync(off));
        assert!(!view.is_engaged());
        assert!(!view.is_finished());
        assert_eq!(view.samples_shown(), 0);
        assert!(view.means().is_none());
    }
}

#[test]
fn a_run_starts_after_the_first_ordinary_pass_and_a_change_starts_it_over() {
    with_frame(|frame| {
        let cancel = AtomicBool::new(false);
        let mut view = AdaptiveView::default();

        // The frame of a reset has no ordinary pass yet: the ordinary path runs it.
        assert!(!view.sync(want(true, false, 128)));
        assert!(!view.is_engaged());
        // The next still frame starts the run, and keeps it.
        assert!(view.sync(want(false, true, 128)));
        assert!(view.sync(want(false, true, 128)));
        assert_eq!(samples_done(&view), 0);
        assert!(
            view.means().is_none(),
            "no pass yet: the ordinary image shows"
        );
        assert_eq!(view.samples_shown(), 1);

        assert_eq!(
            view.trace_pass_on(None, frame, &cancel),
            PassOutcome::Traced
        );
        assert_eq!(samples_done(&view), 16);
        assert_eq!(view.samples_shown(), 16);
        assert!(view.means().is_some());
        assert!(
            view.sync(want(false, true, 128)),
            "kept while nothing changes"
        );
        assert_eq!(samples_done(&view), 16);

        // A pose, design, lighting or size change is a reset: the run is dropped, and the
        // new one starts from zero after the ordinary first pass.
        assert!(!view.sync(want(true, false, 128)));
        assert!(!view.is_engaged());
        assert!(view.sync(want(false, true, 128)));
        assert_eq!(samples_done(&view), 0);
        assert!(view.means().is_none());

        // A new sample cap restarts the run too.
        assert_eq!(
            view.trace_pass_on(None, frame, &cancel),
            PassOutcome::Traced
        );
        assert!(view.sync(want(false, true, 256)));
        assert_eq!(samples_done(&view), 0);

        // The preference going off, or the engine leaving the path tracer, drops it.
        let off = Wanted {
            applies: false,
            ..want(false, true, 256)
        };
        assert!(!view.sync(off));
        assert!(!view.is_engaged());
    });
}

#[test]
fn a_run_refused_for_its_settings_leaves_the_ordinary_accumulation_alone() {
    let mut view = AdaptiveView::with_target_noise(2.0);
    assert!(
        !view.sync(want(false, true, 128)),
        "a relative target of 2 is refused"
    );
    assert!(!view.is_engaged());
    assert!(!view.sync(want(false, true, 128)), "and stays refused");
    // A reset clears the refusal (the next accumulation tries again).
    assert!(!view.sync(want(true, false, 128)));
    assert!(!view.refused);
}

#[test]
fn a_cancelled_pass_leaves_the_run_and_its_buffers_untouched() {
    with_frame(|frame| {
        let mut view = AdaptiveView::default();
        assert!(view.sync(want(false, true, 128)));
        let cancel = AtomicBool::new(true);
        assert_eq!(
            view.trace_pass_on(None, frame, &cancel),
            PassOutcome::Cancelled
        );
        assert_eq!(samples_done(&view), 0);
        let engaged = view.engaged.as_ref().expect("a run is engaged");
        assert!(engaged.sum.iter().all(|value| *value == Vec3::ZERO));
        assert!(engaged.half_b.iter().all(|value| *value == Vec3::ZERO));
        assert!(view.means().is_none());

        // The same pass is asked for again and traces once the cancellation is gone.
        cancel.store(false, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            view.trace_pass_on(None, frame, &cancel),
            PassOutcome::Traced
        );
        assert_eq!(samples_done(&view), 16);
    });
}

#[test]
fn a_cpu_viewport_run_stops_background_tiles_before_the_cap_and_shows_each_pixels_own_mean() {
    const CAP: u32 = 64;
    with_frame(|frame| {
        let cancel = AtomicBool::new(false);
        // A loose target: the empty corners of the frame are clean after the minimum. A tile
        // stops only when its eight neighbours are within twice the target, and every corner of
        // this 3 x 3 frame touches the stone's tile, whose worst pixel reads near 1 on the
        // relative scale; 0.9 puts that limit at 1.8.
        let mut view = AdaptiveView::with_target_noise(0.9);
        assert!(view.sync(want(false, true, CAP)));
        for _ in 0..=CAP {
            if view.is_finished() {
                break;
            }
            assert_eq!(
                view.trace_pass_on(None, frame, &cancel),
                PassOutcome::Traced
            );
        }
        assert!(view.is_finished());
        assert_eq!(view.samples_shown(), CAP, "finished reads as the full cap");
        assert_eq!(
            view.trace_pass_on(None, frame, &cancel),
            PassOutcome::Finished
        );

        let engaged = view.engaged.as_ref().expect("a run is engaged");
        let counts = engaged.run.pixel_counts();
        assert_eq!(counts.len(), (SIZE.0 * SIZE.1) as usize);
        assert!(
            counts
                .iter()
                .all(|&count| (VIEWPORT_MIN_SPP..=CAP).contains(&count)),
            "no tile stops before the minimum or runs past the cap"
        );
        assert!(
            counts.iter().any(|&count| count < CAP),
            "some tile stopped before the cap"
        );

        // The picture shows every pixel's sum over that pixel's own count.
        let means = view.means().expect("the run traced passes");
        assert_eq!(means.len(), counts.len());
        for ((mean, sum), &count) in means.iter().zip(&engaged.sum).zip(&counts) {
            let count = f32::from(u16::try_from(count).expect("a small count"));
            assert_eq!(*mean, *sum / count);
        }
        // The sample counts add up to what was traced.
        let traced: u64 = counts.iter().map(|&count| u64::from(count)).sum();
        assert_eq!(traced, engaged.run.samples_traced());
    });
}
