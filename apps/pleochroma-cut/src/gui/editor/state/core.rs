//! [`EditorState`] itself: its fields, construction (`fresh`/`fresh_from_template`),
//! wholesale replacement, and the pending-action/pending-remap payload types its
//! fields hold.

use crate::{
    gui::editor::{
        baseline::{self, Origin},
        deep_solve, optimize_solve, retarget, trials,
    },
    settings::SettingsPersister,
};
use pleochroma_cut_core::{FreshDesignSpec, OptimizeOutcome, RemapRounding};
use pleochroma_editor::{EditorSession, material::MaterialComboCache, scratch::PushedScratch};
use std::{
    cell::RefCell,
    ops::{Deref, DerefMut},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};
use tracing::warn;

use super::file_extras::DesignFileExtras;

/// The coalescing window of every `EditorState` history (defined in
/// `pleochroma_editor::session`).
pub(in crate::gui::editor) use pleochroma_editor::session::ANGLE_NUDGE_COALESCE_WINDOW;

/// The `AppSettings::suppressed_confirmations` key the anchor explainer card's
/// "Don't show again" persists.
const ANCHOR_EXPLAINER_SUPPRESS_KEY: &str = "anchor_explainer";

thread_local! {
    /// Whether this session has already decided to open the anchor explainer (shown it, or
    /// found it suppressed). A design that stays `MissingAnchor` then neither reopens the card
    /// nor rereads the settings on every refresh. The permanent "Don't show again" lives in
    /// `AppSettings` (see [`ANCHOR_EXPLAINER_SUPPRESS_KEY`]).
    static ANCHOR_EXPLAINER_DECIDED_THIS_SESSION: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// Whether the anchor explainer's "Don't show again" has been recorded. Read from the
/// application's [`SettingsPersister`], so a choice made earlier in this session counts.
/// With no persister installed, nothing is suppressed.
///
/// Follows the pattern of `native_io::confirm`'s suppressions, which are private to that
/// module.
fn anchor_explainer_is_suppressed() -> bool {
    SettingsPersister::installed_for_this_thread().is_some_and(|persister| {
        persister
            .snapshot()
            .settings
            .is_confirm_suppressed(ANCHOR_EXPLAINER_SUPPRESS_KEY)
    })
}

/// Records the anchor explainer's "Don't show again", the write half of
/// [`anchor_explainer_is_suppressed`]. Writes through the [`SettingsPersister`], never to
/// the file: every close flushes its in-memory snapshot over the file, so a write around it
/// would be erased on exit.
pub(in crate::gui::editor) fn anchor_explainer_suppress_permanently() {
    let Some(persister) = SettingsPersister::installed_for_this_thread() else {
        warn!("No settings persister is installed; not suppressing the anchor explainer");
        return;
    };
    persister.update(|file| {
        file.settings
            .suppress_confirm(ANCHOR_EXPLAINER_SUPPRESS_KEY);
    });
}

/// Whether the anchor explainer card should open now. `design_has_missing_anchor` is `true`
/// when this refresh's solve returned `Err(MissingAnchor)`. The decision is made once per
/// session (see [`ANCHOR_EXPLAINER_DECIDED_THIS_SESSION`]) and a permanent "Don't show
/// again" also stops it (see [`anchor_explainer_is_suppressed`]). The session flag is set on
/// the first decision, open or not, so later refreshes read a `Cell` and not the settings file.
#[must_use]
pub(in crate::gui::editor) fn should_open_anchor_explainer(
    design_has_missing_anchor: bool,
) -> bool {
    if !design_has_missing_anchor {
        return false;
    }
    if ANCHOR_EXPLAINER_DECIDED_THIS_SESSION.with(std::cell::Cell::get) {
        return false;
    }
    ANCHOR_EXPLAINER_DECIDED_THIS_SESSION.with(|decided| decided.set(true));
    !anchor_explainer_is_suppressed()
}

/// A gear-change the design settings panel has previewed but not yet confirmed --
/// `setup_gear_apply_callback` fills this in and
/// `setup_gear_remap_confirm_callback`/`setup_gear_remap_cancel_callback` are the only
/// two consumers (apply, or discard). `symmetry_order`/`mirror` are the design's own
/// current values at preview time, carried here so confirming needs no second read of
/// `design.meta`.
pub(in crate::gui::editor) struct PendingGearRemap {
    pub(in crate::gui::editor) from_gear: i32,
    pub(in crate::gui::editor) to_gear: i32,
    pub(in crate::gui::editor) symmetry_order: u32,
    pub(in crate::gui::editor) mirror: bool,
    pub(in crate::gui::editor) rounding: RemapRounding,
}

/// A destructive action (New, Load Selected or Open) that would replace [`EditorState`]
/// while the design has unsaved edits. The action's callback stores it here when
/// [`EditorState::is_dirty`] is true. The Save/Discard/Cancel dialog resolves it
/// (`callbacks::tier_actions::lifecycle::setup_unsaved_guard_dispatch`). Cancel clears it.
///
/// `New` carries the parsed [`FreshDesignSpec`], taken when "Create" was clicked. The dialog's
/// fields could change while the confirmation is up, and re-reading them would create a
/// different design. `LoadSelected` and `OpenNative` carry nothing and re-read their inputs
/// when resumed.
pub(in crate::gui) enum PendingUnsavedAction {
    /// Resume by building this spec into a fresh design (see
    /// `callbacks::tier_actions::new_design::do_new_design_create`).
    ///
    /// `template_index` is the "Start from" choice, kept here because the dialog is already
    /// closed when the guard resolves.
    New {
        spec: Box<FreshDesignSpec>,
        template_index: i32,
    },
    /// Resume by re-running "Load Selected" against the library's current selection (see
    /// `callbacks::tier_actions::lifecycle::do_load_selected`).
    LoadSelected,
    /// Resume by re-running "Open", which shows the file picker again (see
    /// `native_io::open_native::do_open_native`).
    OpenNative,
}

/// What to do once the Save started by an unsaved-changes guard lands. Save is asynchronous:
/// the write runs on a spawned thread, so a guard cannot check `is_dirty` right after calling
/// it. Instead the save's own completion (`native_io::save_finish::finish_save_native_success`,
/// the one place that knows the write landed) runs the action, through the listeners of
/// `native_io::notify_save_completed`.
///
/// Set by `gui::window_close`'s close-confirm Save and by the Save resolution of
/// [`PendingUnsavedAction`], just before each calls `EditorModel.save_native`.
///
/// Cleared on every path that does not end in a completed save: the Save-As picker's cancel
/// (`native_io::save::save_native_via_dialog`), the write-confirm Cancel
/// (`native_io::confirm::setup_write_confirm_dialog_callbacks`) and the background write's
/// error path (`native_io::save::write_native_save`). A stale value would make a later,
/// unrelated save close the window or resume an old action.
///
/// `pub(in crate::gui)` because `gui::window_close` is a sibling of `gui::editor`.
pub(in crate::gui) enum AfterSave {
    /// `gui::window_close`'s close-confirm guard: hide the window once the save lands.
    CloseWindow,
    /// The New/Load Selected/Open unsaved-changes guard: resume this
    /// action once the save actually lands.
    Resume(PendingUnsavedAction),
}

/// The editor's live state: the shared [`EditorSession`] (design, undo/redo history,
/// multi-selection, generations; every mutation goes through it, so the desktop and the web
/// app edit identically) plus desktop-only bookkeeping (Deep Solve and Optimize handles, the
/// paired `.asc`, the catalogue row, the unsaved-changes guard). It derefs to the session, so
/// `state.design` and `state.generation` read straight through. See the `mod.rs` docs for why
/// [`Self::apply`], [`Self::undo`] and [`Self::redo`] are the only functions that touch design
/// and history together.
///
/// Shared as `Rc<RefCell<..>>` because every callback that touches it runs on the UI thread.
/// A lock would add no safety here and would trip clippy's `significant_drop_tightening` lint.
pub(in crate::gui) struct EditorState {
    /// The design, its history, the multi-selection and the generation counters --
    /// see [`EditorSession`]'s own field docs (`generation` is an `Arc<AtomicU64>`
    /// so a Deep Solve/Optimize completion handler on a worker thread can notice the
    /// design changed without capturing this non-`Send` state).
    pub(in crate::gui::editor) session: EditorSession,
    /// This design's printed proportions (`Vol/W^3`, `L/W`, `C/W`, `P/W`, `H/W`), when
    /// loaded from a catalogue entry that has them -- Deep Solve's external
    /// verification targets. `None` for a brand-new design or one loaded with those
    /// columns unpopulated; either way Deep Solve must be disabled rather than run
    /// against an unmeasurable target.
    pub(in crate::gui::editor) printed_proportions:
        Option<pleochroma::geometry::stone_metrics::ExternalProportions>,
    /// Bumped only by [`Self::replace_wholesale`], so exactly when New, Load Selected or Open
    /// swap in a different design.
    ///
    /// `generation` counts edits and replacements alike and cannot tell them apart. After an
    /// edit, a finished background run still describes this design, so it may be shown with a
    /// caveat. After a replacement it describes a different stone, so its completion must stay
    /// silent.
    ///
    /// The `Arc` is carried across a replacement rather than replaced, so every clone a
    /// background closure captured observes the bump.
    pub(in crate::gui::editor) design_epoch: Arc<AtomicU64>,
    /// See [`PendingUnsavedAction`]. `None` whenever the Save/Discard/Cancel dialog
    /// is closed (the common state).
    pub(in crate::gui::editor) pending_unsaved_action: Option<PendingUnsavedAction>,
    /// See [`AfterSave`]. `None` unless a close or replace guard's Save is running. An
    /// ordinary Save never touches this field.
    pub(in crate::gui) after_save: Option<AfterSave>,
    /// The Deep Solve handle, so `setup_deep_solve_cancel_callback` can reach it. `None` when
    /// none has run. Whether one is running is tracked by the `editor_deep_solve_running`
    /// property, not by this field. A finished run's handle stays until the next run replaces it.
    pub(in crate::gui::editor) deep_solve: Option<deep_solve::DeepSolveHandle>,
    /// The in-flight Optimize run's handle -- same role as `deep_solve` above.
    pub(in crate::gui::editor) optimize: Option<optimize_solve::OptimizeSolveHandle>,
    /// The latest completed or cancelled Optimize result. It is never applied automatically.
    /// `setup_optimize_apply_callback` commits it through [`Self::apply_optimize_outcome`], and
    /// a new run replaces it. It is paired with the design `generation` the search ran
    /// against, so an Apply after the design changed is refused instead of using stale tier
    /// indices. The completion handler runs on a worker thread, hence the `Arc`. The `Mutex`
    /// is needed because the whole [`OptimizeOutcome`] is not atomic.
    pub(in crate::gui::editor) pending_optimize: Arc<Mutex<Option<(OptimizeOutcome, u64)>>>,
    /// The design [`Self::generation`] that the displayed Deep Solve verdict
    /// (`EditorModel.deep_solve_status`) was computed for. `None` before any Deep Solve has
    /// completed for this design. Unlike `pending_optimize`'s value, which is checked once at
    /// Apply time, this is compared with the live generation on every edit
    /// (`view::push_stale_content`, via `pleochroma_editor::session::result_is_stale`). That
    /// shows the "Stale: design changed" badge as soon as the design changes. The one writer is
    /// `apply_deep_solve_outcome` in `callbacks::solve_actions`.
    pub(in crate::gui::editor) deep_solve_result_generation: Option<u64>,
    /// The paired `.asc`'s bare file name and exact original text, when this design's
    /// schedule came from a real `.asc` file on disk (a catalogue attachment, or a
    /// previous save/open). `None` for a brand-new design or one reconstructed
    /// from the angle-table placeholder. Fed to [`pleochroma_cut_core::catalogue_asc`]'s
    /// `original_asc_text` parameter, which lets a Save leave the catalogue `.asc`
    /// byte-for-byte untouched instead of regenerating it.
    pub(in crate::gui::editor) asc_filename: Option<String>,
    pub(in crate::gui::editor) original_asc_text: Option<String>,
    /// The catalogue row this design belongs to, once it has one.
    ///
    /// Set when Load Selected opens a local row, and by the first Save of a design that had
    /// none (that Save inserts a row). Later saves update the same row, so export and
    /// reimport do not add near-duplicate entries. `None` for a design never in the catalogue,
    /// and for a remote library: remote and local row ids are separate id spaces.
    ///
    /// A plain edit does not reset it. Only a wholesale replacement does.
    pub(in crate::gui::editor) source_entry_id: Option<i64>,
    /// Whether the masts came from the angle-table reconstruction fallback (no attached `.asc`
    /// was found, so every mast is a placeholder `0.0`). Save and Export then stamp
    /// `pleochroma_formats::asc::mark_reconstructed` into the file header.
    pub(in crate::gui::editor) used_placeholder: bool,
    /// The `[meta]` table and attachments of the `.psdd` file this design came
    /// from, so a Save writes them back unchanged -- see [`DesignFileExtras`].
    pub(in crate::gui::editor) file_extras: DesignFileExtras,
    /// See [`PendingGearRemap`]. `None` whenever the gear remap confirmation panel is
    /// closed (the common state).
    pub(in crate::gui::editor) pending_gear_remap: Option<PendingGearRemap>,
    /// The last-built "Retarget for material" proposal, paired with the design `generation` it
    /// was built against. Cleared by `setup_retarget_apply_callback` after applying and by
    /// `setup_retarget_close_callback` on cancel.
    pub(in crate::gui::editor) pending_retarget: Option<(retarget::RetargetProposal, u64)>,
    /// The values the design-settings, preform and yield scratch fields last received from
    /// `design`. Refreshes write those fields only when the design value changed, so an
    /// unrelated edit does not overwrite what the user is typing. See
    /// `pleochroma_editor::scratch::ScratchDelta`.
    ///
    /// It is a `RefCell` so the `view` refresh functions, which only receive `&EditorState`,
    /// can update it.
    pub(in crate::gui::editor) last_pushed_scratch: RefCell<PushedScratch>,
    /// Cache for the material combo's options. See [`Self::material_combo_options`]. It is a
    /// `RefCell` for the same reason as `last_pushed_scratch`.
    pub(in crate::gui::editor) material_combo_cache: RefCell<MaterialComboCache>,
    /// The material name that Load Selected applied from the library entry because the design
    /// named none (`pleochroma_editor::loading::apply_library_material`). `None` otherwise. It
    /// only drives the "inferred" badge (`material_lookup::applied_material_guess`), which
    /// goes away once the design's material is changed. Never saved.
    pub(in crate::gui::editor) applied_library_material: Option<String>,
    /// Whether this is a real design (created, loaded or opened), rather than the startup
    /// placeholder of [`Self::fresh`]. Mirrored into `EditorModel.has_design` by
    /// `view::push_has_design`, which gates the empty-state card grid. A real design with zero
    /// tiers must stay visible.
    pub(in crate::gui::editor) has_design: bool,
}

impl EditorState {
    /// A brand-new design: a generously sized cylindrical preform and an empty schedule, so the
    /// viewport starts with a real, closed stone (see [`EditorSession::fresh`]). Its history
    /// uses [`ANGLE_NUDGE_COALESCE_WINDOW`], so a slow scroll-wheel nudge merges into one undo
    /// step. This is the startup placeholder, so [`Self::has_design`] is `false`.
    pub(in crate::gui::editor) fn fresh() -> Self {
        // The startup placeholder is not a design the user asked for.
        Self::wrapping(EditorSession::fresh(), false)
    }

    /// A brand-new design from the New Design dialog's [`FreshDesignSpec`] (preform, gear,
    /// symmetry, mirror, starting material), seeded with template `template_index`'s tiers
    /// (`0` is "Empty"). See [`EditorSession::from_template`].
    pub(in crate::gui::editor) fn fresh_from_template(
        spec: FreshDesignSpec,
        template_index: i32,
    ) -> Self {
        Self::wrapping(EditorSession::from_template(spec, template_index), true)
    }

    /// A brand-new design around an already-built `session`. The template path builds it with
    /// `pleochroma_editor::templates::create_from_template`, which remaps the gear, sets the
    /// stone width and adapts the facet angles before any history exists.
    pub(in crate::gui::editor) fn fresh_from_session(session: EditorSession) -> Self {
        Self::wrapping(session, true)
    }

    /// `session` with every desktop-only field at its "nothing pending" default.
    fn wrapping(session: EditorSession, has_design: bool) -> Self {
        Self {
            session,
            printed_proportions: None,
            design_epoch: Arc::new(AtomicU64::new(0)),
            pending_unsaved_action: None,
            after_save: None,
            deep_solve: None,
            optimize: None,
            pending_optimize: Arc::new(Mutex::new(None)),
            deep_solve_result_generation: None,
            asc_filename: None,
            original_asc_text: None,
            pending_gear_remap: None,
            pending_retarget: None,
            source_entry_id: None,
            used_placeholder: false,
            // A new design, and the startup placeholder, get a fresh UUID at once (see
            // `state::design_identity`).
            file_extras: DesignFileExtras::default().with_design_uuid_assigned(None),
            last_pushed_scratch: RefCell::new(PushedScratch::default()),
            material_combo_cache: RefCell::new(MaterialComboCache::default()),
            applied_library_material: None,
            has_design,
        }
    }

    /// The UUID that names this design in the library database. The vault's per-design tables
    /// (saved variants, cutting progress, lighting choice) are keyed by it.
    ///
    /// It is the design's `[meta].id`, so Save, Save As and autosave keep it, and a copy keeps
    /// its side data. See `state::design_identity` for how it is chosen on open.
    pub(in crate::gui) fn design_uuid(&self) -> &str {
        &self.file_extras.metadata.id
    }

    /// The file extras a write (Save, Save As, autosave, an export) puts into the file.
    ///
    /// The RI range choice lives in `Design::ri_range_choice` while the design is open, so
    /// that Undo and Redo move it; the file keeps it in the `[meta]` table. This is the one way
    /// out: a copy of [`Self::file_extras`] whose `[meta]` states the design's choice. Metadata
    /// that already agrees is left exactly as loaded, so an untouched design writes the same
    /// bytes as before.
    pub(in crate::gui::editor) fn file_extras_for_write(&self) -> DesignFileExtras {
        let mut extras = self.file_extras.clone();
        if extras.metadata.ri_range_choice() != self.session.design.ri_range_choice {
            extras
                .metadata
                .set_ri_range_choice(self.session.design.ri_range_choice);
        }
        extras
    }

    /// Re-stamps a finished Deep Solve verdict and a pending Optimize result as current after
    /// an edit that moved no geometry (the RI range choice).
    ///
    /// `generation_before` is the generation the edit started from. A result stamped with it
    /// was current then, and still describes the design, so it takes the generation the edit
    /// produced. A result that was already stale stays stale.
    pub(in crate::gui::editor) fn keep_results_current(&mut self, generation_before: u64) {
        let now = self.session.current_generation();
        if self.deep_solve_result_generation == Some(generation_before) {
            self.deep_solve_result_generation = Some(now);
        }
        let mut pending = self
            .pending_optimize
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((_, started)) = pending.as_mut()
            && *started == generation_before
        {
            *started = now;
        }
    }

    /// Replaces `self` with `replacement`, a new `New`, `Load Selected` or `Open` state.
    ///
    /// The `generation` and [`Self::design_epoch`] counters are carried over and bumped, not
    /// taken from `replacement`. A background closure that captured the old `Arc` then sees
    /// the bump, so a search started against the old design cannot be applied to the new one.
    /// The replacement reads as clean at once (`saved_generation` is set to match), so
    /// `is_dirty` is false.
    pub(in crate::gui::editor) fn replace_wholesale(&mut self, mut replacement: Self) {
        // Cancel any Deep Solve or Optimize still running for the old design. Dropping the
        // handles does not stop a run, and it would keep a worker busy for minutes. The
        // completion handlers check `design_epoch`, so they return promptly once cancelled.
        // The busy flags are reset by `callbacks::solve_actions::clear_analysis_results`.
        if let Some(handle) = self.deep_solve.take() {
            handle.cancel();
        }
        if let Some(handle) = self.optimize.take() {
            handle.cancel();
        }
        replacement.session.continue_generation_from(&self.session);
        // Bumped here, in the one method every replacement path goes through, so New, Load
        // Selected, Open and plain `.asc` open all bump it.
        replacement.design_epoch = Arc::clone(&self.design_epoch);
        replacement
            .design_epoch
            .fetch_add(1, AtomicOrdering::Relaxed);
        *self = replacement;
        // The RI range choice is stored in the file's `[meta]`; the design holds it from here on,
        // so Undo and Redo move it (see `Self::file_extras_for_write` for the way back out).
        self.session.design.ri_range_choice = self.file_extras.metadata.ri_range_choice();
        // The design just opened is the new Baseline, and the figures of the one before it
        // are gone. This also clears the Baseline a previous design left behind.
        baseline::install(Origin::Opened, self);
        // The trials kept for the design before are gone with it.
        trials::clear();
    }
}

impl Deref for EditorState {
    type Target = EditorSession;

    fn deref(&self) -> &EditorSession {
        &self.session
    }
}

impl DerefMut for EditorState {
    fn deref_mut(&mut self) -> &mut EditorSession {
        &mut self.session
    }
}
