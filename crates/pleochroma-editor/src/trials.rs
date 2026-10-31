//! Trials: the numbered list of the states a cutter tried by hand, with their figures, to
//! compare, restore or keep. Session only: nothing here is written to the library.
//!
//! This is the toolkit-free core; the desktop holds one [`TrialList`] per window and maps
//! [`TrialRowView`] to its Slint rows.
//!
//! - [`TrialList`] numbers the trials (numbers only go up), caps the list at
//!   [`MAX_TRIALS`] (the oldest unstarred trial drops first), and keeps notes and stars.
//! - [`trial_label`] writes the automatic label from the tier diff: "P1 40.75 → 41.25".
//! - [`restore_edit`] turns a trial into the one-step edit that brings its design back,
//!   refusing a trial of a design that is no longer open.
//! - [`trial_rows`] is the table: per-figure text, the change against the Baseline (the
//!   [`crate::live_deltas`] rule), the origin, the star and the best mark per column.

mod rows;
#[cfg(test)]
mod tests;

pub use rows::{BestMarks, TrialCell, TrialRowView, best_marks, trial_rows, trial_rows_sorted};

use crate::{live_deltas::DeltaFigure, retarget::plan::tier_display_name, sweep::RowMetrics};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::{Design, Edit, diff_concave_tiers, diff_tiers};

/// The most trials a list holds.
pub const MAX_TRIALS: usize = 50;

/// How many changes an automatic label names before it says "and N more".
const LABEL_CHANGES: usize = 3;

/// The refusal for a trial of another design.
const OTHER_DESIGN: &str = "This trial belongs to a design that is no longer open.";

/// Where a trial came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrialOrigin {
    /// Kept by hand ("Keep as Trial") or recorded after an edit.
    Manual,
    /// An angle taken from a one-tier sweep.
    Sweep,
    /// A pair of angles taken from a two-tier sweep.
    TwoTierSweep,
    /// A candidate applied from Optimize.
    OptimizeCandidate,
    /// The design as it was just before an Optimize apply.
    BeforeOptimize,
    /// The design as it was just before a Retarget apply.
    BeforeRetarget,
    /// A tier accepted in Tune.
    Tune,
}

impl TrialOrigin {
    /// The name shown in the table.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manual",
            Self::Sweep => "Sweep",
            Self::TwoTierSweep => "Two-Tier Sweep",
            Self::OptimizeCandidate => "Optimize Candidate",
            Self::BeforeOptimize => "Before Optimize",
            Self::BeforeRetarget => "Before Retarget",
            Self::Tune => "Tune",
        }
    }
}

/// One state the cutter tried.
#[derive(Debug, Clone)]
pub struct Trial {
    /// The trial's number: starts at 1, goes up, never reused while the list lives.
    pub number: u32,
    /// The design, as it was.
    pub design: Design,
    /// Its solved masts, when it had a solve.
    pub solved: Option<Vec<SolvedTier>>,
    /// Its scored figures; `None` when they were not scored (yet).
    pub metrics: Option<RowMetrics>,
    /// The material the figures were scored in; a trial of another material shows its
    /// figures grey until re-scored.
    pub material_key: String,
    /// The epoch of the design the trial belongs to (see [`restore_edit`]).
    pub design_epoch: u64,
    /// The automatic label, from the tier diff ([`trial_label`]).
    pub label: String,
    /// The cutter's own note.
    pub note: String,
    /// Where the trial came from.
    pub origin: TrialOrigin,
    /// Whether the cutter starred it; a starred trial outlives the cap.
    pub starred: bool,
}

/// What a caller hands to [`TrialList::push`]: a trial before it has a number.
#[derive(Debug, Clone)]
pub struct TrialDraft {
    /// The design to keep.
    pub design: Design,
    /// Its solved masts, when it had a solve.
    pub solved: Option<Vec<SolvedTier>>,
    /// Its scored figures, when scored.
    pub metrics: Option<RowMetrics>,
    /// The material the figures were scored in.
    pub material_key: String,
    /// The epoch of the design the trial belongs to.
    pub design_epoch: u64,
    /// The label (see [`trial_label`]).
    pub label: String,
    /// Where the trial came from.
    pub origin: TrialOrigin,
}

/// The order of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialSort {
    /// By trial number, oldest first.
    Number,
    /// By one figure, best first; trials without the figure come last.
    Figure(DeltaFigure),
}

/// The trials of one open design.
#[derive(Debug, Clone)]
pub struct TrialList {
    trials: Vec<Trial>,
    next_number: u32,
}

impl Default for TrialList {
    fn default() -> Self {
        Self::new()
    }
}

