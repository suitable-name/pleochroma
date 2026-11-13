//! The per-tile convergence state of an adaptive render.
//!
//! A tile **converges** when it has at least `min_spp` samples, its error is at most the target
//! and all eight neighbours' errors are at most twice the target (one-tile dilation, against
//! isolated fireflies at tile borders). A converged tile keeps the error it had when it stopped
//! and never reopens.

use super::LiveTiles;

/// Convergence state of one tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileState {
    /// `true` while the tile still receives samples.
    pub live: bool,
    /// Samples per pixel the tile has received.
    pub spp: u32,
    /// The tile's last measured error, in the unit of the noise metric; infinite until measured.
    pub error: f32,
}

/// The state of every tile outside a sparse map's active set: stopped, never sampled, calm.
const STOPPED: TileState = TileState {
    live: false,
    spp: 0,
    error: 0.0,
};

/// Which tiles the stored states belong to.
#[derive(Clone, Debug)]
enum Layout {
    /// One stored state per tile of the frame, in row-major tile order.
    Full,
    /// One stored state per active tile, `members` ascending; every other tile is [`STOPPED`].
    Sparse(Vec<u32>),
}

/// The state of every tile of a frame, in row-major tile order.
///
/// A map is either **full** ([`Self::new`]: a state is stored per tile of the frame) or **sparse**
/// ([`Self::sparse`]: states are stored for the active tiles only and every other tile is stopped
/// with no samples and error 0). Both answer every query for every tile of
/// the frame and compare equal when all their tiles do.
#[derive(Clone, Debug)]
pub struct ConvergenceMap {
    /// The frame geometry; it holds no tile indices.
    frame: LiveTiles,
    layout: Layout,
    states: Vec<TileState>,
}

impl PartialEq for ConvergenceMap {
    fn eq(&self, other: &Self) -> bool {
        self.frame == other.frame
            && (0..self.tile_count()).all(|index| self.state(index) == other.state(index))
    }
}

impl ConvergenceMap {
    /// A map of a `width` x `height` frame cut into `tile`-pixel tiles, every tile live.
    #[must_use]
    pub fn new(width: u32, height: u32, tile: u32) -> Self {
        let frame = LiveTiles::from_indices(width, height, tile, Vec::new());
        let count = frame.tiles_x() as usize * frame.tiles_y() as usize;
        let states = vec![
            TileState {
                live: true,
                spp: 0,
                error: f32::INFINITY,
            };
            count
        ];
        Self {
            frame,
            layout: Layout::Full,
            states,
        }
    }

    /// A sparse map of the same frame that stores states only for the tiles `members`, which are
    /// live and unmeasured. Every other tile reads as stopped with no samples and error 0.
    ///
    /// Memory and time are proportional to `members`, not to the frame, which keeps the beam
    /// residual's per-batch restriction cheap. `members` need not be sorted; out-of-frame entries
    /// are dropped.
    #[must_use]
    pub fn sparse(width: u32, height: u32, tile: u32, members: &[u32]) -> Self {
        let frame = LiveTiles::from_indices(width, height, tile, Vec::new());
        let count = frame.tiles_x() * frame.tiles_y();
        let mut active: Vec<u32> = members
            .iter()
            .copied()
            .filter(|&index| index < count)
            .collect();
        active.sort_unstable();
        active.dedup();
        let states = vec![
            TileState {
                live: true,
                spp: 0,
                error: f32::INFINITY,
            };
            active.len()
        ];
        Self {
            frame,
            layout: Layout::Sparse(active),
            states,
        }
    }

    /// Number of tiles of the frame.
    const fn tile_count(&self) -> u32 {
        self.frame.tiles_x() * self.frame.tiles_y()
    }

    /// How many tile states the map stores: every tile of a full map, the active tiles of a
    /// sparse one.
    #[must_use]
    pub const fn stored_tiles(&self) -> usize {
        self.states.len()
    }

    /// The tile index of the stored state `slot`.
    fn index_of(&self, slot: usize) -> u32 {
        match &self.layout {
            Layout::Full => u32::try_from(slot).unwrap_or(u32::MAX),
            Layout::Sparse(members) => members.get(slot).copied().unwrap_or(u32::MAX),
        }
    }

    /// The stored slot of tile `index`, `None` when the tile has no stored state.
    fn slot_of(&self, index: u32) -> Option<usize> {
        match &self.layout {
            Layout::Full => ((index as usize) < self.states.len()).then_some(index as usize),
            Layout::Sparse(members) => members.binary_search(&index).ok(),
        }
    }

    /// Tiles per row.
    #[must_use]
    pub const fn tiles_x(&self) -> u32 {
        self.frame.tiles_x()
    }

    /// Tile rows.
    #[must_use]
    pub const fn tiles_y(&self) -> u32 {
        self.frame.tiles_y()
    }

