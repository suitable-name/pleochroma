//! The HDR map's turn in the current-view metrics.

use super::*;
use crate::scene::{CameraSpec, LightingSpec, planes_to_data};
use pleochroma::{geometry::cuts::StandardGemCuts, render_setup::Backdrop};

fn lighting() -> LightingSpec {
    LightingSpec::new(
        LightingPreset::LightTent,
        1.0,
        0.85,
        0.95,
        Backdrop::Grey,
        16.0,
    )
}

fn scene_spec(lighting: LightingSpec) -> SceneSpec {
    SceneSpec {
        planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: 0.6,
            pitch: 0.47,
            distance: 4.2,
        },
        lighting,
        max_bounces: 4,
        width: 4,
        height: 4,
        hdr_id: Some(9),
    }
}

/// A map with one bright lobe at one side of the room (columns 0 and 1 of the second row).
fn lopsided_map() -> EnvironmentMap {
    let mut pixels = vec![[0.05; 3]; 8 * 4];
    pixels[8..10].fill([40.0; 3]);
    EnvironmentMap::from_rgb(8, 4, pixels).expect("a consistent buffer")
}

fn score(params: &MetricsParams, map: &EnvironmentMap) -> MetricsResultData {
    match run_metrics_with_map(&mut None, params, Some((9, map))) {
        SolveResponse::Metrics(data) => data,
        other => panic!("expected metrics, got {other:?}"),
    }
}

#[test]
fn the_metrics_params_carry_the_map_yaw_at_the_scenes_camera() {
    let plain = MetricsParams::from_scene(&scene_spec(lighting()));
    assert_eq!(plain.hdr_yaw.to_bits(), 0.0_f32.to_bits());

    let turned = MetricsParams::from_scene(&scene_spec(lighting().with_hdr_turn(90.0, false)));
    assert!((turned.hdr_yaw - 90.0_f32.to_radians()).abs() < 1e-6);
    assert_ne!(plain, turned);

    // Following the camera adds the scene's camera yaw (0.6).
    let following = MetricsParams::from_scene(&scene_spec(lighting().with_hdr_turn(90.0, true)));
    assert!((following.hdr_yaw - (90.0_f32.to_radians() + 0.6)).abs() < 1e-6);
}

#[test]
fn a_map_turned_half_way_round_scores_the_stone_differently() {
    let map = lopsided_map();
    let unturned = MetricsParams::from_scene(&scene_spec(lighting()));
    let turned = MetricsParams::from_scene(&scene_spec(lighting().with_hdr_turn(180.0, false)));
    let before = score(&unturned, &map);
    let after = score(&turned, &map);
    assert_eq!(before.scored_under, ScoredUnder::HdrMap(9));
    assert_eq!(after.scored_under, ScoredUnder::HdrMap(9));
    assert_ne!(
        before, after,
        "the lobe moved to the other side of the stone"
    );
}
