use std::sync::Arc;

use super::*;
use crate::platform_mock::MockPlatform;

fn state() -> AppState {
    AppState::new(Arc::new(MockPlatform::default()))
}

#[test]
fn restarting_needs_a_failure_to_start_over_from() {
    let mut app = state();
    assert!(!restart_failed(&mut app));
}

#[test]
fn the_driver_forgets_its_run_and_its_failed_tries() {
    let mut driver = RenderDriver::default();
    for _ in 0..FAILURE_RETRY_DELAYS.len() {
        let _ = driver.retry.decide(Duration::ZERO);
        let _ = driver.retry.decide(Duration::from_secs(60));
    }
    assert_eq!(
        driver.retry.decide(Duration::from_secs(120)),
        RetryStep::GiveUp
    );
    driver.forget_run();
    assert!(matches!(
        driver.retry.decide(Duration::from_secs(120)),
        RetryStep::Wait(_)
    ));
}

#[test]
#[expect(
    clippy::suboptimal_flops,
    reason = "a test bound; the unfused multiply-then-subtract is exact enough for it"
)]
fn trace_size_follows_the_view_and_the_class() {
    // A phone viewport is narrower than the minimum aspect: the height is cut.
    assert_eq!(trace_size((390.0, 320.0), true, false), (512, 320));
    // A wide tablet viewport keeps its own aspect.
    let (w, h) = trace_size((800.0, 390.0), false, false);
    assert_eq!(w, 768);
    assert!((372..=376).contains(&h), "{h}");
    // Battery saver shrinks it; nothing is ever below the minimum edge.
    assert_eq!(trace_size((800.0, 400.0), true, true).0, 384);
    let (w, h) = trace_size((0.0, 0.0), true, false);
    assert!(w >= MIN_EDGE && h >= MIN_EDGE);
    // A tall view is the long edge in height, then letterboxed to the aspect.
    let (w, h) = trace_size((200.0, 800.0), true, false);
    assert!(f64::from(w) >= f64::from(h) * f64::from(SOLID_MIN_ASPECT) - 1.0);
}

#[test]
fn the_gate_lets_a_still_picture_restart_at_once() {
    let mut gate = RestartGate::default();
    let t0 = Duration::from_secs(10);
    assert_eq!(gate.decide(t0, true), GateDecision::Start);
    gate.started(t0);
    assert_eq!(gate.decide(t0, false), GateDecision::Start);
}

#[test]
fn the_gate_coalesces_restarts_while_a_finger_is_down() {
    let mut gate = RestartGate::default();
    let t0 = Duration::from_secs(10);
    gate.started(t0);
    let soon = t0 + Duration::from_millis(40);
    assert_eq!(
        gate.decide(soon, true),
        GateDecision::Wait(Duration::from_millis(110))
    );
    assert_eq!(
        gate.decide(t0 + Duration::from_millis(150), true),
        GateDecision::Start
    );
    assert_eq!(
        gate.decide(t0 + Duration::from_millis(400), true),
        GateDecision::Start
    );
}

#[test]
fn touch_state_tracks_both_kinds_of_touch() {
    let mut touch = TouchState::default();
    assert!(!touch.is_down());
    touch.set_pointer(true);
    assert!(touch.is_down());
    touch.set_pointer(false);
    touch.set_pinch(true);
    assert!(touch.is_down());
    touch.set_pinch(false);
    assert!(!touch.is_down());
}

#[test]
fn a_light_drag_reports_the_movement_between_events() {
    let mut drag = LightDrag::default();
    assert_eq!(drag.moved(5.0, 5.0), None);
    drag.begin(10.0, 20.0);
    assert_eq!(drag.moved(14.0, 18.0), Some((4.0, -2.0)));
    assert_eq!(drag.moved(14.0, 30.0), Some((0.0, 12.0)));
    drag.end();
    assert_eq!(drag.moved(1.0, 1.0), None);
}

