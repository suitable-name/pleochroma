//! Tests of the adaptive-sampling types.

use super::*;

const fn settings() -> AdaptiveSettings {
    AdaptiveSettings {
        target_noise: 0.01,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 512,
        max_spp: 8192,
        pass_spp: 64,
        tile: 16,
    }
}

#[test]
fn the_provisional_defaults_are_valid() {
    assert_eq!(settings().validate(), Ok(()));
}

#[test]
fn the_target_noise_presets_are_fine_standard_draft_and_every_one_is_valid() {
    let labels: Vec<&str> = TARGET_NOISE_PRESETS
        .iter()
        .map(|(label, _)| *label)
        .collect();
    assert_eq!(labels, ["Fine", "Standard", "Draft"]);
    let targets: Vec<u32> = TARGET_NOISE_PRESETS
        .iter()
        .map(|(_, target)| target.to_bits())
        .collect();
    assert_eq!(
        targets,
        [0.005_f32.to_bits(), 0.01_f32.to_bits(), 0.02_f32.to_bits()]
    );
    // Finer means a smaller target; Standard is the provisional default.
    assert!(
        TARGET_NOISE_PRESETS
            .windows(2)
            .all(|pair| pair[0].1 < pair[1].1)
    );
    assert_eq!(
        TARGET_NOISE_PRESETS[DEFAULT_TARGET_NOISE_PRESET]
            .1
            .to_bits(),
        NoiseMetric::RelativeLstar.default_target().to_bits()
    );
    for (_, target) in TARGET_NOISE_PRESETS {
        let settings = AdaptiveSettings {
            target_noise: target,
            ..AdaptiveSettings::provisional(1024)
        };
        assert_eq!(settings.validate(), Ok(()));
    }
}

#[test]
fn the_provisional_constructor_is_valid_for_every_sensible_cap() {
    for max_spp in [1, 2, 15, 16, 17, 100, 511, 512, 513, 8192, 32_768] {
        let provisional = AdaptiveSettings::provisional(max_spp);
        assert_eq!(provisional.validate(), Ok(()), "max_spp {max_spp}");
        assert_eq!(provisional.max_spp, max_spp);
    }
    let big = AdaptiveSettings::provisional(8192);
    assert_eq!((big.min_spp, big.pass_spp, big.tile), (512, 16, 16));
    assert_eq!(
        AdaptiveSettings::provisional(0).validate(),
        Err(AdaptiveSettingsError::SampleCounts {
            pass_spp: 1,
            min_spp: 1,
            max_spp: 0,
        })
    );
}

#[test]
fn a_target_noise_out_of_range_is_rejected() {
    for bad in [0.0, -0.01, f32::NAN, f32::INFINITY, 1.0, 3.0] {
        let rejected = AdaptiveSettings {
            target_noise: bad,
            ..settings()
        };
        assert!(
            matches!(
                rejected.validate(),
                Err(AdaptiveSettingsError::TargetNoise(_))
            ),
            "relative target {bad}"
        );
    }
}

#[test]
fn the_absolute_metric_accepts_targets_of_one_and_above() {
    let absolute = AdaptiveSettings {
        target_noise: 3.0,
        metric: NoiseMetric::AbsoluteDeltaE,
        ..settings()
    };
    assert_eq!(absolute.validate(), Ok(()));
    let zero = AdaptiveSettings {
        target_noise: 0.0,
        ..absolute
    };
    assert_eq!(
        zero.validate(),
        Err(AdaptiveSettingsError::TargetNoise(0.0))
    );
}

#[test]
fn sample_counts_out_of_order_are_rejected() {
    for (pass_spp, min_spp, max_spp) in [(0, 512, 8192), (1024, 512, 8192), (64, 512, 256)] {
        let rejected = AdaptiveSettings {
            min_spp,
            max_spp,
            pass_spp,
            ..settings()
        };
        assert_eq!(
            rejected.validate(),
            Err(AdaptiveSettingsError::SampleCounts {
                pass_spp,
                min_spp,
                max_spp,
            })
        );
    }
}

#[test]
fn a_minimum_that_is_not_a_multiple_of_the_pass_is_rejected() {
    let rejected = AdaptiveSettings {
        pass_spp: 48,
        ..settings()
    };
    assert_eq!(
        rejected.validate(),
        Err(AdaptiveSettingsError::MinNotMultipleOfPass {
            pass_spp: 48,
            min_spp: 512,
        })
    );
}

#[test]
fn a_tile_that_is_not_a_power_of_two_in_range_is_rejected() {
    for tile in [0, 4, 12, 48, 128] {
        let rejected = AdaptiveSettings { tile, ..settings() };
        assert_eq!(rejected.validate(), Err(AdaptiveSettingsError::Tile(tile)));
    }
    for tile in [8, 16, 32, 64] {
        assert_eq!(AdaptiveSettings { tile, ..settings() }.validate(), Ok(()));
    }
}

#[test]
fn live_tiles_cover_partial_edge_tiles() {
    let live = LiveTiles::all(40, 20, 16);
    assert_eq!((live.tiles_x(), live.tiles_y()), (3, 2));
    assert!(live.is_full());
    assert_eq!(live.tile_rect(5), (32, 16, 40, 20));
    assert_eq!(live.pixel_count(), 800);
}

#[test]
fn from_indices_sorts_dedups_and_drops_out_of_frame_tiles() {
    let live = LiveTiles::from_indices(40, 20, 16, vec![4, 1, 4, 99]);
    assert_eq!(live.indices(), &[1, 4]);
    assert!(!live.is_full());
}
