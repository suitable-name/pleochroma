//! The Stone tab's Abraded Edges group: the conversion between its two controls (Width and
//! Wear, `RenderEffectsModel`) and the design's [`EdgeAbrasionEffects`].
//!
//! The rim polish boost (`halo_rq_nm`) has no control; it stays whatever the design holds.

use super::inclusion_effects::{keep_or, narrow};
use crate::{MainWindow, RenderEffectsModel};
use pleochroma_cut_core::EdgeAbrasionEffects;
use slint::ComponentHandle;

/// The wear a band starts with when its width is first raised from nothing: a worn band needs
/// some roughness to show.
pub(in crate::gui) const STARTING_WEAR: f32 = 0.3;

/// What the Abraded Edges controls hold.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(in crate::gui) struct EdgeControls {
    pub(in crate::gui) width: f32,
    pub(in crate::gui) wear: f32,
}

/// The controls that show `effects`.
#[must_use]
pub(in crate::gui) const fn controls_for(effects: &EdgeAbrasionEffects) -> EdgeControls {
    EdgeControls {
        width: narrow(effects.width),
        wear: narrow(effects.alpha),
    }
}

/// `controls` with the starting wear when the band has width but no wear yet (a band of no wear
/// is invisible, so raising the width would seem to do nothing).
#[must_use]
pub(in crate::gui) fn with_starting_wear(controls: EdgeControls) -> EdgeControls {
    if controls.width > 0.0 && controls.wear <= 0.0 {
        EdgeControls {
            wear: STARTING_WEAR,
            ..controls
        }
    } else {
        controls
    }
}

/// The design's setting the controls stand for, keeping the design's own values where the
/// controls still hold them and the design's rim polish boost always.
#[must_use]
pub(in crate::gui) fn effects_from_controls(
    controls: EdgeControls,
    base: &EdgeAbrasionEffects,
) -> EdgeAbrasionEffects {
    EdgeAbrasionEffects {
        width: keep_or(Some(base.width), controls.width),
        alpha: keep_or(Some(base.alpha), controls.wear),
        halo_rq_nm: base.halo_rq_nm,
    }
    .sanitised()
}

/// Reads the Abraded Edges controls.
#[must_use]
pub(in crate::gui) fn read_controls(ui: &MainWindow) -> EdgeControls {
    let model = ui.global::<RenderEffectsModel>();
    EdgeControls {
        width: model.get_edge_width(),
        wear: model.get_edge_wear(),
    }
}

/// Shows `controls` in the Abraded Edges controls. Setting a property from Rust fires no
/// callback.
pub(in crate::gui) fn write_controls(ui: &MainWindow, controls: EdgeControls) {
    let model = ui.global::<RenderEffectsModel>();
    model.set_edge_width(controls.width);
    model.set_edge_wear(controls.wear);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_round_trip_and_keep_the_rim_boost() {
        let effects = EdgeAbrasionEffects {
            width: 0.034_567_89,
            alpha: 0.4,
            halo_rq_nm: 12.5,
        };
        let controls = controls_for(&effects);
        assert_eq!(effects_from_controls(controls, &effects), effects);
        // Another design's boost is not touched by the two controls.
        let moved = EdgeControls {
            width: 0.05,
            ..controls
        };
        let written = effects_from_controls(moved, &effects);
        assert_eq!(narrow(written.width).to_bits(), 0.05f32.to_bits());
        assert_eq!(written.halo_rq_nm.to_bits(), 12.5f64.to_bits());
    }

    #[test]
    fn the_default_is_off_and_a_first_width_gets_some_wear() {
        let off = EdgeAbrasionEffects::default();
        assert_eq!(effects_from_controls(controls_for(&off), &off), off);
        let raised = with_starting_wear(EdgeControls {
            width: 0.02,
            wear: 0.0,
        });
        assert_eq!(raised.wear.to_bits(), STARTING_WEAR.to_bits());
        let kept = with_starting_wear(EdgeControls {
            width: 0.02,
            wear: 0.7,
        });
        assert_eq!(kept.wear.to_bits(), 0.7f32.to_bits());
        let none = with_starting_wear(EdgeControls::default());
        assert_eq!(none, EdgeControls::default());
    }
}
