//! Tests of the Ray-Traced view's beam job: the start rule, the cancellation, the routing of a
//! lighting-only change, the inert state with the preference off, and the shown picture against
//! the leaf cache's own output for a fixed cache.

use super::*;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, surface::SurfaceScene},
    },
};
use std::time::{Duration, Instant};

/// The frame is small: the shown-picture test builds several beam trees at the production
/// settings, and the equality with the leaf cache's own output holds at any size.
const SIZE: (u32, u32) = (16, 16);

/// The pixels of a frame of `SIZE`.
const PIXELS: usize = 16 * 16;

/// The longest a test waits for a thread.
const PATIENCE: Duration = Duration::from_secs(60);

/// The tap count of the finished Whitted pictures in these tests.
const TAPS: u32 = 4;

fn spinel() -> GemMaterial {
    GemMaterial::by_name("Spinel").expect("Spinel is a built-in material")
}

/// Runs `check` on a Ray-Traced frame of the standard brilliant in `material`, lit with
/// `exposure`.
fn with_frame<R>(
    material: &GemMaterial,
    exposure: f32,
    check: impl FnOnce(&BackendFrame<'_>) -> R,
) -> R {
    let planes = StandardGemCuts::standard_round_brilliant();
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
        material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(exposure, 0.4, 0.35),
        spp: 1,
        sample_offset: 0,
    };
    check(&frame)
}

fn want(converged: bool) -> RayBeamWanted {
    RayBeamWanted {
        applies: true,
        pose: 1,
        scene: 1,
        size: SIZE,
        converged,
    }
}

/// `want(converged)` after a lighting-only change to scene generation `scene`.
fn relit(scene: u64, converged: bool) -> RayBeamWanted {
    RayBeamWanted {
        scene,
        ..want(converged)
    }
}

/// Waits until `done` holds.
fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "timed out");
        thread::sleep(Duration::from_millis(1));
    }
}

/// Syncs until the running job (if any) has been adopted, and returns the step of the last sync.
fn settle(beams: &mut RayBeams, wanted: RayBeamWanted) -> RayBeamStep {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let step = beams.sync(wanted);
        if !beams.is_running() {
            return step;
        }
        assert!(Instant::now() < deadline, "the job did not end");
        thread::sleep(Duration::from_millis(1));
    }
}

/// Starts a job that waits for its token and reports a cancellation.
fn spawn_waiting(beams: &mut RayBeams) {
    beams.spawn_job(|own| {
        while !own.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        Outcome::Cancelled
    });
}

/// Runs a job that finishes at once, for the scene `wanted` describes.
fn finish_a_job(beams: &mut RayBeams, wanted: RayBeamWanted) {
    assert_ne!(beams.sync(wanted), RayBeamStep::Idle);
    beams.spawn_job(|_| Outcome::Done);
    assert_eq!(settle(beams, wanted), RayBeamStep::Idle);
    assert!(beams.shows(), "the finished job is shown");
}

#[test]
fn beams_apply_to_a_still_ray_traced_local_view_only() {
    assert!(applies(true, true, false, RenderPhase::Final));
    assert!(!applies(false, true, false, RenderPhase::Final), "off");
    assert!(
        !applies(true, false, false, RenderPhase::Final),
        "Path-Traced"
    );
    assert!(!applies(true, true, true, RenderPhase::Final), "remote");
    assert!(!applies(true, true, false, RenderPhase::Moving));
    assert!(!applies(true, true, false, RenderPhase::Bootstrap));
}

#[test]
fn a_job_is_due_once_the_ray_traced_picture_is_finished() {
    let mut beams = RayBeams::default();
    assert_eq!(beams.sync(want(false)), RayBeamStep::Idle);
    assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
    assert_eq!(
        beams.sync(want(true)),
        RayBeamStep::Build,
        "due until the loop launches it"
    );
    let not_applicable = RayBeamWanted {
        applies: false,
        ..want(true)
    };
    assert_eq!(beams.sync(not_applicable), RayBeamStep::Idle);
}

