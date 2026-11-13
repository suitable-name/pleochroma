//! Profile-guided-optimization training workload for `pleochroma-cut-core` itself.
//!
//! Run alongside `crates/pleochroma/examples/pgo_train/main.rs` by `scripts/pgo-build.ps1` and
//! `scripts/pgo-bolt-build.sh` in an instrumented build. `pleochroma`'s own `pgo_train` example
//! trains the geometry, solver and renderer crate directly. This one trains the editor-core
//! layer built on it: [`Design`] construction, [`Design::solve`], `.asc` export, the `.psdd`
//! design file save/load round trip, [`resolve_after_edit`], and [`optimize_design`]'s search.
//!
//! For each built-in template in [`templates::TEMPLATES`] (small designs, all pinned to
//! `MeetConstraint::ScaleReference`, see that module's doc comment) and the CrackOtto-Step
//! cost-probe `.asc` fixture (103 tiers, the one real, heavily meet-derived design this crate
//! ships), the pipeline is: `Design::solve` -> `to_asc_schedule_from_solved` ->
//! `pleochroma_formats::asc::to_asc_string` -> `native::catalogue_asc` ->
//! `native::design_to_string` -> `native::design_from_str` -> `resolve_after_edit` for one
//! `Edit::MoveTier` -> `optimize_design` with a tiny evaluation budget.
//!
//! Each `Design::solve` on CrackOtto-Step takes over a second, and `optimize`'s module docs
//! explain why no incremental resolve helps. So this file solves it directly and never
//! repeatedly. `optimize_design` runs on it without a forced free tier, which exercises the
//! baseline-scoring and zero-free-tiers early return. The coordinate-search loop gets its
//! coverage from the templates, where [`pick_adoptable_free_tier`] frees one tier. Not every
//! template can take that safely. Deterministic: every stage folds a checksum from solved
//! masts, exported text lengths and search outcomes, so two runs print identical checksums.
//! The whole run keeps under roughly 30 s.

