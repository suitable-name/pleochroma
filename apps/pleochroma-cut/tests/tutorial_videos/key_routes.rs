//! Filmed key presses of the Keyboard Shortcuts lesson: the digit keys that switch the Solid
//! viewport's view mode, Ctrl+E between Live Render and Edit, and the ? key that lists every
//! shortcut. The lesson teaches the keys, so no pill, menu or palette stands in for them, and each
//! press is followed by the visible change it must cause.

use crate::{director::Film, pacing, support::app::pump, targets::Spot};
use pleochroma_cut::{ShortcutsModel, SolidPreviewModel};
use slint::ComponentHandle;
use std::time::Duration;

impl Film<'_> {
    /// Presses the digit `key` (1 to 4) and requires the Solid viewport to show view mode `mode`.
    pub(super) fn press_view_key(&mut self, key: &str, mode: i32) -> Result<(), String> {
        self.key(key)?;
        pump(80);
        let shown = self.ui.global::<SolidPreviewModel>().get_view_mode();
        if shown != mode {
            return Err(format!(
                "pressing {key} left the view mode at {shown}, not {mode}; the Edit tab must be \
                 open and no text box may have the keyboard"
            ));
        }
        // The picture of the new mode arrives a frame or two later (the Diagram's first frame
        // would otherwise be shot as the "Solve to preview" placeholder).
        let ui = self.ui;
        let _ = pacing::wait_real(Duration::from_secs(2), || {
            let solid = ui.global::<SolidPreviewModel>();
            if mode == 3 {
                solid.get_has_diagram()
            } else {
                solid.get_has_solid()
            }
        });
        pump(30);
        self.hold(pacing::RESULT_HOLD)
    }

    /// Presses Ctrl+E to leave the Edit tab for Live Render and once more to come back, and
    /// requires the tab to change both times. The app restores its global shortcut scope after
    /// switching view subtrees; a click on the view pill would not focus it.
    pub(super) fn swap_tabs_with_keys(&mut self) -> Result<(), String> {
        if self.ui.get_active_tab() != 0 || self.ui.get_render_view_tab() != 1 {
            return Err(format!(
                "Ctrl+E lesson must start in the Edit view: top_tab={}, render_view={}",
                self.ui.get_active_tab(),
                self.ui.get_render_view_tab()
            ));
        }
        self.key("Ctrl+E")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            self.ui.get_active_tab() == 0 && self.ui.get_render_view_tab() == 0
        }) {
            return Err(format!(
                "Ctrl+E did not leave Edit for Live Render: top_tab={}, render_view={}",
                self.ui.get_active_tab(),
                self.ui.get_render_view_tab()
            ));
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.key("Ctrl+E")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            self.ui.get_active_tab() == 0 && self.ui.get_render_view_tab() == 1
        }) {
            return Err(format!(
                "the second Ctrl+E did not bring Edit back: top_tab={}, render_view={}",
                self.ui.get_active_tab(),
                self.ui.get_render_view_tab()
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Presses ? to open the list of shortcuts and Esc to close it, and requires it to open and to
    /// close.
    pub(super) fn open_shortcuts_with_key(&mut self) -> Result<(), String> {
        self.key("?")?;
        pump(80);
        let shortcuts = self.ui.global::<ShortcutsModel>();
        if !shortcuts.get_open() {
            return Err(
                "pressing ? did not open the shortcuts list; no text box may have the keyboard"
                    .to_owned(),
            );
        }
        let view = self.view();
        let overlay = view
            .root
            .query_descendants()
            .match_inherits("ShortcutOverlay")
            .find_first()
            .filter(|element| Spot::of(element).usable(view.size))
            .ok_or("Keyboard Shortcuts overlay is not visible")?;
        let window_size = view.size;
        let heading_visible = overlay
            .query_descendants()
            .match_predicate(move |element| {
                element
                    .accessible_label()
                    .is_some_and(|label| label == "Keyboard Shortcuts")
                    && Spot::of(element).usable(window_size)
            })
            .find_first()
            .is_some();
        if !heading_visible {
            return Err("Keyboard Shortcuts overlay has no visible heading".to_owned());
        }
        self.hold(Duration::from_secs(3))?;
        self.key("Escape")?;
        pump(80);
        if shortcuts.get_open() {
            return Err("Esc did not close the shortcuts list".to_owned());
        }
        Ok(())
    }
}
