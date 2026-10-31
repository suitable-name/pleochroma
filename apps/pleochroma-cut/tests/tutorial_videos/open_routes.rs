//! Filmed routes that open a design: an imported design from the library (the Adopt Imported
//! Meets lesson's first step and the Deep Solve lesson's Setup) and the design file the Open
//! lesson has just saved.
//!
//! Every click is on a visible control. The native file dialog of File > Open is answered with
//! `queue_picker_path` after the filmed click that opens it, as everywhere in the harness.

use crate::{
    debug_log::debug_check,
    director::Film,
    pacing,
    scripts::pure_tables::output_path,
    support::app::pump,
    targets::{self, Spot},
};
use pleochroma_cut::{EditorModel, LibraryModel, harness_support};
use slint::{ComponentHandle, Model};
use std::time::Duration;

/// How many designs of the library list are tried, from the top, before the video is abandoned.
/// A design that is not an import (a native file, an angle table) has no imported meet.
const MAX_DESIGNS: usize = 4;

/// How long one load may take to show its tiers (real time, shown as a time-lapse).
const LOAD_TIMEOUT: Duration = Duration::from_secs(20);

/// The file name the Save step of a lesson writes (`routes.rs`, the `design_saved` route).
const SAVED_DESIGN: &str = "tutorial-design.psdd";

/// The label of the Adopt button every imported row of the tier table carries.
const ADOPT_BUTTON: &str = "Adopt";

/// The label of the chip a row of an unanchored block carries (the tier table's missing-anchor
/// remedy; it opens the Tier form on that row, set to an exact scale value).
const ANCHOR_BUTTON: &str = "Add Anchor";

