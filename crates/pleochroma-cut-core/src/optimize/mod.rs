//! Optimizing a [`crate::design::Design`]'s authored facet *angles* against the optical
//! metrics `pleochroma::color::metrics` already measures.
//!
//! Windowing, extinction, and the tilt-performance sweep, instead of only reporting
//! them after the fact.
//!
//! # Cost
//!
//! Every evaluation pays for a [`crate::design::Design::solve`], which dominates the cost.
//! [`crate::design::Design::resolve_dirty`] does not help once a design has more than one
//! non-`ScaleReference` tier (see [`crate::resolve`]), which a typical free set is.
//!
//! [`cost_probe`] (`#[cfg(test)]`, `#[ignore]`d: `cargo test -p pleochroma-cut-core
//! --release --lib cost_probe -- --ignored --nocapture`) measures one full objective
//! evaluation end to end.
//!
//! At [`ObjectiveFidelity::Fast`], which the search loop spends its budget on, one
//! evaluation is dominated by the solve once a design has real meet-derived structure. A
//! proposal is a bounded projection, which pays many solves and one finite-difference probe
//! per variable per iteration (see "Varying anchored tiers" below). There is no cheap
//! incremental resolve to fall back on once tiers are genuinely free.
//!
//! Asking for several candidates ([`OptimizeOptions::keep_candidates`]) adds one `Full`
//! scoring per extra candidate, once, at the end. The search loop itself pays only for the
//! bookkeeping of its pool (a clone of every accepted state that beats the start's `Fast`
//! score). A run whose states fail the `Full` gate scores up to
//! [`OptimizeOptions::pool_capacity`] states before it reports no change. A run whose states
//! pass never scores more than it keeps. The girdle guard
//! ([`OptimizeOptions::min_girdle_fraction`]) adds a `measure_solid` and a walk over the
//! girdle walls per candidate that closes.
//!
//! # Which tiers are free (never moves a pinned facet)
//!
//! [`free_tier_indices`] returns every tier index whose current constraint is *not*
//! [`pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference`] -- a
//! `ScaleReference` tier's mast is never solver-derived, so its plane should never
//! be an optimizer-derived guess either. Import pins every tier to `ScaleReference`
//! (see [`crate::design::Design::from_asc_schedule`]), so a design fresh off the
//! catalogue has **zero** free tiers until the user adopts one back to its real meet
//! constraint via [`crate::edit::Edit::SetConstraint`]; this optimizer cannot
//! second-guess which imported tiers are adoptable meet structure versus genuinely
//! authored scale references, so [`optimize_design`] on a freshly-imported design
//! returns an [`OptimizeOutcome`] with zero evaluations and an unchanged score --
//! an honest "nothing to do here yet", not an error. (A request can opt in to the
//! other answer, see "Varying anchored tiers" below: with
//! [`OptimizeOptions::vary_anchored`] a pinned horizontal tier or a relation
//! follower contributes a depth variable, so such a design runs evaluations and can
//! return mast changes without any angle change.)
//!
//! Two further exclusions apply to every tier, whatever its constraint. A
//! **horizontal** tier (`|angle| < 1e-9`: the table at `+0.0`, the culet at `-0.0`)
//! has no angle to optimize, and a step off zero would put the facet on the wrong
//! side of the girdle. A **vertical** tier (`|angle| >= 89.5` degrees, a real girdle
//! facet at `-90.0`) can never pass the safety gate in either direction, and one such
//! tier in the free set would make the polish stage reject every point it proposes.
//! See [`free_tier_indices`]'s own doc comment.
//!
//! A tier's `angle_deg` is the only field this module ever changes on a free tier --
//! never `indices`/`detached`/`name` -- and, for an anchored tier only, the mast inside
//! its `ScaleReference` constraint, which follows the angle (see below). Since all of a
//! [`crate::design::ConstraintTier`]'s `indices` share one `angle_deg`, moving that
//! shared field moves every orbit member together by construction, keeping symmetry
//! intact without this module ever touching `crate::orbit`'s membership operations.
//!
//! # Varying anchored tiers (opt-in)
//!
//! With [`OptimizeOptions::vary_anchored`] on, pinned tiers with authorized hinges can
//! supply angle and depth variables. Horizontal pinned depths and relation-follower
//! depths can also participate, while follower angles remain relation-owned. Explicit
//! dimensional targets, unselected tiers and vertical girdle depths remain locked.
//! Hinges seed local proposals; bounded coupled projection preserves the complete
//! original junction network rather than fixing just one point per moving plane.
//! A common axial-affine family proposes coordinated motion before coordinate steps.
//! Every accepted state is re-solved and checked against all original supports, live
//! surfaces, protected edges and cyclic boundary order. Separate hard shape limits
//! constrain Optimize and the more conservative Retarget policy.
//! Accepted angle and mast changes travel together through scoring, previews, worker
//! messages and one atomic Apply. Ordinary meet-derived authoring stays derived.
//!
//! # Bounds, guards and candidates
//!
//! [`OptimizeOptions::angle_bounds`] confines a tier to a signed angle range: a
//! coordinate step that overshoots is clamped to the range edge (as long as that still
//! moves the tier), and a polish point outside it is rejected. With
//! [`OptimizeOptions::min_girdle_fraction`] set, a candidate whose girdle band vanishes
//! or thins below that fraction of the starting band is rejected, and so is one whose
//! band runs to a knife edge anywhere around the outline (the thinnest point is measured
//! too, see [`crate::design::girdle_band_in`]: a facet turned about its girdle edge keeps
//! the extreme vertices the overall thickness is read from, so that figure alone cannot
//! see a corner pinch out), or that loses a girdle wall or the table facet the starting
//! design had (found through the table tier's own plane, not the solid's table
//! percentage). The thinnest point takes the same fraction unless
//! [`OptimizeOptions::min_girdle_thinnest_fraction`] gives it one of its own.
//!
//! [`OptimizeOptions::keep_candidates`] keeps a bounded list of the best *distinct* states
//! the search saw (some free angle differing by at least [`OptimizeConfig::min_step_deg`],
//! or [`OptimizeOptions::candidate_separation_deg`]), plus a reserve of fallbacks
//! ([`OptimizeOptions::pool_capacity`]). At the end they are re-scored at full fidelity,
//! best fast score first, until `keep_candidates` pass the gate (no worse than the start at
//! full fidelity). Those are returned, best first, as [`OptimizeResult::candidates`]; the
//! plain [`OptimizeOutcome`] describes the first. The reserve exists because the `Fast`
//! objective the search descends on and the `Full` gate can disagree; see
//! `search::result::build_result`.
//! [`OptimizeOptions::shape_target`] keeps the stone's look: every score, the starting
//! design's included, adds a penalty for drifting from a table size and a
//! crown-to-pavilion ratio ([`ShapeTarget`]); `None` changes nothing.
//! Named objective weightings for a front end are [`ObjectivePreset`].
//!
//! # Face-up tone
//!
//! The objective has a fifth, optional term: the colour the stone shows face-up
//! ([`FaceUpTone`]: CIELAB `L*`, chroma, hue and an sRGB swatch of the returned light),
//! with a goal ([`ToneGoal`]: lighter or deeper) at [`ObjectiveWeights::tone_weight`].
//! Weight `0` (the default) measures nothing for this term and leaves every score unchanged.
//! At `Fast` the tone comes from the same trace as the components; at `Full` it costs one
//! more table-up trace. Every `Full` scoring (the start, the end point, each alternative)
//! reports the tone whether it is weighted or not.
//!
//! The tone is the table-up pose, under [`OptimizeConfig::lighting`]'s own light and
//! white point (a UV lamp has no visible white and falls back to D65). The stone size enters
//! through [`pleochroma::optics::materials::GemMaterial::absorption_path_scale`]; the caller
//! sizes the material (`sized_material_for_optimize` in `pleochroma-editor`).
//! [`OptimizeResult::lighting`] records the preset a run scored under.
//!
//! # Where the compute runs, and why not remote
//!
//! The search loop is plain, synchronous Rust with no threading of its own, so
//! `pleochroma-cut` wraps it in a cancellable background thread like
//! `gui::editor::deep_solve`. Its remote-worker infrastructure can't help: a worker
//! only knows already-solved plane geometry, not `Design`/`Design::solve`
//! (`pleochroma-cut-core` is intentionally not a dependency of it), and solving --
//! not the optics metrics a worker can already compute -- is the dominant cost, so
//! dispatching candidates remotely would never reach the expensive part.
//!
//! **Local parallelism** is used here: a coordinate search's two candidates per free tier
//! per sweep (`angle + step`, `angle - step`) are independent `Design::solve` calls with no
//! shared state, so [`optimize_design`] evaluates the pair concurrently via
//! `std::thread::scope` (see [`candidate::evaluate_candidate_pair`]). That roughly halves
//! each tier's decision cost on two free cores. It is the only parallelism available, since
//! sweep N+1 starts from whichever of sweep N's candidates were accepted.
//!
//! # Never proposes geometry that fails to close, or moves a tier past its own block boundary
//!
//! Every candidate is solved and meshed before it is scored; one that fails to solve or
//! does not close into [`pleochroma::geometry::stone_metrics::SolidStatus::Closed`] is
//! rejected outright (see [`candidate::evaluate_candidate_controlled`]). A candidate is
//! also rejected before solving if the proposed angle would change sign or land on zero,
//! for every tier alike: such an edit can remove a block's own anchor by reclassifying the
//! tier's [`pleochroma::geometry::meet_solver::Block`] (see
//! [`candidate::candidate_angle_is_safe`]).
//!
//! # Manufacturability must not regress
//!
//! An accepted candidate's [`crate::manufacturability::check_manufacturability`]
//! warning counts for the two mesh-dependent checks (vanishing/undersized facets --
//! the only two an angle-only edit can change) must not exceed the running
//! baseline's own counts. See `candidate::manufacturability_regressed`.
//!
//! # `History` remains the sole mutator
//!
//! [`optimize_design`] takes a `&Design` and returns a plain, inert
//! [`OptimizeOutcome`] describing which tiers it would change and to what angle --
//! it never mutates a [`Design`] itself, and neither does [`optimize_design_with`].
//! [`apply_optimize_outcome`] turns an outcome into one [`crate::edit::Edit::Batch`]
//! of `ModifyTier` sub-edits applied via a single [`crate::edit::History::apply`]
//! call, so an applied optimization is undoable as ONE step regardless of how many
//! tiers it touched. [`apply_optimize_result`] and [`apply_optimize_candidate`] do the
//! same for a result with mast changes, or for one of its ranked candidates: angles and
//! masts of one tier travel in the same `ModifyTier`, the stale guard compares both
//! bit for bit, and one undo restores the design exactly.
//!
//! # Determinism
//!
//! [`OptimizeConfig::seed`] is a required, explicit `u64`: the only
//! nondeterministic-looking part of a coordinate search is the order free tiers are
//! visited each sweep, and that comes from `search::seeded_permutation`, a
//! deterministic splitmix64-based Fisher-Yates shuffle. Identical
//! `(Design, OptimizeConfig, OptimizeOptions)` input always produces an identical
//! [`OptimizeResult`], candidates and mast changes included.
//!
//! # Why the extended request is not a field of `OptimizeConfig`
//!
//! `OptimizeConfig` is `Copy` and is stored in `pleochroma-editor`'s `RetargetMode`
//! (also `Copy`), and the web crates build [`OptimizeOutcome`] by full struct literal. The
//! extended request and answer therefore live in their own types ([`OptimizeOptions`],
//! [`OptimizeResult`]), and the older types keep their shape. Every accepted state is
//! checked against the captured network and stamped with provenance. See [`options`]'s
//! module docs.
//!
//! # Coordinate descent stalls on diagonal ridges
//!
//! [`search`]'s coordinate descent tries one free tier's angle at a time; optical
//! objectives (windowing, extinction, tilt brilliance) have ridges where paired
//! angles (e.g. a crown break and a pavilion main) must move together to hold a
//! critical-angle relation, and axis-aligned moves stall there. [`polish`] is the second
//! stage: once the coordinate step is small, a deterministic Nelder-Mead simplex over the
//! whole free-angle vector can move along a ridge that no single-axis move climbs. See
//! [`search::optimize_design`]'s
//! own doc comment ("Two stages") and [`polish::run_polish`]'s for the algorithm and
//! knobs ([`search::OptimizeConfig::polish_start_step_deg`]/
//! [`search::OptimizeConfig::polish_max_evaluations`]).
//!
//! # Several starts
//!
//! The objective the search descends on is a quantised landscape of cliffs (critical
//! angles, lit/unlit exits) and ridges, so one coordinate descent stalls on the first
//! plateau. With [`OptimizeConfig::starts`] above one, [`optimize_design_with`] runs
//! several descents and keeps the best:
//!
//! 1. **Screen.** Draw `4 * (starts - 1)` points (at most 64) from the search box (each
//!    free tier's bounds, or +-5 degrees around its angle, see
//!    `SearchSpace::start_box`) with a scrambled Halton sequence; every third
//!    draw is local (30 % of the box around the design). Each draw is one `Fast`
//!    evaluation, reported as [`SearchStage::Screening`]. The best distinct draws become
//!    the extra starts; the design itself is always start 0 and sweeps with the run's own
//!    seed.
//! 2. **Descend.** Each start gets `max_evaluations / starts` evaluations and its own
//!    sweep seed. Descents that stop early on a plateau leave budget behind; while at
//!    least `8 * free` evaluations are left the next best screened draws run in another
//!    wave, until a whole wave improves nothing.
//! 3. **Polish** the best `max(1, keep_candidates)` starts (not all: polishing every
//!    start would cost more than the screening and never reach the candidate list).
//! 4. **Merge** every start's candidate pool in start order and finish exactly as the
//!    single-start search does (the `Full` scorings, the `<= before` gate and its
//!    fallbacks).
//!
//! Cost rule: [`effective_starts`] caps the starts at the coordinate budget divided by
//! `8 * free`, so each start gets eight sweeps. A large, slow design therefore degrades to
//! one start on its own, and `starts <= 1` is bit for bit the single-start search.
//!
//! Determinism: per-start budgets, seeds and start points are fixed before a wave
//! begins, results are collected by start index and merged in that order, so the result
//! is the same for any [`OptimizeConfig::max_lanes`]. Lanes are `std::thread::scope`
//! threads (the calling thread runs lane 0 and forwards progress; each lane's own
//! candidate pair still uses two solve threads); `wasm32` runs the starts in sequence.
//! A cancelled run returns the merged best so far, as today. [`SearchHooks::on_start`]
//! tells the caller which start its thread's lane is on; [`OptimizeResult::starts_run`]
//! and [`OptimizeResult::best_start`] say what ran and what won.
//! [`inclusive_max_evaluations_for`] extends the honest evaluation total with the
//! screening draws and one polish budget per polished start. The polish budget is a hard
//! cap and never more starts than [`effective_starts`] are polished (extra waves can leave
//! more states than that), so a run exceeds the figure by at most the coordinate stage's
//! own soft overshoot (one candidate pair) per descent.
//!
//! # Module layout
//!
//! [`objective`] (the optical score itself), [`candidate`] (per-candidate
//! solve/close/guard/manufacturability gates, the `+step`/`-step` pair, and building a
//! candidate from a whole free-angle vector), [`search`] (the coordinate-descent
//! stage and its user-visible config/outcome types), [`polish`] (the Nelder-Mead
//! second stage, generic over the scoring function so it is testable without a real
//! [`crate::design::Design`]), [`options`] (the extended request and result types),
//! [`space`] (angle bounds and the hinge geometry), [`guard`] (the girdle/table
//! guard), [`pool`] (the bounded list of distinct candidates), `multistart` (the generic
//! multi-start driver; the design-specific engine is `search::multi`), [`preset`] (named
//! objective weightings), [`constrained`] (the meet-preserving projection), and
//! [`materialize`] and [`provenance`] (turning changes into `History` edits, and the
//! operation identity).

