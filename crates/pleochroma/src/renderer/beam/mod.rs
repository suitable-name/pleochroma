//! Beam tracing of faceted stones (beam guide v1.4).
//!
//! Beams are convex polygons on a facet plane with an apex and an image homography. The CPU
//! builds the beam trees, the CPU or the GPU shades the leaves, and a residual of path samples
//! with the reference tracer covers everything the beams leave out, so the image converges to
//! the path tracer's (guide §1, §12.3). Compiled only with the `beam` cargo feature.

pub mod aliasing;
#[expect(
    clippy::module_inception,
    reason = "beam::beam holds the Beam type and its kin, as the guide's module tree names it"
)]
pub mod beam;
pub mod birefringence;
pub mod engines;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
pub mod engines_test_hooks;
pub mod entry;
pub mod frame;
pub mod geom;
pub mod interact;
pub mod leaf;
pub mod leaf_cache;
pub mod lighting;
pub mod pipeline;
pub mod primary;
pub mod propagate;
pub mod raster;
pub mod residual;
pub mod retrace;
pub mod snap;
pub mod spectral;
pub mod stats;
pub mod stone;
pub mod subnodes;
pub mod tile_residual;

pub use super::accel_beam::{
    BeamOff, BeamSettings, DEFAULT_BEAM_MEMORY_CAP_BYTES, EXPORT_LEAF_CACHE_BYTES, RegionMode,
    ResidualMode, VIEWPORT_LEAF_CACHE_BYTES,
};

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d1_equivalence;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d1_lighting;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d1_raster;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d2_equivalence;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d2a_entry;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d2b1_propagate;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d2b2_snap;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d2b3_interact;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d3_equivalence;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d3_intervals;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d3_subnodes;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d4_biaxial;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d4_uniaxial;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d5_hybrid;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d5_residual;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d6_cost;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d6_persist;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d6_relight;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d7_audit;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d7_failures;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d7_stress;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_retrace;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_stone;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
pub mod tests_support;

// The phase lives in `accel_beam` (compiled in every build), so progress types of crates built
// without the `beam` feature (the render jobs) can carry it.
pub use crate::renderer::accel_beam::BeamPhase;

/// The progress of a beam render: `done = traced_fraction · w_tree + finished_fraction · (1 −
/// w_tree)`.
///
/// It is monotone by construction, with `w_tree` measured on the first batch and then fixed
/// (§12.4).
///
/// `finished_fraction` counts the batch in progress by the share of its residual budget its engine
/// has traced so far, reported after every adaptive pass. One long batch then moves the bar
/// instead of leaving it still until the batch ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeamProgress {
    /// The current phase.
    pub phase: BeamPhase,
    /// The share done, 0 to 1, never decreasing.
    pub done: f32,
    /// The estimated time left, `elapsed · (1 − done) / done`; `None` before the first batch is
    /// finished and while `done` is below 2 %.
    pub eta: Option<std::time::Duration>,
}

/// The progress note of a beam render: "Beams: 142318 leaves, 0.8 % residual".
///
/// The leaf count is printed exact, not abbreviated, so readers get both numbers. Readers also
/// accept the older `142k` form.
#[must_use]
pub fn beam_note(leaves: u64, residual_share: f64) -> String {
    format!(
        "Beams: {leaves} leaves, {:.1} % residual",
        residual_share * 100.0
    )
}

/// The note when beams and the path cache were both asked for: beams win on eligible
/// stones (§14.6).
pub const NOTE_BEAMS_REPLACE_PATH_CACHE: &str =
    "Beam tracing renders this stone; the path cache is not used for it";
/// The note when the GPU-resident convergence was asked for with beams (§12.4).
pub const NOTE_BEAMS_HOST_CONVERGENCE: &str =
    "Beam tracing checks convergence on the host; GPU-resident convergence is not used";
/// The note when remote rendering was chosen: beam tracing then renders on this computer.
pub const NOTE_BEAMS_LOCAL_ONLY: &str = "Beam tracing renders on this computer";

/// The hook that receives each finished batch's tiles and final pixel means.
pub type BatchHook<'h> = &'h mut (dyn FnMut(&[primary::TileRect], &[glam::Vec3]) + 'h);

/// The per-run options and hooks of `cpu_frame::render_beam_hybrid_surface` (beam guide §12.4,
/// §14.8–14.10).
///
/// `BeamRun::new(&mut progress)` is a whole-frame render with no hooks.
pub struct BeamRun<'h> {
    /// Render only these tiles (a remote worker's request, §14.8); `None` renders the frame.
    /// Pixels outside them stay zero in the returned image, with zero counts.
    pub tiles: Option<&'h crate::renderer::adaptive::LiveTiles>,
    /// Two-phase progress (§12.4).
    pub progress: &'h mut dyn FnMut(BeamProgress),
    /// Each finished batch's tiles and final pixel means (`engines::BATCH_PIXEL_ORDER`), for the
    /// viewport (§14.9).
    pub on_batch: Option<BatchHook<'h>>,
    /// Each finished batch's leaves and kept index instead of freeing them, for the leaf cache
    /// (§14.10).
    pub retain: Option<&'h mut dyn FnMut(leaf_cache::RetainedBatch)>,
    /// Kept beams of an earlier render of the same pose (§14.10): a batch whose tiles are all in
    /// this cache skips its tree and uses the stored leaves, then runs the fragments and the full
    /// adaptive residual exactly as a fresh render does. Batches the cache lacks are traced. The
    /// cache must come from the same stone, pose and beam settings (lighting may differ); such
    /// batches are not handed to `retain` again, they are already kept.
    pub preloaded: Option<&'h leaf_cache::LeafCache>,
}

impl<'h> BeamRun<'h> {
    /// A whole-frame run reporting to `progress`, without hooks.
    pub fn new(progress: &'h mut dyn FnMut(BeamProgress)) -> Self {
        Self {
            tiles: None,
            progress,
            on_batch: None,
            retain: None,
            preloaded: None,
        }
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod beam_note_tests {
    use super::beam_note;

    #[test]
    fn the_note_prints_the_exact_leaf_count() {
        assert_eq!(
            beam_note(142_318, 0.008),
            "Beams: 142318 leaves, 0.8 % residual"
        );
        assert_eq!(beam_note(7, 0.0), "Beams: 7 leaves, 0.0 % residual");
    }
}
