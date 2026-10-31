//! The verdict reasons popup during the reading steps of the verdict lessons.

use super::{Film, Snap};
use crate::{pacing, support::app::pump, targets::Spot};
use pleochroma_cut::GuideModel;
use slint::ComponentHandle;
use uiprobe::AccessibleRole;

impl Film<'_> {
    /// Whether the `VerdictPopover`'s own reading heading is visible. The Slint model has no
    /// popover-open property, so this checks the visible popover subtree through accessibility.
    pub(super) fn verdict_reading_popup_open(&self) -> bool {
        let view = self.view();
        view.root
            .query_descendants()
            .match_inherits("VerdictPopover")
            .find_first()
            .is_some_and(|popup| {
                popup
                    .query_descendants()
                    .match_accessible_role(AccessibleRole::Text)
                    .find_all()
                    .iter()
                    .any(|element| {
                        element
                            .accessible_label()
                            .is_some_and(|label| label == "Overall Verdict")
                            && Spot::of(element).usable(view.size)
                    })
            })
    }

    /// The verdict lesson deliberately leaves the reasons open for its final reading step.
    /// Escape closes that modal after the reading hold and before `GuidePanel` Next can receive it.
    /// `VerdictPopover` inherits `PopupWindow`, whose subtree may not appear in the main window's
    /// descendant query, so the authored close must not depend on the accessibility probe.
    pub(super) fn close_verdict_reading_popup(&mut self, step: &Snap) -> Result<(), String> {
        let reading_open_reasons = matches!(
            (step.id.as_str(), step.title.as_str()),
            (
                "solving-verdict",
                "Read the reasons" | "Where the limits come from"
            ) | ("solving-verdict-fixes", "The other three Fix buttons")
        );
        if !reading_open_reasons {
            return Ok(());
        }
        self.rec
            .press_keys(self.ui.window(), "Escape")
            .map_err(|error| {
                format!(
                    "{} step {}: Escape verdict popup: {error}",
                    step.id,
                    step.index + 1
                )
            })?;
        pump(30);
        // The popup closes with its own animation; poll for it instead of trusting one pump.
        if !pacing::wait_real(
            pacing::STEP_TIMEOUT.min(std::time::Duration::from_secs(3)),
            || !self.verdict_reading_popup_open(),
        ) {
            return Err(format!(
                "{} step {} {:?}: Escape did not close the visible verdict reasons",
                step.id,
                step.index + 1,
                step.title
            ));
        }
        self.hold(pacing::AFTER_NEXT_HOLD)
    }

    /// What the guide and the popup show, for the failure message of a step that did not move.
    pub(super) fn guide_state_line(&self) -> String {
        let guide = self.ui.global::<GuideModel>();
        let now = super::snap(self.ui);
        format!(
            "open={}, index={}/{}, done={}, manual={}, title={:?}, waiting={:?}, verdict_popup_open={}, toast={:?}",
            now.open,
            now.index,
            now.total,
            now.done,
            now.manual,
            now.title,
            guide.get_current().waiting,
            self.verdict_reading_popup_open(),
            self.ui.get_toast_message()
        )
    }
}
