//! The material editor's per-axis curves section: the extraordinary-ray curve of a uniaxial
//! material, or the alpha and gamma curves of a biaxial one, typed as Sellmeier or Cauchy
//! coefficients. The state lives in `AxisCurvesModel`; [`push`] fills it when the dialog's
//! pre-fill is pushed and [`read_form`] reads it when Save runs. No database here.
//!
//! The coefficient fields, their readout and the validation are the Dispersion section's own
//! (`gui::optics::dispersion_editor`), so a curve typed here is accepted exactly when the same
//! text would be accepted there.

use crate::{
    AxisCurvesModel, MainWindow,
    gui::optics::dispersion_editor::{FIELD_COUNT, ModelKind, Readout, prefill_for},
};
use pleochroma::optics::{dispersion::DispersionModel, materials::OpticalCharacter};
use pleochroma_cut_core::{
    material::{PerAxisDispersion, per_axis_dispersion_from_json},
    native::dispersion_model_from_json,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// How many curves the section holds: the extraordinary ray (uniaxial) or alpha, then gamma
/// (biaxial).
const CURVES: usize = 2;

/// What the section holds right now, in the shape Save needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CurvesForm {
    /// The Curves mode is on (the Simple mode stores no curve).
    pub(super) on: bool,
    /// The model-combo index of each curve.
    pub(super) kinds: [i32; CURVES],
    /// The coefficient field texts of each curve, [`FIELD_COUNT`] each.
    pub(super) texts: [Vec<String>; CURVES],
}

impl Default for CurvesForm {
    fn default() -> Self {
        Self {
            on: false,
            kinds: [ModelKind::Cauchy.index(); CURVES],
            texts: [
                vec![String::new(); FIELD_COUNT],
                vec![String::new(); FIELD_COUNT],
            ],
        }
    }
}

/// The model of one curve from its combo index and field texts, or `None` when the fields do not
/// make a usable model (the Dispersion section's own checks).
fn curve_model(kind: i32, texts: &[String]) -> Option<DispersionModel> {
    let kind = ModelKind::from_index(kind)?;
    let readout = Readout::new(kind, texts);
    if !readout.valid {
        return None;
    }
    dispersion_model_from_json(&readout.model_json)
}

impl CurvesForm {
    /// The section as a stored row opens it: the Curves mode on the stored curves, or the Simple
    /// mode for a row with none (or text that reads as none).
    pub(super) fn from_stored(text: Option<&str>) -> Self {
        let Some(curves) = text.and_then(per_axis_dispersion_from_json) else {
            return Self::default();
        };
        let models = match curves {
            PerAxisDispersion::Uniaxial(e) => vec![e],
            PerAxisDispersion::Biaxial([alpha, gamma]) => vec![alpha, gamma],
        };
        let mut form = Self {
            on: true,
            ..Self::default()
        };
        for (slot, model) in models.iter().enumerate() {
            let prefill = prefill_for(Some(model));
            form.kinds[slot] = prefill.kind;
            form.texts[slot] = prefill.texts;
        }
        form
    }

    /// The form from the raw values of the Slint global (`kinds`: one per curve; `texts`:
    /// [`FIELD_COUNT`] per curve). Missing entries read as the default.
    pub(super) fn from_raw(mode: i32, kinds: &[i32], texts: &[String]) -> Self {
        let mut form = Self {
            on: mode == 1,
            ..Self::default()
        };
        for slot in 0..CURVES {
            if let Some(kind) = kinds.get(slot) {
                form.kinds[slot] = *kind;
            }
            form.texts[slot] = (0..FIELD_COUNT)
                .map(|i| {
                    texts
                        .get(slot * FIELD_COUNT + i)
                        .cloned()
                        .unwrap_or_default()
                })
                .collect();
        }
        form
    }

    /// The curves a material of `character` stores, or `None` for the Simple mode and for an
    /// isotropic material (it has one ray).
    ///
    /// # Errors
    ///
    /// The sentence for the toast when a curve the character needs is not a usable model.
    pub(super) fn resolve(
        &self,
        character: OpticalCharacter,
    ) -> Result<Option<PerAxisDispersion>, String> {
        let curve = |slot: usize, name: &str| {
            curve_model(self.kinds[slot], &self.texts[slot]).ok_or_else(|| {
                format!(
                    "The {name} curve cannot be used. Check the Per-Axis Curves section and try again."
                )
            })
        };
        if !self.on {
            return Ok(None);
        }
        match character {
            OpticalCharacter::Isotropic => Ok(None),
            OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => {
                curve(0, "extraordinary-ray").map(|e| Some(PerAxisDispersion::Uniaxial(e)))
            }
            OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative => {
                let alpha = curve(0, "alpha-ray")?;
                let gamma = curve(1, "gamma-ray")?;
                Ok(Some(PerAxisDispersion::Biaxial([alpha, gamma])))
            }
        }
    }
}

