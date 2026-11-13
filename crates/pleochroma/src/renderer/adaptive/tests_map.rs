//! Tests of the per-tile convergence map.

use super::map::ConvergenceMap;

const TARGET: f32 = 0.01;
const MIN_SPP: u32 = 32;

/// A map of `tiles_x` x `tiles_y` 16-pixel tiles with `spp` samples everywhere and the given
/// per-tile errors (in units of the target).
fn measured(tiles_x: u32, tiles_y: u32, spp: u32, errors_in_targets: &[f32]) -> ConvergenceMap {
    let mut map = ConvergenceMap::new(tiles_x * 16, tiles_y * 16, 16);
    map.add_samples(spp);
    let errors: Vec<(u32, f32)> = errors_in_targets
        .iter()
        .zip(0_u32..)
        .map(|(&error, index)| (index, error * TARGET))
        .collect();
    map.record_errors(&errors);
    map
}

fn live_indices(map: &ConvergenceMap) -> Vec<u32> {
    map.live_tiles().indices().to_vec()
}

#[test]
fn a_new_map_has_every_tile_live_and_unmeasured() {
    let map = ConvergenceMap::new(40, 20, 16);
    assert_eq!((map.tiles_x(), map.tiles_y()), (3, 2));
    assert_eq!(map.live_count(), 6);
    assert!(!map.is_done());
    assert!(
        map.states()
            .iter()
            .all(|s| s.live && s.spp == 0 && s.error.is_infinite())
    );
    assert!(map.live_tiles().is_full());
}

#[test]
fn a_calm_tile_with_enough_samples_stops() {
    let mut map = measured(1, 1, MIN_SPP, &[0.5]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 1);
    assert!(map.is_done());
    assert!(map.live_tiles().is_empty());
}

#[test]
fn no_tile_stops_before_the_minimum_samples() {
    let mut map = measured(2, 1, MIN_SPP - 1, &[0.0, 0.0]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 0);
    assert_eq!(map.live_count(), 2);
}

#[test]
fn a_tile_above_the_target_stays_live() {
    let mut map = measured(1, 1, MIN_SPP, &[1.01]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 0);
    // Exactly at the target is converged.
    let mut at_target = measured(1, 1, MIN_SPP, &[1.0]);
    assert_eq!(at_target.converge(MIN_SPP, TARGET), 1);
}

#[test]
fn a_clean_tile_next_to_a_noisy_one_stays_live() {
    // Five tiles in a row; the last is noisy. Tile 3 touches it, tiles 0..=2 do not.
    let mut map = measured(5, 1, MIN_SPP, &[0.0, 0.0, 0.0, 0.0, 10.0]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 3);
    assert_eq!(live_indices(&map), vec![3, 4]);
}

#[test]
fn the_dilation_includes_the_diagonal_neighbours() {
    // 3 x 3 tiles; the bottom-right one is noisy.
    let mut errors = [0.0_f32; 9];
    errors[8] = 10.0;
    let mut map = measured(3, 3, MIN_SPP, &errors);
    map.converge(MIN_SPP, TARGET);
    let live = live_indices(&map);
    // The tiles touching tile 8 (5, 7 and the diagonal 4) stay, so does tile 8 itself.
    assert_eq!(live, vec![4, 5, 7, 8]);
}

#[test]
fn neighbours_up_to_twice_the_target_do_not_block() {
    // Tile 0 is at half the target; its neighbour at 1.5 targets is above its own target
    // (so tile 1 stays) but within 2x (so tile 0 stops).
    let mut map = measured(2, 1, MIN_SPP, &[0.5, 1.5]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 1);
    assert_eq!(live_indices(&map), vec![1]);
    // Above 2x blocks.
    let mut blocked = measured(2, 1, MIN_SPP, &[0.5, 2.5]);
    assert_eq!(blocked.converge(MIN_SPP, TARGET), 0);
}

#[test]
fn neighbours_outside_the_frame_are_ignored() {
    let mut map = measured(1, 1, MIN_SPP, &[0.0]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 1);
    let mut corner = measured(2, 2, MIN_SPP, &[0.0, 0.0, 0.0, 0.0]);
    assert_eq!(corner.converge(MIN_SPP, TARGET), 4);
}

#[test]
fn an_unmeasured_or_nan_neighbour_blocks_convergence() {
    let mut map = ConvergenceMap::new(32, 16, 16);
    map.add_samples(MIN_SPP);
    map.record_errors(&[(0, 0.0)]);
    // Tile 1 was never measured (infinite error).
    assert_eq!(map.converge(MIN_SPP, TARGET), 0);
    map.record_errors(&[(1, f32::NAN)]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 0);
}

#[test]
fn the_decision_does_not_depend_on_tile_order() {
    // Two calm tiles next to each other both stop in the same check, even though each is the
    // other's neighbour.
    let mut map = measured(2, 1, MIN_SPP, &[0.9, 0.9]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 2);
}

#[test]
fn a_converged_tile_never_reopens_and_keeps_its_error() {
    let mut map = measured(2, 1, MIN_SPP, &[0.2, 0.4]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 2);
    let stopped_error = map.states()[0].error;

    // Later, noisy measurements and more samples change nothing for stopped tiles.
    map.add_samples(64);
    map.record_errors(&[(0, 50.0), (1, 50.0)]);
    assert_eq!(map.converge(MIN_SPP, TARGET), 0);
    assert!(map.is_done());
    assert_eq!(map.states()[0].error.to_bits(), stopped_error.to_bits());
    assert_eq!(map.states()[0].spp, MIN_SPP);
}

#[test]
fn a_stopped_neighbour_keeps_blocking_with_its_old_error() {
    // Tile 0 stops at once. Tile 1 is held by the noisy tile 2, and tile 0, now stopped, still
    // counts as a calm neighbour with the error it stopped at.
    let mut map = measured(3, 1, MIN_SPP, &[0.9, 0.9, 5.0]);
    map.converge(MIN_SPP, TARGET);
    assert_eq!(live_indices(&map), vec![1, 2]);
    // Tile 2 improves; tile 1 (live, neighbour 0 stopped at 0.9) can now stop, then tile 2.
    map.record_errors(&[(1, 0.5 * TARGET), (2, 0.5 * TARGET)]);
    map.converge(MIN_SPP, TARGET);
    assert!(map.is_done());
}

#[test]
fn pixel_counts_and_samples_traced_follow_the_tiles() {
    // 40 x 20 with 16-pixel tiles: tiles of 256, 256, 128 / 64, 64, 32 pixels.
    let mut map = ConvergenceMap::new(40, 20, 16);
    map.add_samples(10);
    map.record_errors(&[(0, 0.0), (1, 0.0), (3, 0.0), (4, 0.0)]);
    // Tiles 2 and 5 (right edge) are unmeasured, so they block their neighbours 1 and 4; only
    // tiles 0 and 3 stop.
    assert_eq!(map.converge(10, TARGET), 2);
    map.add_samples(30);
    assert_eq!(map.pixel_spp(0, 0), 10);
    assert_eq!(map.pixel_spp(20, 0), 40);
    assert_eq!(map.pixel_spp(39, 19), 40);
    assert_eq!(map.pixel_spp(40, 0), 0);
    let expected = 10 * (256 + 64) + 40 * (256 + 128 + 64 + 32);
    assert_eq!(map.samples_traced(), expected);
}
