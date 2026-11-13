//! Counters of a beam render (beam guide §13.6, §14.5): beams, leaves, fragments, drops by reason
//! and the depth histogram, summed per tile and merged in tile order.

/// Why energy left the beams for the residual (or for nothing, at `max_bounces`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DropReason {
    /// The beam's throughput fell below the cutoff (§12.1).
    Cutoff,
    /// A polygon became a sliver below `sliver_area_px2`.
    Sliver,
    /// The split depth cap was reached.
    DepthCap,
    /// An expansion mode fell below `MIN_PROBABILITY` (`expand_reporting`).
    MinProbability,
    /// The fragment subdivision cap was reached without a smooth fit.
    FragmentCap,
    /// The interpolation check failed at the depth cap.
    Interpolation,
    /// A child of the shared expansion diverged (§8.5) where the interval cannot split further.
    Diverged,
    /// Boundary snapping found no bracket after the allowed re-partitions (§7.4 step 8).
    BracketFailure,
    /// A beam came within `OPTIC_AXIS_GUARD_RAD` of an optic axis (§10.2).
    OpticAxis,
}

/// The number of [`DropReason`]s.
pub const DROP_REASONS: usize = 9;

impl DropReason {
    /// The counter slot of this reason.
    #[must_use]
    pub const fn slot(self) -> usize {
        match self {
            Self::Cutoff => 0,
            Self::Sliver => 1,
            Self::DepthCap => 2,
            Self::MinProbability => 3,
            Self::FragmentCap => 4,
            Self::Interpolation => 5,
            Self::Diverged => 6,
            Self::BracketFailure => 7,
            Self::OpticAxis => 8,
        }
    }
}

/// The deepest bounce count the histogram resolves; deeper beams land in the last bin.
pub const DEPTH_BINS: usize = 32;

/// Counters of a beam render; `Default` is all zero.
#[derive(Clone, Debug, PartialEq)]
pub struct BeamStats {
    /// Primary beams (front facets clipped to tiles).
    pub primary_beams: u64,
    /// Every beam created, primaries included.
    pub beams: u64,
    /// Leaves (before the GPU pre-split).
    pub leaves: u64,
    /// Fragments shaded (pixel pieces, before lighting subdivision).
    pub fragments: u64,
    /// Fragment pieces after lighting subdivision.
    pub fragment_pieces: u64,
    /// Fragments that took the exact sun-disc path.
    pub sun_disc_fragments: u64,
    /// Leaves split for the GPU's vertex cap.
    pub gpu_presplits: u64,
    /// Count of drops per [`DropReason::slot`].
    pub drops: [u64; DROP_REASONS],
    /// Dropped energy per [`DropReason::slot`] (image-area-weighted mean intensity).
    pub dropped_energy: [f64; DROP_REASONS],
    /// Beams per bounce count.
    pub depth_histogram: [u64; DEPTH_BINS],
    /// Entry refraction pieces (child beams after the apex fit, §8.3).
    pub entry_pieces: u64,
    /// Entry splits for the apex fit or the interpolation check.
    pub apex_splits: u64,
    /// Partitions by exit facet (§7.2).
    pub partitions: u64,
    /// Re-partitions after a conservation or bracket failure.
    pub repartitions: u64,
    /// Snapped boundary vertices (endpoints, junctions, chord points, §7.4).
    pub snapped_vertices: u64,
    /// Bisections whose ends classified alike or inconsistently (§7.4 step 8).
    pub bracket_failures: u64,
    /// TIR curve splits (§8.5).
    pub tir_splits: u64,
    /// Spectral interval splits per `IntervalSplit::slot` (§9.2).
    pub interval_splits: [u64; super::beam::INTERVAL_SPLIT_CAUSES],
    /// Image area (px²) of the end-node bands left to the residual (§9.2), for the band share.
    pub residual_band_area: f64,
    /// The worst exit-direction step between neighbouring nodes in any leaf, in radians (§9.3).
    pub worst_node_step: f64,
    /// Mode children created at birefringent interactions (§10.1, §10.3).
    pub mode_children: u64,
    /// Apex refits of non-mirror reflections (e-ray, biaxial, §10.1).
    pub apex_refits: u64,
    /// Topology splits inside a birefringent beam (critical curves, relabelling flips, §10.2).
    pub mode_splits: u64,
    /// Every beam created in the tile, accepted or not, including those of a discarded spectral
    /// interval attempt; counts against [`TILE_ALLOWANCE`].
    pub attempted_beams: u64,
    /// Every entry piece, interaction piece and exit leaf attempted in the tile, accepted or not,
    /// including those of a discarded spectral interval attempt; counts against
    /// [`TILE_ALLOWANCE`].
    pub attempted_pieces: u64,
    /// Exit leaves with a node that could not be recovered (its centre weight underflowed while a
    /// sub-node is bright); the node is left out of every kept region and the residual samples it.
    /// Not a drop: no energy is known for it.
    pub lost_nodes: u64,
    /// Partitions whose pieces did not tile the polygon within the conservation tolerance after
    /// the retry; kept apart from `bracket_failures` so a log tells the two causes apart.
    pub conservation_failures: u64,
    /// Silent gaps the residual covers: front facets partly behind the camera, tile-clip slivers
    /// and sliver energies lost with a failed interaction call. Not drops: no energy is known for
    /// them.
    pub silent_gaps: u64,
    /// Image area (px²) of the silent gaps that have one (tile-clip slivers).
    pub silent_gap_area: f64,
}