mod apply;
mod candidate;
mod constrained;
mod families;
pub use constrained::{
    ProjectionReport, project_preserved_design, project_preserved_design_controlled,
    project_preserved_design_report,
};
mod guard;
mod materialize;
mod multistart;
pub use materialize::{MaterializeError, materialize_preserved_changes};
mod objective;
mod options;
mod polish;
mod pool;
mod preset;
mod provenance;
pub use provenance::{
    OptimizationProvenance, authoring_identity, materialize_candidate, materialize_outcome,
};
mod scored_stone;
mod search;
mod space;
#[cfg(test)]
mod tests;

pub use apply::{apply_optimize_candidate, apply_optimize_outcome, apply_optimize_result};
pub use candidate::{
    FinishedScore, MAX_SAFE_CANDIDATE_ANGLE_DEG, ScoringSetup, angle_is_variable,
    free_tier_indices, free_tier_indices_with, score_finished_design,
};
pub use objective::{
    CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW, CANONICAL_LIGHTING_PRESET, ObjectiveComponents,
    ObjectiveFidelity, ObjectiveWeights, ToneGoal, canonical_environment, display_score,
    evaluate_objective, evaluate_objective_finished_under,
    evaluate_objective_finished_under_threaded, evaluate_objective_geom_under,
    evaluate_objective_geom_under_threaded, evaluate_objective_threaded, evaluate_objective_under,
    evaluate_objective_under_threaded, evaluate_objective_with_tone,
    evaluate_objective_with_tone_finished_under_threaded,
    evaluate_objective_with_tone_geom_under_threaded, evaluate_objective_with_tone_under,
    evaluate_objective_with_tone_under_threaded,
};
pub use options::{MastChange, OptimizeCandidate, OptimizeOptions, OptimizeResult, ShapeTarget};
pub use pleochroma::color::metrics::FaceUpTone;
pub use preset::ObjectivePreset;
pub use search::{
    AngleChange, OptimizeConfig, OptimizeOutcome, SearchHooks, SearchStage, StartProgress,
    effective_starts, inclusive_max_evaluations, inclusive_max_evaluations_for, optimize_design,
    optimize_design_with,
};
