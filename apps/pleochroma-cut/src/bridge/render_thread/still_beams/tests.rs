//! Tests of the viewport's beam job: the start rule, the cancellation, the routing of a
//! lighting-only change, the overlay and the inert state with the preference off.
//!
//! Most jobs under test are small closures; one test compares the job's image with a direct render
//! through the real beam entry.

use super::*;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::{LightingPreset, surface::SurfaceScene},
};
use std::{sync::atomic::AtomicUsize, time::Instant};

/// The frame is small: the checks (routing, equality with the direct render) hold at any size.
const SIZE: (u32, u32) = (16, 16);

/// The pixels of a frame of `SIZE`.
const PIXELS: usize = 16 * 16;

/// The longest a test waits for a thread.
const PATIENCE: Duration = Duration::from_secs(10);

fn spinel() -> GemMaterial {
    GemMaterial::by_name("Spinel").expect("Spinel is a built-in material")
}

/// Runs `check` on a frame of the standard brilliant in `material`.
fn with_frame<R>(material: &GemMaterial, check: impl FnOnce(&BackendFrame<'_>) -> R) -> R {
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
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        spp: 1,
        sample_offset: 0,
    };
    check(&frame)
}

fn want(samples: u32) -> BeamWanted {
    BeamWanted {
        applies: true,
        pose: 1,
        scene: 1,
        size: SIZE,
        samples,
        instant_lighting: false,
    }
}

/// `want(samples)` after a lighting-only change to scene generation `scene`.
fn relit(scene: u64, samples: u32, instant_lighting: bool) -> BeamWanted {
    BeamWanted {
        scene,
        instant_lighting,
        ..want(samples)
    }
}

/// The longest a real beam render test waits: a render takes over a minute at the test profile.
const RENDER_PATIENCE: Duration = Duration::from_mins(15);

/// Waits until `done` holds.
fn wait_for(done: impl FnMut() -> bool) {
    wait_for_within(PATIENCE, done);
}

/// Waits until `done` holds, for at most `patience`.
fn wait_for_within(patience: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + patience;
    while !done() {
        assert!(
            Instant::now() < deadline,
            "timed out after {patience:?} waiting for the condition"
        );
        thread::sleep(Duration::from_millis(1));
    }
}

