//! Publishes the Cut slider's fields into `SolidModel` for the design on screen.
//!
//! The slider walks the design's cutting steps in cutting order (see
//! `pleochroma_web_core::cut_slider`, which holds the mapping and its tests); this module only
//! reads the stored `SolidModel.tier-cutoff`, decodes it for the design's step count and writes
//! back what the toolbar shows: the step count, the handle, the label, and a cutoff that no
//! longer fits (a tier was removed) normalised to the finished stone. Setting a property to the
//! value it already has changes nothing, so this is cheap to call on every refresh.

use crate::{AppWindow, SolidModel};
use pleochroma_cut_core::Design;
use pleochroma_web_core::cut_slider::CutSlider;
use slint::ComponentHandle;

/// Brings `SolidModel`'s Cut slider fields in line with `design` and returns the slider as it
/// now stands. `design` is the committed design, never the Slice tool's provisional one.
pub fn publish(ui: &AppWindow, design: &Design) -> CutSlider {
    let model = ui.global::<SolidModel>();
    let stored = model.get_tier_cutoff();
    let slider = CutSlider::new(stored, design.preview_step_count());
    model.set_cut_step_count(step_count_value(slider.step_count));
    if slider.tier_cutoff() != stored {
        model.set_tier_cutoff(slider.tier_cutoff());
    }
    model.set_cut_position(slider.handle());
    model.set_cut_label(slider.label().into());
    slider
}

/// A step count as Slint's `int`.
fn step_count_value(step_count: usize) -> i32 {
    i32::try_from(step_count).unwrap_or(i32::MAX)
}

/// The step count `SolidModel` last published, for a handle value that arrives from the slider.
pub fn published_step_count(ui: &AppWindow) -> usize {
    usize::try_from(ui.global::<SolidModel>().get_cut_step_count()).unwrap_or(0)
}
