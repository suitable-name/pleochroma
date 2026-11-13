//! Corpus-wide measurement for `pleochroma_cut_core::manufacturability` checks.
//!
//! Reads real `.asc` files directly out of the user's own `facet_diagrams.sqlite`
//! catalogue (one per design, deduplicated by `detail_id`, the same pattern
//! `crates/pleochroma/examples/meet_solver_validation/main.rs` uses) and reports how many
//! designs trip each check. It lives in `examples/`, not the test suite, because it needs a
//! large catalogue this repository does not ship.
//!
//! Opened strictly read-only (`OpenFlags::SQLITE_OPEN_READ_ONLY`): the catalogue is the
//! user's irreplaceable data, and this probe only runs `SELECT`.
//!
//! # Two very different costs, measured separately
//!
//! Checks 3 (gear quantization) and 4 (cut order) need no solved mast (see
//! `pleochroma_cut_core::manufacturability`'s module doc comment). They read straight off a
//! parsed `.asc` schedule's `indices` and meet-instruction text, so this probe runs them over
//! every design with an attached `.asc` file at the cost of a parse and a name-resolution pass.
//!
//! Checks 1 (vanishing facets) and 2 (undersized facets) need a real solved plane arrangement.
//! A design imported with `Design::from_asc_schedule` (which pins every tier to its recorded
//! mast; see that function's doc comment) solves in about 90 ms, since an all-`ScaleReference`
//! design never enters `meet_solver`'s candidate search. The probe therefore runs these two over
//! a bounded sample to keep its wall-clock time short, not because the per-design cost is high.
//!
//! Run from `private/`:
//! ```text
//! cargo run -p pleochroma-cut-core --release --example manufacturability_corpus_probe
//! ```

use pleochroma_cut_core::{
    Design, PreformSpec,
    manufacturability::{
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, ManufacturabilityWarning, check_cut_order,
        check_gear_quantization, check_manufacturability,
    },
};
use rusqlite::{Connection, OpenFlags};
use std::time::Instant;

/// Bound on how many designs get a real solve (checks 1 and 2). A wall-clock bound on the
/// probe itself, not a per-design cost limit (see the module docs).
const SOLVED_SAMPLE_SIZE: usize = 300;