use pleochroma::{
    geometry::meet_solver::{self, Block, MeetConstraint},
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{
    Design, Edit, OptimizeConfig, PreformSpec, SearchHooks, catalogue_asc, free_tier_indices,
    native::{DesignExtras, design_from_str, design_to_string},
    optimize_design, resolve_after_edit, templates,
};
use std::time::Instant;

/// The CrackOtto-Step cost-probe fixture, included directly (not copied) from the file the
/// `optimize` cost probe measures.
const CRACKOTTO_ASC: &str = include_str!("../src/optimize_cost_probe_crackotto_step.asc");

/// Picks one tier `train_one_design` can safely adopt away from
/// `MeetConstraint::ScaleReference` so `optimize_design`'s free-tier coordinate-search
/// loop has something to move.
///
/// Not just any non-girdle tier. Every tier starts pinned `ScaleReference` (see [`templates`]
/// and `Design::from_asc_schedule`), and `Design::solve` requires at least one `ScaleReference`
/// tier per crown, pavilion and girdle block that has tiers (see `crate::design`'s module docs,
/// "Scale anchoring"). Freeing a block's only anchor makes the baseline solve fail instead of
/// running the search, as it does for the "Simple Teaching Design" template, whose crown block
/// is a single tier. So this picks only a tier whose block keeps at least one other
/// `ScaleReference` tier, classified with [`meet_solver::classify_blocks`] (the classification
/// `Design::solve` checks against). `None` when no tier qualifies: the caller leaves `design`
/// untouched and `optimize_design` takes its zero-free-tiers early return, as for CrackOtto-Step.
fn pick_adoptable_free_tier(design: &Design) -> Option<usize> {
    let inputs = design.meet_tier_inputs();
    let blocks = meet_solver::classify_blocks(&inputs);
    let anchor_count = |target: Block| {
        inputs
            .iter()
            .zip(&blocks)
            .filter(|(t, b)| {
                **b == target && matches!(t.constraint, MeetConstraint::ScaleReference(_))
            })
            .count()
    };
    let counts = [
        (Block::Crown, anchor_count(Block::Crown)),
        (Block::Pavilion, anchor_count(Block::Pavilion)),
        (Block::Girdle, anchor_count(Block::Girdle)),
    ];
    inputs.iter().zip(&blocks).position(|(t, b)| {
        matches!(t.constraint, MeetConstraint::ScaleReference(_))
            // The optimizer's own free-tier angle rule (neither horizontal nor
            // vertical, see `free_tier_indices`): a table or culet adopted here would
            // still not be free, and the search loop would never run.
            && (1e-9..89.5).contains(&t.angle_deg.abs())
            && counts.iter().any(|&(block, n)| block == *b && n >= 2)
    })
}

/// Runs the full pipeline (see this file's module doc comment) against one design,
/// folding a checksum of everything it touches into `checksum`.
///
/// `original_asc` is the real `.asc` text `design` was imported from, if any (for
/// `catalogue_asc`'s preserve-original-text path); `None` for a template built
/// straight from [`templates::TemplateSpec::tiers`], which never had one.
///
/// `force_free_tier`, when `true`, adopts one tier's constraint away from
/// `ScaleReference` before calling [`optimize_design`] so its coordinate-search loop
/// actually runs (every template and every `Design::from_asc_schedule` import pins
/// every tier to `ScaleReference` -- see [`templates`]'s and
/// [`Design::from_asc_schedule`]'s own doc comments -- so without this,
/// `optimize_design` always takes its zero-free-tiers early return). `false` for
/// CrackOtto-Step -- see this file's module doc comment for why.
///
/// # Panics
///
/// If any pipeline step fails. Every step must succeed for a template or a well-formed design
/// (see `templates::tests::every_template_solves_and_closes`), so a failure is a regression
/// that this run reports loudly rather than skips.
fn train_one_design(
    name: &str,
    design: &Design,
    original_asc: Option<&str>,
    force_free_tier: bool,
    checksum: &mut f64,
) {
    let t0 = Instant::now();

    let solved = design
        .solve()
        .unwrap_or_else(|e| panic!("{name}: Design::solve failed: {e}"));
    *checksum += solved.iter().map(|s| s.mast).sum::<f64>();

    let schedule = design.to_asc_schedule_from_solved(&solved);
    let asc_text =
        pleochroma_formats::asc::to_asc_string(&schedule).expect("training design writes");
    *checksum = (asc_text.len() as f64).mul_add(1e-3, *checksum);

    let saved = catalogue_asc(design, original_asc, None)
        .unwrap_or_else(|e| panic!("{name}: catalogue_asc failed: {e}"));
    *checksum = (saved.asc_text.len() as f64).mul_add(1e-3, *checksum);

    let design_text = design_to_string(design, None, &DesignExtras::default())
        .unwrap_or_else(|e| panic!("{name}: design_to_string failed: {e}"));
    *checksum = (design_text.len() as f64).mul_add(1e-3, *checksum);

    let loaded = design_from_str(&design_text)
        .unwrap_or_else(|e| panic!("{name}: design_from_str failed: {e}"));
    let mut reloaded = loaded.design;
    *checksum += reloaded.tiers.len() as f64;

    // `resolve_after_edit` for one `Edit::MoveTier` -- swap the last two tiers (a
    // real, index-renumbering move whenever there are at least two tiers). `solved`
    // (from the very first `design.solve()` above) is a valid `previous` baseline
    // for `reloaded` too: the design file round-trips every tier's
    // angle/indices/constraint losslessly, so `reloaded` solves identically to
    // `design` -- reusing it here avoids yet another full solve on a design as
    // heavy as CrackOtto-Step.
    if reloaded.tiers.len() >= 2 {
        let from = reloaded.tiers.len() - 1;
        let to = reloaded.tiers.len() - 2;
        let edit = Edit::MoveTier { from, to };
        reloaded
            .apply_edit(edit.clone())
            .unwrap_or_else(|e| panic!("{name}: MoveTier failed: {e}"));
        let resolved = resolve_after_edit(&reloaded, &solved, &edit)
            .unwrap_or_else(|e| panic!("{name}: resolve_after_edit failed: {e}"));
        *checksum += resolved.iter().map(|s| s.mast).sum::<f64>();
    }

    // `optimize_design` with a tiny budget -- see this function's own doc comment
    // for why `force_free_tier` is `false` for CrackOtto-Step, and
    // `pick_adoptable_free_tier`'s own doc comment for why not every template with
    // `force_free_tier: true` actually gets a free tier either.
    if force_free_tier && let Some(index) = pick_adoptable_free_tier(&reloaded) {
        reloaded.tiers[index].constraint = MeetConstraint::MeetExisting;
    }
    let material = GemMaterial::diamond();
    let config = OptimizeConfig {
        max_evaluations: 4,
        polish_start_step_deg: None,
        ..OptimizeConfig::default()
    };
    let outcome = optimize_design(&reloaded, &material, &config, &SearchHooks::default())
        .unwrap_or_else(|e| panic!("{name}: optimize_design failed: {e}"));
    *checksum =
        (f64::from(outcome.after_score) + outcome.evaluations as f64).mul_add(1e-3, *checksum);

    println!(
        "  cut-core: {name}: {:.2?} ({} free tier(s), {} optimize evaluation(s))",
        t0.elapsed(),
        free_tier_indices(&reloaded).len(),
        outcome.evaluations
    );
}

fn main() {
    let start = Instant::now();
    let mut checksum = 0.0f64;

    for spec in templates::TEMPLATES {
        let design = Design::new(
            PreformSpec::cylinder(spec.gear_teeth.unsigned_abs() as usize, 1.5, 1.0, 1.5),
            spec.schedule_meta(),
            spec.tiers(),
        );
        train_one_design(spec.name, &design, None, true, &mut checksum);
    }

    let crackotto_schedule =
        pleochroma_formats::asc::parse_asc(CRACKOTTO_ASC).expect("CrackOtto-Step fixture parses");
    let crackotto_design = Design::from_asc_schedule(
        PreformSpec::cylinder(crackotto_schedule.gear_teeth_abs() as usize, 1.5, 1.0, 1.5),
        &crackotto_schedule,
    );
    train_one_design(
        "CrackOtto-Step",
        &crackotto_design,
        Some(CRACKOTTO_ASC),
        false,
        &mut checksum,
    );

    println!(
        "pgo_train_cutcore (pleochroma-cut-core) done in {:.1?} (checksum={checksum:.6})",
        start.elapsed()
    );
}
