//! Tests of the instant-lighting cache of the Ray-Traced view: when it is wanted, built and
//! dropped, that a change of lighting keeps it, and that a re-light changes the picture. No
//! adapter needed.

use super::*;
use crate::{
    bridge::render_thread::local_preview::{AfterFinal, PreviewState},
    settings::model::LiveComputeTarget,
};
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::{BACKDROP_GREY, LightingPreset},
    renderer::engine::RenderEngine,
};

const SIZE: (u32, u32) = (24, 24);
const TAPS: u32 = 4;

fn light_tent() -> EnvironmentSource<'static> {
    LightingPreset::LightTent
        .studio(1.0, 0.4, 0.35)
        .with_backdrop(BACKDROP_GREY)
}

fn daylight() -> EnvironmentSource<'static> {
    LightingPreset::Daylight
        .studio(1.6, 0.4, 0.35)
        .with_backdrop(BACKDROP_GREY)
}

/// Runs `check` on a Ray-Traced view of the standard brilliant in `material` under `environment`.
fn with_view<R>(
    material: &GemMaterial,
    environment: EnvironmentSource<'static>,
    check: impl FnOnce(&RayView<'_>) -> R,
) -> R {
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.3, 0.4, 5.0, 18.0);
    let view = RayView {
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
        material,
        max_bounces: 6,
        environment,
    };
    check(&view)
}

fn spinel() -> GemMaterial {
    GemMaterial::by_name("Spinel").expect("Spinel is a built-in material")
}

fn want(pose: u64, converged: bool) -> ReshadeWanted {
    ReshadeWanted {
        applies: true,
        pose,
        converged,
        size: SIZE,
    }
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

fn ray_engine() -> EngineSpec {
    EngineSpec {
        engine: RenderEngine::RayTraced,
        ..EngineSpec::default()
    }
}

/// The job of scene `generation` (a lighting change is a new scene generation, which keys the
/// Ray-Traced engine's prepared scene).
fn job(taps: u32, generation: u64) -> RelightJob<'static> {
    RelightJob {
        engine: ray_engine(),
        generation,
        shared_hdr: None,
        taps,
    }
}

fn blank() -> Vec<Vec3> {
    vec![Vec3::ZERO; (SIZE.0 * SIZE.1) as usize]
}

fn centre(accum: &[Vec3]) -> Vec3 {
    accum[(SIZE.1 / 2 * SIZE.0 + SIZE.0 / 2) as usize]
}

#[test]
fn the_feature_needs_the_preference_the_ray_engine_and_no_remote_lane() {
    assert!(possible(true, true, false));
    assert!(!possible(false, true, false), "preference off");
    assert!(!possible(true, false, false), "Path-Traced");
    assert!(!possible(true, true, true), "a remote lane is involved");
    assert!(in_final(RenderPhase::Final));
    assert!(!in_final(RenderPhase::Moving));
    assert!(!in_final(RenderPhase::Bootstrap));
}

/// Off means identical: a view the feature does not apply to (the preference off, another engine,
/// another phase) never asks for a build and never holds a cache, whatever the picture does.
#[test]
fn a_view_the_feature_does_not_apply_to_never_builds_or_holds_anything() {
    let mut held = ReshadeView::default();
    for pose in 1..4 {
        let off = ReshadeWanted {
            applies: false,
            ..want(pose, true)
        };
        assert!(!held.sync(off), "pose {pose}");
        assert!(!held.holds(pose));
        assert_eq!(held.built_bytes(), 0);
    }
}

