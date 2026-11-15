//! Meet-point solver.
//!
//! Derives `GemCAD` "mast" (facet plane offset) distances from angles, index positions,
//! gear, and meet constraints. This is the inverse of the forward plane construction in
//! [`super::cuts`].
//!
//! # The meet model: vertex incidence, not tangency
//!
//! Every facet plane is `n . x = mast`, with `n` fixed by angle, index and gear. The
//! format means **vertex incidence**, not tangency (cutting inward until first touch,
//! `mast = max_v n . v`). The recorded mast is almost never that maximum. Instead, each
//! facet is cut past first touch until its plane passes through a vertex of the other
//! facets' arrangement. Among the candidate vertex levels ordered by `n . v` descending,
//! the true one is usually the first level past first touch, and within the first three
//! in most cases.
//!
//! # What meets alone cannot determine: per-block anchors
//!
//! Some continuous degrees of freedom preserve every vertex incidence. Girdle planes are
//! vertical (`n.y = 0`), so shifting a whole crown or pavilion block, or scaling the
//! girdle radius, coherently is invisible to meets. These are the dimensions a designer
//! chooses and a printed diagram states (`C/W`, `P/W`, `H/W`). A caller must therefore
//! supply one [`MeetConstraint::ScaleReference`] per block that the schedule does not
//! anchor; the masts within each block are then meet-derived from that anchor.
//! [`meet_tier_inputs_from_asc`] classifies stated anchors. [`apply_ratio_anchors`] fills
//! in the rest from printed `C/W`/`P/W` proportions. That source has real residual error,
//! so prefer real recorded masts when available.
//!
//! # Solving strategy
//!
//! Which vertex a tier's plane passes through depends on where every *other*
//! tier's plane sits, and real schedules can have genuinely mutual dependencies.
//! The solve runs in three phases:
//!
//! 1. **Constructive pass in file order** ([`SolveStrategy::DependencyOrder`]):
//!    tiers settle one at a time in schedule order, each against the arrangement
//!    of everything settled so far -- the shallowest candidate vertex level
//!    incident to every resolved `"Meet <names>"` reference when the schedule
//!    states one, the rank-1 level otherwise. A tier that cannot settle yet is
//!    retried on later passes.
//! 2. **Block estimate**: every still-unsettled tier gets a per-block least-squares
//!    estimate (`mast ~ a*cos(theta) + b*sin(theta)`, the planes-through-a-circle
//!    model) as a starting point.
//! 3. **Nearest-level refinement** ([`SolveStrategy::JointGroup`]): Jacobi sweeps
//!    over the full arrangement where every meet-derived tier snaps to the
//!    shallowest candidate vertex level incident to its resolved named references
//!    when one exists, else the level nearest its current mast. The true
//!    configuration is a stable fixed point of this update; the sweeps both settle
//!    mutually-dependent groups and polish phase-1 values.
//!
//! `"Meet <names>"` references are resolved by [`MeetNameResolver`], which handles
//! informal reference styles (unnamed girdle/culet/table references,
//! compound `"1-2-G1"` vertex specs, connective prose, case and side-prefix
//! mismatches).
//!
//! # Picked corners
//!
//! A [`MeetTierInput::pick`] names the corner a tier is cut to as three facet planes
//! ([`MeetPick`]); the corner is the point where those planes meet. It decides between
//! corners that the shallowest-level rule cannot tell apart. A pick is usable when the
//! tier is not an anchor, its three planes belong to other tiers of the slice at
//! existing index positions, and their normals are independent
//! (`|n1 . (n2 x n3)| >= PICK_MIN_DET`). An unusable pick leaves the tier solved exactly
//! as if it had none.
//!
//! * **Phase 1**: the tier waits (like a named tier) until the three picked tiers have
//!   settled, then takes the mast of its facets' deepest plane through the corner:
//!   `max_j n_j . v` over its own instances, the only mast at which the corner survives
//!   the tier's own facets. The tier is settled as [`SolveStrategy::DependencyOrder`] with
//!   no vertex-level pick, so the repair search of [`solve_meet_points_verified`] never
//!   moves it. When that value is not finite, not positive or above the domination limit,
//!   the tier falls back to the named or rank-1 rule.
//! * **Phase 3**: every sweep recomputes the corner from the current masts and sets the
//!   tier's mast from it, so the pick follows its facets.
//!
//! The solver does not test whether the corner lies on the stone: a meet point is often
//! cut away again by later tiers, and only a caller that knows the cutting order can
//! tell. The pick is purely algebraic.
//!
//! All geometry runs in `f64` on a deterministic candidate-vertex primitive (every
//! well-conditioned plane triple, solved directly, filtered by feasibility) -- no
//! convex-hull library, no hashed iteration, byte-identical results run to run.
//!
//! # External verification: printed proportions
//!
//! A wrong solve is self-consistent: every tier still lands on a real meet vertex. So
//! nothing internal to the arrangement separates it from the truth. What does separate
//! them is external: the proportion figures printed on a diagram (`Vol/W^3`, `L/W`,
//! `C/W`, `P/W`, `H/W`). The true configuration reproduces them to about 0.1 % (median),
//! while a wrong solve is typically off by about 30 % (calibration in
//! [`super::stone_metrics`]). [`solve_meet_points_verified`] uses this with a greedy
//! repair search over phase-1 vertex-level picks, scored by
//! [`ExternalProportions::combined_deviation`].
//!
//! # Validating a schedule (forward direction)
//!
//! [`vertex_meet_groups`] is the reverse capability: given an already-solved
//! [`GemPolyhedron`], it reports which input planes actually touch at each
//! vertex -- geometric ground truth independent of a file's `G`-field text.
//!
//! # Cost envelope
//!
//! [`candidates::enumerate_candidate_vertices_cancellable`] tests every well-conditioned
//! triple of the `P` planes, which is `O(P^3)` per call. Phase 1 avoids paying that per
//! tier through `phase1_cache`'s incremental candidate set. Phase 3 re-enumerates the
//! whole arrangement from scratch on each sweep, up to [`MAX_REFINE_SWEEPS`] times. This
//! is deliberate: most tiers move on every sweep, so a persistent cache costs more in
//! bookkeeping than it saves (see the note in `candidates`).
//!
//! [`MAX_PLANES`] (400) caps what is solved at all. Above it, [`solve_meet_points`] and
//! [`solve_meet_points_verified`] return immediately with [`SolveStrategy::Failed`] tiers.
//! The repair search budgets its pipeline runs per design (see `VERIFY_RUN_BUDGET` in
//! `verify`), since one run costs roughly `planes^4` once tier count is included. Large
//! designs therefore get little or no repair search.
//!
//! Practical expectation: a design of roughly 60-150 planes solves in a fraction of a
//! second to a few seconds, including the repair search. A 300+-plane design takes
//! several seconds and gets little or no repair search. A caller on an interactive UI
//! should run the solve off the main thread and size any timeout to the plane count.
//!
//! # Cancellation and progress
//!
//! [`solve_meet_points_with`] and [`solve_meet_points_verified_with`] take a
//! [`SolveControl`]. The plain entry points pass a no-op control, which costs one
//! `Option::is_some` check per cancel point and changes no output. Cancel points run from
//! coarse to fine: once per constructive sweep and once per unsettled tier within it, once
//! per refinement sweep, and twice inside the candidate enumeration (per outer and per
//! middle plane). That last pair bounds the longest uninterrupted work to one `O(P)` loop.
//! A cancel on a 103-tier design is observed within 500 ms (see
//! `pleochroma-cut-core`'s `design::tests::cancel_stops_a_large_real_solve_quickly`).
//! The repair search's pipeline runs use the same control, so a cancel or progress report
//! from inside them surfaces like a plain solve's.
//!
//! # Module layout
//!
//! Split by seam, not by size: [`blocks`] classifies each tier into
//! crown/pavilion/girdle; [`anchors`] fills in per-block scale references from
//! printed proportions; [`names`] resolves stated `"Meet <names>"` text against
//! tier names; [`candidates`] is the deterministic candidate-vertex primitive
//! (plane triples -> feasible vertices -> `n . v` levels); `phase1_cache` is
//! [`solve`]'s incremental candidate-vertex cache; [`solve`] holds the
//! three-phase pipeline ([`SolveContext`](solve::SolveContext),
//! [`solve_meet_points`]); `verify` layers the externally-verified repair search
//! ([`solve_meet_points_verified`]) on top; `validation` holds the reverse and
//! schedule-export helpers.