#[test]
fn the_step_is_a_relight_only_when_the_leaves_are_kept() {
    assert_eq!(due_step(false), RayBeamStep::Build);
    assert_eq!(due_step(true), RayBeamStep::Relight);
}

#[test]
fn a_running_job_is_left_alone_while_the_scene_stands() {
    let mut beams = RayBeams::default();
    assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
    spawn_waiting(&mut beams);
    let token = beams.job_token().expect("running");
    for _ in 0..3 {
        assert_eq!(beams.sync(want(true)), RayBeamStep::Idle);
        assert!(beams.is_running());
    }
    assert!(!token.load(Ordering::Acquire));
}

#[test]
fn a_pose_change_cancels_the_job_and_no_old_pixel_reaches_the_new_pose() {
    let mut beams = RayBeams::default();
    assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
    let overlay = Arc::clone(&beams.overlay);
    let generation = lock(&overlay).generation;
    beams.spawn_job(move |own| {
        while !own.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        // A frame that lands after the cancellation.
        lock(&overlay).write_frame(generation, &[Vec3::ONE; PIXELS]);
        Outcome::Done
    });
    let token = beams.job_token().expect("running");

    let step = beams.sync(RayBeamWanted {
        pose: 2,
        ..want(false)
    });
    assert!(token.load(Ordering::Acquire), "the token is raised at once");
    assert!(!beams.is_running());
    assert_eq!(
        step,
        RayBeamStep::Idle,
        "the new pose waits for its picture"
    );
    thread::sleep(Duration::from_millis(50));
    assert!(
        !beams.compose_into(&vec![Vec3::ZERO; PIXELS], 1),
        "nothing of the old pose is shown"
    );
}

#[test]
fn a_new_size_or_a_view_beams_no_longer_apply_to_cancels_the_job_too() {
    let changes = [
        RayBeamWanted {
            size: (SIZE.0 + 1, SIZE.1),
            ..want(true)
        },
        RayBeamWanted {
            applies: false,
            ..want(true)
        },
    ];
    for change in changes {
        let mut beams = RayBeams::default();
        assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
        spawn_waiting(&mut beams);
        let token = beams.job_token().expect("running");
        beams.sync(change);
        assert!(token.load(Ordering::Acquire), "{change:?}");
        assert!(!beams.is_running());
        assert!(!beams.shows());
    }
}

#[test]
fn a_lighting_only_change_cancels_the_job_and_builds_again_while_no_leaves_are_kept() {
    let mut beams = RayBeams::default();
    assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
    spawn_waiting(&mut beams);
    let token = beams.job_token().expect("running");

    // The Ray-Traced picture restarts under the new lighting: not finished, so no job yet.
    assert_eq!(beams.sync(relit(2, false)), RayBeamStep::Idle);
    assert!(token.load(Ordering::Acquire), "the stale job is cancelled");
    assert!(!beams.is_running());
    assert_eq!(beams.sync(relit(2, true)), RayBeamStep::Build);
}

#[test]
fn a_lighting_only_change_relights_the_kept_leaves_once_the_whitted_picture_is_in() {
    let mut beams = RayBeams::default();
    finish_a_job(&mut beams, want(true));
    beams.mark_cache_built();

    assert_eq!(beams.sync(relit(2, false)), RayBeamStep::Idle);
    assert!(!beams.shows(), "the stale picture is dropped");
    assert_eq!(beams.sync(relit(2, true)), RayBeamStep::Relight);
    assert_eq!(
        beams.sync(relit(2, true)),
        RayBeamStep::Relight,
        "due until the loop launches it"
    );
    // The re-light lands; the next lighting change relights again.
    beams.spawn_job(|_| Outcome::Done);
    assert_eq!(settle(&mut beams, relit(2, true)), RayBeamStep::Idle);
    assert!(beams.shows());
    assert_eq!(beams.sync(relit(3, false)), RayBeamStep::Idle);
    assert_eq!(beams.sync(relit(3, true)), RayBeamStep::Relight);
}

