//! The material editor's Gem Data section: the values a user enters for a custom material
//! (Mohs range, toughness range, cleavage, fracture, care and source) as the dialog holds them,
//! their check, and their conversion to and from the stored JSON of a [`CustomGemDataTable`].
//!
//! The fields live in `ViewportModel.gem_data_*`. [`GemDataForm::push`] fills them when the
//! dialog's pre-fill is pushed and [`GemDataForm::read`] reads them when Save runs, so the save
//! callback keeps its signature. No database here.

use crate::{MainWindow, ViewportModel};
use pleochroma_cut_core::material::LibraryMaterial;
use pleochroma_formats::native::{CustomGemDataTable, GEM_TOUGHNESS_WORDS};
use slint::ComponentHandle;

/// The most characters the care note holds.
pub(super) const CARE_LIMIT: usize = 200;

/// The gem data of the dialog's Gem Data section, exactly as typed or chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct GemDataForm {
    /// The Mohs "from" field; blank means not recorded.
    pub(super) mohs_from: String,
    /// The Mohs "to" field; blank means the same as "from".
    pub(super) mohs_to: String,
    /// The toughness "from" combo: 0 is "Not recorded", 1 to 6 are the grades weakest first.
    pub(super) toughness_from: i32,
    /// The toughness "to" combo, in the same numbering.
    pub(super) toughness_to: i32,
    /// The cleavage text.
    pub(super) cleavage: String,
    /// The fracture text.
    pub(super) fracture: String,
    /// The care text.
    pub(super) care: String,
    /// The source text.
    pub(super) source: String,
}

/// The Mohs figure typed in `text`: `Ok(None)` for a blank field, an error for anything that is
/// not a number from 1 to 10. Only a decimal point is read, as in the dialog's own field.
fn parse_mohs(text: &str) -> Result<Option<f64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && (1.0..=10.0).contains(value))
        .map(Some)
        .ok_or_else(|| "The Mohs hardness must be a number from 1 to 10.".to_owned())
}

/// The file's toughness word for a combo index (1 to 6), `None` for "Not recorded".
fn toughness_word(index: i32) -> Option<String> {
    let position = usize::try_from(index.checked_sub(1)?).ok()?;
    GEM_TOUGHNESS_WORDS
        .get(position)
        .map(|word| (*word).to_owned())
}

