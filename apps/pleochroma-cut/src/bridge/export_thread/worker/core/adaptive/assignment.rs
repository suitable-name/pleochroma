//! Which engine traces which tile of an adaptive render: [`TileAssignment`].
//!
//! A render with a remote lane has more than one engine -- this computer and one or more remote
//! workers -- and every pass splits its live tiles between them **by tile**. The split is sticky:
//! a tile keeps its engine for the whole render (so a worker that holds per-tile state, such as a
//! path cache, keeps hitting it) unless that engine fails or leaves, and then the tile moves,
//! deterministically, to the next engine.
//!
//! # The rule
//!
//! Engines are numbered in a fixed order (this computer first when it takes part, then the remote
//! workers in the order they were configured). Tile `i` starts on engine `i % engines`: no state,
//! no dependence on timing, and a mix of stone and background tiles on every engine, which keeps
//! the engines about equally loaded without measuring them. When engine `e` fails, every tile it
//! owns moves to the first living engine after `e` in the cyclic order; a tile never moves for any
//! other reason, so what the other engines trace does not change.
//!
//! The image does not depend on the assignment at all (a pixel's samples are a function of the
//! pixel and the absolute sample index alone, whichever engine traces them); the assignment only
//! decides who does the work, and is deterministic so a run can be reproduced.

/// Which engine owns each tile of the frame, and which engines are still alive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileAssignment {
    /// Per tile (row-major), the index of the engine that traces it.
    owner: Vec<usize>,
    /// Per engine, whether it can still take tiles.
    alive: Vec<bool>,
}

impl TileAssignment {
    /// The assignment of `tiles` tiles over `engines` engines: tile `i` on engine `i % engines`.
    ///
    /// Zero engines gives an assignment that owns nothing.
    #[must_use]
    pub fn new(tiles: usize, engines: usize) -> Self {
        let owner = if engines == 0 {
            Vec::new()
        } else {
            (0..tiles).map(|tile| tile % engines).collect()
        };
        Self {
            owner,
            alive: vec![true; engines],
        }
    }

    /// The number of engines, living or not.
    #[cfg(test)]
    #[must_use]
    pub const fn engines(&self) -> usize {
        self.alive.len()
    }

    /// Whether engine `engine` can still take tiles.
    #[must_use]
    pub fn is_alive(&self, engine: usize) -> bool {
        self.alive.get(engine).copied().unwrap_or(false)
    }

    /// The engine that owns `tile`, or `None` for a tile outside the frame.
    #[must_use]
    pub fn owner(&self, tile: u32) -> Option<usize> {
        self.owner.get(tile as usize).copied()
    }

    /// Splits the live `tiles` (ascending tile indices) by owner: element `e` is the tiles engine
    /// `e` traces this pass, still ascending. A tile outside the frame is dropped.
    #[must_use]
    pub fn split(&self, tiles: &[u32]) -> Vec<Vec<u32>> {
        let mut parts = vec![Vec::new(); self.alive.len()];
        for &tile in tiles {
            if let Some(engine) = self.owner(tile)
                && let Some(part) = parts.get_mut(engine)
            {
                part.push(tile);
            }
        }
        parts
    }