#[test]
fn a_pose_change_forgets_the_kept_leaves() {
    let mut beams = RayBeams::default();
    finish_a_job(&mut beams, want(true));
    beams.mark_cache_built();
    let moved = RayBeamWanted {
        pose: 2,
        ..want(true)
    };
    assert_eq!(beams.sync(moved), RayBeamStep::Build);
}

#[test]
fn a_pose_change_hands_the_next_pose_a_fresh_cache_handle() {
    let mut beams = RayBeams::default();
    finish_a_job(&mut beams, want(true));
    let old = beams.cache_handle();
    let moved = RayBeamWanted {
        pose: 2,
        ..want(true)
    };
    beams.sync(moved);
    assert!(!Arc::ptr_eq(&old, &beams.cache_handle()));
}

#[test]
fn the_whitted_image_is_the_mean_of_the_taps() {
    let accum = [Vec3::splat(8.0), Vec3::new(4.0, 0.0, 2.0)];
    assert_eq!(
        whitted_image(&accum, 4),
        vec![Vec3::splat(2.0), Vec3::new(1.0, 0.0, 0.5)]
    );
    assert_eq!(whitted_image(&accum, 1), accum.to_vec());
    assert_eq!(
        whitted_image(&accum, 0),
        accum.to_vec(),
        "no divide by zero"
    );
}

#[test]
fn a_declined_cache_leaves_the_ray_traced_picture_untouched() {
    let mut beams = RayBeams::default();
    assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
    beams.spawn_job(|_| Outcome::Declined);
    assert_eq!(settle(&mut beams, want(true)), RayBeamStep::Idle);
    assert!(beams.is_refused());
    assert!(!beams.shows());

    let accum = vec![Vec3::splat(2.0); PIXELS];
    assert!(!beams.compose_into(&accum, 2), "nothing to compose");
    assert_eq!(beams.composite().len(), 0);
    assert_eq!(beams.sync(want(true)), RayBeamStep::Idle, "no second try");
    // The lighting can change what the stone allows, so a lighting change forgives it.
    assert_eq!(beams.sync(relit(2, false)), RayBeamStep::Idle);
    assert_eq!(beams.sync(relit(2, true)), RayBeamStep::Build);
}

#[test]
fn with_the_preference_off_the_worker_state_stays_inert() {
    let mut beams = RayBeams::default();
    let off = RayBeamWanted {
        applies: false,
        ..want(true)
    };
    for _ in 0..3 {
        assert_eq!(beams.sync(off), RayBeamStep::Idle);
        assert!(beams.is_inactive());
        assert!(!beams.shows());
        assert!(!beams.take_news(), "no republication");
    }
    assert!(!beams.compose_into(&[Vec3::ONE; 4], 1));
    assert_eq!(beams.composite().len(), 0);
}

#[test]
fn a_finished_frame_makes_news_for_the_display_once() {
    let mut beams = RayBeams::default();
    assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
    assert!(beams.take_news(), "the new pose cleared the overlay");
    assert!(!beams.take_news());
    let overlay = Arc::clone(&beams.overlay);
    let generation = lock(&overlay).generation;
    lock(&overlay).write_frame(generation, &[Vec3::ONE; PIXELS]);
    assert!(beams.take_news());
    assert!(!beams.take_news());
}

#[test]
fn an_opal_is_refused_and_a_standard_brilliant_is_launched() {
    let mut opal = spinel();
    opal.name = "Precious Opal (White)".to_owned();
    with_frame(&opal, 1.0, |frame| {
        let mut beams = RayBeams::default();
        assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
        let cancel = Arc::new(AtomicBool::new(false));
        let accum = vec![Vec3::ZERO; PIXELS];
        beams.launch(
            RayBeamStep::Build,
            frame,
            &RayLaunch {
                hdr: None,
                frame_cancel: &cancel,
                accum: &accum,
                taps: TAPS,
            },
        );
        assert!(beams.is_refused());
        assert!(!beams.shows());
        assert!(!beams.compose_into(&accum, 1));
        assert_eq!(beams.sync(want(true)), RayBeamStep::Idle);
    });
}

