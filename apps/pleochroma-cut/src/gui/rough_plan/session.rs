//! UI-thread session state for the rough planner window.

use super::{
    axis::AxisState, base::blank_model, inputs::FilterSnapshot, mesh_task::MeshJobs, run::RunState,
    saved::SavedState, view::ViewState,
};
use pleochroma::geometry::stone_metrics::SolidMesh;
use pleochroma_cut_core::rough_plan::{
    RoughLayout, RoughMeasure, RoughModel,
    orientation::{PlanSettingsOverrides, StoneChoice},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

/// The material the window selects on its first open.
pub const DEFAULT_MATERIAL: &str = "Quartz";

/// The message shown when the active library is a remote one.
pub const REMOTE_MESSAGE: &str = "Switch to the local library to plan a rough.";

/// How many earlier models the undo and redo stacks each keep.
pub(super) const UNDO_CAP: usize = 100;

/// The layouts the cutter changed on the shown results (a stone re-fitted to its piece, a stone
/// turned by hand), by the result's place in the shown results: the whole layout as it is now. A
/// result that is not in the map is as the planner made it.
pub(super) type LayoutEdits = BTreeMap<u32, RoughLayout>;

/// One undo step: the rough model and the orientation settings beside it (the plan's rule, the
/// rules and Reasons of the designs, the choices of the stones) and the layouts the cutter
/// changed, so a rule edit, an override and a re-fit are undone and redone like any edit of the
/// rough.
pub(super) type UndoEntry = (RoughModel, PlanSettingsOverrides, LayoutEdits);

/// One entry of the window's material list: a name and its known specific gravity.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialChoice {
    /// The material's name.
    pub name: String,
    /// Its specific gravity.
    pub specific_gravity: f64,
}

/// The candidate design ids the two counts in the window were taken from, filled by the
/// background query that opening the window starts. A display aid only: Plan resolves
/// its own ids on the worker thread, and the lists keep the designs excluded from the
/// planner that the shown counts leave out.
#[derive(Default)]
pub struct CachedIds {
    /// Bumped on every open; a slower, older query sees the mismatch and drops its result.
    pub generation: u64,
    /// The ids of the current library filter, sorted.
    pub filter: Option<Vec<i64>>,
    /// The ids of the whole library, sorted.
    pub library: Option<Vec<i64>>,
}

impl CachedIds {
    /// Forgets the cached ids and returns the new generation.
    pub fn reset(&mut self) -> u64 {
        self.generation += 1;
        self.filter = None;
        self.library = None;
        self.generation
    }
}

/// What the callbacks share on the UI thread (they all run there).
pub(super) struct Session {
    /// The model being edited, including edits that are not committed yet.
    pub(super) model: RoughModel,
    /// The orientation settings and overrides of the plan, edited beside the model (the
    /// default for a plan that never used the crystal axis).
    pub(super) overrides: PlanSettingsOverrides,
    /// The layouts the cutter changed, edited beside the model (empty for results as planned).
    pub(super) edits: LayoutEdits,
    /// Earlier committed entries, oldest first (at most [`UNDO_CAP`]).
    pub(super) undo: Vec<UndoEntry>,
    /// Entries undone, most recently undone last.
    pub(super) redo: Vec<UndoEntry>,
    /// The entry as of the last commit, undo or redo: what [`Session::commit`] compares the
    /// live model and settings against.
    pub(super) committed: UndoEntry,
    /// Bumped on every model change (live or committed); older shape results are stale.
    pub(super) revision: u64,
    /// The materials behind `RoughPlanModel.materials`, index for index.
    pub(super) choices: Vec<MaterialChoice>,
    /// The running plan's cancel flag, if a plan was ever started.
    pub(super) cancel: Option<Arc<AtomicBool>>,
    /// The design ids the open-time background query found, shared with that thread.
    pub(super) ids: Arc<Mutex<CachedIds>>,
    /// The designs excluded from the planner, as last read from the library; the rows'
    /// Excluded pills and the Candidate Designs list mirror it.
    pub(super) excluded: BTreeMap<i64, String>,
    /// The latest shape worker mesh, in the centred frame (bounding-box centre at the
    /// origin, mm).
    pub(super) model_mesh: Option<Arc<SolidMesh>>,
    /// The inclusion shells of the latest shape worker mesh, in the same frame (empty
    /// without inclusions): the view draws them as their own see-through layer.
    pub(super) model_inclusions: Arc<SolidMesh>,
    /// The latest shape worker measurement.
    pub(super) model_measure: Option<RoughMeasure>,
    /// The carat text the window last wrote into the carat field itself (the model's own
    /// weight for the picked material). While the field still holds it, nobody has typed
    /// a weighed carat over it.
    pub(super) carat_shown: Option<String>,
    /// Cut fields (row, field number) whose text is not a number right now.
    pub(super) bad_fields: BTreeSet<(usize, i32)>,
    /// The azimuth typed for a face cut, by cut index. A vertical face cannot tell its
    /// azimuth, so the window keeps the typed one to show it and to turn back to when
    /// the elevation leaves the pole. Cleared whenever the rows are rebuilt from the
    /// model (undo, redo, load, adding or removing a cut), which may renumber the cuts.
    pub(super) face_azimuths: BTreeMap<usize, f64>,
    /// The library filter the design counts in the window were last asked for; the
    /// counts are asked again when the filter differs.
    pub(super) counted_filter: Option<FilterSnapshot>,
    /// The view's state.
    pub(super) view: ViewState,
    /// The plan run's state.
    pub(super) run: RunState,
    /// The Crystal Axis state of the window that is not undoable (the pick under way, the stone
    /// turned by hand, the materials looked up).
    pub(super) axis: AxisState,
    /// The saved plans' state.
    pub(super) saved: SavedState,
    /// The background mesh job (an OBJ import or a mesh scaling) the window waits for.
    pub(super) mesh_jobs: MeshJobs,
}

