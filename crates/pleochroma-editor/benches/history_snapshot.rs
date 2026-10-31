//! P14 measurement: what the History tab's refresh copies per edit
//! (`FABLE_perf_verification.md`, item P14).
//!
//! `history_panel::sync` (apps/pleochroma-cut/src/gui/editor/history_panel/mod.rs:205-225)
//! runs on every edit, undo and redo while the tab is visible. It calls
//! `EditorSession::history_entries`, then `EditorSession::history_snapshot` (a deep clone of
//! the design AND the whole history), then `RenderSnapshot::new`, which walks the snapshot's
//! entries once more (worker.rs:47-57). `RenderSnapshot` lives in the app crate and cannot be
//! reached from a bench, so `sync_work` below reproduces its public-API part: entries plus
//! snapshot plus the revision list.
//!
//! Fixtures: a session on the Emerald Step Cut template (a multi-tier step cut; `main`
//! prints its tier count), then 200 history steps.
//!
//! - `small_edits`: 200 `Edit::ModifyTier` steps (a small payload each).
//! - `with_replace_schedule`: the same 200 steps, but every 25th is an
//!   `Edit::ReplaceSchedule` (a boxed whole-schedule payload, 8 of them).
//!
//! `main` prints the step counts actually recorded. The history clone is O(steps); the
//! interesting question is how much the big `ReplaceSchedule` payloads add on top.
//!
//! Run: `cargo bench -p pleochroma-editor --bench history_snapshot`.

use divan::black_box;
use pleochroma_cut_core::{Edit, ScheduleState, templates::TEMPLATES};
use pleochroma_editor::EditorSession;

/// History steps in each fixture.
const STEPS: u32 = 200;
/// Every this-many-th step of the mixed fixture is a `ReplaceSchedule`.
const REPLACE_EVERY: u32 = 25;

fn main() {
    for (label, replaces) in [("small_edits", false), ("with_replace_schedule", true)] {
        let session = session(replaces);
        eprintln!(
            "[history_snapshot] {label}: {} history entries, {} tiers",
            session.history_entries().len(),
            session.design.tiers.len(),
        );
    }
    divan::main();
}

/// A session with `STEPS` recorded steps; `replaces` swaps every `REPLACE_EVERY`-th for a
/// whole-schedule replacement.
fn session(replaces: bool) -> EditorSession {
    let position = TEMPLATES
        .iter()
        .position(|template| template.name == "Emerald Step Cut")
        .expect("the Emerald Step Cut template exists");
    let template = &TEMPLATES[position];
    let index = i32::try_from(position + 1).expect("template index fits i32");
    let mut session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        index,
    );
    for step in 0..STEPS {
        let nudge = 0.001 * f64::from(step + 1);
        let edit = if replaces && step % REPLACE_EVERY == REPLACE_EVERY - 1 {
            let mut state = ScheduleState::of(&session.design);
            state.tiers[2].angle_deg += nudge;
            Edit::ReplaceSchedule(Box::new(state))
        } else {
            let mut tier = session.design.tiers[2].clone();
            tier.angle_deg += nudge;
            Edit::ModifyTier { index: 2, tier }
        };
        session
            .apply(edit)
            .expect("a small angle change must apply");
    }
    session
}

/// `history_snapshot()` alone.
fn snapshot(bencher: divan::Bencher, replaces: bool) {
    let session = session(replaces);
    bencher.bench_local(|| black_box(black_box(&session).history_snapshot()));
}

/// Everything `sync` does on the session side: the entry list, the snapshot, and the
/// revision list `RenderSnapshot::new` builds from the snapshot's entries.
fn sync_work(bencher: divan::Bencher, replaces: bool) {
    let session = session(replaces);
    bencher.bench_local(|| {
        let entries = black_box(&session).history_entries();
        let position = session.history_position();
        let snapshot = session.history_snapshot();
        let revisions: Vec<u64> = std::iter::once(0)
            .chain(snapshot.entries().iter().map(|entry| entry.revision))
            .collect();
        black_box((entries, position, snapshot, revisions))
    });
}

/// `history_snapshot()`, 200 small edits.
#[divan::bench(sample_count = 200)]
fn snapshot_small_edits(bencher: divan::Bencher) {
    snapshot(bencher, false);
}

/// `history_snapshot()`, 200 steps with 8 `ReplaceSchedule`.
#[divan::bench(sample_count = 200)]
fn snapshot_with_replace_schedule(bencher: divan::Bencher) {
    snapshot(bencher, true);
}

/// The session-side work of one `sync`, 200 small edits.
#[divan::bench(sample_count = 200)]
fn sync_small_edits(bencher: divan::Bencher) {
    sync_work(bencher, false);
}

/// The session-side work of one `sync`, 200 steps with 8 `ReplaceSchedule`.
#[divan::bench(sample_count = 200)]
fn sync_with_replace_schedule(bencher: divan::Bencher) {
    sync_work(bencher, true);
}