#[test]
fn dragging_right_and_up_turns_the_light_and_raises_it() {
    let values = LightValues {
        exposure: 1.0,
        azimuth_deg: 100.0,
        elevation_deg: 50.0,
        head_shadow_deg: 10.0,
        surface_glare_pct: 100.0,
    };
    let moved = light_after_drag(values, 10.0, -10.0);
    assert!((moved.azimuth_deg - 104.0).abs() < 1e-4);
    assert!((moved.elevation_deg - 53.0).abs() < 1e-4);
    assert!((moved.exposure - 1.0).abs() < f32::EPSILON);
}

#[test]
fn dragging_the_light_stores_clamped_values_in_the_settings() {
    let mut app = state();
    let before = app.settings.lighting.azimuth_deg;
    drag_light(&mut app, 100.0, 0.0);
    let after = app.settings.lighting.azimuth_deg;
    assert!(
        (after - (before + 40.0).rem_euclid(360.0)).abs() < 1e-3,
        "{after}"
    );
    // A huge upward drag stops at the elevation limit.
    drag_light(&mut app, 0.0, -100_000.0);
    assert!(app.settings.lighting.elevation_deg <= 88.0 + f32::EPSILON);
}

/// With an HDR map as the lighting a drag is available and turns the map; its `dy` leaves the
/// light's elevation alone.
#[test]
fn dragging_with_an_hdr_map_turns_the_map_and_keeps_the_elevation() {
    let mut app = state();
    app.update_settings(|settings| settings.lighting.elevation_deg = 50.0);
    let mut bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 32 +X 64\n".to_vec();
    for _ in 0..64 * 32 {
        bytes.extend_from_slice(&[120, 130, 140, 129]);
    }
    let file = crate::platform::PickedFile {
        display_name: "sky.hdr".to_owned(),
        bytes,
    };
    crate::hdr::import(&mut app, &file).expect("imports");
    assert!(light_drag_available(&app), "a map is turned by the drag");

    let rig_azimuth = app.settings.lighting.azimuth_deg;
    drag_light(&mut app, 100.0, 40.0);
    assert!((app.settings.lighting.hdr_yaw_deg - 40.0).abs() < 1e-3);
    assert!((app.settings.lighting.elevation_deg - 50.0).abs() < 1e-6);
    assert!((app.settings.lighting.azimuth_deg - rig_azimuth).abs() < 1e-6);
}

#[test]
fn export_options_respect_the_caps() {
    assert_eq!(size_options(1024), vec![512, 1024]);
    assert_eq!(size_options(768), vec![512]);
    assert_eq!(size_options(300), vec![300]);
    assert_eq!(spp_options(256), vec![64, 128, 256]);
    assert_eq!(spp_options(100), vec![64]);
    assert_eq!(spp_options(10), vec![10]);
}

#[test]
fn export_request_picks_the_chosen_options_within_the_caps() {
    let settings = RenderSettings::default();
    let request = export_request(&settings, 1, 2, ExportColorSpace::DisplayP3);
    assert_eq!(request.long_edge_px, 1024);
    assert_eq!(request.spp, 256);
    assert_eq!(request.color_space, ExportColorSpace::DisplayP3);
    // An index past the end picks the last option.
    let request = export_request(&settings, 9, 9, ExportColorSpace::Srgb);
    assert_eq!((request.long_edge_px, request.spp), (1024, 256));
    // A lower cap lowers the choices.
    let capped = RenderSettings {
        export_cap_px: 600,
        export_cap_spp: 100,
        ..RenderSettings::default()
    };
    let request = export_request(&capped, 1, 2, ExportColorSpace::Srgb);
    assert_eq!((request.long_edge_px, request.spp), (512, 64));
}

#[test]
fn color_space_indices_map_to_the_two_spaces() {
    assert_eq!(color_space_from_index(0), ExportColorSpace::Srgb);
    assert_eq!(color_space_from_index(1), ExportColorSpace::DisplayP3);
    assert_eq!(color_space_from_index(7), ExportColorSpace::Srgb);
}