impl Film<'_> {
    /// Films the step "Open an imported design": the library list is shown, a design is picked in
    /// it and File > Load Selected opens it, until the design has a meet its file stated.
    ///
    /// The lesson starts from a design built in the editor, which has nothing to adopt. Replacing
    /// it makes the editor ask about its unsaved changes, and the question is answered with its
    /// own Discard button, as a learner would.
    pub(super) fn open_imported_library_design(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        let listed = library.get_diagram_list().row_count();
        if listed == 0 {
            return Err("the library lists no design to open".to_owned());
        }
        let tries = listed.min(MAX_DESIGNS);
        for at in 0..tries {
            self.show_library_list()?;
            let row = library
                .get_diagram_list()
                .row_data(at)
                .ok_or_else(|| format!("library row {at} disappeared"))?;
            self.click_library_row(at, row.title.as_str(), row.id)?;
            self.load_selected_design()?;
            if self.wait_for_imported_meet()? {
                return self.hold(pacing::RESULT_HOLD);
            }
        }
        Err(format!(
            "none of the first {tries} designs of the library has an imported meet to adopt; \
             PLEOCHROMA_HARNESS_LIBRARY must name a library of imported designs"
        ))
    }

    /// Films the first part of the Deep Solve lesson's Setup: a catalogue design is loaded from
    /// the library, the way [`Self::open_imported_library_design`] films it.
    ///
    /// The lesson's first step says Deep Solve stays dim until a design with printed proportions
    /// is loaded from the library and a tier adopted, and a template design (see
    /// `Film::film_design_setup`) has neither. So, as the step says, the learner does both before
    /// starting the lesson.
    pub(crate) fn film_catalogue_setup(&mut self) -> Result<(), String> {
        self.rec.chapter("Setup");
        self.rec.caption(
            "Deep Solve needs a catalogue design: load one from the library, adopt a tier and give its block an anchor.",
        );
        self.open_imported_library_design()
    }

    /// Films the second part of the Deep Solve lesson's Setup, as a learner would do it, and
    /// leaves Deep Solve enabled on a design that solves:
    ///
    /// 1. Adopt is clicked on the first imported tier (the Pavilion's only one). The solve then
    ///    fails with "Pavilion has no anchor", and that row shows Add Anchor beside Adopt.
    /// 2. Add Anchor is clicked on that row. It opens the Tier form on Exact Scale Value; the
    ///    scale the file recorded for the tier is typed again and the tier is saved, which
    ///    gives the block its anchor and makes the design solve.
    /// 3. Every tier is pinned again, and Deep Solve is dim ("nothing to repair"), so one more
    ///    tier is adopted, from a block that keeps another pinned tier as its anchor.
    ///
    /// Deep Solve must then be enabled and the design must solve, so the lesson's run has a real
    /// search to do and a verdict to read.
    pub(crate) fn adopt_for_deep_solve(&mut self) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        // What each row recorded in the file, before anything is adopted: the scale an anchor
        // types back in.
        let recorded: Vec<(i32, String)> = editor
            .get_tiers()
            .iter()
            .map(|row| (row.constraint_kind, row.constraint_text.to_string()))
            .collect();
        let first = self.adopt_imported_tier(None)?;
        pump(100);
        self.add_anchor_after_adopt(first, &recorded)?;
        // The anchor pins its tier again; Deep Solve needs a tier that is not pinned.
        if !pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || !editor.get_solve_running())? {
            return Err("the solve after Add Anchor did not finish".to_owned());
        }
        if !editor.get_deep_solve_available() {
            self.rec.caption(
                "Deep Solve repairs tiers that are not pinned, so adopt a tier of a block that keeps its anchor.",
            );
            let second = self.adopt_imported_tier(Some(first))?;
            pump(100);
            if !pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
                editor.get_deep_solve_available() && !editor.get_solve_running()
            })? {
                let row = editor.get_tiers().row_data(second);
                return Err(format!(
                    "Deep Solve stays dim after adopting Tier {}; kind={}, available={}, hint={:?}",
                    second + 1,
                    row.map_or(-1, |row| row.constraint_kind),
                    editor.get_deep_solve_available(),
                    editor.get_deep_solve_hint()
                ));
            }
        }
        // The lesson's run needs a design that solves, or its verdict has nothing to read.
        if !pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
            !editor.get_solve_running() && !editor.get_status_is_problem()
        })? {
            return Err(format!(
                "the Deep Solve setup design does not solve: {:?}",
                editor.get_status_text()
            ));
        }
        self.rec.caption("");
        self.hold(pacing::RESULT_HOLD)
    }

    /// Clicks Add Anchor on the row of the unanchored block, types the scale the file recorded
    /// for that tier into the Tier form the click opened, and saves the tier. Nothing happens
    /// (and a note says so) when no row asks for an anchor.
    fn add_anchor_after_adopt(
        &mut self,
        adopted: usize,
        recorded: &[(i32, String)],
    ) -> Result<(), String> {
        let editor = self.ui.global::<EditorModel>();
        // The failed solve names the block: its rows carry the Add Anchor chip.
        if !pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
            !editor.get_solve_running() && editor.get_tiers().iter().any(|row| row.needs_anchor)
        })? {
            self.note_once(
                "deep-solve-no-anchor-row",
                format!(
                    "Deep Solve setup: no tier row asked for an anchor after adopting Tier {} \
                     (status {:?}); Add Anchor was not clicked",
                    adopted + 1,
                    editor.get_status_text()
                ),
            );
            return Ok(());
        }
        let target = editor
            .get_tiers()
            .iter()
            .position(|row| row.needs_anchor)
            .ok_or("the row that needs an anchor disappeared")?;
        let scale = recorded
            .get(target)
            .filter(|(kind, _)| *kind == 2)
            .or_else(|| recorded.get(adopted).filter(|(kind, _)| *kind == 2))
            .map(|(_, text)| text.clone())
            .ok_or("the tier that needs an anchor recorded no scale value in the file")?;
        if scale.parse::<f64>().is_err() {
            return Err(format!(
                "the recorded scale value {scale:?} is not a number"
            ));
        }
        self.rec
            .caption("The Pavilion has no anchor: Add Anchor on its row.");
        let prefix = format!("Tier {},", target + 1);
        let (chip, turns) = self.row_chip_for_row(&prefix, ANCHOR_BUTTON)?;
        self.click_and_invoke(&chip)?;
        let selected = i32::try_from(target).unwrap_or(-1);
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            editor.get_selected_tier_index() == selected && editor.get_inspector_tab() == 0
        }) {
            return Err(format!(
                "Add Anchor on {prefix:?} did not open the Tier form on that row (selected row {}, inspector tab {})",
                editor.get_selected_tier_index(),
                editor.get_inspector_tab()
            ));
        }
        self.unturn_table(turns)?;
        self.hold(pacing::RESULT_HOLD)?;
        self.save_exact_scale(&scale)?;
        pump(100);
        if !pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
            !editor.get_solve_running() && !editor.get_tiers().iter().any(|row| row.needs_anchor)
        })? {
            return Err(format!(
                "Add Anchor and Save Tier left {prefix:?} without an anchor: {:?}",
                editor.get_status_text()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Clicks Adopt on one imported row that still has its pinned source constraint, then checks
    /// that the same row changed before restoring the table's scroll position.
    pub(super) fn adopt_one_imported_tier(&mut self) -> Result<(), String> {
        self.adopt_imported_tier(None).map(|_| ())
    }

    /// Clicks Adopt on an imported, still-pinned row and returns its index. With `after_anchor`
    /// (the row Add Anchor pinned again) the row must not be that one and must sit in a block that
    /// keeps another pinned tier, so the adoption cannot leave a block without an anchor.
    fn adopt_imported_tier(&mut self, after_anchor: Option<usize>) -> Result<usize, String> {
        let editor = self.ui.global::<EditorModel>();
        let rows = editor.get_tiers();
        let pinned_in_block = |block: &str| {
            rows.iter()
                .filter(|row| row.block.as_str() == block && row.constraint_kind == 2)
                .count()
        };
        let (index, row) = rows
            .iter()
            .enumerate()
            .find(|(at, row)| {
                !row.imported_meet_text.is_empty()
                    && row.constraint_kind == 2
                    && after_anchor
                        .is_none_or(|skip| *at != skip && pinned_in_block(row.block.as_str()) >= 2)
            })
            .ok_or("there is no imported, still-pinned tier to adopt")?;
        let prefix = format!("Tier {},", index + 1);
        let (adopt, turns) = self.row_chip_for_row(&prefix, ADOPT_BUTTON)?;
        self.click_and_invoke(&adopt)?;
        let after = editor
            .get_tiers()
            .row_data(index)
            .ok_or("the imported tier disappeared after Adopt")?;
        if after.constraint_kind == 2 || after.constraint_kind == row.constraint_kind {
            return Err(format!(
                "Adopt did not change {prefix:?} from its pinned constraint (kind {})",
                after.constraint_kind
            ));
        }
        self.unturn_table(turns)?;
        Ok(index)
    }

    /// The chip `label` (Adopt or Add Anchor) of the row `row_prefix` names, with the sideways
    /// turns it took to bring the exact row and control into the safe viewport. Leave the row
    /// visible for review.
    ///
    /// The chips sit in the pinned ACTIONS group at the right edge of the table, which is
    /// visible without scrolling sideways, so the route normally needs no sideways turns. The
    /// exact row's control must still have its center and most of its bounds inside the
    /// dock-clipped viewport, clear of both overlay scrollbars. Real vertical (and, if ever
    /// needed, sideways) wheel turns reveal it, with fresh row-scoped queries after each turn.
    fn row_chip_for_row(&mut self, row_prefix: &str, label: &str) -> Result<(Spot, usize), String> {
        let row_index = row_prefix
            .strip_prefix("Tier ")
            .and_then(|s| s.split(',').next())
            .and_then(|s| s.parse::<i32>().ok())
            .map(|n| n - 1)
            .ok_or("invalid tier row label")?;
        let mut row_revealed = false;
        let mut last_row_y: Option<f32> = None;
        for _ in 0..40 {
            let viewport = self.safe_tier_table_viewport()?;
            let row = self.tier_row_spot(row_index)?;
            let inside = (row.y + row.h).min(viewport.y + viewport.h) - row.y.max(viewport.y);
            if row.h > 0.0 && inside >= 0.6 * row.h {
                debug_check("tier row visible share", &true, &true);
                row_revealed = true;
                break;
            }
            if last_row_y.is_some_and(|y| (y - row.y).abs() <= 0.5) {
                debug_check("tier row visible share", &true, &false);
                return Err(format!(
                    "tier row {row_prefix:?} never fit inside the safe table viewport: table \
                     cannot scroll further; row {:?}, safe viewport {:?}",
                    (row.x, row.y, row.w, row.h),
                    (viewport.x, viewport.y, viewport.w, viewport.h)
                ));
            }
            last_row_y = Some(row.y);
            let delta = (row.center().1 - viewport.center().1).clamp(-120.0, 120.0);
            let wheel = if delta.abs() >= 1.0 {
                -delta
            } else if row.y < viewport.y {
                80.0
            } else {
                -80.0
            };
            self.wheel_over(&viewport, wheel)?;
            pump(60);
        }
        if !row_revealed {
            return Err(format!(
                "tier row {row_prefix:?} never fit inside the safe table viewport"
            ));
        }
        let (button, horizontal_turns) = self
            .reveal_table_button(row_index, |film, _area| {
                film.tier_row_control(row_index, uiprobe::AccessibleRole::Button, label)
                    .ok()
            })
            .map_err(|why| format!("{label} for {row_prefix:?}: {why}"))?;
        Ok((button, horizontal_turns))
    }

    /// Films the step "Open it" of the Open lesson: File > Open on the file the lesson's Save step
    /// wrote, which the picker answers with.
    pub(super) fn open_saved_design(&mut self) -> Result<(), String> {
        let path = output_path(&self.id, SAVED_DESIGN);
        if !path.is_file() {
            return Err(format!(
                "File > Open: the saved design {} is missing",
                path.display()
            ));
        }
        self.ensure_edit_view()?;
        harness_support::queue_picker_path(path);
        let file = targets::file_button(&self.view()).ok_or("File button is missing")?;
        self.click_and_invoke(&file)?;
        self.hold(pacing::MENU_HOLD)?;
        let open = targets::menu_item(&self.view(), "Open").ok_or("File > Open is missing")?;
        self.click_and_invoke(&open)?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            harness_support::pending_picker_answers() == 0
        }) {
            return Err("File > Open did not consume its picker path".to_owned());
        }
        let editor = self.ui.global::<EditorModel>();
        if !pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
            editor.get_has_design() && !editor.get_solve_running()
        })? {
            return Err(
                "File > Open consumed the picker path but the design did not finish loading".into(),
            );
        }
        Ok(())
    }

    /// Expands the library panel when the stage collapsed it, by clicking its rail button.
    fn show_library_list(&mut self) -> Result<(), String> {
        let library = self.ui.global::<LibraryModel>();
        if !library.get_panel_collapsed() {
            return Ok(());
        }
        let button = targets::show_library_button(&self.view())
            .ok_or("the Show Library button is missing")?;
        self.click_and_invoke(&button)?;
        pump(30);
        if library.get_panel_collapsed() {
            return Err("the Show Library click did not expand the panel".to_owned());
        }
        self.hold(pacing::STAGE_HOLD)
    }

    /// Clicks File, then Load Selected, and answers the unsaved-changes question when the editor
    /// asks it.
    fn load_selected_design(&mut self) -> Result<(), String> {
        // The File menu is in the Edit view's command bar; the app starts on Live Render.
        self.ensure_edit_view()?;
        let file = targets::file_button(&self.view()).ok_or("File button is missing")?;
        self.click_and_invoke(&file)?;
        self.hold(pacing::MENU_HOLD)?;
        let load = targets::menu_item(&self.view(), "Load Selected")
            .ok_or("File > Load Selected is missing")?;
        self.click_and_invoke(&load)?;
        pump(150);
        if self.ui.global::<EditorModel>().get_unsaved_dialog_open() {
            self.hold(pacing::RESULT_HOLD)?;
            let discard = self.scoped_button("ConfirmActionDialog", "Discard")?;
            self.click_and_invoke(&discard)?;
            pump(100);
        }
        Ok(())
    }

    /// Waits for the loaded design to show a tier with a meet its file stated; whether it did.
    fn wait_for_imported_meet(&mut self) -> Result<bool, String> {
        let editor = self.ui.global::<EditorModel>();
        pacing::wait_lapse(&mut self.rec, LOAD_TIMEOUT, || {
            editor.get_has_design()
                && !editor.get_solve_running()
                && editor
                    .get_tiers()
                    .iter()
                    .any(|row| !row.imported_meet_text.is_empty() && row.constraint_kind == 2)
        })
    }
}
