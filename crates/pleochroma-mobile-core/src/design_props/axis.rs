//! The Crystal Axis of the Stone tab beyond its two sliders: the presets, "As Material", the
//! Second Axis of a biaxial stone, the material's own default orientation, and the orientation
//! the stone renders with (what Cutting Mode tells the cutter).
//!
//! The rules are the desktop's (`pleochroma_cut_core::crystal_axis`): the design's own setting
//! beats the material's default, which beats the material's built-in axis. Every edit here is
//! one undoable `Edit::SetRenderEffects`, like the sliders.

use glam::DVec3;
use pleochroma::{
    geometry::stone_metrics::{caliper_frame, measure_solid_with_vertices},
    optics::materials::{GemMaterial, OpticalCharacter},
};
use pleochroma_cut_core::{
    AxisPreset, CrystalOrientation, color_presets, crystal_axis::orientation_sentence,
    stone_orientation, width_azimuth_deg,
};

use super::{
    materials::resolved_gem,
    stone::{round6, update_effects},
};
use crate::app_state::AppState;

/// One button of the Crystal Axis presets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisPresetRow {
    /// The button label (Title Case): `Through the Table`, `Best Blue Face-Up`, ...
    pub label: String,
    /// One plain sentence about what the preset does.
    pub hint: String,
}

/// The number of geometry presets; color-named presets follow them.
const GEOMETRY_PRESETS: usize = AxisPreset::ALL.len();

/// Whether `gem` has a crystal axis to orient (an isotropic material has none).
pub(super) fn has_axis(gem: &GemMaterial) -> bool {
    gem.optical_character != OpticalCharacter::Isotropic
}

/// The default orientation the design's material brings.
///
/// A custom material's stored default
/// (its tilt and azimuth, and the Second Axis when it has one). `None` for a built-in material,
/// for a custom one that stores none, and when no material is set.
#[must_use]
pub fn material_default_orientation(state: &AppState) -> Option<CrystalOrientation> {
    let name = state.session.design.material.name.as_deref()?;
    let (tilt, azimuth, second) = state.props.snapshot_for(name)?.default_orientation();
    Some(CrystalOrientation::from_angles(
        f64::from(tilt?),
        f64::from(azimuth?),
        second.map(f64::from),
    ))
}

/// Whether the design's material is biaxial (then the Second Axis is part of its orientation).
/// `false` for an unknown material.
#[must_use]
pub fn axis_is_biaxial(state: &AppState) -> bool {
    resolved_gem(state).is_some_and(|gem| gem.biaxial_delta_beta_alpha.is_some())
}

/// The orientation the stone renders with.
///
/// The design's own setting when its override is on,
/// else the material's default. `None` when the stone keeps the material's built-in axis, and
/// for an isotropic material (it has no axis to describe).
#[must_use]
pub fn effective_orientation(state: &AppState) -> Option<CrystalOrientation> {
    if resolved_gem(state).is_some_and(|gem| !has_axis(&gem)) {
        return None;
    }
    stone_orientation(
        &state.session.design.render_effects,
        material_default_orientation(state),
    )
}

/// The sentence Cutting Mode shows on its first step: how the crystal axis must run in the
/// rough. `None` when the stone keeps its material's built-in axis or the material is isotropic.
#[must_use]
pub fn orientation_line(state: &AppState) -> Option<String> {
    effective_orientation(state).map(|orientation| orientation_sentence(&orientation))
}

/// The azimuth in `[0, 180)` of the width direction of the solved stone (the rotating-caliper
/// minimum width of its outline), `None` before the design is solved or when it does not close.
fn design_width_azimuth_deg(state: &AppState) -> Option<f64> {
    let planes = state
        .solve
        .current_planes(state.session.current_generation())?;
    let converted: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|plane| {
            (
                DVec3::new(
                    f64::from(plane.normal[0]),
                    f64::from(plane.normal[1]),
                    f64::from(plane.normal[2]),
                ),
                -f64::from(plane.d),
            )
        })
        .collect();
    let (_, vertices) = measure_solid_with_vertices(&converted)?;
    let outline: Vec<(f64, f64)> = vertices.iter().map(|v| (v.x, v.z)).collect();
    width_azimuth_deg(caliper_frame(&outline)?.width_dir)
}

/// The preset buttons of the design's material.
///
/// The three geometry presets, then the
/// color-named presets of a trichroic material (Tanzanite, Alexandrite, Andalusite). Empty for an
/// isotropic material. The color presets are worked out from the material's absorption, so
/// ask only when the material changed.
#[must_use]
pub fn axis_preset_rows(state: &AppState) -> Vec<AxisPresetRow> {
    let Some(gem) = resolved_gem(state).filter(has_axis) else {
        return Vec::new();
    };
    let geometry = AxisPreset::ALL.into_iter().map(|preset| AxisPresetRow {
        label: preset.label().to_owned(),
        hint: preset.hint().to_owned(),
    });
    let colors = color_presets(&gem).into_iter().map(|preset| AxisPresetRow {
        label: preset.id.label().to_owned(),
        hint: format!(
            "Sets the crystal axis and the second axis for {}.",
            preset.id.label()
        ),
    });
    geometry.chain(colors).collect()
}

/// Applies the preset at `index` of [`axis_preset_rows`]: the Crystal Axis override is switched
/// on and its angles set (a color-named preset sets the Second Axis too).
///
/// # Errors
///
/// A sentence when there is no such preset, the material has no crystal axis, or the design
/// has to be solved before the width direction is known.
pub fn apply_axis_preset(state: &mut AppState, index: usize) -> Result<bool, String> {
    let Some(gem) = resolved_gem(state).filter(has_axis) else {
        return Err("This material has no crystal axis to orient.".to_owned());
    };
    let (tilt, azimuth, second) = if let Some(preset) = AxisPreset::ALL.get(index) {
        let width = design_width_azimuth_deg(state);
        let (tilt, azimuth) = preset.angles(width).ok_or_else(|| {
            "Solve the design first: the width of the stone is not known yet.".to_owned()
        })?;
        (tilt, azimuth, None)
    } else {
        let preset = color_presets(&gem)
            .into_iter()
            .nth(index.saturating_sub(GEOMETRY_PRESETS))
            .ok_or_else(|| "That orientation is not available for this material.".to_owned())?;
        (
            preset.orientation.axis.tilt_deg(),
            preset.orientation.axis.azimuth_deg(),
            preset.orientation.second_turn_deg,
        )
    };
    update_effects(state, |effects| {
        effects.c_axis_override_enabled = true;
        effects.c_axis_tilt_deg = round6(tilt);
        effects.c_axis_azimuth_deg = round6(azimuth);
        // A geometry preset leaves the Second Axis as it is; a color-named one sets it.
        if second.is_some() {
            effects.c_axis_second_azimuth_deg = second.map(round6);
        }
    })
}

/// "As Material": switches the design's own Crystal Axis off, so the stone keeps its material's
/// axis again (the material's default, else the built-in one).
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub fn use_material_axis(state: &mut AppState) -> Result<bool, String> {
    update_effects(state, |effects| effects.c_axis_override_enabled = false)
}

/// Sets or clears the Second Axis of a biaxial stone: `turn_deg` (clamped to a half turn)
/// when `on`, else none.
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub fn set_second_axis(state: &mut AppState, on: bool, turn_deg: f32) -> Result<bool, String> {
    let turn = if turn_deg.is_finite() {
        turn_deg.clamp(0.0, 180.0)
    } else {
        0.0
    };
    update_effects(state, |effects| {
        effects.c_axis_second_azimuth_deg = on.then(|| round6(f64::from(turn)));
    })
}
