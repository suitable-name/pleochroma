//! Tests of the adaptive positions and of whole tiles.

use super::{
    ExitRecord, TileBuilder,
    build::TraceKit,
    layout::{TileCache, tile_rect},
    positions::{BuildSettings, K_MAX, K0, build_pixel},
    tests_support::{Rig, glass},
};
use crate::{optics::raytracer::transport::PreparedMaterial, renderer::whitted::CUTOFF};

/// A 12 x 12 frame of a cube seen from above: background, interior and edge pixels.
const SIZE: u32 = 12;

/// Positions per pixel of the whole frame, row-major.
fn position_counts(settings: BuildSettings) -> Vec<usize> {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let prepared = PreparedMaterial::new(&rig.material);
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (scene.max_bounces, CUTOFF),
        settings.dispersion_gate,
    );
    let mut counts = Vec::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            counts.push(build_pixel(&kit, &scene, (x, y), &settings).len());
        }
    }
    counts
}

#[test]
fn an_edge_pixel_gets_more_positions_than_an_interior_pixel() {
    let counts = position_counts(BuildSettings {
        k_max: 16,
        ..BuildSettings::default()
    });
    let stone: Vec<usize> = counts.iter().copied().filter(|&c| c > 0).collect();
    assert!(!stone.is_empty(), "the cube covers part of the frame");
    assert!(counts.contains(&0), "the frame has background pixels");
    let fewest = stone.iter().copied().min().unwrap_or(0);
    let most = stone.iter().copied().max().unwrap_or(0);
    assert_eq!(
        fewest, K0 as usize,
        "an interior pixel keeps its first level"
    );
    assert!(most > fewest, "an edge pixel refines: {most} vs {fewest}");
    assert!(most <= 16);
}

#[test]
fn positions_are_capped_at_k_max() {
    let counts = position_counts(BuildSettings {
        k_max: 8,
        target_noise: 1.0e-9,
        ..BuildSettings::default()
    });
    assert!(counts.iter().all(|&c| c <= 8));
    assert!(counts.contains(&8), "an edge pixel reaches the cap");
    const {
        assert!(K_MAX >= 8);
    }

    let default_counts = position_counts(BuildSettings::default());
    assert!(default_counts.iter().all(|&c| c <= K_MAX as usize));
}

#[test]
fn tile_layout_is_consistent_and_non_stone_pixels_have_no_positions() {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let prepared = PreparedMaterial::new(&rig.material);
    let settings = BuildSettings {
        tile: 8,
        k_max: 8,
        ..BuildSettings::default()
    };
    let builder = TileBuilder::new(&scene, &prepared, &rig.soa, settings);
    assert_eq!(builder.tile_count(), 4);

    let mut any_stone = false;
    for index in 0..builder.tile_count() {
        let tile = builder.build_tile(index);
        let (x0, y0, x1, y1) = tile_rect(SIZE, SIZE, 8, index);
        let header = tile.header();
        assert_eq!(header.tile_index, index);
        assert_eq!(header.pixel_count, (x1 - x0) * (y1 - y0));
        assert_eq!(tile.pixel_offsets().len(), header.pixel_count as usize + 1);
        assert_eq!(tile.pixel_offsets().first(), Some(&0));
        assert_eq!(tile.pixel_offsets().last(), Some(&header.position_count));
        assert_eq!(
            tile.position_offsets().len(),
            header.position_count as usize + 1
        );
        assert_eq!(tile.position_offsets().last(), Some(&header.record_count));
        assert_eq!(tile.records().len(), header.record_count as usize);
        assert_eq!(
            TileCache::from_words(tile.as_words().to_vec()).expect("a valid buffer"),
            tile
        );
        for position in 0..header.position_count as usize {
            assert!(tile.position_residual(position) >= 0.0);
            assert_ne!(tile.position_records(position), [] as [ExitRecord; 0]);
        }
        any_stone |= header.position_count > 0;
    }
    assert!(any_stone);

    // The frame corner is background: tile 0, pixel 0.
    let first = builder.build_tile(0);
    assert!(first.pixel_positions(0).is_empty());
}

#[test]
fn building_a_tile_twice_gives_byte_identical_buffers() {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let prepared = PreparedMaterial::new(&rig.material);
    let settings = BuildSettings {
        tile: 8,
        k_max: 8,
        ..BuildSettings::default()
    };
    let first = TileBuilder::new(&scene, &prepared, &rig.soa, settings).build_tile(3);
    let second = super::build_tile(&scene, &prepared, &rig.soa, 3, &settings);
    assert!(first.header().position_count > 0);
    assert_eq!(first.as_words(), second.as_words());
}

#[test]
fn a_tile_past_the_last_one_is_empty() {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let prepared = PreparedMaterial::new(&rig.material);
    let tile = super::build_tile(&scene, &prepared, &rig.soa, 99, &BuildSettings::default());
    assert_eq!(tile.header().pixel_count, 0);
    assert_eq!(tile.header().position_count, 0);
    assert!(TileCache::from_words(tile.as_words().to_vec()).is_ok());
}

#[test]
fn stone_positions_carry_energy_and_never_more_than_the_light_they_receive() {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(SIZE, 8);
    let prepared = PreparedMaterial::new(&rig.material);
    let builder = TileBuilder::new(
        &scene,
        &prepared,
        &rig.soa,
        BuildSettings {
            tile: SIZE,
            k_max: 8,
            ..BuildSettings::default()
        },
    );
    let tile = builder.build_tile(0);
    let mut checked = 0;
    for position in 0..tile.header().position_count as usize {
        let kept: f32 = tile
            .position_records(position)
            .iter()
            .map(|record| record.weights().iter().sum::<f32>() / 8.0)
            .sum();
        let total = kept + tile.position_residual(position);
        assert!(total <= 1.0 + 1.0e-2, "position {position}: {total}");
        assert!(total > 0.5, "position {position}: {total}");
        checked += 1;
    }
    assert!(checked > 0);
}