/// Syncs until the running job (if any) has been adopted, and returns the step of the last sync.
fn settle(beams: &mut StillBeams, wanted: BeamWanted) -> BeamStep {
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
fn spawn_waiting(beams: &mut StillBeams, kind: JobKind) {
    beams.spawn_job(kind, |own| {
        while !own.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        Outcome::Cancelled
    });
}

/// Runs a render job that finishes at once, for the scene `wanted` describes.
fn finish_a_render(beams: &mut StillBeams, wanted: BeamWanted) {
    assert_eq!(beams.sync(wanted), BeamStep::Start);
    beams.spawn_job(JobKind::Render, |_| Outcome::Done);
    assert_eq!(settle(beams, wanted), BeamStep::Idle);
    assert!(beams.owns_stone(), "the finished render is shown");
}

#[test]
fn beams_apply_to_a_still_path_traced_local_view_only() {
    assert!(applies(true, false, false, RenderPhase::Final));
    assert!(!applies(false, false, false, RenderPhase::Final), "off");
    assert!(
        !applies(true, true, false, RenderPhase::Final),
        "Ray-Traced"
    );
    assert!(!applies(true, false, true, RenderPhase::Final), "remote");
    assert!(!applies(true, false, false, RenderPhase::Moving));
    assert!(!applies(true, false, false, RenderPhase::Bootstrap));
}

#[test]
fn a_job_is_due_once_the_pose_has_refined_for_thirty_two_samples() {
    assert_eq!(VIEWPORT_BEAMS_AFTER_SPP, 32);
    let mut beams = StillBeams::default();
    for samples in [0, 1, 16, 31] {
        assert_eq!(beams.sync(want(samples)), BeamStep::Idle, "{samples}");
    }
    assert_eq!(beams.sync(want(32)), BeamStep::Start);
    assert_eq!(
        beams.sync(want(33)),
        BeamStep::Start,
        "due until the loop launches it"
    );
    let not_applicable = BeamWanted {
        applies: false,
        ..want(500)
    };
    assert_eq!(beams.sync(not_applicable), BeamStep::Idle);
}

#[test]
fn a_running_job_is_left_alone_while_the_pose_stands() {
    let mut beams = StillBeams::default();
    assert_eq!(beams.sync(want(32)), BeamStep::Start);
    spawn_waiting(&mut beams, JobKind::Render);
    let token = beams.job_token().expect("running");
    for samples in [33, 64, 128] {
        assert_eq!(beams.sync(want(samples)), BeamStep::Idle);
        assert!(beams.is_running());
        assert!(beams.owns_stone());
    }
    assert!(!token.load(Ordering::Acquire));
}

#[test]
fn a_pose_change_cancels_the_job_within_one_batch() {
    let mut beams = StillBeams::default();
    assert_eq!(beams.sync(want(32)), BeamStep::Start);
    let overlay = Arc::clone(&beams.overlay);
    let generation = lock(&overlay).generation;
    let batches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&batches);
    beams.spawn_job(JobKind::Render, move |own| {
        let tile = TileRect {
            x0: 0,
            y0: 0,
            x1: 2,
            y1: 1,
        };
        while !own.load(Ordering::Acquire) {
            lock(&overlay).write_batch(generation, &[tile], &[Vec3::ONE; 2]);
            counter.fetch_add(1, Ordering::AcqRel);
            thread::sleep(Duration::from_millis(1));
        }
        Outcome::Cancelled
    });
    wait_for(|| batches.load(Ordering::Acquire) >= 2);
    assert!(beams.compose_into(&vec![Vec3::ZERO; PIXELS], 1));
    let token = beams.job_token().expect("running");

    let step = beams.sync(BeamWanted {
        pose: 2,
        ..want(32)
    });
    assert!(token.load(Ordering::Acquire), "the token is raised at once");
    assert!(!beams.is_running());
    assert_eq!(step, BeamStep::Start, "the new pose starts afresh");
    let at_cancel = batches.load(Ordering::Acquire);
    thread::sleep(Duration::from_millis(50));
    assert!(
        batches.load(Ordering::Acquire) <= at_cancel + 1,
        "at most the batch in flight finishes"
    );
    assert!(
        !beams.compose_into(&vec![Vec3::ZERO; PIXELS], 1),
        "no batch of the old pose reaches the new one"
    );
}

#[test]
fn a_new_size_or_a_view_beams_no_longer_apply_to_cancels_the_job_too() {
    let changes = [
        BeamWanted {
            size: (SIZE.0 + 1, SIZE.1),
            ..want(32)
        },
        BeamWanted {
            applies: false,
            ..want(32)
        },
    ];
    for change in changes {
        let mut beams = StillBeams::default();
        assert_eq!(beams.sync(want(32)), BeamStep::Start);
        spawn_waiting(&mut beams, JobKind::Render);
        let token = beams.job_token().expect("running");
        beams.sync(change);
        assert!(token.load(Ordering::Acquire), "{change:?}");
        assert!(!beams.is_running());
        assert!(!beams.owns_stone());
    }
}

#[test]
fn a_lighting_only_change_restarts_the_job_when_instant_lighting_is_off() {
    let mut beams = StillBeams::default();
    assert_eq!(beams.sync(want(32)), BeamStep::Start);
    spawn_waiting(&mut beams, JobKind::Render);
    let token = beams.job_token().expect("running");

    // The accumulation restarts under the new lighting: no samples yet, so no job yet.
    assert_eq!(beams.sync(relit(2, 0, false)), BeamStep::Idle);
    assert!(token.load(Ordering::Acquire), "the stale job is cancelled");
    assert!(!beams.is_running());
    assert_eq!(beams.sync(relit(2, 31, false)), BeamStep::Idle);
    assert_eq!(beams.sync(relit(2, 32, false)), BeamStep::Start);
}