use pleochroma_formats::asc::AscSchedule;
use std::sync::atomic::{AtomicBool, Ordering};

mod anchors;
mod blocks;
mod candidates;
mod names;
mod phase1_cache;
mod solve;
mod validation;
mod verify;

pub use anchors::apply_ratio_anchors;
pub use blocks::{Block, classify_blocks};
pub use candidates::tier_instance_normals;
pub use names::{MeetNameResolver, ResolvedNames, TokenResolution};
pub use solve::{solve_meet_points, solve_meet_points_with};
pub use validation::{build_reconstructed_schedule, vertex_meet_groups};
pub use verify::{
    VERIFY_ACCEPT_TOL, VerifiedSolveReport, solve_meet_points_verified,
    solve_meet_points_verified_with,
};

/// Half-extent of the bounding box standing in for the uncut rough stone. Real
/// `.asc` masts sit close to 1.0, so this never masquerades as a real facet; it only
/// keeps the candidate-vertex feasibility test well-defined before the arrangement
/// closes up.
///
/// Shared with [`super::stone_metrics`], which re-exports it so both arrangement
/// walks use one definition.
pub(crate) const BLANK_HALF_EXTENT: f64 = 64.0;

/// Feasibility slack: a candidate vertex may poke this far (absolute; masts are ~1)
/// beyond a plane before that plane's tier counts as violated. Shared with
/// [`super::stone_metrics`].
pub(crate) const EPS_FEAS: f64 = 1e-5;

