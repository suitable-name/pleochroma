//! The design legend under the 3D view: a click on a design selects that design in the selected
//! result and lights its stones in the view.
//!
//! The legend is a second way to pick the design row of the selected result, so the pick itself
//! is the window's own `select_group` (the same one a click on the design row makes, including
//! the second click that clears it); this module mirrors the outcome into
//! `RoughPlanModel.legend_selected` and tells the view which design to light.

use super::{
    host::{Host, on_host},
    view,
};
use crate::RoughPlanModel;
use slint::ComponentHandle;
use std::rc::Rc;

/// The design the legend shows as picked: the selected design of the selected result, `-1` for
/// none.
#[must_use]
const fn legend_group(selected_result: i32, selected_group: i32) -> i32 {
    if selected_result >= 0 && selected_group >= 0 {
        selected_group
    } else {
        -1
    }
}

/// Registers `RoughPlanModel.legend_picked` on the planner window.
pub(super) fn setup(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_legend_picked(|group| on_host(|host| picked(host, group)));
}

/// A click on design `group` in the legend: the design row of the selected result is picked (a
/// second click on the same design clears it), then the legend and the view follow.
fn picked(host: &Rc<Host>, group: i32) {
    let model = host.window.global::<RoughPlanModel>();
    let result = model.get_selected_result();
    if result < 0 || group < 0 {
        return;
    }
    model.invoke_select_group(result, group);
    sync(host);
}

/// Mirrors the window's selected design into `legend_selected` and the view's highlight when it
/// changed (a design row, a new result or a new plan can change it without the legend).
pub(super) fn sync(host: &Host) {
    let model = host.window.global::<RoughPlanModel>();
    let wanted = legend_group(model.get_selected_result(), model.get_selected_group());
    if model.get_legend_selected() == wanted {
        return;
    }
    model.set_legend_selected(wanted);
    view::set_legend_highlight(host, usize::try_from(wanted).ok());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_legend_shows_a_design_only_while_a_result_is_selected() {
        assert_eq!(legend_group(0, 2), 2);
        assert_eq!(legend_group(3, 0), 0);
        assert_eq!(legend_group(0, -1), -1, "no design picked");
        assert_eq!(legend_group(-1, 2), -1, "no result selected");
        assert_eq!(legend_group(-1, -1), -1);
    }
}
