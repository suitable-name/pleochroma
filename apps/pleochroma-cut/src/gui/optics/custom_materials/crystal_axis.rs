//! The material editor's Crystal Axis row: the default direction of a custom material's crystal
//! axis. The row's state lives in `CrystalAxisEditorModel`; [`push_row`] fills it when the
//! dialog's pre-fill is pushed and [`read_form`] reads it when Save runs. No database here.

use crate::{CrystalAxisEditorModel, MainWindow};
use pleochroma::optics::materials::{GemMaterial, OpticalCharacter};
use pleochroma_vault::model::material::CustomMaterialRow;
use slint::ComponentHandle;

/// Largest tilt of the crystal axis from the vertical, in degrees.
const MAX_TILT_DEG: f32 = 90.0;
/// The azimuth range is `0..=MAX_AZIMUTH_DEG` (a full turn).
const MAX_AZIMUTH_DEG: f32 = 360.0;
/// The Second Axis turn range is `0..=MAX_SECOND_DEG` (a line, so half a turn).
const MAX_SECOND_DEG: f32 = 180.0;

/// The default crystal-axis orientation the dialog hands to Save, in degrees. `tilt_deg` and
/// `azimuth_deg` are both set or both `None` (the material's built-in axis).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "the fields name their unit; the dialog and the stored row use these names"
)]
pub(super) struct DefaultAxisForm {
    /// Tilt from the stone's vertical (0 to 90).
    pub(super) tilt_deg: Option<f32>,
    /// Azimuth around the vertical (0 to 360).
    pub(super) azimuth_deg: Option<f32>,
    /// Second Axis turn of a biaxial material (0 to 180).
    pub(super) second_deg: Option<f32>,
}

impl DefaultAxisForm {
    /// What a material of `character` may store: nothing for an isotropic one, no Second Axis
    /// for a non-biaxial one, and every angle finite and clamped into its range. Tilt and
    /// azimuth are kept or dropped together.
    pub(super) fn resolved(self, character: OpticalCharacter) -> Self {
        if character == OpticalCharacter::Isotropic {
            return Self::default();
        }
        let clamp = |value: Option<f32>, max: f32| {
            value.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, max))
        };
        let (Some(tilt), Some(azimuth)) = (
            clamp(self.tilt_deg, MAX_TILT_DEG),
            clamp(self.azimuth_deg, MAX_AZIMUTH_DEG),
        ) else {
            return Self::default();
        };
        let biaxial = matches!(
            character,
            OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative
        );
        Self {
            tilt_deg: Some(tilt),
            azimuth_deg: Some(azimuth),
            second_deg: if biaxial {
                clamp(self.second_deg, MAX_SECOND_DEG)
            } else {
                None
            },
        }
    }
}

/// The text for a built-in material's own crystal axis, or `""` for an isotropic material (it
/// has none).
pub(super) fn built_in_axis_text(material: &GemMaterial) -> String {
    if material.optical_character == OpticalCharacter::Isotropic {
        return String::new();
    }
    let axis = material.c_axis;
    let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
    if near(axis.x, 1.0) && near(axis.y, 0.0) && near(axis.z, 0.0) {
        "Along the Length (+X)".to_string()
    } else if near(axis.x, 0.0) && near(axis.y, 1.0) && near(axis.z, 0.0) {
        "Through the Table (+Y)".to_string()
    } else {
        "Along a Tilted Direction".to_string()
    }
}

/// Whole degrees as the spin box shows them.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the angles are clamped to 0..=360 and rounded, so they fit an i32 exactly"
)]
const fn whole_degrees(value: f32) -> i32 {
    value.round() as i32
}

/// Fills `CrystalAxisEditorModel` for the material the dialog is about to edit: the row's stored
/// default (or "Not Set") for a custom material, and the read-only text of the selected built-in
/// material's axis otherwise. The baselines are set to what is pushed, so a fresh dialog is
/// clean.
pub(super) fn push_row(ui: &MainWindow, row: Option<&CustomMaterialRow>, selected_name: &str) {
    let model = ui.global::<CrystalAxisEditorModel>();
    let stored = row.and_then(|r| Some((r.c_axis_tilt_deg?, r.c_axis_azimuth_deg?)));
    let (axis_mode, tilt, azimuth) = stored.map_or((0, 0, 0), |(tilt, azimuth)| {
        (1, whole_degrees(tilt), whole_degrees(azimuth))
    });
    let second = stored.and_then(|_| row.and_then(|r| r.c_axis_second_azimuth_deg));
    let (second_mode, second_deg) = second.map_or((0, 0), |s| (1, whole_degrees(s)));
    model.set_axis_mode(axis_mode);
    model.set_tilt_deg(tilt);
    model.set_azimuth_deg(azimuth);
    model.set_second_mode(second_mode);
    model.set_second_deg(second_deg);
    model.set_stored_tilt_deg(stored.map_or(0.0, |(tilt, _)| tilt));
    model.set_stored_azimuth_deg(stored.map_or(0.0, |(_, azimuth)| azimuth));
    model.set_stored_second_deg(second.unwrap_or(0.0));
    model.set_baseline_axis_mode(axis_mode);
    model.set_baseline_tilt_deg(tilt);
    model.set_baseline_azimuth_deg(azimuth);
    model.set_baseline_second_mode(second_mode);
    model.set_baseline_second_deg(second_deg);
    let text = if row.is_some() {
        String::new()
    } else {
        GemMaterial::all_materials()
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(selected_name))
            .map(built_in_axis_text)
            .unwrap_or_default()
    };
    model.set_built_in_text(text.into());
}