/// A plane within this absolute distance of a vertex counts as passing through it
/// (used to test incidence with named meet references).
const EPS_INCIDENT: f64 = 1e-4;

/// Two candidate `n . v` values within this absolute distance belong to one vertex
/// "level".
const LEVEL_TOL: f64 = 1e-5;

/// Minimum `|determinant|` for a triple of unit plane normals to define a candidate
/// vertex. Shared with [`super::stone_metrics`] and [`super::brep`].
pub(crate) const MIN_TRIPLE_DET: f64 = 1e-6;

/// Designs with more facet planes than this are not solved (the candidate enumeration
/// is cubic in the plane count). Real designs stay far below it.
const MAX_PLANES: usize = 400;

/// Maximum constructive sweeps (phase 1). Values snap onto exact vertex levels,
/// so convergence, when it happens, is exact; the cap only guards against cycling.
const MAX_CONSTRUCTIVE_SWEEPS: usize = 64;

/// Maximum nearest-level refinement sweeps (phase 3).
///
/// Many runs never meet the `1e-12` convergence test: masts oscillate between vertex
/// levels instead of contracting. Extra sweeps mostly cost time, and convergent runs
/// usually settle within one or two sweeps. Higher caps mostly add time without
/// improving accuracy. A fixed-point acceleration of the vertex map was
/// rejected for the same reason.
const MAX_REFINE_SWEEPS: usize = 4;

/// Which of the three solve phases (see the module docs' "Solving strategy") a
/// [`SolveProgress`] report describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolvePhase {
    /// Phase 1: the file-order constructive pass.
    Constructive,
    /// Phase 2: the per-block least-squares estimate for whatever phase 1
    /// left unsettled.
    LeastSquares,
    /// Phase 3: nearest-level refinement sweeps over the full arrangement.
    Refine,
}

/// One progress report from a `_with` solve entry point's [`SolveControl`].
///
/// `sweep`/`max_sweeps` count within the current [`SolvePhase`] only (each
/// phase's own sweep counter restarts at 1); `blocks_done`/`blocks_total`
/// count tiers processed within the current sweep (`blocks_total` is always
/// the design's tier count). Neither pair is comparable across a phase
/// boundary -- a caller driving a progress bar should key it off
/// `(phase, sweep)`, not assume a single global counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolveProgress {
    /// Which phase this report is from.
    pub phase: SolvePhase,
    /// 1-based sweep index within `phase`.
    pub sweep: u32,
    /// The most sweeps `phase` can run (1 for [`SolvePhase::LeastSquares`],
    /// which has no sweep structure of its own).
    pub max_sweeps: u32,
    /// Tiers processed so far within this sweep.
    pub blocks_done: u32,
    /// Total tiers in the design -- `blocks_done` reaches this at the end of
    /// every completed sweep.
    pub blocks_total: u32,
}