#[test]
fn the_cache_waits_for_a_finished_picture_and_a_lighting_change_keeps_it() {
    let material = spinel();
    with_view(&material, light_tent(), |ray| {
        let mut held = ReshadeView::default();
        assert!(!held.sync(want(1, false)), "taps still coming in");
        assert!(held.sync(want(1, true)), "the picture is finished");
        assert!(held.build(ray, &never()));
        assert!(held.holds(1));
        assert!(held.built_bytes() > 0);
        let bytes = held.built_bytes();

        // A lighting edit leaves the pose number alone: nothing is due and nothing is dropped.
        assert!(!held.sync(want(1, false)));
        assert!(!held.sync(want(1, true)));
        assert!(held.holds(1) && held.is_built());
        assert_eq!(held.built_bytes(), bytes);
        assert!(!held.holds(2), "not the cache of another pose");
    });
}

#[test]
fn a_pose_change_a_new_size_or_a_view_it_no_longer_applies_to_drops_the_cache() {
    let material = spinel();
    with_view(&material, light_tent(), |ray| {
        for dropped_by in ["pose", "size", "applies"] {
            let mut held = ReshadeView::default();
            assert!(held.sync(want(1, true)));
            assert!(held.build(ray, &never()), "{dropped_by}");
            let next = match dropped_by {
                "pose" => want(2, true),
                "size" => ReshadeWanted {
                    size: (SIZE.0 + 1, SIZE.1),
                    ..want(1, true)
                },
                _ => ReshadeWanted {
                    applies: false,
                    ..want(1, true)
                },
            };
            held.sync(next);
            assert!(!held.is_built(), "{dropped_by} drops the cache at once");
            assert_eq!(held.built_bytes(), 0);
        }
    });
}

#[test]
fn a_stone_the_cache_cannot_serve_is_refused_until_the_pose_changes() {
    let mut opal = spinel();
    opal.name = "Precious Opal (White)".to_owned();
    with_view(&opal, light_tent(), |ray| {
        let mut held = ReshadeView::default();
        assert!(held.sync(want(1, true)));
        assert!(!held.build(ray, &never()));
        assert!(held.is_refused());
        assert!(!held.sync(want(1, true)), "no second attempt");
        assert!(!held.holds(1));
        // Another pose forgives it.
        assert!(held.sync(want(2, true)));
        assert!(!held.is_refused());
    });
}

#[test]
fn a_cancelled_build_leaves_nothing_and_is_tried_again() {
    let material = spinel();
    with_view(&material, light_tent(), |ray| {
        let mut held = ReshadeView::default();
        assert!(held.sync(want(1, true)));
        assert!(!held.build(ray, &AtomicBool::new(true)));
        assert!(!held.is_built());
        assert!(!held.is_refused(), "a cancel is not a refusal");
        assert!(held.sync(want(1, true)), "the next frame asks again");
        assert!(held.build(ray, &never()));
    });
}

#[test]
fn a_new_lighting_re_lights_the_stone_from_the_same_tiles() {
    let material = spinel();
    let mut held = ReshadeView::default();
    let mut engine = EngineAccumulator::default();
    assert!(held.sync(want(1, true)));
    let tent = with_view(&material, light_tent(), |ray| {
        assert!(held.build(ray, &never()));
        let mut accum = blank();
        let outcome = held.relight(ray, &mut engine, &job(TAPS, 1), &mut accum, &never());
        assert_eq!(outcome, Relight::Done);
        accum
    });
    let bytes = held.built_bytes();
    let day = with_view(&material, daylight(), |ray| {
        let mut accum = blank();
        let outcome = held.relight(ray, &mut engine, &job(TAPS, 2), &mut accum, &never());
        assert_eq!(outcome, Relight::Done);
        accum
    });
    assert_eq!(held.built_bytes(), bytes, "new lighting builds nothing");
    assert!(held.holds(1));
    assert!(tent.iter().chain(&day).all(|pixel| pixel.is_finite()));
    assert!(centre(&tent).y > 0.0, "the stone has light");
    assert!(centre(&day).y > 0.0);
    let changed = tent
        .iter()
        .zip(&day)
        .filter(|(a, b)| (a.y - b.y).abs() > 1.0e-3 * a.y.max(1.0e-6))
        .count();
    assert!(
        changed > 20,
        "another preset and exposure: {changed} pixels differ"
    );
}