fn find_db_path() -> String {
    for candidate in [
        "facet_diagrams.sqlite",
        "../facet_diagrams.sqlite",
        "../../facet_diagrams.sqlite",
    ] {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "facet_diagrams.sqlite".to_string()
}

struct AscRow {
    detail_id: i64,
    content: String,
}

fn load_asc_rows(conn: &Connection) -> Vec<AscRow> {
    let mut stmt = conn
        .prepare(
            "SELECT af.detail_id, af.content \
             FROM attached_files af \
             WHERE af.name LIKE '%.asc' \
             ORDER BY af.detail_id, af.id",
        )
        .expect("prepare attached_files query");
    // `attached_files.content` is declared BLOB (raw bytes), not TEXT -- real `.asc`
    // files are plain ASCII/UTF-8 text stored as bytes, so this reads the column as
    // `Vec<u8>` and decodes it, rather than asking rusqlite to coerce a blob-typed
    // value straight into a `String` (which fails the type check and would silently
    // drop every row through the `filter_map(Result::ok)` below).
    let rows: Vec<AscRow> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .expect("query attached_files")
        .filter_map(Result::ok)
        .map(|(detail_id, bytes)| AscRow {
            detail_id,
            content: String::from_utf8_lossy(&bytes).into_owned(),
        })
        .collect();

    // One `.asc` per design: a design can have more than one attached file (revisions,
    // alternate cuts), and the dedup rule is the one `meet_solver_validation` uses.
    let mut seen = std::collections::BTreeSet::new();
    rows.into_iter()
        .filter(|r| seen.insert(r.detail_id))
        .collect()
}

fn main() {
    let db_path = find_db_path();
    println!("Using database (read-only): {db_path}");
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open facet_diagrams.sqlite read-only");

    let rows = load_asc_rows(&conn);
    println!("{} unique designs with an attached .asc file\n", rows.len());

    run_solve_free_checks(&rows);
    run_solve_dependent_checks(&rows);
}

/// Checks 3 and 4, over every design that parses -- no `Design`/solve involved at
/// all (see the module doc comment).
fn run_solve_free_checks(rows: &[AscRow]) {
    let start = Instant::now();
    let mut parsed = 0usize;
    let mut with_fractional_index = 0usize;
    let mut fractional_index_tokens = 0usize;
    let mut total_index_tokens = 0usize;
    let mut with_out_of_order_meet = 0usize;
    let mut out_of_order_meet_tiers = 0usize;
    let mut with_any_meet_named = 0usize;
    let mut gear_example: Option<(i64, ManufacturabilityWarning)> = None;
    let mut order_example: Option<(i64, ManufacturabilityWarning)> = None;

    for row in rows {
        let Ok(schedule) = pleochroma_formats::asc::parse_asc(&row.content) else {
            continue;
        };
        parsed += 1;

        // Check 3 needs only the tiers and meta. `from_asc_schedule` builds a real `Design`
        // without a second tier-construction path, and costs nothing extra (no solve here).
        let design = Design::from_asc_schedule(PreformSpec::block(1.0, 1.0, 1.0), &schedule);
        let gear_warnings = check_gear_quantization(&design);
        if !gear_warnings.is_empty() {
            with_fractional_index += 1;
            if gear_example.is_none() {
                gear_example = Some((row.detail_id, gear_warnings[0].clone()));
            }
        }
        fractional_index_tokens += gear_warnings.len();
        total_index_tokens += schedule
            .tiers
            .iter()
            .map(|t| t.indices.len())
            .sum::<usize>();

        // Check 4 needs the schedule's own meet classification (`MeetNamed`, `MeetExisting`,
        // `ScaleReference` from the real `G`-field text), not the pinned import of
        // `from_asc_schedule`, which can never trip this check (see that method's doc).
        // `check_cut_order` reads only `design.tiers` and `design.meta`, so no solve runs.
        let genuine = genuine_meet_design(&schedule);
        let has_named = genuine.tiers.iter().any(|t| {
            matches!(
                t.constraint,
                pleochroma::geometry::meet_solver::MeetConstraint::MeetNamed(_)
            )
        });
        if has_named {
            with_any_meet_named += 1;
        }
        let order_warnings = check_cut_order(&genuine);
        if !order_warnings.is_empty() {
            with_out_of_order_meet += 1;
            if order_example.is_none() {
                order_example = Some((row.detail_id, order_warnings[0].clone()));
            }
        }
        out_of_order_meet_tiers += order_warnings.len();
    }

    let elapsed = start.elapsed();
    println!("=== Checks 3+4 (no solve needed) -- full corpus ===");
    println!("{parsed} designs parsed in {elapsed:?}");
    println!(
        "Check 3 (gear quantization): {with_fractional_index}/{parsed} designs \
         ({:.2}%) have >=1 fractional index; {fractional_index_tokens}/{total_index_tokens} \
         index tokens overall ({:.3}%) -- expected corpus figure: ~0.2% of tokens",
        pct(with_fractional_index, parsed),
        pct(fractional_index_tokens, total_index_tokens)
    );
    println!(
        "Check 4 (cut order): {with_any_meet_named}/{parsed} designs ({:.2}%) have >=1 real \
         MeetNamed tier once genuine meet structure is reconstructed; of those, \
         {with_out_of_order_meet}/{parsed} designs ({:.2}%) have >=1 forward/self reference \
         ({out_of_order_meet_tiers} tier(s) total)",
        pct(with_any_meet_named, parsed),
        pct(with_out_of_order_meet, parsed)
    );
    if let Some((detail_id, w)) = gear_example {
        println!("  example (detail_id={detail_id}): {w}");
    }
    if let Some((detail_id, w)) = order_example {
        println!("  example (detail_id={detail_id}): {w}");
    }
    println!();
}

/// Checks 1 and 2 over a bounded sample of designs imported with `Design::from_asc_schedule`.
fn run_solve_dependent_checks(rows: &[AscRow]) {
    let sample: Vec<&AscRow> = rows.iter().take(SOLVED_SAMPLE_SIZE).collect();
    let start = Instant::now();

    let mut solved_ok = 0usize;
    let mut missing_anchor = 0usize;
    let mut not_closed = 0usize;
    let mut with_vanishing = 0usize;
    let mut with_undersized = 0usize;
    let mut vanishing_tiers = 0usize;
    let mut undersized_facets = 0usize;
    let mut vanishing_example: Option<(i64, ManufacturabilityWarning)> = None;
    let mut undersized_example: Option<(i64, ManufacturabilityWarning)> = None;

    for row in &sample {
        let Ok(schedule) = pleochroma_formats::asc::parse_asc(&row.content) else {
            continue;
        };
        // A generous CYLINDER preform, not a block: most real designs are round or near-round,
        // and a block's flat sides and sharp corners can swallow a girdle-tier facet that a
        // round rough keeps. That is a preform artifact, not a schedule problem.
        // `cylinder_for_schedule` sides the prism to the schedule's gear-tooth count. Sized from
        // the schedule's largest mast so the rough itself never fails to close.
        let max_mast = schedule
            .tiers
            .iter()
            .map(|t| t.mast.abs())
            .fold(1.0_f64, f64::max);
        let preform =
            PreformSpec::cylinder_for_schedule(&schedule, max_mast * 2.0, 1.0, max_mast * 4.0);
        let design = Design::from_asc_schedule(preform, &schedule);

        let Ok(solved) = design.solve() else {
            missing_anchor += 1;
            continue;
        };
        solved_ok += 1;

        let planes = design.planes_from_solved(&solved);
        if !matches!(
            pleochroma::geometry::stone_metrics::build_solid_mesh(&planes),
            pleochroma::geometry::stone_metrics::SolidStatus::Closed(_)
        ) {
            not_closed += 1;
            continue;
        }

        let warnings =
            check_manufacturability(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2);
        let this_vanishing = warnings
            .iter()
            .filter(|w| matches!(w, ManufacturabilityWarning::VanishingFacet { .. }))
            .count();
        let this_undersized = warnings
            .iter()
            .filter(|w| matches!(w, ManufacturabilityWarning::UndersizedFacet { .. }))
            .count();
        if this_vanishing > 0 {
            with_vanishing += 1;
            if vanishing_example.is_none() {
                vanishing_example = warnings
                    .iter()
                    .find(|w| matches!(w, ManufacturabilityWarning::VanishingFacet { .. }))
                    .map(|w| (row.detail_id, w.clone()));
            }
        }
        if this_undersized > 0 {
            with_undersized += 1;
            if undersized_example.is_none() {
                undersized_example = warnings
                    .iter()
                    .find(|w| matches!(w, ManufacturabilityWarning::UndersizedFacet { .. }))
                    .map(|w| (row.detail_id, w.clone()));
            }
        }
        vanishing_tiers += this_vanishing;
        undersized_facets += this_undersized;
    }

    let elapsed = start.elapsed();
    let per_design = elapsed.as_secs_f64() / sample.len().max(1) as f64;
    println!(
        "=== Checks 1+2 (need a solve) -- sample of {} designs ===",
        sample.len()
    );
    println!(
        "{elapsed:?} total, {per_design:.4}s/design average; {solved_ok} solved, \
         {missing_anchor} MissingAnchor, {not_closed} solved-but-not-closed"
    );
    println!(
        "Check 1 (vanishing facets): {with_vanishing}/{solved_ok} closed designs ({:.2}%) \
         have >=1 vanished facet ({vanishing_tiers} tier(s) total)",
        pct(with_vanishing, solved_ok)
    );
    println!(
        "Check 2 (undersized facets, threshold={:.0e} of W^2): {with_undersized}/{solved_ok} \
         closed designs ({:.2}%) have >=1 undersized facet ({undersized_facets} facet(s) total)",
        DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2,
        pct(with_undersized, solved_ok)
    );
    if let Some((detail_id, w)) = vanishing_example {
        println!("  example (detail_id={detail_id}): {w}");
    }
    if let Some((detail_id, w)) = undersized_example {
        println!("  example (detail_id={detail_id}): {w}");
    }
}

/// Rebuilds a [`Design`] with the schedule's own meet classification (`MeetNamed`,
/// `MeetExisting`, `ScaleReference` as `meet_tier_inputs_from_asc` reads them), not the
/// pinned import of `Design::from_asc_schedule`. Each crown, pavilion and girdle block
/// without a scale anchor gets one, at its first tier's recorded mast. This is the technique
/// of the `design_with_real_meet_structure` helper in `src/design/tests/fixtures.rs`, which is
/// private to that test tree, so it is repeated here against the public `meet_solver` API.
///
/// Never calls `solve()`: `check_cut_order` reads only `design.tiers` and `design.meta`.
fn genuine_meet_design(schedule: &pleochroma_formats::asc::AscSchedule) -> Design {
    use pleochroma::geometry::meet_solver::{
        Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc,
    };

    let mut inputs = meet_tier_inputs_from_asc(schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| pleochroma_cut_core::ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        pleochroma_cut_core::ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes.clone(),
        },
        tiers,
    )
}

fn pct(n: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        100.0 * n as f64 / total as f64
    }
}