impl TrialList {
    /// An empty list; the first trial is number 1.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            trials: Vec::new(),
            next_number: 1,
        }
    }

    /// Adds a trial and returns its number.
    ///
    /// When the list then holds more than [`MAX_TRIALS`], the oldest unstarred trial
    /// drops. If every trial is starred, the oldest one drops as a last resort, so the cap
    /// always holds.
    pub fn push(&mut self, draft: TrialDraft) -> u32 {
        let number = self.next_number;
        self.next_number = self.next_number.saturating_add(1);
        self.trials.push(Trial {
            number,
            design: draft.design,
            solved: draft.solved,
            metrics: draft.metrics,
            material_key: draft.material_key,
            design_epoch: draft.design_epoch,
            label: draft.label,
            note: String::new(),
            origin: draft.origin,
            starred: false,
        });
        while self.trials.len() > MAX_TRIALS {
            let drop = self
                .trials
                .iter()
                .position(|trial| !trial.starred)
                .unwrap_or(0);
            self.trials.remove(drop);
        }
        number
    }

    /// Removes a trial; whether there was one.
    pub fn remove(&mut self, number: u32) -> bool {
        let before = self.trials.len();
        self.trials.retain(|trial| trial.number != number);
        self.trials.len() != before
    }

    /// Sets a trial's note; whether there was such a trial.
    pub fn set_note(&mut self, number: u32, note: &str) -> bool {
        self.get_mut(number).is_some_and(|trial| {
            note.clone_into(&mut trial.note);
            true
        })
    }

    /// Flips a trial's star and returns the new state; `None` when there is no such trial.
    pub fn toggle_star(&mut self, number: u32) -> Option<bool> {
        let trial = self.get_mut(number)?;
        trial.starred = !trial.starred;
        Some(trial.starred)
    }

    /// Stores re-scored figures (and the material they were scored in) on a trial; whether
    /// there was such a trial.
    pub fn set_metrics(
        &mut self,
        number: u32,
        metrics: Option<RowMetrics>,
        material_key: &str,
    ) -> bool {
        self.get_mut(number).is_some_and(|trial| {
            trial.metrics = metrics;
            material_key.clone_into(&mut trial.material_key);
            true
        })
    }

    /// Empties the list, for a newly opened design. Numbering starts again at 1.
    pub fn clear(&mut self) {
        self.trials.clear();
        self.next_number = 1;
    }

    /// The trial with `number`.
    #[must_use]
    pub fn get(&self, number: u32) -> Option<&Trial> {
        self.trials.iter().find(|trial| trial.number == number)
    }

    fn get_mut(&mut self, number: u32) -> Option<&mut Trial> {
        self.trials.iter_mut().find(|trial| trial.number == number)
    }

    /// The trials, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Trial> {
        self.trials.iter()
    }

    /// How many trials the list holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.trials.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.trials.is_empty()
    }

    /// The trials in the order of `sort`: by number, or by one figure with the best first
    /// (ties by number, trials without the figure last).
    #[must_use]
    pub fn sort_by(&self, sort: TrialSort) -> Vec<&Trial> {
        let mut sorted: Vec<&Trial> = self.trials.iter().collect();
        if let TrialSort::Figure(figure) = sort {
            let value = |trial: &Trial| trial.metrics.as_ref().and_then(|m| figure.value(m));
            sorted.sort_by(|a, b| match (value(a), value(b)) {
                (Some(x), Some(y)) => {
                    let order = x.total_cmp(&y);
                    let order = if figure.higher_is_better() {
                        order.reverse()
                    } else {
                        order
                    };
                    order.then(a.number.cmp(&b.number))
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.number.cmp(&b.number),
            });
        }
        sorted
    }
}

/// The automatic label of a trial: the tier changes from `previous` to `design`, e.g.
/// `P1 40.75 → 41.25, C1 34.50 → 34.75`.
///
/// Angles are magnitudes, like everything a cutter reads. At most three changes are named,
/// then "and N more"; "No angle change" when no tier moved.
#[must_use]
pub fn trial_label(previous: &Design, design: &Design) -> String {
    let mut changes: Vec<String> = Vec::new();
    for delta in diff_tiers(&previous.tiers, None, &design.tiers, None) {
        let name = if delta.removed() {
            tier_display_name(previous, delta.index)
        } else {
            tier_display_name(design, delta.index)
        };
        match (delta.angle_before, delta.angle_after) {
            (Some(was), Some(now)) if delta.angle_changed() => {
                changes.push(format!("{name} {:.2} \u{2192} {:.2}", was.abs(), now.abs()));
            }
            (None, _) => changes.push(format!("{name} added")),
            (_, None) => changes.push(format!("{name} removed")),
            _ if delta.indices_changed() => changes.push(format!("{name} indices")),
            _ => {}
        }
    }
    for delta in diff_concave_tiers(&previous.concave_tiers, &design.concave_tiers) {
        let name = delta
            .after
            .as_ref()
            .or(delta.before.as_ref())
            .map_or_else(String::new, |tier| tier.name.clone());
        let what = match (&delta.before, &delta.after) {
            (None, _) => "added",
            (_, None) => "removed",
            _ => "changed",
        };
        changes.push(format!("{name} {what}").trim().to_owned());
    }
    if changes.is_empty() {
        return "No angle change".to_owned();
    }
    let more = changes.len().saturating_sub(LABEL_CHANGES);
    changes.truncate(LABEL_CHANGES);
    let mut label = changes.join(", ");
    if more > 0 {
        label = format!("{label} and {more} more");
    }
    label
}

/// The edit that brings `trial`'s design back into `current`, for the caller to apply with
/// `try_apply_mapped(edit, Some(format!("Restore Trial {n}")))` (one undo step).
///
/// `Ok(None)` means the trial is the current design: nothing to apply. A trial of another
/// epoch (a design that is no longer open) is refused.
///
/// # Errors
/// The refusal text for a trial of another epoch, or the reason the two designs cannot be
/// bridged by an edit.
pub fn restore_edit(
    current: &Design,
    trial: &Trial,
    current_epoch: u64,
) -> Result<Option<Edit>, String> {
    if trial.design_epoch != current_epoch {
        return Err(OTHER_DESIGN.to_owned());
    }
    crate::variants::replacement_edit(current, &trial.design)
}
