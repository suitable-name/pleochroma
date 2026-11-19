//! "Your designs that work here": the "Mine" designs whose stored RI range holds the shown
//! material's index.
//!
//! The ranges come from the vault's `ri_ranges` table, filled by the editor's worker for the open
//! design and by the Compute RI Ranges batch (`gui::batch::ri_range`). A design is judged by the
//! low bound alone while `HIGH_BOUND_CALIBRATED` is false, by both bounds once it is true. Every
//! row says it is an estimate. [`designs_for`] is plain data, so the rules are unit tests; [`push`]
//! copies the result into `MaterialReferenceModel`.

use crate::{DesignRow, MaterialReferenceWindow};
use pleochroma_editor::ri_range::{
    HIGH_BOUND_CALIBRATED, RiSource, RiStatement, THRESHOLD_VERSION, range_text,
};
use pleochroma_vault::{db::sqlite::Database, model::ri_range::StoredRiRange};
use slint::{ComponentHandle as _, ModelRc, VecModel};
use std::{
    cell::RefCell,
    sync::{Arc, Mutex, PoisonError},
};
use tracing::warn;

/// How many designs the section lists before it says "and N more".
const MAX_ROWS: usize = 12;

/// Slack for comparing indices that went through floating-point arithmetic.
const EPS: f64 = 1e-9;

thread_local! {
    /// The vault the list reads. Set once by `gui::batch::ri_range::setup`; UI-thread only.
    static DATABASE: RefCell<Option<Arc<Mutex<Database>>>> = const { RefCell::new(None) };
}

/// Gives the section the vault it reads its ranges from.
pub(in crate::gui) fn set_database(db: &Arc<Mutex<Database>>) {
    DATABASE.with(|cell| *cell.borrow_mut() = Some(Arc::clone(db)));
}

/// Forgets the vault, so a previous main window's database connection can close.
pub(super) fn release() {
    let old = DATABASE.with(|cell| cell.borrow_mut().take());
    drop(old);
}

/// Whether the section holds a vault. UI harness only.
#[cfg(feature = "ui-harness")]
pub(super) fn holds_database() -> bool {
    DATABASE.with(|cell| cell.borrow().is_some())
}

/// What the section shows, as `MaterialReferenceModel.design_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum State {
    /// The designs that work, in `rows`.
    List,
    /// No range is computed yet: the section offers the batch.
    NoneComputed,
    /// Ranges exist, but none holds this material.
    NoneWork,
}

impl State {
    /// The value of `MaterialReferenceModel.design_state`.
    const fn index(self) -> i32 {
        match self {
            Self::List => 0,
            Self::NoneComputed => 1,
            Self::NoneWork => 2,
        }
    }
}

/// One listed design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    /// The library entry id.
    pub(super) id: i64,
    /// The design's title.
    pub(super) title: String,
    /// "RI 1.63 and up (estimate)".
    pub(super) range: String,
}

/// The section's content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DesignList {
    /// What to show.
    pub(super) state: State,
    /// At most [`MAX_ROWS`] designs.
    pub(super) entries: Vec<Entry>,
    /// How many more designs work than are listed.
    pub(super) more: usize,
}

/// Whether the stored range holds index `n`: the low bound, and the high one when `calibrated`.
/// The index is compared at the hundredth, as `RiRange::contains_as` does.
fn works_in(stored: &StoredRiRange, n: f64, calibrated: bool) -> bool {
    let n = (n * 100.0).round() / 100.0;
    let Some(low) = stored.n_low else {
        return false;
    };
    if n + EPS < low {
        return false;
    }
    !calibrated || stored.n_high.is_none_or(|high| n <= high + EPS)
}

/// How a stored range prints: "RI 1.63 and up (estimate)", with the high end only when
/// `calibrated`.
fn range_line(stored: &StoredRiRange, low: f64, calibrated: bool) -> String {
    let statement = RiStatement {
        low,
        high: if calibrated { stored.n_high } else { None },
        source: RiSource::Estimate,
    };
    format!("RI {} (estimate)", range_text(&statement))
}

/// The designs of `ranges` ("Mine" designs with a stored range, in the order given) that work in
/// a material of index `n`.
pub(super) fn designs_for(
    ranges: &[(i64, String, StoredRiRange)],
    n: f64,
    calibrated: bool,
) -> DesignList {
    if ranges.is_empty() {
        return DesignList {
            state: State::NoneComputed,
            entries: Vec::new(),
            more: 0,
        };
    }
    let mut working: Vec<Entry> = ranges
        .iter()
        .filter(|(_, _, stored)| works_in(stored, n, calibrated))
        .filter_map(|(id, title, stored)| {
            Some(Entry {
                id: *id,
                title: title.clone(),
                range: range_line(stored, stored.n_low?, calibrated),
            })
        })
        .collect();
    if working.is_empty() {
        return DesignList {
            state: State::NoneWork,
            entries: Vec::new(),
            more: 0,
        };
    }
    let more = working.len().saturating_sub(MAX_ROWS);
    working.truncate(MAX_ROWS);
    DesignList {
        state: State::List,
        entries: working,
        more,
    }
}