#[test]
fn the_estimate_scales_with_pixels_and_samples() {
    let base = ExportRequest {
        long_edge_px: 512,
        spp: 64,
        color_space: ExportColorSpace::Srgb,
    };
    let estimate = ExportEstimate::new(&base, Some(10.0));
    let double_spp = ExportRequest { spp: 128, ..base };
    let double_edge = ExportRequest {
        long_edge_px: 1024,
        ..base
    };
    assert!((estimate.for_request(&base).unwrap() - 10.0).abs() < 1e-3);
    assert!((estimate.for_request(&double_spp).unwrap() - 20.0).abs() < 1e-3);
    assert!((estimate.for_request(&double_edge).unwrap() - 40.0).abs() < 1e-3);
    assert_eq!(ExportEstimate::new(&base, None).for_request(&base), None);
    assert_eq!(
        ExportEstimate::new(&base, Some(f32::NAN)).for_request(&base),
        None
    );
}

#[test]
fn the_estimate_text_reads_naturally() {
    assert!(estimate_text(None).contains("unavailable"));
    assert_eq!(estimate_text(Some(3.0)), "Under 10 seconds.");
    assert_eq!(estimate_text(Some(42.0)), "About 40 seconds.");
    assert_eq!(estimate_text(Some(300.0)), "About 5 minutes.");
    assert_eq!(estimate_text(Some(100.0)), "About 2 minutes.");
}

#[test]
fn file_names_are_safe_and_end_in_png() {
    assert_eq!(
        export_file_name("Standard Round Brilliant"),
        "Standard Round Brilliant.png"
    );
    assert_eq!(export_file_name("  a/b:c*d  "), "a b c d.png");
    assert_eq!(export_file_name(""), "Pleochroma Render.png");
    assert_eq!(export_file_name("///"), "Pleochroma Render.png");
    assert_eq!(export_file_name("..."), "Pleochroma Render.png");
    let long = "x".repeat(200);
    assert_eq!(export_file_name(&long).len(), 64);
}

#[test]
fn byte_sizes_read_naturally() {
    assert_eq!(byte_size_text(12), "12 bytes");
    assert_eq!(byte_size_text(2048), "2 KB");
    assert_eq!(byte_size_text(1_572_864), "1.5 MB");
}

#[test]
fn qualities_cycle_and_have_labels() {
    assert_eq!(next_quality(RenderQuality::Draft), RenderQuality::Normal);
    assert_eq!(next_quality(RenderQuality::Normal), RenderQuality::Fine);
    assert_eq!(next_quality(RenderQuality::Fine), RenderQuality::Draft);
    assert_eq!(quality_index(RenderQuality::Fine), 2);
    assert_eq!(quality_label(RenderQuality::Normal), "Normal");
}

#[test]
fn progress_follows_the_status() {
    assert_eq!(
        progress_fraction(&RenderStatus::Tracing {
            samples: 16,
            target: 64
        }),
        Some(0.25)
    );
    assert_eq!(
        progress_fraction(&RenderStatus::Done { samples: 64 }),
        Some(1.0)
    );
    assert_eq!(progress_fraction(&RenderStatus::Idle), Some(0.0));
    assert_eq!(
        progress_fraction(&RenderStatus::Paused(PauseReason::User)),
        None
    );
    assert_eq!(
        progress_fraction(&RenderStatus::Tracing {
            samples: 1,
            target: 0
        }),
        Some(0.0)
    );
}

#[test]
fn an_unsolved_design_says_to_solve_it() {
    let app = state();
    assert_eq!(status_line(&app), "Solve the design to render it");
}

/// A state whose Render mode is on show (a design, the Design destination, a view size).
/// It has no solve, so a trace that is allowed to start ends as `Blocked`; that is how
/// these tests see that a start was attempted.
fn render_on_show() -> AppState {
    let mut app = state();
    app.document.origin = crate::document::DocumentOrigin::New;
    app.destination = Destination::Design;
    app.viewport.set_view(390.0, 320.0, 1.0, true);
    app.viewport.set_mode(super::super::ViewMode::Render);
    assert!(wants(&app));
    app
}