#[test]
fn a_lighting_only_change_during_a_render_restarts_it_even_with_instant_lighting_on() {
    // The leaves are retained only once a render has finished.
    let mut beams = StillBeams::default();
    assert_eq!(beams.sync(want(32)), BeamStep::Start);
    spawn_waiting(&mut beams, JobKind::Render);
    assert_eq!(beams.sync(relit(2, 0, true)), BeamStep::Idle);
    assert_eq!(beams.sync(relit(2, 32, true)), BeamStep::Start);
}

#[test]
fn a_lighting_only_change_relights_when_instant_lighting_is_on_and_the_leaves_are_kept() {
    let mut beams = StillBeams::default();
    finish_a_render(&mut beams, want(32));
    beams.mark_cache_ready();

    let step = beams.sync(relit(2, 0, true));
    assert_eq!(step, BeamStep::Relight);
    assert!(!beams.owns_stone(), "the stale picture is dropped");
    assert_eq!(
        beams.sync(relit(2, 0, true)),
        BeamStep::Relight,
        "due until the loop launches it"
    );
    // The re-light lands; the next lighting change relights again.
    beams.spawn_job(JobKind::Relight, |_| Outcome::Done);
    assert_eq!(settle(&mut beams, relit(2, 0, true)), BeamStep::Idle);
    assert!(beams.owns_stone());
    assert_eq!(beams.sync(relit(3, 0, true)), BeamStep::Relight);
}

#[test]
fn a_cancelled_relight_is_retried_as_a_relight_on_the_kept_leaves() {
    let mut beams = StillBeams::default();
    finish_a_render(&mut beams, want(32));
    beams.mark_cache_ready();
    let kept = beams.cache_handle();

    spawn_waiting(&mut beams, JobKind::Relight);
    if let Stage::Running(job) = &beams.stage {
        job.cancel.store(true, Ordering::Release);
    }
    // Not a render on the shared handle: the cancelled re-light is due again, and the leaves stay.
    assert_eq!(settle(&mut beams, want(32)), BeamStep::Relight);
    assert!(
        Arc::ptr_eq(&kept, &beams.cache_handle()),
        "a re-light keeps the cache"
    );
}

#[test]
fn the_restart_forgets_the_kept_leaves() {
    let mut beams = StillBeams::default();
    finish_a_render(&mut beams, want(32));
    beams.mark_cache_ready();
    // Instant lighting off: the change restarts the job and drops the leaves ...
    assert_eq!(beams.sync(relit(2, 0, false)), BeamStep::Idle);
    assert_eq!(beams.sync(relit(2, 32, false)), BeamStep::Start);
    beams.spawn_job(JobKind::Render, |_| Outcome::Done);
    assert_eq!(settle(&mut beams, relit(2, 32, false)), BeamStep::Idle);
    // ... and the render's own stub cache is empty, so the next change cannot re-light.
    assert_eq!(beams.sync(relit(3, 0, true)), BeamStep::Idle);
    assert!(!beams.owns_stone());
}

#[test]
fn the_lighting_route_is_a_relight_only_with_the_preference_and_the_leaves() {
    assert_eq!(lighting_route(true, true), LightingRoute::Relight);
    assert_eq!(lighting_route(true, false), LightingRoute::Restart);
    assert_eq!(lighting_route(false, true), LightingRoute::Restart);
    assert_eq!(lighting_route(false, false), LightingRoute::Restart);
}

#[test]
fn a_declined_entry_leaves_the_progressive_image_untouched() {
    let mut beams = StillBeams::default();
    assert_eq!(beams.sync(want(32)), BeamStep::Start);
    // What the entry answers for a stone it does not beam trace.
    beams.spawn_job(JobKind::Render, |_| Outcome::Declined(BeamOff::Unavailable));
    assert_eq!(settle(&mut beams, want(32)), BeamStep::Idle);
    assert!(beams.is_refused());
    assert!(!beams.owns_stone());

    let accum = vec![Vec3::splat(2.0); PIXELS];
    assert!(!beams.compose_into(&accum, 2), "nothing to compose");
    assert_eq!(beams.composite().len(), 0);
    assert!(accum.iter().all(|pixel| *pixel == Vec3::splat(2.0)));
    assert_eq!(beams.sync(want(500)), BeamStep::Idle, "no second attempt");
    // The lighting can change what the stone allows, so a lighting change forgives it.
    assert_eq!(beams.sync(relit(2, 32, false)), BeamStep::Start);
}