/// Fills `AxisCurvesModel` for the material the dialog is about to edit.
pub(super) fn push(ui: &MainWindow, form: &CurvesForm) {
    let model = ui.global::<AxisCurvesModel>();
    model.set_mode(i32::from(form.on));
    model.set_kinds(ModelRc::new(VecModel::from(form.kinds.to_vec())));
    let texts: Vec<SharedString> = form
        .texts
        .iter()
        .flat_map(|curve| {
            curve
                .iter()
                .cloned()
                .chain(std::iter::repeat(String::new()))
                .take(FIELD_COUNT)
        })
        .map(SharedString::from)
        .collect();
    model.set_texts(ModelRc::new(VecModel::from(texts)));
}

/// The section as it stands right now (what the dialog's Save stores).
pub(super) fn read_form(ui: &MainWindow) -> CurvesForm {
    let model = ui.global::<AxisCurvesModel>();
    let kinds: Vec<i32> = model.get_kinds().iter().collect();
    let texts: Vec<String> = model.get_texts().iter().map(String::from).collect();
    CurvesForm::from_raw(model.get_mode(), &kinds, &texts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::per_axis_dispersion_to_json;

    fn cauchy(a: &str, b: &str) -> Vec<String> {
        let mut texts = vec![a.to_owned(), b.to_owned(), "0".to_owned()];
        texts.resize(FIELD_COUNT, String::new());
        texts
    }

    fn typed(on: bool) -> CurvesForm {
        CurvesForm {
            on,
            kinds: [ModelKind::Cauchy.index(); CURVES],
            texts: [cauchy("1.76", "0.004"), cauchy("1.78", "0.005")],
        }
    }

    #[test]
    fn the_simple_mode_and_an_isotropic_material_store_no_curve() {
        assert_eq!(
            typed(false).resolve(OpticalCharacter::UniaxialNegative),
            Ok(None)
        );
        assert_eq!(typed(true).resolve(OpticalCharacter::Isotropic), Ok(None));
    }

    #[test]
    fn a_uniaxial_material_takes_one_curve_and_a_biaxial_one_takes_two() {
        let uniaxial = typed(true)
            .resolve(OpticalCharacter::UniaxialPositive)
            .expect("usable");
        assert!(matches!(uniaxial, Some(PerAxisDispersion::Uniaxial(_))));
        let biaxial = typed(true)
            .resolve(OpticalCharacter::BiaxialNegative)
            .expect("usable");
        let Some(PerAxisDispersion::Biaxial([alpha, gamma])) = biaxial else {
            panic!("a biaxial material takes the alpha and gamma curves");
        };
        assert!(
            gamma.n_d() > alpha.n_d(),
            "the second curve is the gamma ray"
        );
    }

    #[test]
    fn an_unusable_curve_is_refused_with_its_name() {
        let mut form = typed(true);
        form.texts[1] = vec![String::new(); FIELD_COUNT];
        assert!(
            form.resolve(OpticalCharacter::UniaxialPositive).is_ok(),
            "a uniaxial material never reads the second curve"
        );
        let refused = form
            .resolve(OpticalCharacter::BiaxialPositive)
            .expect_err("the gamma curve is empty");
        assert!(refused.contains("gamma-ray"));
    }

    #[test]
    fn a_stored_curve_opens_in_the_curves_mode_and_saves_back_the_same_models() {
        let form = typed(true);
        let curves = form
            .resolve(OpticalCharacter::BiaxialPositive)
            .expect("usable")
            .expect("on");
        let text = per_axis_dispersion_to_json(&curves).expect("serialises");
        let reopened = CurvesForm::from_stored(Some(&text));
        assert!(reopened.on);
        assert_eq!(
            reopened.resolve(OpticalCharacter::BiaxialPositive),
            Ok(Some(curves)),
            "the reopened fields make the same curves"
        );
        assert_eq!(CurvesForm::from_stored(None), CurvesForm::default());
        assert_eq!(
            CurvesForm::from_stored(Some("nonsense")),
            CurvesForm::default()
        );
    }

    #[test]
    fn raw_values_fill_missing_entries_with_blanks() {
        let form = CurvesForm::from_raw(1, &[ModelKind::Sellmeier1.index()], &["1.2".to_owned()]);
        assert!(form.on);
        assert_eq!(form.kinds[0], ModelKind::Sellmeier1.index());
        assert_eq!(form.kinds[1], ModelKind::Cauchy.index());
        assert_eq!(form.texts[0][0], "1.2");
        assert!(form.texts[1].iter().all(String::is_empty));
        assert!(!CurvesForm::from_raw(0, &[], &[]).on);
    }
}