#[test]
fn a_camera_drag_starts_no_trace_until_the_finger_lifts_then_one() {
    let mut app = render_on_show();
    app.viewport.touch.set_pointer(true);
    let mut now = Duration::from_secs(10);
    // Sixty orbit events over about a second: every look is cheap and starts nothing.
    for _ in 0..60 {
        app.viewport.orbit(9.0, 2.0);
        now += Duration::from_millis(16);
        assert_eq!(maintain(&mut app, now), Maintain::Held);
    }
    assert!(
        app.viewport.render_live.solid(),
        "the live pose is the raster"
    );
    assert!(
        app.viewport.driver.blocked_message().is_none(),
        "no start was attempted during the drag"
    );
    assert_eq!(
        status_line(&app),
        "Solve the design to render it",
        "an unsolved design still says so"
    );

    // The finger lifts 16 ms after the last move: the rest of the settle time is asked for.
    app.viewport.touch.set_pointer(false);
    now += Duration::from_millis(16);
    assert_eq!(
        maintain(&mut app, now),
        Maintain::Wait(
            super::super::render_drag::SETTLE
                .checked_sub(Duration::from_millis(16))
                .expect("the settle time is longer than 16 ms")
        )
    );
    assert!(app.viewport.driver.blocked_message().is_none());

    // Once it has rested, one start is attempted at the final pose, and only one.
    now += super::super::render_drag::SETTLE;
    assert!(matches!(maintain(&mut app, now), Maintain::Blocked(_)));
    assert!(app.viewport.driver.blocked_message().is_some());
    assert!(!app.viewport.render_live.owes_restart());
    assert!(
        !app.viewport.render_live.solid(),
        "a failed start gives the area back to the status text"
    );
}

#[test]
fn a_pinch_counts_as_a_finger_down() {
    let mut app = render_on_show();
    app.viewport.touch.set_pinch(true);
    app.viewport.zoom_camera(1.4);
    let now = Duration::from_secs(5);
    assert_eq!(maintain(&mut app, now), Maintain::Held);
    app.viewport.touch.set_pinch(false);
    assert!(matches!(
        maintain(&mut app, now + Duration::from_millis(1)),
        Maintain::Wait(_)
    ));
}

#[test]
fn a_light_drag_keeps_the_picture_and_starts_one_trace_after_the_release() {
    let mut app = render_on_show();
    app.viewport.light_drag.on = true;
    app.viewport.touch.set_pointer(true);
    app.viewport.light_drag.begin(100.0, 100.0);
    let mut now = Duration::from_secs(20);
    for step in 0..40_u8 {
        let (dx, dy) = app
            .viewport
            .light_drag
            .moved(100.0 + f32::from(step), 100.0)
            .unwrap_or_default();
        drag_light(&mut app, dx, dy);
        now += Duration::from_millis(16);
        assert_eq!(maintain(&mut app, now), Maintain::Held);
    }
    // A light cannot be shown by the raster: the traced picture stays on show.
    assert!(!app.viewport.render_live.solid());
    assert!(!live_solid(&app));
    assert!(app.viewport.render_live.owes_restart());
    assert!(app.viewport.driver.blocked_message().is_none());
    assert_eq!(status_line(&app), "Solve the design to render it");

    app.viewport.light_drag.end();
    app.viewport.touch.set_pointer(false);
    now += Duration::from_millis(16);
    assert!(matches!(maintain(&mut app, now), Maintain::Wait(_)));
    now += super::super::render_drag::SETTLE;
    assert!(matches!(maintain(&mut app, now), Maintain::Blocked(_)));
    assert!(!app.viewport.render_live.owes_restart());
}

#[test]
fn leaving_the_render_mode_drops_a_drag_in_progress() {
    let mut app = render_on_show();
    app.viewport.touch.set_pointer(true);
    app.viewport.orbit(30.0, 0.0);
    assert!(app.viewport.render_live.owes_restart());
    app.viewport.set_mode(super::super::ViewMode::Solid);
    assert!(!app.viewport.render_live.owes_restart());
    assert_eq!(
        maintain(&mut app, Duration::from_secs(1)),
        Maintain::Idle,
        "nothing is wanted in the Solid view, so nothing is held either"
    );
}