#[test]
fn with_the_preference_off_the_worker_state_stays_inert() {
    let mut beams = StillBeams::default();
    let off = BeamWanted {
        applies: false,
        ..want(500)
    };
    for _ in 0..3 {
        assert_eq!(beams.sync(off), BeamStep::Idle);
        assert!(beams.is_inactive());
        assert!(!beams.owns_stone());
        assert!(!beams.take_news(), "no republication");
    }
    assert!(!beams.compose_into(&[Vec3::ONE; 4], 1));
    assert_eq!(beams.composite().len(), 0);
}

#[test]
fn a_batch_replaces_its_pixels_and_the_rest_stay_progressive() {
    let mut overlay = Overlay::default();
    overlay.reset((4, 2));
    let generation = overlay.generation;
    let tile = TileRect {
        x0: 1,
        y0: 0,
        x1: 3,
        y1: 2,
    };
    let means = [
        Vec3::splat(10.0),
        Vec3::splat(20.0),
        Vec3::splat(30.0),
        Vec3::splat(40.0),
    ];
    overlay.write_batch(generation, &[tile], &means);
    assert_eq!(overlay.filled, 4);

    let accum = vec![Vec3::splat(8.0); 8];
    let mut out = Vec::new();
    assert!(overlay.compose(&accum, 4, &mut out));
    // Tile pixels (1, 0), (2, 0), (1, 1), (2, 1) in row-major order within the tile.
    let progressive = Vec3::splat(2.0);
    assert_eq!(
        out,
        vec![
            progressive,
            means[0],
            means[1],
            progressive,
            progressive,
            means[2],
            means[3],
            progressive,
        ]
    );
}

#[test]
fn an_old_poses_cache_handle_cannot_touch_the_new_poses_cache() {
    let mut beams = StillBeams::default();
    let old = beams.cache_handle();
    // A pose change drops the pose and hands out a fresh handle.
    beams.invalidate_cache();
    let new = beams.cache_handle();
    assert!(!Arc::ptr_eq(&old, &new));
    // A delayed reset of the old job, even one holding its lock, leaves the new cache alone.
    let held = lock(&old);
    assert_eq!(lock(&new).cap_bytes(), VIEWPORT_LEAF_CACHE_BYTES);
    drop(held);
    *lock(&old) = LeafCache::new(1);
    assert_eq!(lock(&new).cap_bytes(), VIEWPORT_LEAF_CACHE_BYTES);
}

#[test]
fn a_panicking_job_body_ends_the_watcher_and_propagates() {
    let (sender, receiver) = std::sync::mpsc::channel();
    thread::spawn(move || {
        let frame = AtomicBool::new(false);
        let own = AtomicBool::new(false);
        let tokens = Tokens {
            frame: &frame,
            own: &own,
        };
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_combined_token(&tokens, |_| -> u32 { panic!("the work panics") })
        }));
        let _ = sender.send(caught.is_err());
    });
    assert_eq!(receiver.recv_timeout(PATIENCE), Ok(true));
}

#[test]
fn a_stale_generation_writes_nothing_and_the_finished_frame_fills_the_overlay() {
    let mut overlay = Overlay::default();
    overlay.reset((2, 2));
    let stale = overlay.generation;
    overlay.reset((2, 2));
    let tile = TileRect {
        x0: 0,
        y0: 0,
        x1: 2,
        y1: 2,
    };
    overlay.write_batch(stale, &[tile], &[Vec3::ONE; 4]);
    assert_eq!(overlay.filled, 0);
    overlay.write_frame(stale, &[Vec3::ONE; 4]);
    assert_eq!(overlay.filled, 0);

    let frame = [Vec3::X, Vec3::Y, Vec3::Z, Vec3::ONE];
    overlay.write_frame(overlay.generation, &frame);
    assert_eq!(overlay.filled, 4);
    let mut out = Vec::new();
    assert!(overlay.compose(&[Vec3::ZERO; 4], 1, &mut out));
    assert_eq!(out, frame.to_vec());
    assert!(
        !overlay.compose(&[Vec3::ZERO; 3], 1, &mut out),
        "a frame of another size is not composed"
    );
}