/// The leaf cache's own output for `frame` over `background`, from a cache it builds itself.
fn direct_relight(
    frame: &BackendFrame<'_>,
    cache: &mut LeafCache,
    background: &[Vec3],
) -> Vec<Vec3> {
    let inputs = BeamJobInputs::capture(frame, None, TAPS, 2).expect("eligible");
    let never = AtomicBool::new(false);
    inputs
        .with_scene(|scene| {
            cache.relight_ray_traced(scene, &viewport_beam_settings(), background, &never)
        })
        .expect("a ray-traced re-light")
}

/// A Whitted-like background that differs from pixel to pixel.
fn background() -> Vec<Vec3> {
    (0..SIZE.0 * SIZE.1)
        .map(|index| {
            Vec3::splat(0.001_f32.mul_add(f32::from(u16::try_from(index).unwrap_or(0)), 0.25))
        })
        .collect()
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 04b"]
fn the_shown_picture_equals_the_leaf_caches_output_for_a_fixed_cache() {
    with_frame(&spinel(), 1.0, |frame| {
        let background = background();
        let mut fixed = LeafCache::new(VIEWPORT_LEAF_CACHE_BYTES);
        let direct = direct_relight(frame, &mut fixed, &background);
        assert_eq!(direct.len(), background.len());
        assert!(direct.iter().all(|pixel| pixel.is_finite()));

        let mut beams = RayBeams::default();
        assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
        let cancel = Arc::new(AtomicBool::new(false));
        // The running sum of TAPS identical taps: the job divides it back to the background.
        let accum: Vec<Vec3> = background
            .iter()
            .map(|pixel| *pixel * f32::from(u16::try_from(TAPS).unwrap_or(1)))
            .collect();
        let launch = RayLaunch {
            hdr: None,
            frame_cancel: &cancel,
            accum: &accum,
            taps: TAPS,
        };
        beams.launch(RayBeamStep::Build, frame, &launch);
        wait_for(|| {
            beams.sync(want(true));
            !beams.is_running()
        });
        assert!(beams.shows(), "the job ended with the whole frame");
        assert!(beams.compose_into(&vec![Vec3::ZERO; PIXELS], 1));
        assert_eq!(beams.composite(), direct.as_slice());

        // The leaves are kept: the next lighting change relights them under the new lighting.
        assert_eq!(beams.sync(relit(2, false)), RayBeamStep::Idle);
        assert_eq!(beams.sync(relit(2, true)), RayBeamStep::Relight);
    });
    // A lighting-only change of the same pose: the shown picture is the cache's re-light.
    with_frame(&spinel(), 1.0, |first| {
        with_frame(&spinel(), 2.0, |second| {
            let background = background();
            let mut fixed = LeafCache::new(VIEWPORT_LEAF_CACHE_BYTES);
            let _ = direct_relight(first, &mut fixed, &background);
            let direct = direct_relight(second, &mut fixed, &background);

            let mut beams = RayBeams::default();
            let cancel = Arc::new(AtomicBool::new(false));
            let accum: Vec<Vec3> = background
                .iter()
                .map(|pixel| *pixel * f32::from(u16::try_from(TAPS).unwrap_or(1)))
                .collect();
            let launch = RayLaunch {
                hdr: None,
                frame_cancel: &cancel,
                accum: &accum,
                taps: TAPS,
            };
            assert_eq!(beams.sync(want(true)), RayBeamStep::Build);
            beams.launch(RayBeamStep::Build, first, &launch);
            wait_for(|| {
                beams.sync(want(true));
                !beams.is_running()
            });
            assert_eq!(beams.sync(relit(2, true)), RayBeamStep::Relight);
            beams.launch(RayBeamStep::Relight, second, &launch);
            wait_for(|| {
                beams.sync(relit(2, true));
                !beams.is_running()
            });
            assert!(beams.compose_into(&vec![Vec3::ZERO; PIXELS], 1));
            assert_eq!(beams.composite(), direct.as_slice());
        });
    });
}