#[test]
fn a_traced_frame_of_the_running_epoch_hands_the_picture_back() {
    let mut app = render_on_show();
    app.viewport.orbit(30.0, 0.0);
    let camera = app.viewport.camera;
    app.viewport.render_live.started(4, camera);
    // Without a solve the viewport never shows the raster, so nothing to switch.
    assert!(!live_solid(&app));
    assert!(frame_landed(&mut app, 4));
    assert!(!app.viewport.render_live.solid());
    // The camera moves again before a later epoch's frame: the raster stays.
    app.viewport.orbit(30.0, 0.0);
    assert!(!frame_landed(&mut app, 5));
    assert!(app.viewport.render_live.solid());
}

#[test]
fn the_picture_is_stale_only_while_a_moved_light_owes_a_trace() {
    let mut app = render_on_show();
    app.viewport.light_drag.on = true;
    assert!(!light_picture_stale(&app), "nothing moved yet");
    drag_light(&mut app, 12.0, -5.0);
    assert!(light_picture_stale(&app));
    app.viewport.light_drag.on = false;
    assert!(!light_picture_stale(&app), "the Light chip is off");
    app.viewport.light_drag.on = true;
    app.viewport.render_live.abandoned();
    assert!(!light_picture_stale(&app), "nothing is owed any more");
}

#[test]
fn a_failed_trace_restarts_after_growing_pauses_then_gives_up() {
    let mut retry = FailureRetry::default();
    let mut now = Duration::from_secs(100);
    for delay in FAILURE_RETRY_DELAYS {
        // The first look only starts the clock; the same look again asks for the rest.
        assert_eq!(retry.decide(now), RetryStep::Wait(delay));
        assert_eq!(
            retry.decide(now + Duration::from_millis(10)),
            RetryStep::Wait(delay.saturating_sub(Duration::from_millis(10)))
        );
        now += delay;
        assert_eq!(retry.decide(now), RetryStep::Now);
    }
    assert_eq!(
        retry.decide(now + Duration::from_secs(60)),
        RetryStep::GiveUp
    );
}

#[test]
fn a_trace_that_merged_a_pass_clears_the_tries() {
    let mut retry = FailureRetry::default();
    let now = Duration::from_secs(5);
    let _ = retry.decide(now);
    assert_eq!(retry.decide(now + FAILURE_RETRY_DELAYS[0]), RetryStep::Now);
    // Started but nothing merged yet: the tries stay used.
    retry.note(&RenderStatus::Tracing {
        samples: 0,
        target: 8,
    });
    assert_ne!(retry, FailureRetry::default());
    retry.note(&RenderStatus::Tracing {
        samples: 1,
        target: 8,
    });
    assert_eq!(retry, FailureRetry::default());
}

#[test]
fn nothing_is_wanted_while_the_solid_view_is_up() {
    let mut app = state();
    assert!(!wants(&app));
    assert_eq!(maintain(&mut app, Duration::from_secs(1)), Maintain::Idle);
    assert!(!app.viewport.driver.is_active());
}

#[test]
fn a_remote_computer_that_sent_nothing_yet_is_named_in_the_status() {
    let waiting = "Remote · Waiting for Studio PC…";
    let preparing = "Remote · Preparing Studio PC's graphics card… 12 of 40";
    let none = RenderStatus::Tracing {
        samples: 0,
        target: 64,
    };
    assert_eq!(
        waiting_notice(&none, waiting).as_deref(),
        Some("Waiting for Studio PC…")
    );
    assert_eq!(
        waiting_notice(&none, preparing).as_deref(),
        Some("Preparing Studio PC's graphics card… 12 of 40")
    );
    // Once samples exist the normal status line returns.
    let some = RenderStatus::Tracing {
        samples: 1,
        target: 64,
    };
    assert_eq!(waiting_notice(&some, waiting), None);
    assert_eq!(waiting_notice(&some, preparing), None);
    assert_eq!(
        waiting_notice(&RenderStatus::Done { samples: 64 }, preparing),
        None
    );
    // Other reports never replace the status.
    for report in [
        "Remote · Ray-Traced · Coordinator · FullData",
        "Remote · reconnecting",
        "Device GPU · Adreno",
        "Preparing renderer",
    ] {
        assert_eq!(waiting_notice(&none, report), None, "{report}");
    }
}

