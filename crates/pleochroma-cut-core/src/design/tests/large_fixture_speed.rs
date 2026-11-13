//! Speed and cancellation, on a real, large (103-tier) design: PC 05.115
//! "CrackOtto-Step" (see [`super::fixtures::large_fixture`] for provenance).

use super::fixtures::{design_with_real_meet_structure, large_fixture};
use crate::{
    design::{Design, DesignSolveError},
    preform::PreformSpec,
};
use pleochroma::geometry::meet_solver::{
    MeetConstraint, SolveControl, SolveError, meet_tier_inputs_from_asc,
};

/// Timing of a full `Design::solve` against `Design::resolve_dirty` on PC 05.115
/// "CrackOtto-Step", 103 tiers (see [`super::fixtures::large_fixture`]).
///
/// A full solve takes about 5.9 seconds, which is why the editor has an explicit
/// "Solve" button rather than solving on every keystroke.
///
/// All 103 tiers are implicit `MeetExisting`. After anchoring each block as
/// [`super::fixtures::design_with_real_meet_structure`] does, editing any tier affects
/// nearly the whole non-anchor remainder (see [`crate::resolve::affected_tiers`]). This
/// measures that realistic case, not a best case.
///
/// `#[ignore]`d so the default suite stays fast. Run `cargo test -p pleochroma-cut-core
/// --release -- --ignored --nocapture resolve_dirty_speed_on_a_large_real_design`.
#[test]
#[ignore = "timing measurement, not a correctness check -- run with \
            --release --ignored --nocapture, see doc comment"]
fn resolve_dirty_speed_on_a_large_real_design() {
    let design =
        design_with_real_meet_structure("CrackOtto-Step", large_fixture::PC_05_115_CRACKOTTO_STEP);
    assert_eq!(
        design.tiers.len(),
        103,
        "fixture must have its real tier count"
    );

    let baseline = design.solve().expect("must solve to get a starting point");

    let time = |f: &dyn Fn() -> Vec<pleochroma::geometry::meet_solver::SolvedTier>| {
        let start = std::time::Instant::now();
        let result = f();
        (result, start.elapsed())
    };

    let (_, full_time) = time(&|| design.solve().expect("full solve"));

    // Deliberately avoids tiers 0-12 (exactly +-90 degrees, the "G1".."G13" girdle
    // facets) and tier 50 (exactly 0 degrees, "Table"): a +0.5 edit there
    // reclassifies the tier's crown/pavilion/girdle block entirely, which can
    // remove the very anchor this benchmark's fixture was bootstrapped with --
    // not the case this benchmark exists to time. 20, 60 and 95 sit safely inside
    // one block each (54.6, -70.0, -43.4 degrees).
    for &tier_index in &[20usize, 60, 95] {
        let mut edited = design.clone();
        edited.tiers[tier_index].angle_deg += 0.5;
        let dirty = std::collections::BTreeSet::from([tier_index]);
        let (subgraph_result, subgraph_time) = time(&|| {
            edited
                .resolve_dirty(&baseline, &dirty)
                .expect("subgraph resolve")
        });
        let (full_result, edited_full_time) = time(&|| edited.solve().expect("full solve"));

        for (sub, full) in subgraph_result.iter().zip(&full_result) {
            assert_eq!(sub.mast.to_bits(), full.mast.to_bits());
        }

        println!(
            "CrackOtto-Step (103 tiers): edit tier {tier_index}: full solve \
             {edited_full_time:?}, subgraph resolve {subgraph_time:?} \
             ({:.1}x)",
            edited_full_time.as_secs_f64() / subgraph_time.as_secs_f64().max(1e-12)
        );
    }
    println!("CrackOtto-Step (103 tiers): baseline full solve (unedited): {full_time:?}");

    // The same planes imported the normal way (`Design::from_asc_schedule`), which pins
    // every tier to its recorded mast. Only the edited tier then needs re-solving (see
    // "What this still buys" in `crate::resolve`'s module docs).
    let schedule = pleochroma_formats::asc::parse_asc(large_fixture::PC_05_115_CRACKOTTO_STEP)
        .expect("fixture must parse");
    let imported = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    assert!(
        imported
            .tiers
            .iter()
            .all(|t| matches!(t.constraint, MeetConstraint::ScaleReference(_))),
        "import must pin every tier -- see Design::from_asc_schedule's doc comment"
    );
    let imported_baseline = imported.solve().expect("every tier is its own anchor");

    let mut edited_import = imported.clone();
    edited_import.tiers[20].angle_deg += 0.5;
    let dirty = std::collections::BTreeSet::from([20]);
    let (subgraph_result, import_subgraph_time) = time(&|| {
        edited_import
            .resolve_dirty(&imported_baseline, &dirty)
            .expect("subgraph resolve")
    });
    let (full_result, import_full_time) = time(&|| edited_import.solve().expect("full solve"));
    for (sub, full) in subgraph_result.iter().zip(&full_result) {
        assert_eq!(sub.mast.to_bits(), full.mast.to_bits());
    }
    println!(
        "CrackOtto-Step (103 tiers), FRESHLY IMPORTED (every tier pinned): edit tier 20: \
         full solve {import_full_time:?}, subgraph resolve {import_subgraph_time:?} \
         ({:.0}x -- both cheap: an all-ScaleReference solve never enters \
         meet_solver's expensive candidate search at all, see below)",
        import_full_time.as_secs_f64() / import_subgraph_time.as_secs_f64().max(1e-12)
    );

    // A partially adopted design: the pavilion tiers (52-102) are back on their
    // meet-derived constraints, as after one-click adoption, while the crown and girdle
    // (0-50) stay pinned. A subgraph resolve of a pinned crown tier skips the pavilion;
    // a full solve re-derives it.
    let mut half_adopted = imported;
    let real_pavilion_inputs = meet_tier_inputs_from_asc(&schedule);
    // Tier 51 (the pavilion's first tier) stays exactly as import pinned it, so
    // the pavilion block keeps a real anchor once the rest of it (52-102) is
    // adopted back to genuine `MeetExisting`.
    for (tier, input) in half_adopted.tiers[52..103]
        .iter_mut()
        .zip(&real_pavilion_inputs[52..103])
    {
        tier.constraint = input.constraint.clone();
    }
    let half_adopted_baseline = half_adopted
        .solve()
        .expect("pavilion's own first tier is still that block's ScaleReference anchor");

    let mut edited_half = half_adopted.clone();
    edited_half.tiers[20].angle_deg += 0.5; // still-pinned crown tier
    let dirty = std::collections::BTreeSet::from([20]);
    let (subgraph_result, half_subgraph_time) = time(&|| {
        edited_half
            .resolve_dirty(&half_adopted_baseline, &dirty)
            .expect("subgraph resolve")
    });
    let (full_result, half_full_time) = time(&|| edited_half.solve().expect("full solve"));
    for (sub, full) in subgraph_result.iter().zip(&full_result) {
        assert_eq!(sub.mast.to_bits(), full.mast.to_bits());
    }
    println!(
        "CrackOtto-Step (103 tiers), HALF ADOPTED (pavilion meet-derived, crown/girdle \
         still pinned): edit a still-pinned crown tier (20): full solve \
         {half_full_time:?}, subgraph resolve {half_subgraph_time:?} ({:.1}x)",
        half_full_time.as_secs_f64() / half_subgraph_time.as_secs_f64().max(1e-12)
    );
}

