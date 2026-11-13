//! Tests of the viewport's path cache: when it applies, when it is built and dropped, a stone it
//! refuses, and the mean of a cached CPU pass against the traced one. No adapter needed.

use super::*;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, surface::SurfaceScene},
    },
    renderer::cpu_frame::trace_pixels_interleaved_surface,
};

const SIZE: (u32, u32) = (32, 32);

/// Runs `check` on a frame of the standard brilliant in `material`, `spp` samples from
/// `sample_offset`.
fn with_frame<R>(
    material: &GemMaterial,
    (sample_offset, spp): (u32, u32),
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
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        spp,
        sample_offset,
    };
    check(&frame)
}

fn spinel() -> GemMaterial {
    GemMaterial::by_name("Spinel").expect("Spinel is a built-in material")
}

fn want(samples: u32, reset: bool) -> CacheWanted {
    CacheWanted {
        applies: true,
        reset,
        samples,
        size: SIZE,
    }
}

fn never() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn the_viewport_cache_applies_to_a_still_path_traced_view_only() {
    let branching = Estimator::Branching;
    assert!(applies_with(true, false, RenderPhase::Final, branching));
    assert!(
        !applies_with(false, false, RenderPhase::Final, branching),
        "preference off"
    );
    assert!(
        !applies_with(true, true, RenderPhase::Final, branching),
        "Ray-Traced"
    );
    assert!(!applies_with(true, false, RenderPhase::Moving, branching));
    assert!(!applies_with(
        true,
        false,
        RenderPhase::Bootstrap,
        branching
    ));
}

#[test]
fn the_viewport_cache_never_applies_under_exit_collection() {
    let collect = Estimator::ExitCollection;
    for phase in [
        RenderPhase::Final,
        RenderPhase::Moving,
        RenderPhase::Bootstrap,
    ] {
        assert!(!applies_with(true, false, phase, collect));
    }
    assert!(!applies_with(true, true, RenderPhase::Final, collect));
}

#[test]
fn the_cache_waits_for_sixty_four_still_samples() {
    assert_eq!(VIEWPORT_CACHE_AFTER_SPP, 64);
    let mut cache = ViewportCache::default();
    for samples in [0, 1, 32, 63] {
        assert!(!cache.sync(want(samples, false)), "{samples} samples");
    }
    assert!(cache.sync(want(64, false)));
    assert!(cache.sync(want(65, false)));
    let off = CacheWanted {
        applies: false,
        ..want(500, false)
    };
    assert!(!cache.sync(off), "a view the cache does not apply to");
    assert!(!cache.is_built());
}

#[test]
fn a_built_cache_is_dropped_by_a_pose_change_a_new_size_or_a_view_it_no_longer_applies_to() {
    let material = spinel();
    with_frame(&material, (0, 1), |frame| {
        let cancel = never();
        for dropped_by in ["reset", "size", "applies"] {
            let mut cache = ViewportCache::default();
            assert!(cache.sync(want(64, false)));
            assert!(cache.ensure_built(frame, &cancel), "{dropped_by}");
            assert!(cache.is_built());
            assert!(cache.built_bytes() > 0);
            // The same pose keeps it.
            assert!(cache.sync(want(80, false)));
            assert!(cache.is_built(), "a still pose keeps its cache");

            let next = match dropped_by {
                "reset" => want(0, true),
                "size" => CacheWanted {
                    size: (SIZE.0 + 1, SIZE.1),
                    ..want(80, false)
                },
                _ => CacheWanted {
                    applies: false,
                    ..want(80, false)
                },
            };
            cache.sync(next);
            assert!(!cache.is_built(), "{dropped_by} drops the cache at once");
            assert_eq!(cache.built_bytes(), 0);
        }
    });
}