#[test]
fn the_device_phrase_says_this_device_in_plain_words() {
    assert_eq!(
        device_phrase("Device GPU · Adreno").as_deref(),
        Some("on this device's GPU")
    );
    assert_eq!(
        device_phrase("Device CPU").as_deref(),
        Some("on this device's CPU")
    );
    assert_eq!(
        device_phrase("Device CPU · frame exceeds the GPU memory budget").as_deref(),
        Some("on this device's CPU (picture too large for the GPU)")
    );
    assert_eq!(
        device_phrase("Remote · reconnecting").as_deref(),
        Some("on the server")
    );
    assert_eq!(device_phrase("Preparing renderer"), None);
    for report in ["Device GPU", "Device CPU · exceeds", "Remote"] {
        let text = device_phrase(report).unwrap_or_default();
        assert!(!text.contains("phone"), "{text}");
    }
}

/// A preparing status of `step` at `done` of `total`, `elapsed_s` seconds in.
const fn preparing(step: PrepareStep, done: u32, total: u32, elapsed_s: u32) -> RenderStatus {
    RenderStatus::Preparing {
        step,
        done,
        total,
        elapsed_s,
    }
}

#[test]
fn a_preparing_status_has_its_own_progress_retry_and_picture_rules() {
    let status = preparing(PrepareStep::BuildingShaders, 3, 18, 5);
    // The bar waits at zero; a render that only prepares has not shown that it runs.
    assert_eq!(progress_fraction(&status), Some(0.0));
    let mut retry = FailureRetry::default();
    let now = Duration::from_secs(5);
    let _ = retry.decide(now);
    assert_eq!(retry.decide(now + FAILURE_RETRY_DELAYS[0]), RetryStep::Now);
    retry.note(&status);
    assert_ne!(retry, FailureRetry::default(), "preparing is not healthy");
}

#[test]
fn the_preparing_texts_are_the_decided_ones() {
    assert_eq!(
        preparing_line(PrepareStep::BuildingShaders, 3, 18, 4),
        "Building shaders \u{b7} 3 of 18"
    );
    assert_eq!(
        preparing_detail(PrepareStep::BuildingShaders, 3, 18, 4),
        "Building shaders 3 of 18"
    );
    assert_eq!(
        preparing_line(PrepareStep::StartingGpu, 0, 0, 2),
        "Starting the graphics card\u{2026}"
    );
    assert_eq!(
        preparing_line(PrepareStep::Uploading, 0, 0, 2),
        "Uploading the stone\u{2026}"
    );
    // The Ray-Traced build is one step of one: it shows its time from the start.
    assert_eq!(
        preparing_line(PrepareStep::BuildingShaders, 0, 1, 12),
        "Building the Ray-Traced shader \u{b7} 12 s"
    );
    // A long step keeps its progress and adds the time.
    assert_eq!(
        preparing_line(PrepareStep::BuildingShaders, 3, 18, 47),
        "Building shaders \u{b7} 3 of 18 \u{b7} 47 s"
    );
    assert_eq!(
        preparing_detail(PrepareStep::BuildingShaders, 3, 18, 47),
        "Building shaders 3 of 18 \u{b7} 47 s"
    );
    // No device phrase while preparing.
    for step in [
        PrepareStep::StartingGpu,
        PrepareStep::BuildingShaders,
        PrepareStep::Uploading,
        PrepareStep::FirstTrace,
    ] {
        let detail = preparing_detail(step, 3, 18, 60);
        assert!(!detail.contains("device"), "{detail}");
    }
}