/// Why a `_with` solve entry point ([`solve_meet_points_with`],
/// [`solve_meet_points_verified_with`]) returned early instead of a solved result.
///
/// The plain (non-`_with`) entry points never produce this: an unused
/// [`SolveControl`] never cancels, and they return an
/// all-[`SolveStrategy::Failed`] result for [`Self::TooManyPlanes`] instead
/// of erroring (see each variant's doc comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveError {
    /// [`SolveControl`]'s cancel flag was observed set at a cancel point (see
    /// the module docs, "Cancellation and progress"). No partial result is
    /// returned -- a cancelled solve has no use for an incomplete one.
    Cancelled,
    /// More than `max` facet-plane instances (`planes` of them) -- the same
    /// [`MAX_PLANES`] cap [`solve_meet_points`]/[`solve_meet_points_verified`]
    /// silently return an all-[`SolveStrategy::Failed`] result for instead.
    TooManyPlanes {
        /// The design's actual plane-instance count.
        planes: usize,
        /// The cap it exceeded ([`MAX_PLANES`]).
        max: usize,
    },
    /// Tier `tier` (file order) carries a non-finite (NaN or infinite) `field`.
    ///
    /// Checked before any candidate-vertex geometry runs. A NaN determinant fails every
    /// `<` comparison, so a threshold check would treat it as non-degenerate, and a NaN
    /// or infinite mast could yield a plausible solid with a facet missing. The `.asc`
    /// parser also rejects `nan`/`inf` tokens, but this solver does not rely on that.
    NonFiniteInput {
        /// Index (file order) of the first tier found with a non-finite value.
        tier: usize,
        /// Which field on that tier was non-finite.
        field: NonFiniteField,
    },
}

/// Which field of a [`MeetTierInput`] was found non-finite by
/// [`SolveError::NonFiniteInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonFiniteField {
    /// [`MeetTierInput::angle_deg`].
    AngleDeg,
    /// One of [`MeetTierInput::indices`].
    Index,
    /// The mast in a [`MeetConstraint::ScaleReference`].
    Mast,
}

impl std::fmt::Display for NonFiniteField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::AngleDeg => "angle_deg",
            Self::Index => "an index",
            Self::Mast => "a ScaleReference mast",
        })
    }
}

impl std::fmt::Display for SolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "solve cancelled"),
            Self::TooManyPlanes { planes, max } => write!(
                f,
                "design has {planes} facet-plane instances, above the {max}-plane cap for \
                 candidate-vertex enumeration"
            ),
            Self::NonFiniteInput { tier, field } => write!(
                f,
                "tier {tier} has a non-finite (NaN or infinite) value in {field}"
            ),
        }
    }
}

impl std::error::Error for SolveError {}

/// Scans `tiers` in file order for the first non-finite `angle_deg`, index, or
/// `ScaleReference` mast, returning it as a [`SolveError::NonFiniteInput`].
///
/// Shared by [`solve_meet_points_with`] and [`solve_meet_points_verified_with`]
/// -- both must reject a non-finite input before any candidate-vertex geometry
/// runs on it (see [`SolveError::NonFiniteInput`]'s doc comment for why a
/// downstream check alone is not enough).
pub(super) fn first_non_finite_tier(tiers: &[MeetTierInput]) -> Option<SolveError> {
    for (i, t) in tiers.iter().enumerate() {
        if !t.angle_deg.is_finite() {
            return Some(SolveError::NonFiniteInput {
                tier: i,
                field: NonFiniteField::AngleDeg,
            });
        }
        if t.indices.iter().any(|idx| !idx.is_finite()) {
            return Some(SolveError::NonFiniteInput {
                tier: i,
                field: NonFiniteField::Index,
            });
        }
        if let MeetConstraint::ScaleReference(v) = &t.constraint
            && !v.is_finite()
        {
            return Some(SolveError::NonFiniteInput {
                tier: i,
                field: NonFiniteField::Mast,
            });
        }
    }
    None
}

/// Cancellation and progress-reporting hooks for a `_with` solve entry point.
///
/// [`Self::default`] is the no-op control every plain (non-`_with`) entry
/// point passes internally: `cancel` absent and `progress` absent, so every
/// cancel/progress point in the pipeline costs one cheap `Option::is_some`
/// check and calls nothing. Build one with [`Self::default`] then
/// [`Self::with_cancel`]/[`Self::with_progress`] (either or both) to opt in.
#[derive(Default, Clone, Copy)]
pub struct SolveControl<'a> {
    cancel: Option<&'a AtomicBool>,
    stop: Option<&'a dyn Fn() -> bool>,
    progress: Option<&'a dyn Fn(SolveProgress)>,
}

impl<'a> SolveControl<'a> {
    /// A control that checks `cancel` at every cancel point (see the module
    /// docs) and reports no progress. `cancel` is read with
    /// [`Ordering::Relaxed`] -- a solve only ever needs to observe the flag
    /// eventually, not synchronize any other state through it.
    #[must_use]
    pub const fn with_cancel(cancel: &'a AtomicBool) -> Self {
        Self {
            cancel: Some(cancel),
            stop: None,
            progress: None,
        }
    }