    /// Takes `engine` out: it receives no more tiles, and every tile it owns moves to the first
    /// living engine after it in the cyclic order.
    ///
    /// Returns the engine that took the tiles, or `None` when no engine is left alive (the
    /// tiles then stay where they are and the caller has to end the render). Failing an engine
    /// that is already out, or one that does not exist, changes nothing and returns the engine
    /// its tiles would go to.
    pub fn fail(&mut self, engine: usize) -> Option<usize> {
        if let Some(alive) = self.alive.get_mut(engine) {
            *alive = false;
        }
        let engines = self.alive.len();
        let heir = (1..=engines)
            .map(|step| (engine + step) % engines.max(1))
            .find(|&candidate| self.is_alive(candidate))?;
        for owner in &mut self.owner {
            if *owner == engine {
                *owner = heir;
            }
        }
        Some(heir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(tiles: usize) -> Vec<u32> {
        (0..u32::try_from(tiles).unwrap()).collect()
    }

    #[test]
    fn tiles_start_on_the_engine_their_index_selects() {
        let assignment = TileAssignment::new(7, 3);
        let owners: Vec<usize> = (0..7).map(|t| assignment.owner(t).unwrap()).collect();
        assert_eq!(owners, [0, 1, 2, 0, 1, 2, 0]);
        assert_eq!(assignment.owner(7), None, "outside the frame");
        assert_eq!(assignment.engines(), 3);
    }

    #[test]
    fn a_pass_is_split_by_owner_and_every_live_tile_goes_to_exactly_one_engine() {
        let assignment = TileAssignment::new(10, 2);
        let split = assignment.split(&all(10));
        assert_eq!(split, vec![vec![0, 2, 4, 6, 8], vec![1, 3, 5, 7, 9]]);
        let mut merged: Vec<u32> = split.concat();
        merged.sort_unstable();
        assert_eq!(merged, all(10));
    }

    #[test]
    fn the_split_is_sticky_as_tiles_stop_and_the_live_set_shrinks() {
        let assignment = TileAssignment::new(12, 3);
        let first = assignment.split(&all(12));
        // Tiles 0, 4, 5 and 9 converged; the others stay live.
        let live: Vec<u32> = all(12)
            .into_iter()
            .filter(|t| ![0, 4, 5, 9].contains(t))
            .collect();
        let later = assignment.split(&live);
        for (engine, tiles) in later.iter().enumerate() {
            for tile in tiles {
                assert!(
                    first[engine].contains(tile),
                    "tile {tile} stays on engine {engine}"
                );
            }
        }
        assert_eq!(later[0], vec![3, 6]);
    }

    #[test]
    fn a_failed_engines_tiles_move_to_the_next_living_engine_and_nothing_else_moves() {
        let mut assignment = TileAssignment::new(9, 3);
        let before: Vec<usize> = (0..9).map(|t| assignment.owner(t).unwrap()).collect();
        assert_eq!(assignment.fail(1), Some(2));
        assert!(!assignment.is_alive(1));
        for tile in 0..9 {
            let now = assignment.owner(tile).unwrap();
            let was = before[tile as usize];
            if was == 1 {
                assert_eq!(now, 2, "tile {tile} moved to the next engine");
            } else {
                assert_eq!(now, was, "tile {tile} did not move");
            }
        }
        // Then the engine that inherited them fails too: the cyclic order wraps past the dead
        // engine 1 to engine 0.
        assert_eq!(assignment.fail(2), Some(0));
        assert!((0..9).all(|t| assignment.owner(t) == Some(0)));
        // The last engine cannot hand its tiles on.
        assert_eq!(assignment.fail(0), None);
    }

    #[test]
    fn two_engines_failing_in_one_round_leave_all_their_tiles_with_the_survivor() {
        // Engines 1 and 2 of three fail in one round, in reply order: engine 1's tiles first go to
        // engine 2, which then fails too. Asking the assignment afterwards (not the heir returned
        // at the time) names engine 0 for every tile of both.
        let mut assignment = TileAssignment::new(9, 3);
        let before = assignment.split(&all(9));
        let mut failed_tiles: Vec<u32> = [before[1].clone(), before[2].clone()].concat();
        failed_tiles.sort_unstable();
        assert_eq!(assignment.fail(1), Some(2), "the heir at that moment");
        assert_eq!(assignment.fail(2), Some(0));
        let next = assignment.split(&failed_tiles);
        assert_eq!(next[0], failed_tiles, "the survivor takes all of them");
        assert!(next[1].is_empty() && next[2].is_empty(), "{next:?}");
    }

    #[test]
    fn failing_an_engine_twice_or_a_missing_one_changes_nothing() {
        let mut assignment = TileAssignment::new(6, 2);
        assert_eq!(assignment.fail(1), Some(0));
        let after = assignment.clone();
        assert_eq!(assignment.fail(1), Some(0));
        assert_eq!(assignment, after);
        assert_eq!(assignment.fail(5), Some(0));
        assert_eq!(assignment, after);
    }

    #[test]
    fn no_engine_owns_nothing() {
        let assignment = TileAssignment::new(4, 0);
        assert_eq!(assignment.split(&all(4)), [] as [Vec<u32>; 0]);
        assert_eq!(assignment.owner(0), None);
    }

    #[test]
    fn the_same_failure_gives_the_same_assignment() {
        let run = || {
            let mut assignment = TileAssignment::new(20, 4);
            assignment.fail(2);
            assignment.fail(0);
            assignment
        };
        assert_eq!(run(), run());
    }
}