/// `shown` whole degrees of the spin box, or the stored value itself while the box still shows
/// its rounding (a stored 12.5 stays 12.5 on a plain re-save).
const fn degrees(shown: i32, stored: f32) -> f32 {
    if shown == whole_degrees(stored) {
        stored
    } else {
        // Whole degrees, at most 360: exact in f32.
        #[expect(
            clippy::cast_precision_loss,
            reason = "the spin box holds whole degrees from 0 to 360, exact in f32"
        )]
        let value = shown as f32;
        value
    }
}

/// The default axis the row holds right now (what the dialog's Save stores).
pub(super) fn read_form(ui: &MainWindow) -> DefaultAxisForm {
    let model = ui.global::<CrystalAxisEditorModel>();
    if model.get_axis_mode() != 1 {
        return DefaultAxisForm::default();
    }
    DefaultAxisForm {
        tilt_deg: Some(degrees(model.get_tilt_deg(), model.get_stored_tilt_deg())),
        azimuth_deg: Some(degrees(
            model.get_azimuth_deg(),
            model.get_stored_azimuth_deg(),
        )),
        second_deg: (model.get_second_mode() == 1)
            .then(|| degrees(model.get_second_deg(), model.get_stored_second_deg())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(tilt: f32, azimuth: f32, second: Option<f32>) -> DefaultAxisForm {
        DefaultAxisForm {
            tilt_deg: Some(tilt),
            azimuth_deg: Some(azimuth),
            second_deg: second,
        }
    }

    #[test]
    fn an_isotropic_material_stores_no_axis() {
        assert_eq!(
            set(30.0, 45.0, Some(10.0)).resolved(OpticalCharacter::Isotropic),
            DefaultAxisForm::default()
        );
    }

    #[test]
    fn a_uniaxial_material_drops_the_second_axis_and_a_biaxial_one_keeps_it() {
        let form = set(30.0, 45.0, Some(10.0));
        assert_eq!(
            form.resolved(OpticalCharacter::UniaxialNegative),
            set(30.0, 45.0, None)
        );
        assert_eq!(
            form.resolved(OpticalCharacter::BiaxialPositive),
            set(30.0, 45.0, Some(10.0))
        );
    }

    #[test]
    fn tilt_and_azimuth_are_kept_or_dropped_together_and_clamped() {
        let half = DefaultAxisForm {
            tilt_deg: Some(30.0),
            ..DefaultAxisForm::default()
        };
        assert_eq!(
            half.resolved(OpticalCharacter::UniaxialPositive),
            DefaultAxisForm::default()
        );
        assert_eq!(
            set(120.0, 400.0, Some(200.0)).resolved(OpticalCharacter::BiaxialNegative),
            set(90.0, 360.0, Some(180.0))
        );
        assert_eq!(
            set(f32::NAN, 10.0, None).resolved(OpticalCharacter::UniaxialPositive),
            DefaultAxisForm::default()
        );
    }

    #[test]
    fn a_plain_resave_keeps_a_stored_fraction() {
        assert!((degrees(13, 12.5) - 12.5).abs() < f32::EPSILON);
        assert!((degrees(20, 12.5) - 20.0).abs() < f32::EPSILON);
    }

    #[test]
    fn the_built_in_axis_reads_as_text() {
        let mut material = GemMaterial::new_custom("Probe", 1.76, 0.02, -0.008, [0.0; 3]);
        material.c_axis = glam::Vec3::X;
        assert_eq!(built_in_axis_text(&material), "Along the Length (+X)");
        material.c_axis = glam::Vec3::Y;
        assert_eq!(built_in_axis_text(&material), "Through the Table (+Y)");
        let isotropic = GemMaterial::new_custom("Iso", 1.5, 0.01, 0.0, [0.0; 3]);
        assert_eq!(built_in_axis_text(&isotropic), "");
    }
}