    /// A control that reports every [`SolveProgress`] point to `progress` and
    /// never cancels.
    #[must_use]
    pub const fn with_progress(progress: &'a dyn Fn(SolveProgress)) -> Self {
        Self {
            cancel: None,
            stop: None,
            progress: Some(progress),
        }
    }

    /// Poll a caller-owned cancellation predicate at every solver checkpoint.
    #[must_use]
    pub const fn with_stop(stop: &'a dyn Fn() -> bool) -> Self {
        Self {
            cancel: None,
            stop: Some(stop),
            progress: None,
        }
    }

    /// Adds cancellation to an existing control (e.g. one already carrying a
    /// `progress` callback via [`Self::with_progress`]).
    #[must_use]
    pub const fn cancelling(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Adds progress reporting to an existing control (e.g. one already
    /// carrying a `cancel` flag via [`Self::with_cancel`]).
    #[must_use]
    pub const fn reporting(mut self, progress: &'a dyn Fn(SolveProgress)) -> Self {
        self.progress = Some(progress);
        self
    }

    /// True when the atomic flag or caller cancellation predicate requests a stop.
    /// No atomic load or callback is performed when neither source is configured.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
            || self.stop.is_some_and(|stop| stop())
    }

    /// Calls `self.progress` with `report` when one is set; a no-op otherwise.
    pub(super) fn report(&self, report: SolveProgress) {
        if let Some(f) = self.progress {
            f(report);
        }
    }
}

/// A picked level more than this many times the design's scale prior almost
/// certainly means the region isn't really bounded there yet (the pick hit
/// bounding-blank geometry); such picks are rejected.
const BLANK_DOMINATION_FACTOR: f64 = 4.0;

/// Default scale assumed when a solve has no scale-reference tier at all.
const DEFAULT_PLAUSIBLE_SCALE: f64 = 1.0;

/// One facet tier awaiting a solved mast distance.
///
/// Mirrors the geometry-relevant fields of [`pleochroma_formats::asc::AscTier`], plus the
/// constraint that determines its mast. Kept independent of `AscTier` so this
/// solver isn't coupled to one file format.
#[derive(Debug, Clone)]
pub struct MeetTierInput {
    /// Signed angle from the girdle plane, in degrees (`GemCAD` convention:
    /// negative is pavilion, positive is crown). A zero angle is the table, unless
    /// it is a sign-negative zero (the culet) -- see
    /// [`crate::geometry::plane::tier_is_crown_side`].
    pub angle_deg: f64,
    /// Index-wheel positions this tier's facet occurs at. Empty means a single
    /// facet with no azimuth of its own (e.g. an unlisted table/culet); its normal
    /// is placed in the `+Z` direction, `(0, +-cos(theta), sin(theta))`, rather than
    /// at index 0 (which would be `+X`). Table and culet angles make `sin(theta)`
    /// zero, so there the azimuth is immaterial.
    pub indices: Vec<f64>,
    /// The constraint this tier's mast is solved against.
    pub constraint: MeetConstraint,
    /// Every name this tier is known by in the source schedule (e.g. `["P1"]`, or
    /// `["c", "d"]` for a tier folded from more than one named group). Used only
    /// to resolve [`MeetConstraint::MeetNamed`]'s references back to tier indices.
    pub names: Vec<String>,
    /// The corner this tier is cut to, for a [`MeetConstraint::MeetNamed`] or
    /// [`MeetConstraint::MeetExisting`] tier. `None` solves by the constraint alone;
    /// a pick that cannot be used also solves as if it were `None` (the tier's
    /// [`SolvedTier::detail`] says why).
    pub pick: Option<MeetPick>,
}

/// One facet plane of the arrangement: a tier of the input slice and one of its
/// index-wheel positions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeetPickPlane {
    /// Position of the tier in the `tiers` slice handed to the solver.
    pub tier: usize,
    /// The index-wheel position of the facet. Ignored for a tier without indices
    /// (table, culet).
    pub index: f64,
}

/// The corner a tier is cut to: the point where three facet planes meet.
///
/// The point is recomputed from the planes' current masts on every solve, so it
/// follows its facets when angles or sizes change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeetPick {
    /// The three facet planes that meet at the corner.
    pub planes: [MeetPickPlane; 3],
}

/// A picked triple is usable when `|n1 . (n2 x n3)|` is at least this.
const PICK_MIN_DET: f64 = 5e-3;