impl Default for Session {
    fn default() -> Self {
        let model = blank_model();
        Self {
            committed: (
                model.clone(),
                PlanSettingsOverrides::default(),
                LayoutEdits::new(),
            ),
            model,
            overrides: PlanSettingsOverrides::default(),
            edits: LayoutEdits::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            choices: Vec::new(),
            cancel: None,
            ids: Arc::default(),
            excluded: BTreeMap::new(),
            model_mesh: None,
            model_inclusions: Arc::default(),
            model_measure: None,
            carat_shown: None,
            bad_fields: BTreeSet::new(),
            face_azimuths: BTreeMap::new(),
            counted_filter: None,
            view: ViewState::default(),
            run: RunState::default(),
            axis: AxisState::default(),
            saved: SavedState::default(),
            mesh_jobs: MeshJobs::default(),
        }
    }
}

impl Session {
    /// Whether the live model or settings differ from the last committed entry.
    fn is_dirty(&self) -> bool {
        self.model != self.committed.0
            || self.overrides != self.committed.1
            || self.edits != self.committed.2
    }

    /// Makes the current model and settings an undo step if they differ from the last
    /// committed ones.
    ///
    /// Committing an unchanged model (Enter followed by the focus-out that ends the same
    /// edit) does nothing. Returns whether a step was recorded.
    pub(super) fn commit(&mut self) -> bool {
        if !self.is_dirty() {
            return false;
        }
        if self.undo.len() >= UNDO_CAP {
            self.undo.remove(0);
        }
        let live = (
            self.model.clone(),
            self.overrides.clone(),
            self.edits.clone(),
        );
        let previous = std::mem::replace(&mut self.committed, live);
        self.undo.push(previous);
        self.redo.clear();
        true
    }

    /// Steps back to the previous committed entry (committing pending edits first).
    /// Returns whether the model or the settings changed.
    pub(super) fn undo_step(&mut self) -> bool {
        self.commit();
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        if self.redo.len() >= UNDO_CAP {
            self.redo.remove(0);
        }
        let current = std::mem::replace(&mut self.committed, previous.clone());
        self.redo.push(current);
        (self.model, self.overrides, self.edits) = previous;
        self.face_azimuths.clear();
        self.revision += 1;
        true
    }

    /// Steps forward to the entry that was undone. Returns whether anything changed.
    ///
    /// Refused while a live edit is pending: redoing would silently throw that edit away,
    /// so the edit has to be committed (or undone) first.
    pub(super) fn redo_step(&mut self) -> bool {
        if self.is_dirty() {
            return false;
        }
        let Some(next) = self.redo.pop() else {
            return false;
        };
        if self.undo.len() >= UNDO_CAP {
            self.undo.remove(0);
        }
        let current = std::mem::replace(&mut self.committed, next.clone());
        self.undo.push(current);
        (self.model, self.overrides, self.edits) = next;
        self.face_azimuths.clear();
        self.revision += 1;
        true
    }

    /// Replaces the model (a reset, or a rough picked anew) as one undo step. The settings go
    /// back to the default: they belonged to the old rough.
    pub(super) fn replace_model(&mut self, model: RoughModel) {
        self.replace_model_with(model, PlanSettingsOverrides::default());
    }

    /// Replaces the model and the settings (a plan loaded from the library or a file) as one
    /// undo step.
    pub(super) fn replace_model_with(
        &mut self,
        model: RoughModel,
        overrides: PlanSettingsOverrides,
    ) {
        self.model = model;
        self.overrides = overrides;
        self.edits.clear();
        // The layouts on screen stay as they are until new ones replace them: nothing to show
        // again for the layouts the cutter changed.
        self.axis.shown_edits.clear();
        self.bad_fields.clear();
        self.face_azimuths.clear();
        self.revision += 1;
        self.commit();
    }

