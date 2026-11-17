//! The remembered view of the Material Reference table: the sort column and direction, the
//! filter chip, the search text and the dispersion scale.
//!
//! The values live in `AppSettings` as plain numbers and text (`material_reference_*`), so a
//! settings file written by a build that knows more or fewer columns still loads: an index outside
//! the table reads as the default. [`ViewPrefs`] is the typed form the window works with.

use crate::settings::{SettingsPersister, model::AppSettings};
use pleochroma_editor::material_reference::{Column, DispersionScale, Filter};

/// Every sortable column, in the order of `MaterialReferenceModel.sort_column`.
pub(super) const COLUMNS: [Column; 10] = [
    Column::Name,
    Column::CrystalSystem,
    Column::OpticCharacter,
    Column::Ri,
    Column::Birefringence,
    Column::Dispersion,
    Column::SpecificGravity,
    Column::Hardness,
    Column::Toughness,
    Column::Cleavage,
];

/// Every filter chip, in the order of `MaterialReferenceModel.filter` and `filter_counts`.
pub(super) const FILTERS: [Filter; 5] = [
    Filter::All,
    Filter::Natural,
    Filter::Synthetic,
    Filter::Glass,
    Filter::Custom,
];

/// The longest search text that is saved, in characters.
const MAX_QUERY_CHARS: usize = 80;

/// The model index of `column`.
#[must_use]
pub(super) fn column_index(column: Column) -> i32 {
    COLUMNS
        .iter()
        .position(|candidate| *candidate == column)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

/// The column at model index `index`, `None` outside the table.
#[must_use]
pub(super) fn column_from_index(index: i32) -> Option<Column> {
    usize::try_from(index)
        .ok()
        .and_then(|i| COLUMNS.get(i).copied())
}

/// The model index of `filter`.
#[must_use]
pub(super) fn filter_index(filter: Filter) -> i32 {
    FILTERS
        .iter()
        .position(|candidate| *candidate == filter)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

/// The filter at model index `index`, `None` outside the chips.
#[must_use]
pub(super) fn filter_from_index(index: i32) -> Option<Filter> {
    usize::try_from(index)
        .ok()
        .and_then(|i| FILTERS.get(i).copied())
}

/// How the table is sorted, filtered and read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ViewPrefs {
    /// The sort column.
    pub(super) column: Column,
    /// Whether the sort runs from the largest value down.
    pub(super) descending: bool,
    /// The filter chip.
    pub(super) filter: Filter,
    /// The search text.
    pub(super) query: String,
    /// The dispersion convention of the Disp. column.
    pub(super) scale: DispersionScale,
}

impl Default for ViewPrefs {
    /// Sorted by name, every material, no search, dispersion as B-G.
    fn default() -> Self {
        Self {
            column: Column::Name,
            descending: false,
            filter: Filter::All,
            query: String::new(),
            scale: DispersionScale::BG,
        }
    }
}

impl ViewPrefs {
    /// The view saved in `settings`; a value this build does not know reads as the default.
    #[must_use]
    pub(super) fn from_settings(settings: &AppSettings) -> Self {
        let defaults = Self::default();
        Self {
            column: column_from_index(i32::from(settings.material_reference_sort_column))
                .unwrap_or(defaults.column),
            descending: settings.material_reference_sort_descending,
            filter: filter_from_index(i32::from(settings.material_reference_filter))
                .unwrap_or(defaults.filter),
            query: settings
                .material_reference_query
                .chars()
                .take(MAX_QUERY_CHARS)
                .collect(),
            scale: if settings.material_reference_dispersion_bg {
                DispersionScale::BG
            } else {
                DispersionScale::FC
            },
        }
    }

    /// Writes this view into `settings`.
    pub(super) fn write_to(&self, settings: &mut AppSettings) {
        settings.material_reference_sort_column =
            u8::try_from(column_index(self.column)).unwrap_or(0);
        settings.material_reference_sort_descending = self.descending;
        settings.material_reference_filter = u8::try_from(filter_index(self.filter)).unwrap_or(0);
        settings.material_reference_query = self.query.chars().take(MAX_QUERY_CHARS).collect();
        settings.material_reference_dispersion_bg = self.scale == DispersionScale::BG;
    }

    /// The view the settings store holds now, the default before the store exists.
    #[must_use]
    pub(super) fn load() -> Self {
        SettingsPersister::installed_for_this_thread().map_or_else(Self::default, |store| {
            Self::from_settings(&store.snapshot().settings)
        })
    }

    /// Saves this view through the settings store (debounced like every other preference).
    pub(super) fn save(&self) {
        if let Some(store) = SettingsPersister::installed_for_this_thread() {
            store.update(|file| self.write_to(&mut file.settings));
        }
    }
}
