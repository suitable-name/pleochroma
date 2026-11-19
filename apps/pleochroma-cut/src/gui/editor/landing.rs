//! A design landing in the editor: New Design, Open, Open Recent, Load Selected and the
//! start-up restore prompt.
//!
//! The three commit tails (`callbacks::tier_actions::new_design::do_new_design_create`,
//! `callbacks::tier_actions::lifecycle::apply_loaded_design` and
//! `native_io::open_commit::finish_state_replace`) call [`design_landed`] once the new design is
//! in the editor state and its wholesale refresh has been asked for. Two things follow:
//!
//! - **The Edit view is shown.** A design that lands in the editor is worked on there, so the
//!   cutter is taken to it from whichever page the action was started on (the palette and the
//!   keys work on every page). Two landings leave the page alone: a design given on the command
//!   line (the app starts on Live Render), and a design a guide asked for, because the guide
//!   shows its own first step's stage when it opens (`guide::launch`).
//! - **The landing is remembered** until the design's first solve has claimed the viewport, so
//!   entering the Edit view meanwhile does not solve the same design a second time
//!   ([`first_solve_claims_viewport`], read by `view::reclaim_viewport_for_editor`).
//!
//! # Order against the refresh
//!
//! `view::refresh_all_now` may put its refresh off by one turn of the event loop, to paint
//! "Solving..." first. Showing the Edit view before that refresh would size the design's first
//! frame from a viewport that is not mounted yet, so such a landing shows the view one turn
//! later too ([`Refresh::MayBeDeferred`]). A page change made in that turn wins: the reveal is
//! dropped when the page is no longer the one the design landed on.

use super::guide;
use crate::{MainWindow, gui::commands};
use slint::ComponentHandle;
use std::{cell::Cell, time::Duration};

/// Whether a design that lands in the editor takes the cutter to the Edit view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gui::editor) enum Landing {
    /// Show the Edit view: New Design, Open, Open Recent, Load Selected, the restore prompt.
    ShowEditView,
    /// Leave the page as it is: a design given on the command line.
    StayOnPage,
}

/// Where the landing's wholesale refresh stands when [`design_landed`] is called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gui::editor) enum Refresh {
    /// `view::refresh_all` has returned.
    Ran,
    /// `view::refresh_all_now` was called, which may run the refresh one turn later.
    MayBeDeferred,
}

/// The window's page: the outer tab and the 3D tab's view.
type Page = (i32, i32);

/// The 3D tab's Edit view.
const EDIT_PAGE: Page = (0, 1);

thread_local! {
    /// The editor generation of the design that landed last, `None` before the first landing.
    /// UI thread only, like the editor state itself.
    static LANDED_GENERATION: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Whether a landing shows the Edit view: it asks for it, and no guide is waiting for this
/// design (the guide shows its own stage when it opens).
const fn shows_edit_view(landing: Landing, guide_launch_pending: bool) -> bool {
    matches!(landing, Landing::ShowEditView) && !guide_launch_pending
}

/// Whether a reveal put off by one turn still happens: the page is the one the design landed
/// on. Anything that changed the page in between (a script, a guide, the cutter) wins.
const fn page_is_unchanged(at_landing: Page, now: Page) -> bool {
    at_landing.0 == now.0 && at_landing.1 == now.1
}

/// Whether the first solve of the design that landed at generation `landed` is still on its
/// way to the viewport: the design has not been edited since (`generation`), a solve is running
/// or about to, and no Library selection or job view took the viewport after the landing.
const fn first_solve_pending(
    landed: Option<u64>,
    generation: u64,
    solve_running: bool,
    library_took_over: bool,
) -> bool {
    matches!(landed, Some(at) if at == generation) && solve_running && !library_took_over
}

/// The page on screen.
fn page(ui: &MainWindow) -> Page {
    (ui.get_active_tab(), ui.get_render_view_tab())
}

/// Shows the Edit view, unless it is the page on screen already (nothing moves then, and the
/// viewport is not asked to change hands).
fn show_edit_view(ui: &MainWindow) {
    if page(ui) != EDIT_PAGE {
        commands::show_edit_view(ui);
    }
}

/// A design has landed in the editor at `generation` and its wholesale refresh was asked for:
/// remembers the landing and shows the Edit view as `landing` says, in step with `refresh`
/// (see the module doc).
///
/// Call it before the worked-example guide is told about the new design: the guide's launch
/// ends on that report, and a design a launch asked for must not show the Edit view here.
pub(in crate::gui::editor) fn design_landed(
    ui: &MainWindow,
    generation: u64,
    landing: Landing,
    refresh: Refresh,
) {
    LANDED_GENERATION.with(|cell| cell.set(Some(generation)));
    if !shows_edit_view(landing, guide::launch_pending()) {
        return;
    }
    match refresh {
        Refresh::Ran => show_edit_view(ui),
        Refresh::MayBeDeferred => {
            let at_landing = page(ui);
            let ui_weak = ui.as_weak();
            slint::Timer::single_shot(Duration::ZERO, move || {
                if let Some(ui) = ui_weak.upgrade()
                    && page_is_unchanged(at_landing, page(&ui))
                {
                    show_edit_view(&ui);
                }
            });
        }
    }
}

/// Whether the design that landed last is still waiting for its first solve to claim the
/// viewport, so the Edit view need not start another one. `generation` is the editor's live
/// generation, `solve_running` is `EditorModel.solve_running` (raised before a deferred refresh
/// and by a background solve), and `library_took_over` is
/// `RenderContext::library_took_over_since(generation)`: a Library row or a job view picked
/// after the landing keeps the viewport, and only the Edit view's own refresh takes it back.
#[must_use]
pub(in crate::gui::editor) fn first_solve_claims_viewport(
    generation: u64,
    solve_running: bool,
    library_took_over: bool,
) -> bool {
    first_solve_pending(
        LANDED_GENERATION.with(Cell::get),
        generation,
        solve_running,
        library_took_over,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_design_that_lands_shows_the_edit_view() {
        assert!(shows_edit_view(Landing::ShowEditView, false));
    }

    #[test]
    fn a_design_from_the_command_line_leaves_the_start_up_page() {
        assert!(!shows_edit_view(Landing::StayOnPage, false));
        assert!(!shows_edit_view(Landing::StayOnPage, true));
    }

    #[test]
    fn a_design_a_guide_asked_for_is_left_to_the_guide() {
        assert!(!shows_edit_view(Landing::ShowEditView, true));
    }

    #[test]
    fn a_deferred_reveal_is_dropped_when_the_page_changed_meanwhile() {
        // Landed on Cutting Instructions and still there: reveal.
        assert!(page_is_unchanged((1, 0), (1, 0)));
        // Something went to Live Render in between: it wins.
        assert!(!page_is_unchanged((1, 0), (0, 0)));
        assert!(!page_is_unchanged((0, 0), (0, 1)));
    }

    #[test]
    fn the_first_solve_of_an_unedited_landing_claims_the_viewport() {
        assert!(first_solve_pending(Some(7), 7, true, false));
    }

    #[test]
    fn the_edit_view_reclaims_the_viewport_itself_in_every_other_case() {
        // Nothing landed yet.
        assert!(!first_solve_pending(None, 7, true, false));
        // The design was edited since it landed.
        assert!(!first_solve_pending(Some(7), 8, true, false));
        // The solve finished, failed or was abandoned.
        assert!(!first_solve_pending(Some(7), 7, false, false));
        // A Library row or a job view took the viewport after the landing.
        assert!(!first_solve_pending(Some(7), 7, true, true));
    }
}