#[test]
fn the_offer_appears_after_45_seconds_in_one_step() {
    let early = preparing(PrepareStep::BuildingShaders, 3, 18, LONG_STEP_OFFER_S - 1);
    assert_eq!(long_step_offer(&early, Engine::PathTraced), None);
    let late = preparing(PrepareStep::BuildingShaders, 3, 18, LONG_STEP_OFFER_S);
    let offer = long_step_offer(&late, Engine::RayTraced).expect("an offer after 45 s");
    assert_eq!(offer.sentence, "This is taking long. Use the CPU instead?");
    assert_eq!(offer.button, "Use the CPU");
    assert_eq!(offer.engine, Engine::RayTraced);
    // Every preparing step is covered; nothing else offers.
    for step in [
        PrepareStep::StartingGpu,
        PrepareStep::Uploading,
        PrepareStep::FirstTrace,
    ] {
        assert!(long_step_offer(&preparing(step, 0, 0, 99), Engine::PathTraced).is_some());
    }
    assert_eq!(
        long_step_offer(
            &RenderStatus::Tracing {
                samples: 0,
                target: 8
            },
            Engine::PathTraced
        ),
        None
    );
    assert_eq!(
        long_step_offer(&RenderStatus::Paused(PauseReason::User), Engine::PathTraced),
        None
    );
}

#[test]
fn render_details_name_the_cache_and_the_preparation() {
    let cache = |decision, offered| details_lines(decision, offered, None, None);
    assert_eq!(cache(CacheDecision::On, Some(true)), ["Shader cache: on"]);
    assert_eq!(cache(CacheDecision::On, None), ["Shader cache: on"]);
    assert_eq!(
        cache(CacheDecision::On, Some(false)),
        ["Shader cache: not offered by this graphics card"]
    );
    assert_eq!(
        cache(CacheDecision::OffThisStart, Some(true)),
        ["Shader cache: off for this start (the app closed while loading it last time)"]
    );
    assert_eq!(
        cache(CacheDecision::OffOnThisDevice, None),
        ["Shader cache: off on this device (the app closed twice while loading it)"]
    );

    let finished = PreparationTime {
        seconds: 72,
        stopped_in: None,
    };
    let lines = details_lines(CacheDecision::On, Some(true), Some(finished), None);
    assert_eq!(lines[1], "Preparation: 1 min 12 s");
    let stopped = PreparationTime {
        seconds: 72,
        stopped_in: Some((PrepareStep::BuildingShaders, 3, 18)),
    };
    let lines = details_lines(
        CacheDecision::On,
        Some(true),
        Some(stopped),
        Some("Path-Traced"),
    );
    assert_eq!(
        lines[1],
        "Preparation: 1 min 12 s, stopped while building shaders (3 of 18)"
    );
    assert_eq!(
        lines[2],
        "The graphics card did not finish preparing; this session renders Path-Traced on the CPU."
    );
    assert_eq!(minutes_text(9), "9 s");
}

#[test]
fn the_probe_gate_words_follow_whether_a_picture_shows() {
    assert_eq!(
        probe_wait_line(false),
        (
            String::new(),
            "The GPU probe is running. The render starts when it finishes.".to_owned()
        )
    );
    assert_eq!(
        probe_wait_line(true),
        ("Waiting".to_owned(), "The GPU probe is running".to_owned())
    );
}

#[test]
fn a_start_asked_for_during_the_probe_waits_and_runs_when_it_ends() {
    let mut app = render_on_show();
    app.render.set_probe_running(Some(true));
    let mut now = Duration::from_secs(3);
    // The driver starts nothing and asks again soon; no start was attempted.
    assert_eq!(maintain(&mut app, now), Maintain::Wait(PROBE_POLL));
    assert!(app.viewport.driver.blocked_message().is_none());
    assert!(!app.viewport.driver.is_active());
    now += PROBE_POLL;
    assert_eq!(maintain(&mut app, now), Maintain::Wait(PROBE_POLL));

    // The probe ended: the remembered start runs (here it ends as Blocked, the design is
    // unsolved, which is how these tests see that a start was attempted).
    app.render.set_probe_running(Some(false));
    now += PROBE_POLL;
    assert!(matches!(maintain(&mut app, now), Maintain::Blocked(_)));
}