/// `text` trimmed, `None` when nothing is left.
fn trimmed(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

impl GemDataForm {
    /// The dialog's current gem data fields.
    pub(super) fn read(ui: &MainWindow) -> Self {
        let model = ui.global::<ViewportModel>();
        Self {
            mohs_from: model.get_gem_data_mohs_from().to_string(),
            mohs_to: model.get_gem_data_mohs_to().to_string(),
            toughness_from: model.get_gem_data_toughness_from(),
            toughness_to: model.get_gem_data_toughness_to(),
            cleavage: model.get_gem_data_cleavage().to_string(),
            fracture: model.get_gem_data_fracture().to_string(),
            care: model.get_gem_data_care().to_string(),
            source: model.get_gem_data_source().to_string(),
        }
    }

    /// Fills the dialog's gem data fields.
    pub(super) fn push(&self, ui: &MainWindow) {
        let model = ui.global::<ViewportModel>();
        model.set_gem_data_mohs_from(self.mohs_from.as_str().into());
        model.set_gem_data_mohs_to(self.mohs_to.as_str().into());
        model.set_gem_data_toughness_from(self.toughness_from);
        model.set_gem_data_toughness_to(self.toughness_to);
        model.set_gem_data_cleavage(self.cleavage.as_str().into());
        model.set_gem_data_fracture(self.fracture.as_str().into());
        model.set_gem_data_care(self.care.as_str().into());
        model.set_gem_data_source(self.source.as_str().into());
    }

    /// The form of a stored table: a Mohs figure off the scale and an unknown toughness word
    /// come back blank, as the file's checked views read them. `None` gives the empty form.
    pub(super) fn from_table(table: Option<&CustomGemDataTable>) -> Self {
        let Some(table) = table else {
            return Self::default();
        };
        let (mohs_from, mohs_to) = table
            .mohs_range()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .unwrap_or_default();
        let (toughness_from, toughness_to) = table
            .toughness_ranks()
            .map(|(from, to)| (i32::from(from) + 1, i32::from(to) + 1))
            .unwrap_or_default();
        Self {
            mohs_from,
            mohs_to,
            toughness_from,
            toughness_to,
            cleavage: table.cleavage.clone().unwrap_or_default(),
            fracture: table.fracture.clone().unwrap_or_default(),
            care: table.care.clone().unwrap_or_default(),
            source: table.source.clone().unwrap_or_default(),
        }
    }

    /// The form of the stored JSON text (the `gem_data_json` column); the empty form for a
    /// missing or unreadable text.
    pub(super) fn from_json(json: Option<&str>) -> Self {
        let table = json.and_then(LibraryMaterial::decode_gem_data);
        Self::from_table(table.as_ref())
    }

    /// The table the form describes: a Mohs field left blank takes the other one's figure (a
    /// single figure is a range of one value), likewise the toughness; blank text is not
    /// recorded. `Ok(None)` when nothing was entered.
    ///
    /// # Errors
    ///
    /// A sentence for a Mohs figure that is not a number from 1 to 10, or a care note over
    /// [`CARE_LIMIT`] characters.
    pub(super) fn to_table(&self) -> Result<Option<CustomGemDataTable>, String> {
        let from = parse_mohs(&self.mohs_from)?;
        let to = parse_mohs(&self.mohs_to)?;
        let mohs = match (from, to) {
            (Some(a), Some(b)) => Some([a, b]),
            (Some(a), None) | (None, Some(a)) => Some([a, a]),
            (None, None) => None,
        };
        let toughness = match (
            toughness_word(self.toughness_from),
            toughness_word(self.toughness_to),
        ) {
            (Some(a), Some(b)) => Some([a, b]),
            (Some(a), None) | (None, Some(a)) => Some([a.clone(), a]),
            (None, None) => None,
        };
        let care = trimmed(&self.care);
        if care
            .as_deref()
            .is_some_and(|text| text.chars().count() > CARE_LIMIT)
        {
            return Err(format!(
                "The care note can be at most {CARE_LIMIT} characters."
            ));
        }
        let table = CustomGemDataTable {
            mohs,
            toughness,
            cleavage: trimmed(&self.cleavage),
            fracture: trimmed(&self.fracture),
            care,
            source: trimmed(&self.source),
        };
        Ok((!table.is_empty()).then_some(table))
    }

    /// The text for `MaterialSaveFields::gem_data_json`: the JSON of [`Self::to_table`], blank
    /// when nothing was entered (which clears the stored data).
    ///
    /// # Errors
    ///
    /// The sentence [`Self::to_table`] gives.
    pub(super) fn to_json(&self) -> Result<String, String> {
        Ok(self
            .to_table()?
            .and_then(|table| LibraryMaterial::encode_gem_data(&table))
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::{CARE_LIMIT, GemDataForm};

    fn entered() -> GemDataForm {
        GemDataForm {
            mohs_from: "7.5".into(),
            mohs_to: "8".into(),
            toughness_from: 4,
            toughness_to: 5,
            cleavage: " Imperfect basal ".into(),
            fracture: "Conchoidal".into(),
            care: "Avoid sudden temperature changes while dopping.".into(),
            source: "My notes, 2026".into(),
        }
    }

    /// Every field reaches the table, the toughness as the file's words and the text trimmed.
    #[test]
    fn a_full_form_becomes_the_table_of_the_file() {
        let table = entered().to_table().expect("valid").expect("not empty");
        assert_eq!(table.mohs, Some([7.5, 8.0]));
        assert_eq!(
            table.toughness,
            Some(["good".to_owned(), "excellent".to_owned()])
        );
        assert_eq!(table.cleavage.as_deref(), Some("Imperfect basal"));
        assert_eq!(table.fracture.as_deref(), Some("Conchoidal"));
        assert_eq!(table.source.as_deref(), Some("My notes, 2026"));
    }

    /// The table read back gives the form it came from (the toughness through its rank).
    #[test]
    fn a_stored_table_fills_the_form_it_was_made_from() {
        let json = entered().to_json().expect("valid");
        let form = GemDataForm::from_json(Some(&json));
        assert_eq!(form.mohs_from, "7.5");
        assert_eq!(form.mohs_to, "8");
        assert_eq!((form.toughness_from, form.toughness_to), (4, 5));
        assert_eq!(form.cleavage, "Imperfect basal");
    }

    /// A blank form stores nothing and clears what was stored (blank JSON text).
    #[test]
    fn a_blank_form_is_blank_text() {
        assert_eq!(GemDataForm::default().to_table(), Ok(None));
        assert_eq!(GemDataForm::default().to_json(), Ok(String::new()));
        assert_eq!(GemDataForm::from_json(None), GemDataForm::default());
        assert_eq!(
            GemDataForm::from_json(Some("not json")),
            GemDataForm::default()
        );
    }

    /// One figure is a range of one value; so is one toughness grade.
    #[test]
    fn a_single_end_fills_both_ends() {
        let form = GemDataForm {
            mohs_to: "6.5".into(),
            toughness_from: 3,
            ..GemDataForm::default()
        };
        let table = form.to_table().expect("valid").expect("not empty");
        assert_eq!(table.mohs, Some([6.5, 6.5]));
        assert_eq!(
            table.toughness,
            Some(["fair".to_owned(), "fair".to_owned()])
        );
    }

    /// A Mohs figure off the scale or not a number, and a care note over the limit, refuse the
    /// save with a sentence.
    #[test]
    fn invalid_entries_are_refused() {
        for bad in ["0.5", "11", "hard", "NaN", "6,5"] {
            let form = GemDataForm {
                mohs_from: bad.into(),
                ..GemDataForm::default()
            };
            assert!(form.to_table().is_err(), "{bad}");
        }
        let long = GemDataForm {
            care: "x".repeat(CARE_LIMIT + 1),
            ..GemDataForm::default()
        };
        assert!(long.to_table().is_err());
        let exact = GemDataForm {
            care: "x".repeat(CARE_LIMIT),
            ..GemDataForm::default()
        };
        assert!(exact.to_table().is_ok());
    }
}