    /// The stored tile states in row-major order: every tile of a full map, the active tiles of a
    /// sparse one (see [`Self::stored_tiles`]; use [`Self::state`] for a tile by index).
    #[must_use]
    pub fn states(&self) -> &[TileState] {
        &self.states
    }

    /// The state of tile `index`, or `None` outside the frame.
    #[must_use]
    pub fn state(&self, index: u32) -> Option<&TileState> {
        match &self.layout {
            Layout::Full => self.states.get(index as usize),
            Layout::Sparse(_) => {
                if index >= self.tile_count() {
                    None
                } else {
                    Some(
                        self.slot_of(index)
                            .and_then(|slot| self.states.get(slot))
                            .unwrap_or(&STOPPED),
                    )
                }
            }
        }
    }

    /// Pixel rectangle `(x0, y0, x1, y1)` (end-exclusive, clipped to the frame) of tile `index`.
    #[must_use]
    pub fn tile_rect(&self, index: u32) -> (u32, u32, u32, u32) {
        self.frame.tile_rect(index)
    }

    /// Number of tiles still live.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.states.iter().filter(|state| state.live).count()
    }

    /// `true` when no tile is live.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.live_count() == 0
    }

    /// The live tiles, row-major, for the next pass.
    #[must_use]
    pub fn live_tiles(&self) -> LiveTiles {
        let indices = self
            .states
            .iter()
            .enumerate()
            .filter(|(_, state)| state.live)
            .map(|(slot, _)| self.index_of(slot))
            .collect();
        LiveTiles::from_indices(
            self.frame.width(),
            self.frame.height(),
            self.frame.tile(),
            indices,
        )
    }

    /// Records that every live tile received `spp` more samples per pixel.
    pub fn add_samples(&mut self, spp: u32) {
        for state in self.states.iter_mut().filter(|state| state.live) {
            state.spp = state.spp.saturating_add(spp);
        }
    }

    /// Stores measured errors for tiles; converged tiles keep the error they stopped with.
    pub fn record_errors(&mut self, errors: &[(u32, f32)]) {
        for &(index, error) in errors {
            if let Some(slot) = self.slot_of(index)
                && let Some(state) = self.states.get_mut(slot)
                && state.live
            {
                state.error = error;
            }
        }
    }

    /// Stops every live tile that meets the convergence rule and returns how many stopped.
    ///
    /// The decision reads one snapshot of the errors, so it does not depend on tile order. A
    /// tile stops only with `spp >= min_spp`, `error <= target` and every in-frame neighbour's
    /// error `<= 2 * target`; a `NaN` error never satisfies the rule.
    pub fn converge(&mut self, min_spp: u32, target: f32) -> usize {
        let tiles_x = self.tiles_x() as usize;
        let tiles_y = self.tiles_y() as usize;
        let neighbour_limit = 2.0 * target;
        // One snapshot decides: the slots to stop are collected before any is stopped. A tile
        // without a stored state (outside a sparse map's active set) has error 0.
        let mut to_stop = Vec::new();
        for (slot, state) in self.states.iter().enumerate() {
            if !state.live || state.spp < min_spp || state.error.is_nan() || state.error > target {
                continue;
            }
            let index = self.index_of(slot) as usize;
            let tx = index % tiles_x;
            let ty = index / tiles_x;
            let mut calm = true;
            for ny in ty.saturating_sub(1)..=(ty + 1).min(tiles_y - 1) {
                for nx in tx.saturating_sub(1)..=(tx + 1).min(tiles_x - 1) {
                    if (nx, ny) != (tx, ty) {
                        let error = u32::try_from(ny * tiles_x + nx)
                            .ok()
                            .and_then(|neighbour| self.slot_of(neighbour))
                            .and_then(|found| self.states.get(found))
                            .map_or(0.0, |neighbour| neighbour.error);
                        calm &= error <= neighbour_limit;
                    }
                }
            }
            if calm {
                to_stop.push(slot);
            }
        }
        for &slot in &to_stop {
            if let Some(state) = self.states.get_mut(slot) {
                state.live = false;
            }
        }
        to_stop.len()
    }

    /// Samples per pixel the pixel `(x, y)` has received; `0` outside the frame.
    #[must_use]
    pub fn pixel_spp(&self, x: u32, y: u32) -> u32 {
        if x >= self.frame.width() || y >= self.frame.height() {
            return 0;
        }
        let tile = self.frame.tile();
        let index = (y / tile) * self.frame.tiles_x() + x / tile;
        self.state(index).map_or(0, |state| state.spp)
    }

    /// Total samples traced so far: every tile's `spp` times its pixel count.
    #[must_use]
    pub fn samples_traced(&self) -> u64 {
        self.states
            .iter()
            .enumerate()
            .map(|(slot, state)| {
                let (x0, y0, x1, y1) = self.frame.tile_rect(self.index_of(slot));
                u64::from(state.spp) * u64::from(x1 - x0) * u64::from(y1 - y0)
            })
            .sum()
    }
}
