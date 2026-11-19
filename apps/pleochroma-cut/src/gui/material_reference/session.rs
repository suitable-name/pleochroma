//! What the Material Reference window shows, as plain data: the rows, the view
//! ([`ViewPrefs`]), the selected material and the page. Nothing here touches Slint, so every
//! rule of the window (a header click reverses a sort, opening a hidden material clears the
//! filter, Copy Table copies what is visible) is a unit test.

use super::prefs::ViewPrefs;
use pleochroma_editor::material_reference::{
    Column, DispersionScale, Filter, Origin, ReferenceRow, filter_rows, sort_rows, table_text, text,
};

/// Which page of the window is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    /// The table of every material.
    Table,
    /// The details of the selected material.
    Details,
}

impl Page {
    /// The value of `MaterialReferenceModel.page`.
    #[must_use]
    pub(super) const fn index(self) -> i32 {
        match self {
            Self::Table => 0,
            Self::Details => 1,
        }
    }
}

/// The window's state between two pushes to Slint.
#[derive(Debug, Clone)]
pub(super) struct Session {
    /// Every material, sorted by the view's column.
    rows: Vec<ReferenceRow>,
    /// The sort, filter, search and dispersion scale.
    prefs: ViewPrefs,
    /// The selected material's name, as the catalogue spells it.
    selected: Option<String>,
    /// The page on screen.
    page: Page,
    /// The key of the color variant shown on the details page; empty is the Material Default.
    variant_key: String,
    /// Whether the spectrum chart shows transmission instead of absorption.
    transmission: bool,
}

impl Session {
    /// A session over `rows` (in any order) with the saved `prefs`, on the table page.
    #[must_use]
    pub(super) fn new(rows: Vec<ReferenceRow>, prefs: ViewPrefs) -> Self {
        let mut session = Self {
            rows,
            prefs,
            selected: None,
            page: Page::Table,
            variant_key: String::new(),
            transmission: false,
        };
        session.resort();
        session
    }

    /// Re-sorts every row by the view's column and direction.
    fn resort(&mut self) {
        sort_rows(&mut self.rows, self.prefs.column, self.prefs.descending);
    }

    /// Replaces the rows (the custom materials changed) and keeps the view and the selection.
    pub(super) fn set_rows(&mut self, rows: Vec<ReferenceRow>) {
        self.rows = rows;
        self.resort();
        let gone = self
            .selected
            .as_deref()
            .is_some_and(|name| self.find(name).is_none());
        if gone {
            self.selected = None;
            self.page = Page::Table;
        }
    }

    /// The view as it stands.
    #[must_use]
    pub(super) const fn prefs(&self) -> &ViewPrefs {
        &self.prefs
    }

    /// The page on screen.
    #[must_use]
    pub(super) const fn page(&self) -> Page {
        self.page
    }

    /// Shows the table page (the "All Materials" button).
    pub(super) const fn back(&mut self) {
        self.page = Page::Table;
    }

    /// The selected material's name.
    #[must_use]
    pub(super) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// The selected material's index: `n_D` of the base curve.
    #[must_use]
    pub(super) fn selected_ri(&self) -> Option<f64> {
        self.find(self.selected.as_deref()?)?.sort.ri
    }

    /// The key of the variant shown on the details page.
    #[must_use]
    pub(super) fn variant_key(&self) -> &str {
        &self.variant_key
    }

    /// Shows the color variant with `key` (empty: the Material Default).
    pub(super) fn set_variant_key(&mut self, key: &str) {
        key.clone_into(&mut self.variant_key);
    }

    /// Whether the spectrum chart shows transmission.
    #[must_use]
    pub(super) const fn transmission(&self) -> bool {
        self.transmission
    }

    /// Switches the spectrum chart between absorption (`false`) and transmission (`true`).
    pub(super) const fn set_transmission(&mut self, transmission: bool) {
        self.transmission = transmission;
    }

