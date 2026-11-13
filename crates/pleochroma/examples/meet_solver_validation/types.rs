//! Shared record types for the meet-solver validation harness: the `AscRow` input record,
//! the `ConstraintKind`/`TierResult` per-tier classification, and the `DesignResult`
//! per-design accumulator.

use pleochroma::geometry::meet_solver::SolveStrategy;

/// Worker count for the parallel solve. Fixed rather than read from
/// `std::thread::available_parallelism`, so a run's shape is the same on every machine.
pub const THREADS: usize = 16;

/// One `.asc` schedule row loaded from the database.
pub struct AscRow {
    /// Database id of the design the file belongs to.
    pub detail_id: i64,
    /// Raw bytes of the attached `.asc` file.
    pub content: Vec<u8>,
    /// Printed crown-height/width proportion from `diagram_details`; `None` when not recorded.
    /// Used only by [`solve_one_ratio_anchored`]; [`solve_one`] ignores it.
    pub cw_ratio: Option<f64>,
    /// Printed pavilion-depth/width proportion.
    pub pw_ratio: Option<f64>,
    /// Printed `Vol/W^3`, used only by the verified reports as a verification target.
    pub volume: Option<f64>,
    /// Printed length/width ratio.
    pub lw_ratio: Option<f64>,
    /// Printed height/width ratio.
    pub hw_ratio: Option<f64>,
}

/// The kind of information that determined a tier's constraint, before any bootstrap
/// fallback. Used only for reporting: separates what the schedule stated from what the
/// solver inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    /// The schedule states the tier's scale reference.
    ScaleReference,
    /// The tier meets named references.
    MeetNamed,
    /// The tier meets existing geometry without names.
    MeetExisting,
}

/// Outcome of solving one tier.
#[derive(Clone)]
pub struct TierResult {
    /// Strategy the solver used for this tier.
    pub strategy: SolveStrategy,
    /// Relative mast error against the recorded mast.
    pub rel_err: f64,
    /// Kind of information that determined the constraint.
    pub kind: ConstraintKind,
    /// Only for `kind == ConstraintKind::MeetNamed`: `Some(true)` if every name in the
    /// stated `"Meet <names>"` resolves to a known tier (via the same `resolve_names` logic
    /// the solver uses; see `classify_named_resolution`), `Some(false)` if any does not,
    /// `None` otherwise. Meet text is free prose and most names do not resolve, so the
    /// resolved and unresolved buckets are reported separately.
    pub named_resolved: Option<bool>,
    /// Whether the solver settled this tier through its resolved named references, read
    /// from the solver's `detail` string.
    pub used_named: bool,
    /// For a rank-1 fallback on a tier with resolved refs, from the solver's `detail` string:
    /// 'u' = refs not settled at release, 'n' = no incident feasible level, ' ' = not a
    /// named fallback.
    pub fallback_cause: char,
}

/// Everything one design's solve contributes to the aggregate report. Produced by
/// [`solve_one`], which is the unit of work parallelized across `THREADS` workers.
#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "report row of independent per-design outcome flags, each counted on its own"
)]
pub struct DesignResult {
    /// The schedule parsed successfully.
    pub parse_ok: bool,
    /// True iff the design had no stated scale-reference tier at all, so the harness
    /// had to bootstrap one from the file's own tier-0 real mast (see `solve_one`).
    /// Only meaningful when `parse_ok`.
    pub no_scale_reference: bool,
    /// Per-tier outcomes.
    pub tier_results: Vec<TierResult>,
    /// Worst meet-derived tier's relative error, or `None` if the design had none to
    /// score. Only meaningful when `parse_ok`.
    pub worst_err: Option<f64>,
    /// Which `ConstraintKind`s appear anywhere in this design's tiers -- for the
    /// per-design bucket counts (a design can and often does mix kinds).
    pub has_meet_named: bool,
    /// Some tier is `MeetExisting`.
    pub has_meet_existing: bool,
    /// Some tier is `ScaleReference`.
    pub has_scale_reference: bool,
    /// Only for [`solve_one_ratio_anchored`]'s results: true iff every block that needed an
    /// anchor got it from a printed `C/W` or `P/W` ratio or a stated scale reference. False
    /// when any block fell back to its tier-0 recorded mast.
    pub fully_ratio_anchored: bool,
    /// Verified reports only ([`solve_one_verified`], [`solve_one_ratio_anchored_verified`]):
    /// the repair search's accounting. `None` for the other reports.
    pub verify: Option<pleochroma::geometry::meet_solver::VerifiedSolveReport>,
}