#[test]
fn a_batch_makes_news_for_the_display_once() {
    let mut beams = StillBeams::default();
    assert_eq!(beams.sync(want(32)), BeamStep::Start);
    assert!(beams.take_news(), "the new pose cleared the overlay");
    assert!(!beams.take_news());
    let overlay = Arc::clone(&beams.overlay);
    let generation = lock(&overlay).generation;
    let tile = TileRect {
        x0: 0,
        y0: 0,
        x1: 1,
        y1: 1,
    };
    lock(&overlay).write_batch(generation, &[tile], &[Vec3::ONE]);
    assert!(beams.take_news());
    assert!(!beams.take_news());
}

#[test]
fn the_viewport_beams_use_the_viewport_leaf_cache_cap() {
    // The coarse test tree keeps the production cap.
    assert_eq!(
        viewport_beam_settings().leaf_cache_bytes,
        VIEWPORT_LEAF_CACHE_BYTES
    );
    assert_eq!(
        viewport_beam_defaults(),
        BeamSettings {
            leaf_cache_bytes: VIEWPORT_LEAF_CACHE_BYTES,
            ..BeamSettings::default()
        }
    );
}

#[test]
fn a_standard_brilliant_is_beam_traced_and_an_opal_is_refused() {
    with_frame(&spinel(), |frame| {
        assert!(check(frame).is_ok());
        let hdr = None;
        let inputs = BeamJobInputs::capture(frame, hdr, 64, 2).expect("eligible");
        assert_eq!(inputs.size, SIZE);
        assert_eq!(inputs.cap, 64);
    });
    let mut opal = spinel();
    opal.name = "Precious Opal (White)".to_owned();
    with_frame(&opal, |frame| {
        assert!(matches!(check(frame), Err(BeamOff::NotEligible(_))));
        let mut beams = StillBeams::default();
        assert_eq!(beams.sync(want(32)), BeamStep::Start);
        let cancel = Arc::new(AtomicBool::new(false));
        beams.launch(
            BeamStep::Start,
            frame,
            &Launch {
                hdr: None,
                cap: 64,
                frame_cancel: &cancel,
            },
        );
        assert!(beams.is_refused());
        assert!(!beams.owns_stone());
        assert!(!beams.compose_into(&vec![Vec3::ZERO; PIXELS], 1));
        assert_eq!(beams.sync(want(64)), BeamStep::Idle);
    });
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_beam_tests.sh step 04b"]
fn the_shown_image_equals_the_direct_entry_render() {
    with_frame(&spinel(), |frame| {
        // Eight residual samples: the equality with the direct render is exact at any cap.
        let inputs = BeamJobInputs::capture(frame, None, 8, 2).expect("eligible");
        let never = AtomicBool::new(false);
        let (direct, _) = render_frame(&inputs, &never, None, None).expect("the real entry");

        let mut beams = StillBeams::default();
        assert_eq!(beams.sync(want(32)), BeamStep::Start);
        let cancel = Arc::new(AtomicBool::new(false));
        beams.launch(
            BeamStep::Start,
            frame,
            &Launch {
                hdr: None,
                cap: 8,
                frame_cancel: &cancel,
            },
        );
        wait_for_within(RENDER_PATIENCE, || {
            beams.sync(want(32));
            !beams.is_running()
        });
        assert!(beams.owns_stone(), "the job ended with the whole frame");
        assert!(beams.compose_into(&vec![Vec3::ZERO; PIXELS], 1));
        let shown = beams.composite();
        let differing = shown
            .iter()
            .zip(&direct.pixels)
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            shown == direct.pixels.as_slice(),
            "{differing} of {} pixels differ (shown {} pixels, direct {}); first shown {:?}, first direct {:?}",
            shown.len(),
            shown.len(),
            direct.pixels.len(),
            shown.first(),
            direct.pixels.first()
        );
    });
}