    /// The row called `name` (case-insensitive).
    fn find(&self, name: &str) -> Option<&ReferenceRow> {
        self.rows
            .iter()
            .find(|row| row.name.eq_ignore_ascii_case(name))
    }

    /// The rows the filter chip and the search text let through, in sort order.
    #[must_use]
    pub(super) fn visible(&self) -> Vec<&ReferenceRow> {
        filter_rows(&self.rows, self.prefs.filter, &self.prefs.query)
    }

    /// The rows the search text lets through, whatever the filter chip: what each chip would show.
    #[must_use]
    pub(super) fn search_matches(&self) -> Vec<&ReferenceRow> {
        filter_rows(&self.rows, Filter::All, &self.prefs.query)
    }

    /// The position of the selected material among the visible rows.
    #[must_use]
    pub(super) fn selected_index(&self) -> Option<usize> {
        let name = self.selected.as_deref()?;
        self.visible()
            .iter()
            .position(|row| row.name.eq_ignore_ascii_case(name))
    }

    /// A header click: sorts by `column` ascending, or reverses the sort when the table is
    /// already sorted by it.
    pub(super) fn click_sort(&mut self, column: Column) {
        if self.prefs.column == column {
            self.prefs.descending = !self.prefs.descending;
        } else {
            self.prefs.column = column;
            self.prefs.descending = false;
        }
        self.resort();
    }

    /// Chooses the filter chip.
    pub(super) const fn set_filter(&mut self, filter: Filter) {
        self.prefs.filter = filter;
    }

    /// Sets the search text.
    pub(super) fn set_query(&mut self, query: &str) {
        query.clone_into(&mut self.prefs.query);
    }

    /// Chooses the dispersion convention.
    pub(super) const fn set_scale(&mut self, scale: DispersionScale) {
        self.prefs.scale = scale;
    }

    /// Selects the material called `name`. A material the filter or the search hides is brought
    /// into view by clearing both. Returns whether the catalogue has such a material, and
    /// whether the view changed to show it.
    pub(super) fn select(&mut self, name: &str) -> Selection {
        let Some(canonical) = self.find(name).map(|row| row.name.clone()) else {
            return Selection::Unknown;
        };
        let changed_material = self
            .selected
            .as_deref()
            .is_none_or(|current| !current.eq_ignore_ascii_case(&canonical));
        if changed_material {
            self.variant_key.clear();
        }
        self.selected = Some(canonical);
        let hidden = self.selected_index().is_none();
        if hidden {
            self.prefs.filter = Filter::All;
            self.prefs.query.clear();
        }
        Selection::Selected {
            view_changed: hidden,
        }
    }

    /// Opens the details of the visible row `index`: selects it and switches to the details
    /// page. Returns the material's name, `None` for an index outside the table.
    pub(super) fn open_row(&mut self, index: usize) -> Option<String> {
        let name = self.visible().get(index)?.name.clone();
        self.select(&name);
        self.page = Page::Details;
        Some(name)
    }

    /// The tab-separated text of the visible rows with its header, and how many rows it holds.
    #[must_use]
    pub(super) fn copy_text(&self) -> (String, usize) {
        let visible = self.visible();
        (table_text(&visible, self.prefs.scale), visible.len())
    }

    /// The subtitle: how many materials the table lists.
    #[must_use]
    pub(super) fn subtitle(&self) -> String {
        let own = self
            .rows
            .iter()
            .filter(|row| row.origin == Origin::Custom)
            .count();
        text::subtitle(self.rows.len() - own, own)
    }

    /// The footer's status line.
    #[must_use]
    pub(super) fn footer(&self) -> String {
        text::footer_text(
            self.visible().len(),
            self.prefs.column,
            self.prefs.descending,
        )
    }
}

/// What [`Session::select`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Selection {
    /// The catalogue has no material of that name.
    Unknown,
    /// The material is selected; `view_changed` when the filter and the search were cleared to
    /// show it.
    Selected {
        /// Whether the filter chip and the search text were reset.
        view_changed: bool,
    },
}