/// The stored ranges of the current thresholds with their designs' titles; empty when there is
/// no vault or it cannot be read.
fn stored_ranges() -> Vec<(i64, String, StoredRiRange)> {
    let Some(db) = DATABASE.with(|cell| cell.borrow().clone()) else {
        return Vec::new();
    };
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    let ranges = guard
        .mine_ri_ranges(i64::from(THRESHOLD_VERSION))
        .unwrap_or_else(|error| {
            warn!("Could not read the stored RI ranges: {error}");
            Vec::new()
        });
    drop(guard);
    ranges
}

/// Fills the section of `window` for a material of index `ri` (`None`: no material shown).
pub(super) fn push(window: &MaterialReferenceWindow, ri: Option<f64>) {
    let list = ri.map_or(
        DesignList {
            state: State::NoneComputed,
            entries: Vec::new(),
            more: 0,
        },
        |n| designs_for(&stored_ranges(), n, HIGH_BOUND_CALIBRATED),
    );
    let rows: Vec<DesignRow> = list
        .entries
        .iter()
        .filter_map(|entry| {
            Some(DesignRow {
                id: i32::try_from(entry.id).ok()?,
                title: entry.title.as_str().into(),
                range: entry.range.as_str().into(),
            })
        })
        .collect();
    let model = window.global::<crate::MaterialReferenceModel>();
    model.set_design_rows(ModelRc::new(VecModel::from(rows)));
    model.set_design_more(i32::try_from(list.more).unwrap_or(i32::MAX));
    model.set_design_state(list.state.index());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(entry_id: i64, n_low: Option<f64>, n_high: Option<f64>) -> StoredRiRange {
        StoredRiRange {
            entry_id,
            planes_hash: 0,
            threshold_version: i64::from(THRESHOLD_VERSION),
            n_low,
            n_high,
        }
    }

    fn ranges(bounds: &[(Option<f64>, Option<f64>)]) -> Vec<(i64, String, StoredRiRange)> {
        bounds
            .iter()
            .enumerate()
            .map(|(i, (low, high))| {
                let id = i64::try_from(i).expect("a small index") + 1;
                (id, format!("Design {id}"), stored(id, *low, *high))
            })
            .collect()
    }

    /// No stored range at all offers the batch.
    #[test]
    fn without_ranges_the_section_offers_the_batch() {
        let list = designs_for(&[], 1.768, false);
        assert_eq!(list.state, State::NoneComputed);
        assert!(list.entries.is_empty());
    }

    /// Uncalibrated, the low bound alone decides, and every row says estimate.
    #[test]
    fn uncalibrated_lists_judge_by_the_low_bound_only() {
        let all = ranges(&[
            (Some(1.63), None),
            (Some(1.80), None),
            (Some(1.50), Some(1.60)),
        ]);
        let list = designs_for(&all, 1.768, false);
        assert_eq!(list.state, State::List);
        let ids: Vec<i64> = list.entries.iter().map(|entry| entry.id).collect();
        assert_eq!(ids, vec![1, 3], "design 3's high end 1.60 is not used");
        assert_eq!(list.entries[0].range, "RI 1.63 and up (estimate)");
        assert_eq!(list.entries[1].range, "RI 1.50 and up (estimate)");
    }

    /// Calibrated, both bounds decide and the row prints both.
    #[test]
    fn calibrated_lists_judge_by_both_bounds() {
        let all = ranges(&[(Some(1.63), Some(2.31)), (Some(1.50), Some(1.60))]);
        let list = designs_for(&all, 1.768, true);
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].id, 1);
        assert_eq!(list.entries[0].range, "RI 1.63 to 2.31 (estimate)");
    }

    /// The index is compared at the hundredth: Sapphire 1.768 passes a low bound of 1.77.
    #[test]
    fn the_index_is_compared_at_the_hundredth() {
        let all = ranges(&[(Some(1.77), None)]);
        assert_eq!(designs_for(&all, 1.768, false).state, State::List);
        assert_eq!(designs_for(&all, 1.764, false).state, State::NoneWork);
    }

    /// A stored range with no low bound (a stone with no geometry) never works anywhere.
    #[test]
    fn a_range_without_a_low_bound_is_never_listed() {
        let all = ranges(&[(None, None)]);
        assert_eq!(designs_for(&all, 1.768, false).state, State::NoneWork);
    }

    /// At most 12 are listed, and the rest are counted.
    #[test]
    fn the_list_stops_at_twelve_and_counts_the_rest() {
        let all = ranges(&[(Some(1.5), None); 15]);
        let list = designs_for(&all, 1.768, false);
        assert_eq!(list.entries.len(), MAX_ROWS);
        assert_eq!(list.more, 3);
        let exactly = designs_for(&ranges(&[(Some(1.5), None); 12]), 1.768, false);
        assert_eq!(exactly.more, 0);
    }

    /// Ranges exist but none holds the material.
    #[test]
    fn ranges_that_do_not_hold_the_material_say_so() {
        let all = ranges(&[(Some(2.10), None)]);
        let list = designs_for(&all, 1.54, false);
        assert_eq!(list.state, State::NoneWork);
        assert_eq!(list.state.index(), 2);
    }
}
