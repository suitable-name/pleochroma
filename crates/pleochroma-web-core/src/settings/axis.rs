//! The Crystal Axis of a render, from the two places it can come from: the design (while the
//! render is linked to it) or the per-user setting of the Render panel.
//!
//! While linked, the design's own `[render_effects]` win, as its stone width and material do:
//! the stone the page shows is the stone the design describes, so a design saved with an axis (or
//! with none) renders the same on every surface. The per-user setting applies only to an
//! unlinked render (a header material with no design behind it).

use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::{CrystalOrientation, Design, StoneRenderEffects};

use super::{RenderSettings, c_axis_from_angles, render_material};

/// Whether the material a render traces is biaxial, so the Render panel offers a Second Axis
/// (a uniaxial or isotropic stone has none). `false` when the linked design's material
/// resolves to nothing.
#[must_use]
pub fn render_material_is_biaxial(
    settings: &RenderSettings,
    design: Option<&Design>,
    custom_materials: &[GemMaterial],
) -> bool {
    let Ok(material) = render_material(settings, design, custom_materials) else {
        return false;
    };
    custom_materials
        .iter()
        .find(|custom| custom.name.eq_ignore_ascii_case(&material.name))
        .cloned()
        .or_else(|| GemMaterial::by_name(&material.name))
        .is_some_and(|gem| gem.biaxial_delta_beta_alpha.is_some())
}

/// The `(Crystal Axis, Second Axis)` override pair of a render: the axis when `on`, else none;
/// the Second Axis (alpha) only with an axis and a turn.
///
/// The axis is the vector the angle sliders have always produced
/// ([`c_axis_from_angles`]), so a render without a Second Axis is byte-identical to what it was
/// before the Second Axis existed. The Second Axis turns about the CANONICAL axis line
/// (`CrystalOrientation::alpha`, the one the planner rows, the hand-off and the sheet use), so a
/// turn means the same direction whichever end of the axis line the slider vector points to.
#[must_use]
pub(super) fn axis_pair(
    on: bool,
    tilt_deg: f32,
    azimuth_deg: f32,
    second_turn_deg: Option<f32>,
) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
    if !on {
        return (None, None);
    }
    let gamma = c_axis_from_angles(tilt_deg, azimuth_deg);
    let alpha = second_turn_deg.and_then(|turn| {
        CrystalOrientation::from_angles(
            f64::from(tilt_deg),
            f64::from(azimuth_deg),
            Some(f64::from(turn)),
        )
        .alpha()
        .map(|alpha| alpha.to_array())
    });
    (Some(gamma.to_array()), alpha)
}

/// [`axis_pair`] of a design's stone effects.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the slider angles are degrees in 0..=360, far inside f32's range"
)]
pub(super) fn design_axis_pair(
    effects: &StoneRenderEffects,
) -> (Option<[f32; 3]>, Option<[f32; 3]>) {
    axis_pair(
        effects.c_axis_override_enabled,
        effects.c_axis_tilt_deg as f32,
        effects.c_axis_azimuth_deg as f32,
        effects.c_axis_second_azimuth_deg.map(|turn| turn as f32),
    )
}