    /// Edits the orientation settings through `edit` and records the change as one undo step.
    /// Returns whether anything changed.
    pub(super) fn edit_overrides(&mut self, edit: impl FnOnce(&mut PlanSettingsOverrides)) -> bool {
        self.edit_stones(|overrides, _| edit(overrides))
    }

    /// Edits the orientation settings and the changed layouts through `edit` and records the
    /// change as one undo step. Returns whether anything changed.
    pub(super) fn edit_stones(
        &mut self,
        edit: impl FnOnce(&mut PlanSettingsOverrides, &mut LayoutEdits),
    ) -> bool {
        self.commit();
        edit(&mut self.overrides, &mut self.edits);
        if self.overrides == self.committed.1 && self.edits == self.committed.2 {
            return false;
        }
        self.revision += 1;
        self.commit()
    }

    /// The shown results are new (a plan finished, or a saved plan was opened): the choices for
    /// their stones are `stones`, no layout was changed by hand, and the history of the model
    /// keeps no choice or change for results that are gone.
    pub(super) fn reset_stone_choices(&mut self, stones: &BTreeMap<(u32, u32), StoneChoice>) {
        self.overrides.stones.clone_from(stones);
        self.edits.clear();
        self.committed.1.stones.clone_from(stones);
        self.committed.2.clear();
        for entry in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            entry.1.stones.clone_from(stones);
            entry.2.clear();
        }
    }

    /// Whether the window's Undo has anything to undo.
    #[must_use]
    pub(super) fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.is_dirty()
    }

    /// Whether the window's Redo has anything to redo (not while a live edit is pending,
    /// see [`Session::redo_step`]).
    #[must_use]
    pub(super) fn can_redo(&self) -> bool {
        !self.redo.is_empty() && !self.is_dirty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::rough_plan::{BoxFace, RoughBase, RoughCut};

    fn block(x: f64) -> RoughModel {
        RoughModel::new(
            RoughBase::Block {
                x_mm: x,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            Vec::new(),
        )
    }

    fn edit(session: &mut Session, x: f64) {
        session.model = block(x);
        session.commit();
    }

    #[test]
    fn committing_the_same_model_twice_records_one_step() {
        let mut session = Session::default();
        assert!(session.undo.is_empty() && !session.can_undo());
        session.model = block(10.0);
        assert!(session.commit());
        // Enter and then the focus-out that follows it commit the same edit.
        assert!(!session.commit());
        assert_eq!(session.undo.len(), 1);
        assert_eq!(session.undo[0].0, blank_model());
    }

    #[test]
    fn a_live_edit_is_not_a_step_until_it_is_committed() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        session.model = block(11.0);
        session.model = block(12.0);
        assert_eq!(session.undo.len(), 1, "typing alone records nothing");
        assert!(session.can_undo());
        assert!(session.commit());
        assert_eq!(session.undo.len(), 2);
        assert_eq!(session.undo[1].0, block(10.0));
    }

    #[test]
    fn undo_and_redo_walk_the_committed_models() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        edit(&mut session, 11.0);
        edit(&mut session, 12.0);

        assert!(session.undo_step());
        assert_eq!(session.model, block(11.0));
        assert!(session.undo_step());
        assert_eq!(session.model, block(10.0));
        assert!(session.can_redo());
        assert!(session.redo_step());
        assert_eq!(session.model, block(11.0));
        assert!(session.redo_step());
        assert_eq!(session.model, block(12.0));
        assert!(!session.redo_step(), "nothing left to redo");
        assert!(!session.can_redo());
    }

    #[test]
    fn undo_first_commits_the_pending_edit_and_a_new_edit_clears_redo() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        session.model = block(11.0);
        assert!(session.undo_step());
        assert!(
            session.model == block(10.0),
            "undo returns to before the pending edit"
        );
        assert!(session.can_redo());
        edit(&mut session, 13.0);
        assert!(!session.can_redo(), "a new edit ends the redo branch");
    }

    #[test]
    fn a_pending_live_edit_blocks_redo_and_keeps_the_redo_stack() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        edit(&mut session, 11.0);
        assert!(session.undo_step());
        assert!(session.can_redo());
        // A live edit that is not committed yet.
        session.model = block(20.0);
        assert!(!session.can_redo(), "redo would discard the live edit");
        assert!(!session.redo_step());
        assert_eq!(session.model, block(20.0), "the live edit survives");
        assert_eq!(session.redo.len(), 1, "the redo stack is untouched");
        // Back to the committed model, redo works again.
        session.model = block(10.0);
        assert!(session.can_redo());
        assert!(session.redo_step());
        assert_eq!(session.model, block(11.0));
    }

    #[test]
    fn undo_on_an_empty_history_does_nothing() {
        let mut session = Session::default();
        assert!(!session.undo_step());
        assert!(!session.can_undo());
        assert_eq!(session.model, blank_model());
    }

    #[test]
    fn the_history_keeps_at_most_the_cap_and_drops_the_oldest() {
        let mut session = Session::default();
        for i in 1..=150_u32 {
            edit(&mut session, f64::from(i));
        }
        assert_eq!(session.undo.len(), UNDO_CAP);
        // The oldest kept model is the one 100 edits back: x = 150 - 100 = 50.
        assert_eq!(session.undo[0].0, block(50.0));
        assert_eq!(session.undo[UNDO_CAP - 1].0, block(149.0));
        for _ in 0..UNDO_CAP {
            assert!(session.undo_step());
        }
        assert_eq!(session.model, block(50.0));
        assert!(!session.undo_step());
        assert_eq!(session.redo.len(), UNDO_CAP);
    }

    #[test]
    fn replacing_the_model_is_one_undo_step_and_bumps_the_revision() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        let before = session.revision;
        let shaped = RoughModel::new(
            RoughBase::Block {
                x_mm: 20.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            vec![RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [1.0, 1.0],
            }],
        );
        session.bad_fields.insert((0, 1));
        session.face_azimuths.insert(0, 45.0);
        session.replace_model(shaped.clone());
        assert!(session.revision > before);
        assert!(session.bad_fields.is_empty());
        assert!(
            session.face_azimuths.is_empty(),
            "typed azimuths belong to the old model's cut numbers"
        );
        assert_eq!(session.undo.len(), 2);
        assert!(session.undo_step());
        assert_eq!(session.model, block(10.0));
        assert!(session.redo_step());
        assert_eq!(session.model, shaped);
    }

    #[test]
    fn a_changed_layout_is_one_undo_step_and_undo_brings_the_planned_one_back() {
        use pleochroma_cut_core::rough_plan::{CutOrder, CutPlan};
        let changed = RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: Vec::new(),
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 2.5,
            total_volume_mm3: 10.0,
            yield_fraction: 0.2,
            exact_fit: true,
        };
        let mut session = Session::default();
        edit(&mut session, 10.0);
        let before = session.undo.len();
        assert!(session.edit_stones(|_, edits| {
            edits.insert(1, changed.clone());
        }));
        assert_eq!(session.undo.len(), before + 1);
        assert_eq!(session.edits.get(&1), Some(&changed));
        // The same edit again changes nothing.
        assert!(!session.edit_stones(|_, edits| {
            edits.insert(1, changed.clone());
        }));
        assert_eq!(session.undo.len(), before + 1);

        assert!(session.undo_step());
        assert!(session.edits.is_empty());
        assert!(session.redo_step());
        assert_eq!(session.edits.get(&1), Some(&changed));
    }

    #[test]
    fn new_results_drop_the_choices_and_the_changed_layouts_of_the_old_ones_from_the_history() {
        use pleochroma_cut_core::rough_plan::orientation::StoneChoice;
        let lock = StoneChoice {
            locked: true,
            ..StoneChoice::default()
        };
        let mut session = Session::default();
        edit(&mut session, 10.0);
        session.edit_overrides(|overrides| overrides.set_stone_choice(0, 0, lock.clone()));
        edit(&mut session, 11.0);
        let fresh = BTreeMap::from([((0, 3), lock.clone())]);
        session.reset_stone_choices(&fresh);
        assert_eq!(session.overrides.stones, fresh);
        assert!(session.edits.is_empty());
        // Going back in time brings no choice for a stone of results that are gone.
        while session.undo_step() {
            assert_eq!(session.overrides.stones, fresh);
        }
    }

    #[test]
    fn a_settings_edit_is_one_undo_step_beside_the_model() {
        use pleochroma_cut_core::rough_plan::orientation::{RuleChoice, RuleSetting};
        let setting = || RuleSetting {
            choice: RuleChoice::Ignore,
            reason: "cut for color".to_string(),
        };
        let mut session = Session::default();
        edit(&mut session, 10.0);
        let before = session.undo.len();
        assert!(session.edit_overrides(|overrides| overrides.set_plan(setting())));
        assert_eq!(session.undo.len(), before + 1);
        assert_eq!(session.model, block(10.0), "the model is untouched");
        // The same edit again changes nothing and records nothing.
        assert!(!session.edit_overrides(|overrides| overrides.set_plan(setting())));
        assert_eq!(session.undo.len(), before + 1);

        assert!(session.undo_step());
        assert!(session.overrides.is_default());
        assert_eq!(session.model, block(10.0));
        assert!(session.redo_step());
        assert_eq!(session.overrides.plan.reason, "cut for color");

        // A new rough drops the settings that belonged to the old one.
        session.replace_model(block(12.0));
        assert!(session.overrides.is_default());
    }
}