/// Two index-wheel positions closer than this (modulo the gear) are the same position.
const PICK_INDEX_TOL: f64 = 1e-6;

/// How a tier's mast distance is determined.
///
/// `PartialEq` is derived for `pleochroma_cut_core::design::ConstraintTier`, which stores
/// this as editable state and needs it for undo/redo round-trip tests.
#[derive(Debug, Clone, PartialEq)]
pub enum MeetConstraint {
    /// Cut past first touch until the plane passes through a meet vertex of the
    /// other facets' arrangement, with no further information about *which*
    /// vertex. Covers "Cut to centerpoint", "Table", and any unnamed meet-style
    /// instruction; solved with the rank-1 prior (see the module docs).
    MeetExisting,
    /// Like [`Self::MeetExisting`], but the schedule states explicitly which
    /// facets this tier closes against (a real `.asc` `"G Meet P1, P2, G1"`
    /// instruction). Resolved against every tier's [`MeetTierInput::names`] via
    /// [`MeetNameResolver`]; an unresolved token is dropped, and a tier with no
    /// resolved tokens degrades to [`Self::MeetExisting`]'s rank-1 handling.
    MeetNamed(Vec<String>),
    /// An externally supplied target mast: "Set girdle thickness" / "Set stone
    /// size" / "Level girdle", or a caller-supplied per-block dimension (see the
    /// module docs on anchors). Not derivable from geometry.
    ScaleReference(f64),
}

/// Builds [`MeetTierInput`]s from a parsed `.asc` schedule, classifying each
/// tier's [`MeetConstraint`] from its `G`-field text.
///
/// See [`pleochroma_formats::asc::AscTier::meet_instruction`]: `Meet` -> `MeetNamed`;
/// `ScaleReference`/`LevelGirdle` -> `ScaleReference` (using the tier's recorded
/// `mast`); everything else -> `MeetExisting`.
///
/// Never fabricates a scale anchor: a block with no stated anchor still needs
/// one from the caller (see the module docs).
#[must_use]
pub fn meet_tier_inputs_from_asc(schedule: &AscSchedule) -> Vec<MeetTierInput> {
    schedule
        .tiers
        .iter()
        .map(|tier| {
            let constraint = match tier.meet_instruction() {
                Some(pleochroma_formats::asc::MeetInstruction::Meet(names)) => {
                    MeetConstraint::MeetNamed(names)
                }
                Some(
                    pleochroma_formats::asc::MeetInstruction::ScaleReference
                    | pleochroma_formats::asc::MeetInstruction::LevelGirdle,
                ) => MeetConstraint::ScaleReference(tier.mast),
                _ => MeetConstraint::MeetExisting,
            };
            MeetTierInput {
                angle_deg: tier.angle_deg,
                indices: tier.indices.clone(),
                constraint,
                names: tier.names().into_iter().map(str::to_string).collect(),
                pick: None,
            }
        })
        .collect()
}

/// Which technique actually produced a tier's solved mast, for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveStrategy {
    /// The mast was supplied directly (a [`MeetConstraint::ScaleReference`]).
    ScaleReference,
    /// Settled in the constructive fixed-point pass: a candidate meet vertex from
    /// already-settled planes, possibly polished by refinement sweeps afterwards.
    DependencyOrder,
    /// Part of a mutually-dependent remainder the constructive pass couldn't
    /// order; settled by refinement sweeps over the full arrangement.
    JointGroup,
    /// No usable candidate vertex; the mast is the per-block
    /// `a*cos(theta) + b*sin(theta)` estimate, not vertex-derived.
    LeastSquaresFallback,
    /// The solve could not produce even an estimate (e.g. exceeds [`MAX_PLANES`]).
    /// The returned mast is a placeholder and should not be trusted.
    Failed,
}

/// One tier's solved result.
#[derive(Debug, Clone)]
pub struct SolvedTier {
    /// The solved mast height.
    pub mast: f64,
    /// Which solve strategy produced `mast`.
    pub strategy: SolveStrategy,
    /// Free-form prose naming how `mast` was actually obtained -- e.g. "given
    /// (scale reference)", "vertex incidence", or which candidate vertex level
    /// and named references a constructive/refinement pass settled on. Meant
    /// to be shown verbatim as a per-tier-row tooltip (the detail behind
    /// `strategy`'s coarse category); it is prose for a human, not a stable
    /// format -- never parse it back out, match on [`SolveStrategy`] instead.
    pub detail: String,
}
