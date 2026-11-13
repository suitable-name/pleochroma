//! Corpus-wide validation harness for `pleochroma::geometry::meet_solver`.
//!
//! Reads every real `.asc` file from `facet_diagrams.sqlite` (one per design, deduplicated
//! by `detail_id`, keeping the first row). It blanks only the *masts*; angles, indices, gear
//! and meet text stay. [`meet_solver::solve_meet_points`] then re-derives the masts, and the
//! result is compared with the file's recorded masts, which serve as ground truth.
//!
//! # Why this lives in `examples/`
//!
//! It needs a real catalogue of thousands of designs, and `facet_diagrams.sqlite` is the
//! user's own library, not shipped with the repository. A `#[test]` cannot depend on data
//! that may be absent. The solver's verify acceptance tolerance is judged against this
//! harness's full-corpus runs (see `geometry::meet_solver::verify`), so the harness is what
//! re-checks it.
//!
//! The solver's unit tests cover synthetic cases. This harness covers whether the solver
//! reproduces real recorded masts.
//!
//! Run from the workspace root:
//! ```text
//! cargo run --profile probe -p pleochroma --example meet_solver_validation
//! ```
//!
//! Reports A (recorded-mast anchors) and B (printed-ratio anchors) always run, at about
//! 45 s each on the full corpus. Append `verified` to also run Reports C and D, which
//! add the externally verified repair search scored against the printed `Vol/W^3`, `L/W`,
//! `C/W`, `P/W` and `H/W` figures. Each design runs up to ~120 pipelines, so the full
//! corpus takes about 25-35 minutes:
//! ```text
//! cargo run --profile probe -p pleochroma --example meet_solver_validation -- verified
//! ```
//!
//! # Parallelism
//!
//! Each design's solve is independent. All rows are read on the main thread first (the
//! sqlite read is cheap; the candidate-vertex enumeration is the cost), then
//! `std::thread::scope` workers take rows in interleaved order, which balances load when
//! per-design cost varies by orders of magnitude. Plain `std::thread` is used rather than
//! `rayon`, because `pleochroma` is kept near-dependency-free. Results are re-slotted by
//! original row index, so the reported statistics do not depend on thread scheduling.

use rusqlite::Connection;
use std::collections::HashSet;

mod db;
mod print_report;
mod run_and_report;
mod scoring;
mod solve_ratio_anchored;
mod solve_real_mast;
mod types;
mod verified_extras;

use db::{find_db_path, load_asc_rows};
use run_and_report::run_and_report;
use solve_ratio_anchored::{solve_one_ratio_anchored, solve_one_ratio_anchored_verified};
use solve_real_mast::{solve_one, solve_one_verified};
use types::AscRow;
use verified_extras::{count_ratio_coverage_for_designs_without_asc, print_verified_extras};

fn main() {
    let db_path = find_db_path();
    println!("Using database: {db_path}");
    let conn = Connection::open(&db_path).expect("open facet_diagrams.sqlite");
    let rows = load_asc_rows(&conn);
    println!(
        "Loaded {} attached .asc rows from the database.",
        rows.len()
    );

    // Dedup by detail_id, keeping the first row per design. Sequential, so the processing
    // order is fixed across runs.
    let mut seen_designs: HashSet<i64> = HashSet::new();
    let unique_rows: Vec<AscRow> = rows
        .into_iter()
        .filter(|row| seen_designs.insert(row.detail_id))
        .collect();
    println!(
        "{} unique designs after dedup by detail_id.",
        unique_rows.len()
    );

    let with_crown_ratio = unique_rows.iter().filter(|r| r.cw_ratio.is_some()).count();
    let with_pavilion_ratio = unique_rows.iter().filter(|r| r.pw_ratio.is_some()).count();
    let with_both = unique_rows
        .iter()
        .filter(|r| r.cw_ratio.is_some() && r.pw_ratio.is_some())
        .count();
    println!(
        "  of those, {with_crown_ratio} have a printed cw_ratio, {with_pavilion_ratio} a printed pw_ratio, {with_both} both."
    );
    let no_asc_ratio_coverage = count_ratio_coverage_for_designs_without_asc(&conn);
    println!(
        "  designs in diagram_details with NO attached .asc at all: {} total, {} with cw_ratio, \
         {} with pw_ratio, {} with both (the population Item 1 exists to serve).",
        no_asc_ratio_coverage.0,
        no_asc_ratio_coverage.1,
        no_asc_ratio_coverage.2,
        no_asc_ratio_coverage.3
    );

    run_and_report(
        "REPORT A -- baseline: bootstrapped from each file's own tier-0 REAL mast \
         (harness-only crutch; not available without an .asc file)",
        &unique_rows,
        solve_one,
        |d| d.no_scale_reference,
    );
    run_and_report(
        "REPORT B -- production path: anchored from printed C/W, P/W proportions \
         (falls back to tier-0 real mast only when a block's ratio is missing)",
        &unique_rows,
        solve_one_ratio_anchored,
        |d| d.parse_ok && !d.fully_ratio_anchored,
    );

    // Reports C and D are opt-in (`verified` argument), see the module doc.
    if std::env::args().any(|a| a == "verified") {
        let results = run_and_report(
            "REPORT C -- externally verified: Report A anchoring plus the printed-proportion \
             repair search (solve_meet_points_verified; pass `verified` to run this)",
            &unique_rows,
            solve_one_verified,
            |d| d.no_scale_reference,
        );
        print_verified_extras(&results);

        let results_d = run_and_report(
            "REPORT D -- production path, calibrated + verified: printed-ratio anchors marked \
             adjustable and calibrated against the printed figures, then the repair search \
             (solve_meet_points_verified with adjustable anchors; pass `verified` to run this)",
            &unique_rows,
            solve_one_ratio_anchored_verified,
            |d| d.parse_ok && !d.fully_ratio_anchored,
        );
        print_verified_extras(&results_d);
    }
}