/// Cancellation latency on the large fixture of
/// [`resolve_dirty_speed_on_a_large_real_design`].
///
/// [`crate::design::Design::solve_with`] on a background thread must return
/// [`pleochroma::geometry::meet_solver::SolveError::Cancelled`] within 500 ms of a flag
/// set 50 ms in.
///
/// The solver checks for cancellation at fine-grained points (see the "Cancellation and
/// progress" section of `pleochroma`'s meet solver docs), so this holds in any build
/// profile, however long the uncancelled solve would take.
///
/// Not `#[ignore]`d: it is a correctness check, and `recv_timeout` returns as soon as the
/// cancellation is observed rather than waiting for the solve to finish.
#[test]
fn cancel_stops_a_large_real_solve_quickly() {
    let design =
        design_with_real_meet_structure("CrackOtto-Step", large_fixture::PC_05_115_CRACKOTTO_STEP);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::scope(|scope| {
        // Borrow into locals, then `move` those references into the closure: `tx` must
        // move in (`Sender` is `Send` but not `Sync`), and a `move` closure over
        // `design`/`cancel` themselves would take them from the assertions below.
        let design_ref = &design;
        let cancel_ref = &cancel;
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        scope.spawn(move || {
            let control = SolveControl::with_cancel(cancel_ref);
            let _ = ready_tx.send(());
            let result = design_ref.solve_with(&control);
            let _ = tx.send(result);
        });

        ready_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the solver thread must start within 30s");
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);

        let result = rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect(
                "Design::solve_with must observe cancellation and return within 500ms of it \
                 being set -- see pleochroma::geometry::meet_solver's cancel-point granularity",
            );
        assert!(
            matches!(result, Err(DesignSolveError::Solve(SolveError::Cancelled))),
            "expected a cancelled DesignSolveError, got {result:?}"
        );
    });
}
