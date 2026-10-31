//! What a trial is made of when it is recorded, and the two rules that decide when "Record Every
//! Change" records one: which edits count, and how often.
//!
//! [`Held`] is a trial before it has a number: built from the editor state at the moment the
//! cutter's change lands (so a hook that records after something else changed the design still
//! describes the design it meant). [`EditWatch`] and [`RecordGate`] are plain data with no window
//! in them.

use super::default_axis;
use crate::gui::editor::{baseline, state::EditorState};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::{CrystalOrientation, Design, MaterialSelection, StoneRenderEffects};
use pleochroma_editor::{sweep::RowMetrics, trials::TrialOrigin};
use std::{collections::HashSet, fmt::Write as _, sync::atomic::Ordering};

/// The shortest time between two trials of "Record Every Change".
pub(super) const MIN_GAP_MS: u64 = 1000;

/// How many revisions an [`EditWatch`] remembers before it starts over (a long session of
/// drags makes one per merged nudge).
const WATCH_LIMIT: usize = 20_000;

/// A trial that has no number yet: the design, its masts and figures when they were known, the
/// material the figures were scored in, and where it came from.
#[derive(Debug, Clone)]
pub(in crate::gui::editor) struct Held {
    pub(super) design: Design,
    pub(super) solved: Option<Vec<SolvedTier>>,
    /// The figures, when the Baseline strip has scored this design already.
    pub(super) metrics: Option<RowMetrics>,
    pub(super) key: String,
    pub(super) epoch: u64,
    /// The design generation whose figures the strip will deliver, when they are not in yet.
    pub(super) awaiting: Option<u64>,
    /// The strip has scored this generation already and it gives no stone: nothing to re-score.
    pub(super) scored_no_stone: bool,
    pub(super) origin: TrialOrigin,
    /// The label; `None` takes the tier diff from the trial before ([`super::Hub::push`]).
    pub(super) label: Option<String>,
}

/// The text a material is told apart by: its `Debug` form, which lists every field that
/// changes how a design scores.
#[must_use]
pub(super) fn material_key(selection: &MaterialSelection) -> String {
    format!("{selection:?}")
}

/// Where a trial key's orientation part starts (see [`trial_key`]).
const AXIS_MARK: &str = ";caxis=";

/// The key trial figures are filed under: [`material_key`] and, when the design's own Crystal
/// Axis is on, that axis (tilt and azimuth, and the Second Axis turn when set), so figures
/// scored for one orientation are never read as the figures of another. With the design's axis
/// off, the custom material's stored default (`material_default`, resolved by the caller before
/// anything is borrowed) is the orientation the stone renders with and is named the same way.
/// A design that sets no axis of its own on a material with no stored default keeps exactly
/// the [`material_key`] text, and so does one that sets no inclusions, play-of-color or abraded
/// edge (those add an `;fx=` part to the material part, see [`effects_part`]).
#[must_use]
pub(super) fn trial_key(
    selection: &MaterialSelection,
    effects: &StoneRenderEffects,
    material_default: Option<CrystalOrientation>,
) -> String {
    let mut key = material_key(selection);
    key.push_str(&effects_part(effects));
    key_with_axis(&key, effects, material_default)
}

/// The `;fx=` part of a trial key: the inclusion, play-of-color and abraded-edge choices of
/// `effects` (their `Debug` text), so figures scored with one are never read as those of another.
/// Empty while none of them is set, which keeps every earlier key.
fn effects_part(effects: &StoneRenderEffects) -> String {
    if effects.inclusions.is_none()
        && effects.structure.is_none()
        && !effects.edge_abrasion().active()
    {
        return String::new();
    }
    format!(
        ";fx={:?}/{:?}/{:?}",
        effects.inclusions, effects.structure, effects.edge_abrasion
    )
}

/// The material part of `key` (a [`material_key`], or a [`trial_key`] whose axis part is
/// replaced) with the orientation `effects` give the stone (the design's own, else
/// `material_default`): what a trial scored in its OWN orientation is filed under (see
/// `rescore`).
#[must_use]
pub(super) fn key_with_axis(
    key: &str,
    effects: &StoneRenderEffects,
    material_default: Option<CrystalOrientation>,
) -> String {
    let mut key = material_part(key).to_owned();
    if effects.c_axis_override_enabled {
        let effects = effects.sanitised();
        write_axis(
            &mut key,
            effects.c_axis_tilt_deg,
            effects.c_axis_azimuth_deg,
            effects.c_axis_second_azimuth_deg,
        );
    } else if let Some(default) = material_default {
        write_axis(
            &mut key,
            default.axis.tilt_deg(),
            default.axis.azimuth_deg(),
            default.second_turn_deg,
        );
    }
    key
}

/// Appends the orientation part of a trial key.
fn write_axis(key: &mut String, tilt: f64, azimuth: f64, second: Option<f64>) {
    let _ = write!(key, "{AXIS_MARK}{tilt:.2}/{azimuth:.2}");
    if let Some(second) = second {
        let _ = write!(key, "/{second:.2}");
    }
}