/// The attempted work (see [`BeamStats::attempted_work`]) one tile may spend before the beams on
/// its stack go to the residual.
///
/// About ten times the attempted work of the measured 12-bounce rows (roughly 46k units per 8 px
/// tile), so no measured tile reaches it, while a runaway tile stops after bounded work. The count
/// is a function of the geometry only, never of timing or thread count.
pub const TILE_ALLOWANCE: u64 = 500_000;

impl Default for BeamStats {
    fn default() -> Self {
        Self {
            primary_beams: 0,
            beams: 0,
            leaves: 0,
            fragments: 0,
            fragment_pieces: 0,
            sun_disc_fragments: 0,
            gpu_presplits: 0,
            drops: [0; DROP_REASONS],
            dropped_energy: [0.0; DROP_REASONS],
            depth_histogram: [0; DEPTH_BINS],
            entry_pieces: 0,
            apex_splits: 0,
            partitions: 0,
            repartitions: 0,
            snapped_vertices: 0,
            bracket_failures: 0,
            tir_splits: 0,
            interval_splits: [0; super::beam::INTERVAL_SPLIT_CAUSES],
            residual_band_area: 0.0,
            worst_node_step: 0.0,
            mode_children: 0,
            apex_refits: 0,
            mode_splits: 0,
            attempted_beams: 0,
            attempted_pieces: 0,
            lost_nodes: 0,
            conservation_failures: 0,
            silent_gaps: 0,
            silent_gap_area: 0.0,
        }
    }
}

impl BeamStats {
    /// Records one drop.
    pub fn record_drop(&mut self, reason: DropReason, energy: f64) {
        self.drops[reason.slot()] += 1;
        self.dropped_energy[reason.slot()] += energy;
    }

    /// Records one beam at `bounces` interactions.
    pub fn record_beam(&mut self, bounces: u32) {
        self.beams += 1;
        self.attempted_beams += 1;
        let bin = (bounces as usize).min(DEPTH_BINS - 1);
        self.depth_histogram[bin] += 1;
    }

    /// Adds `other` into `self`. Merging in tile order keeps the f64 sums deterministic.
    pub fn merge(&mut self, other: &Self) {
        self.primary_beams += other.primary_beams;
        self.beams += other.beams;
        self.leaves += other.leaves;
        self.fragments += other.fragments;
        self.fragment_pieces += other.fragment_pieces;
        self.sun_disc_fragments += other.sun_disc_fragments;
        self.gpu_presplits += other.gpu_presplits;
        for (a, b) in self.drops.iter_mut().zip(other.drops) {
            *a += b;
        }
        for (a, b) in self.dropped_energy.iter_mut().zip(other.dropped_energy) {
            *a += b;
        }
        for (a, b) in self.depth_histogram.iter_mut().zip(other.depth_histogram) {
            *a += b;
        }
        self.entry_pieces += other.entry_pieces;
        self.apex_splits += other.apex_splits;
        self.partitions += other.partitions;
        self.repartitions += other.repartitions;
        self.snapped_vertices += other.snapped_vertices;
        self.bracket_failures += other.bracket_failures;
        self.tir_splits += other.tir_splits;
        for (a, b) in self.interval_splits.iter_mut().zip(other.interval_splits) {
            *a += b;
        }
        self.residual_band_area += other.residual_band_area;
        self.worst_node_step = self.worst_node_step.max(other.worst_node_step);
        self.mode_children += other.mode_children;
        self.apex_refits += other.apex_refits;
        self.mode_splits += other.mode_splits;
        self.attempted_beams += other.attempted_beams;
        self.attempted_pieces += other.attempted_pieces;
        self.lost_nodes += other.lost_nodes;
        self.conservation_failures += other.conservation_failures;
        self.silent_gaps += other.silent_gaps;
        self.silent_gap_area += other.silent_gap_area;
    }

    /// Records one entry piece, interaction piece or exit leaf attempted.
    pub const fn record_attempt(&mut self) {
        self.attempted_pieces += 1;
    }

    /// Zeroed counters that carry `self`'s attempted work, for a spectral interval attempt whose
    /// other counters may be discarded.
    #[must_use]
    pub fn carrying_attempts(&self) -> Self {
        Self {
            attempted_beams: self.attempted_beams,
            attempted_pieces: self.attempted_pieces,
            ..Self::default()
        }
    }

    /// Takes over the attempted work of `attempt`, which started from
    /// [`BeamStats::carrying_attempts`] of `self`; kept even when the attempt's leaves and other
    /// counters are discarded.
    pub const fn adopt_attempts(&mut self, attempt: &Self) {
        self.attempted_beams = attempt.attempted_beams;
        self.attempted_pieces = attempt.attempted_pieces;
    }

    /// The attempted work of the tile so far: beams created plus pieces and leaves attempted.
    #[must_use]
    pub const fn attempted_work(&self) -> u64 {
        self.attempted_beams.saturating_add(self.attempted_pieces)
    }

    /// `true` once the tile has spent its [`TILE_ALLOWANCE`]; what is still pending goes to the
    /// residual as a depth-cap drop.
    #[must_use]
    pub const fn allowance_spent(&self) -> bool {
        self.allowance_spent_at(TILE_ALLOWANCE)
    }

    /// `true` once the tile's attempted work reached `allowance` (see
    /// [`BeamStats::allowance_spent`]).
    #[must_use]
    pub const fn allowance_spent_at(&self, allowance: u64) -> bool {
        self.attempted_work() >= allowance
    }

    /// One log line: `beams 1234, leaves 567, fragments 8901, drops 12`.
    #[must_use]
    pub fn summary(&self) -> String {
        let drops: u64 = self.drops.iter().sum();
        format!(
            "beams {}, leaves {}, fragments {}, drops {drops}",
            self.beams, self.leaves, self.fragments
        )
    }
}