#[test]
fn the_picture_is_a_sum_of_taps_for_the_display_to_divide() {
    let material = spinel();
    let mut held = ReshadeView::default();
    let mut engine = EngineAccumulator::default();
    assert!(held.sync(want(1, true)));
    with_view(&material, light_tent(), |ray| {
        assert!(held.build(ray, &never()));
        let mut four = blank();
        let mut eight = blank();
        assert_eq!(
            held.relight(ray, &mut engine, &job(4, 1), &mut four, &never()),
            Relight::Done
        );
        assert_eq!(
            held.relight(ray, &mut engine, &job(8, 1), &mut eight, &never()),
            Relight::Done
        );
        // The stone pixel is the same mean times the tap count: twice the taps, twice the sum.
        assert_eq!(centre(&eight), centre(&four) * 2.0);
    });
}

#[test]
fn with_nothing_held_a_re_light_leaves_the_picture_to_the_ordinary_pass() {
    let material = spinel();
    with_view(&material, light_tent(), |ray| {
        let mut held = ReshadeView::default();
        let mut engine = EngineAccumulator::default();
        let mut accum = blank();
        assert_eq!(
            held.relight(ray, &mut engine, &job(TAPS, 1), &mut accum, &never()),
            Relight::Unavailable
        );
        assert!(accum.iter().all(|pixel| *pixel == Vec3::ZERO), "untouched");
    });
}

#[test]
fn a_cancelled_re_light_says_so() {
    let material = spinel();
    let mut held = ReshadeView::default();
    let mut engine = EngineAccumulator::default();
    assert!(held.sync(want(1, true)));
    with_view(&material, light_tent(), |ray| {
        assert!(held.build(ray, &never()));
        let mut accum = blank();
        assert_eq!(
            held.relight(
                ray,
                &mut engine,
                &job(TAPS, 1),
                &mut accum,
                &AtomicBool::new(true),
            ),
            Relight::Cancelled
        );
        assert!(held.is_built(), "the tiles survive a cancelled pass");
    });
}

#[test]
fn a_light_change_after_a_final_picture_skips_the_bootstrap_only_when_asked() {
    let select = |state: &mut PreviewState, generation, keep| {
        state.select_keeping(
            generation,
            false,
            false,
            LiveComputeTarget::LocalOnly,
            true,
            AfterFinal::from_keep(keep),
        )
    };
    let mut state = PreviewState::default();
    assert_eq!(select(&mut state, 1, false).0, RenderPhase::Bootstrap);
    state.published();
    assert_eq!(select(&mut state, 1, false).0, RenderPhase::Final);

    // Without the request, a new scene starts over with the bootstrap.
    assert_eq!(select(&mut state, 2, false).0, RenderPhase::Bootstrap);
    state.published();
    assert_eq!(select(&mut state, 2, false).0, RenderPhase::Final);

    // With it, the new scene is final at once, and the phase boundary is reported.
    let (phase, changed) = select(&mut state, 3, true);
    assert_eq!(phase, RenderPhase::Final);
    assert!(changed, "a new scene is a reset either way");

    // Never from a view that was not final.
    let mut fresh = PreviewState::default();
    assert_eq!(select(&mut fresh, 1, true).0, RenderPhase::Bootstrap);
}

#[test]
fn select_is_select_keeping_without_the_request() {
    let mut plain = PreviewState::default();
    let mut keeping = PreviewState::default();
    for generation in [1, 1, 2, 2, 3] {
        let a = plain.select(generation, false, false, LiveComputeTarget::LocalOnly, true);
        let b = keeping.select_keeping(
            generation,
            false,
            false,
            LiveComputeTarget::LocalOnly,
            true,
            AfterFinal::Restart,
        );
        assert_eq!(a, b);
        plain.published();
        keeping.published();
    }
}