/// The axis part of a trial key (`caxis=...`), empty when the key names none: what tells a
/// trial that differs only in its orientation from one of another material.
#[must_use]
pub(super) fn axis_part(key: &str) -> &str {
    key.find(AXIS_MARK).map_or("", |at| &key[at..])
}

/// The [`material_key`] part of a [`trial_key`] (the whole key when it names no axis): what the
/// Baseline's figures, which are scored in the Baseline's own orientation, are matched by.
#[must_use]
pub(super) fn material_part(key: &str) -> &str {
    key.find(AXIS_MARK).map_or(key, |at| &key[..at])
}

/// The design `state` holds now, as a trial of `origin`: its masts and figures when the strip
/// has them for this generation, otherwise figures to fill in when they arrive.
#[must_use]
pub(in crate::gui::editor) fn hold(
    state: &EditorState,
    origin: TrialOrigin,
    label: Option<String>,
) -> Held {
    let generation = state.current_generation();
    // The material's stored default axis, resolved here (the editor state is borrowed by the
    // caller, the hub is not).
    let (key, metrics, awaiting) = match baseline::design_figures(generation) {
        Some((selection, metrics)) => (
            trial_key(
                &selection,
                &state.design.render_effects,
                default_axis::of(&selection),
            ),
            metrics,
            None,
        ),
        None => (
            trial_key(
                &state.design.material,
                &state.design.render_effects,
                default_axis::of(&state.design.material),
            ),
            None,
            Some(generation),
        ),
    };
    let scored_no_stone = metrics.is_none() && awaiting.is_none();
    Held {
        design: state.design.clone(),
        solved: baseline::solved_masts(generation),
        metrics,
        scored_no_stone,
        key,
        epoch: state.design_epoch.load(Ordering::Relaxed),
        awaiting,
        origin,
        label,
    }
}

/// Another design of the open document (the one before a Retarget Apply) as a trial of
/// `origin`. Its figures are not known: "Re-score Trials" scores them on demand.
#[must_use]
pub(in crate::gui::editor) fn hold_design(
    state: &EditorState,
    design: Design,
    solved: Option<Vec<SolvedTier>>,
    origin: TrialOrigin,
    label: Option<String>,
) -> Held {
    Held {
        key: trial_key(
            &design.material,
            &design.render_effects,
            default_axis::of(&design.material),
        ),
        design,
        solved,
        metrics: None,
        scored_no_stone: false,
        epoch: state.design_epoch.load(Ordering::Relaxed),
        awaiting: None,
        origin,
        label,
    }
}

/// The revisions of a history as `(revision, undone)`, for [`EditWatch::observe`].
#[must_use]
pub(super) fn revisions_of(state: &EditorState) -> Vec<(u64, bool)> {
    state
        .history_entries()
        .iter()
        .map(|entry| (entry.revision, entry.undone))
        .collect()
}

/// Tells a committed edit from an Undo, a Redo or a jump along the history, so "Record Every
/// Change" never records one of the last three.
///
/// Every step of the history carries a revision that changes whenever the design at that step
/// does (a new step, or more nudges merged into the newest) and does not change when the step
/// is undone or redone. An edit therefore shows as a revision on the done side that was never
/// seen before; an Undo, a Redo and a jump only revisit revisions that were.
#[derive(Debug, Default)]
pub(super) struct EditWatch {
    epoch: Option<u64>,
    seen: HashSet<u64>,
}

impl EditWatch {
    /// Looks at the history of the design opened at `epoch`: `true` when a committed edit
    /// landed since the last look. The first look at a design only learns what is there.
    pub(super) fn observe(&mut self, epoch: u64, revisions: &[(u64, bool)]) -> bool {
        let first = self.epoch != Some(epoch);
        if first || self.seen.len() > WATCH_LIMIT {
            self.epoch = Some(epoch);
            self.seen.clear();
        }
        let fresh = !first
            && revisions
                .iter()
                .any(|(revision, undone)| !undone && !self.seen.contains(revision));
        self.seen
            .extend(revisions.iter().map(|(revision, _)| *revision));
        fresh
    }

    /// Takes the history as it is as already seen: the change that was just made is the
    /// cutter's Restore, not an edit to record.
    pub(super) fn absorb(&mut self, epoch: u64, revisions: &[(u64, bool)]) {
        let _ = self.observe(epoch, revisions);
    }

    /// Forgets everything (another design opened).
    pub(super) fn reset(&mut self) {
        self.epoch = None;
        self.seen.clear();
    }
}

/// The "at most one per second" rule of "Record Every Change".
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct RecordGate {
    last_ms: Option<u64>,
}

impl RecordGate {
    /// Whether a trial may be recorded at `now_ms` (milliseconds on any steady clock); when it
    /// may, the time is remembered.
    pub(super) fn allow(&mut self, now_ms: u64) -> bool {
        let open = self
            .last_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= MIN_GAP_MS);
        if open {
            self.last_ms = Some(now_ms);
        }
        open
    }
}