#[test]
fn a_stone_the_cache_cannot_serve_is_refused_until_the_accumulation_resets() {
    let mut opal = spinel();
    opal.name = "Precious Opal (White)".to_owned();
    with_frame(&opal, (0, 1), |frame| {
        let cancel = never();
        let mut cache = ViewportCache::default();
        assert!(cache.sync(want(64, false)));
        assert!(!cache.ensure_built(frame, &cancel));
        assert!(cache.is_refused());
        assert!(!cache.sync(want(70, false)), "no second attempt");
        // The ordinary pass is all that is left, and says so.
        let mut accum = vec![Vec3::ZERO; (SIZE.0 * SIZE.1) as usize];
        let mut scratch = Vec::new();
        assert_eq!(
            cache.accumulate(None, frame, &mut accum, &mut scratch, &cancel),
            CacheOutcome::Unavailable
        );
        assert!(accum.iter().all(|pixel| *pixel == Vec3::ZERO), "untouched");
        // A reset forgives it.
        assert!(!cache.sync(want(0, true)));
        assert!(!cache.is_refused());
    });
}

#[test]
fn a_cancelled_build_leaves_nothing_and_is_tried_again() {
    let material = spinel();
    with_frame(&material, (0, 1), |frame| {
        let mut cache = ViewportCache::default();
        assert!(cache.sync(want(64, false)));
        let cancelled = AtomicBool::new(true);
        assert!(!cache.ensure_built(frame, &cancelled));
        assert!(!cache.is_built());
        assert!(!cache.is_refused(), "a cancel is not a refusal");
        assert!(cache.ensure_built(frame, &never()), "the next frame builds");
    });
}

#[test]
fn a_cached_cpu_pass_has_the_traced_mean_and_keeps_its_tiles() {
    let material = spinel();
    // 128 samples after the 64 that stood still.
    let (offset, spp) = (64, 128);
    let reference = with_frame(&material, (0, 512), |frame| {
        let scene = frame_scene(frame);
        let soa = build_plane_soa(frame.planes);
        trace_pixels_interleaved_surface(
            &scene,
            (frame.tools, frame.fluorescence, frame.surface),
            &soa,
            0,
            1,
            0,
            512,
        )
    });
    with_frame(&material, (offset, spp), |frame| {
        let cancel = never();
        let mut cache = ViewportCache::default();
        assert!(cache.ensure_built(frame, &cancel));
        let bytes = cache.built_bytes();
        let mut accum = vec![Vec3::ZERO; (SIZE.0 * SIZE.1) as usize];
        let mut scratch = Vec::new();
        assert_eq!(
            cache.accumulate(None, frame, &mut accum, &mut scratch, &cancel),
            CacheOutcome::Done
        );
        assert!(cache.is_built(), "the tiles come back after the pass");
        assert_eq!(cache.built_bytes(), bytes);
        assert!(accum.iter().all(|pixel| pixel.is_finite()));

        // The centre of the frame is the stone.
        let centre = |sums: &[Vec3], samples: f32| {
            let mut total = 0.0;
            for y in 12..20 {
                for x in 12..20 {
                    total += sums[(y * SIZE.0 + x) as usize].y;
                }
            }
            total / 64.0 / samples
        };
        let (got, want) = (centre(&accum, spp as f32), centre(&reference, 512.0));
        assert!(want > 0.0, "the stone has light");
        assert!(
            (got - want).abs() <= 0.10 * want,
            "cached mean {got} against the traced mean {want}"
        );

        // A second pass adds on, and the tiles survive it too.
        let before = accum.clone();
        assert_eq!(
            cache.accumulate(None, frame, &mut accum, &mut scratch, &cancel),
            CacheOutcome::Done
        );
        assert!(accum.iter().zip(&before).all(|(a, b)| a.y >= b.y));
        assert_eq!(cache.built_bytes(), bytes);
    });
}

#[test]
fn a_cancelled_cached_pass_adds_nothing() {
    let material = spinel();
    with_frame(&material, (64, 8), |frame| {
        let mut cache = ViewportCache::default();
        assert!(cache.ensure_built(frame, &never()));
        let mut accum = vec![Vec3::ZERO; (SIZE.0 * SIZE.1) as usize];
        let mut scratch = Vec::new();
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            cache.accumulate(None, frame, &mut accum, &mut scratch, &cancelled),
            CacheOutcome::Cancelled
        );
        assert!(accum.iter().all(|pixel| *pixel == Vec3::ZERO));
        assert!(cache.is_built());
    });
}

#[test]
fn the_caps_are_the_documented_ones() {
    assert_eq!(VIEWPORT_CACHE_CAP_BYTES, 4 * 1024 * 1024 * 1024);
}
