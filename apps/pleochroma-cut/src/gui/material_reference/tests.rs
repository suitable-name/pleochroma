//! Tests of the Material Reference window's state: opening and selecting (T-G1), the remembered
//! view (T-G2) and the Copy Table text (T-G3). They run on [`Session`] and [`ViewPrefs`], which
//! hold every rule of the window without a Slint window.

use super::{
    prefs::{COLUMNS, FILTERS, ViewPrefs, column_from_index, column_index, filter_from_index},
    session::{Page, Selection, Session},
};
use crate::settings::model::AppSettings;
use pleochroma_cut_core::MaterialCatalogue;
use pleochroma_editor::{
    material_lookup::EditorMaterialLookup,
    material_reference::{Column, DispersionScale, Filter, Origin, ReferenceRow, reference_rows},
};

/// The rows of the built-in catalogue, as the window builds them with no custom material.
fn built_in_rows() -> Vec<ReferenceRow> {
    let catalogue = MaterialCatalogue::build(&[]);
    let lookup = EditorMaterialLookup::new(&[]);
    reference_rows(&catalogue, &lookup, &[])
}

/// A session on the saved view `prefs`.
fn session_with(prefs: ViewPrefs) -> Session {
    Session::new(built_in_rows(), prefs)
}

/// The names of the visible rows.
fn visible_names(session: &Session) -> Vec<String> {
    session
        .visible()
        .iter()
        .map(|row| row.name.clone())
        .collect()
}

// --- T-G1: opening selects the material ----------------------------------------------------

/// Opening for a material selects it among the visible rows, whatever the case of the name.
#[test]
fn opening_for_a_material_selects_it() {
    let mut session = session_with(ViewPrefs::default());
    assert_eq!(session.selected(), None);
    let outcome = session.select("diamond");
    assert_eq!(
        outcome,
        Selection::Selected {
            view_changed: false
        }
    );
    assert_eq!(session.selected(), Some("Diamond"));
    let index = session.selected_index().expect("the selection is visible");
    assert_eq!(session.visible()[index].name, "Diamond");
}

/// Opening again for another material (the window is already open) moves the selection, returns
/// to the table and forgets the variant of the first material.
#[test]
fn opening_again_selects_the_new_material_on_the_table() {
    let mut session = session_with(ViewPrefs::default());
    session.select("Diamond");
    let first = session.open_row(session.selected_index().expect("visible"));
    assert_eq!(first.as_deref(), Some("Diamond"));
    assert_eq!(session.page(), Page::Details);
    session.set_variant_key("some-variant");
    session.back();
    assert_eq!(session.page(), Page::Table);
    session.select("Topaz");
    assert_eq!(session.selected(), Some("Topaz"));
    assert_eq!(session.variant_key(), "");
}

/// Selecting the material that is already selected keeps its variant.
#[test]
fn reselecting_the_same_material_keeps_its_variant() {
    let mut session = session_with(ViewPrefs::default());
    session.select("Topaz");
    session.set_variant_key("a-variant");
    session.select("topaz");
    assert_eq!(session.variant_key(), "a-variant");
}

/// A material the filter or the search hides is brought into view by clearing both, and the
/// caller is told so it can save the view.
#[test]
fn opening_a_hidden_material_clears_the_filter_and_the_search() {
    let mut session = session_with(ViewPrefs {
        filter: Filter::Glass,
        query: "no such text".to_owned(),
        ..ViewPrefs::default()
    });
    assert!(session.visible().is_empty());
    let outcome = session.select("Diamond");
    assert_eq!(outcome, Selection::Selected { view_changed: true });
    assert_eq!(session.prefs().filter, Filter::All);
    assert!(session.prefs().query.is_empty());
    assert!(session.selected_index().is_some());
}

/// A name the catalogue does not have changes nothing.
#[test]
fn an_unknown_material_leaves_the_selection_alone() {
    let mut session = session_with(ViewPrefs::default());
    session.select("Diamond");
    assert_eq!(session.select("Not A Material"), Selection::Unknown);
    assert_eq!(session.selected(), Some("Diamond"));
    assert!(session.open_row(10_000).is_none());
}

/// A header click sorts ascending, a second click on the same column reverses it, and a click on
/// another column starts ascending again.
#[test]
fn a_header_click_sorts_and_a_second_click_reverses() {
    let mut session = session_with(ViewPrefs::default());
    session.click_sort(Column::Hardness);
    assert_eq!(session.prefs().column, Column::Hardness);
    assert!(!session.prefs().descending);
    let softest = visible_names(&session);
    session.click_sort(Column::Hardness);
    assert!(session.prefs().descending);
    assert!(
        session.footer().contains("hardest first"),
        "{}",
        session.footer()
    );
    let hardest = visible_names(&session);
    assert_eq!(hardest.first().map(String::as_str), Some("Diamond"));
    assert_ne!(softest, hardest);
    session.click_sort(Column::Ri);
    assert_eq!(session.prefs().column, Column::Ri);
    assert!(!session.prefs().descending);
}

/// Replacing the rows (a custom material was saved) keeps the view and the selection, and drops a
/// selection that no longer exists.
#[test]
fn new_rows_keep_the_view_and_the_selection() {
    let mut session = session_with(ViewPrefs::default());
    session.click_sort(Column::Ri);
    session.select("Ruby");
    session.set_rows(built_in_rows());
    assert_eq!(session.prefs().column, Column::Ri);
    assert_eq!(session.selected(), Some("Ruby"));
    session.set_rows(
        built_in_rows()
            .into_iter()
            .filter(|row| row.name != "Ruby")
            .collect(),
    );
    assert_eq!(session.selected(), None);
}

/// The subtitle and the footer count what the table holds.
#[test]
fn the_subtitle_and_the_footer_count_the_rows() {
    let session = session_with(ViewPrefs::default());
    let built_in = session
        .visible()
        .iter()
        .filter(|row| row.origin != Origin::Custom)
        .count();
    assert!(
        session
            .subtitle()
            .starts_with(&format!("{built_in} built-in materials.")),
        "{}",
        session.subtitle()
    );
    assert!(
        session
            .footer()
            .starts_with(&format!("{} materials, by name.", session.visible().len())),
        "{}",
        session.footer()
    );
}

// --- T-G2: the view is remembered -----------------------------------------------------------

/// The model indices of the columns and the chips round-trip, and an index outside the table
/// reads as nothing.
#[test]
fn model_indices_round_trip() {
    for (index, column) in COLUMNS.iter().enumerate() {
        let index = i32::try_from(index).expect("a small index");
        assert_eq!(column_index(*column), index);
        assert_eq!(column_from_index(index), Some(*column));
    }
    assert_eq!(column_from_index(-1), None);
    assert_eq!(column_from_index(10), None);
    assert_eq!(
        filter_from_index(i32::try_from(FILTERS.len()).expect("small")),
        None
    );
}

/// Sort, filter, search and the dispersion scale survive closing and reopening: the saved
/// settings give the same view, and a session built on it shows the same rows.
#[test]
fn the_view_survives_closing_and_reopening() {
    let mut session = session_with(ViewPrefs::default());
    session.click_sort(Column::Hardness);
    session.click_sort(Column::Hardness);
    session.set_filter(Filter::Natural);
    session.set_query("quartz");
    session.set_scale(DispersionScale::FC);
    let shown = visible_names(&session);

    let mut settings = AppSettings::default();
    session.prefs().write_to(&mut settings);
    let text = toml::to_string(&settings).expect("the settings serialize");
    let restored: AppSettings = toml::from_str(&text).expect("the settings load");
    assert_eq!(restored.material_reference_query, "quartz");

    let prefs = ViewPrefs::from_settings(&restored);
    assert_eq!(&prefs, session.prefs());
    let reopened = session_with(prefs);
    assert_eq!(visible_names(&reopened), shown);
    assert_eq!(reopened.prefs().scale, DispersionScale::FC);
}

/// A new install shows the table by name, every material, no search and B-G dispersion; a value
/// this build does not know reads as the default.
#[test]
fn a_settings_file_without_the_view_shows_the_default_table() {
    assert_eq!(
        ViewPrefs::from_settings(&AppSettings::default()),
        ViewPrefs::default()
    );
    let defaults = ViewPrefs::default();
    assert_eq!(defaults.column, Column::Name);
    assert_eq!(defaults.scale, DispersionScale::BG);
    let settings = AppSettings {
        material_reference_sort_column: 200,
        material_reference_filter: 200,
        ..AppSettings::default()
    };
    let prefs = ViewPrefs::from_settings(&settings);
    assert_eq!(prefs.column, Column::Name);
    assert_eq!(prefs.filter, Filter::All);
}

/// The search text that is saved is bounded.
#[test]
fn a_very_long_search_is_cut_when_saved() {
    let prefs = ViewPrefs {
        query: "x".repeat(500),
        ..ViewPrefs::default()
    };
    let mut settings = AppSettings::default();
    prefs.write_to(&mut settings);
    assert_eq!(settings.material_reference_query.chars().count(), 80);
}

// --- T-G3: Copy Table -----------------------------------------------------------------------

/// Copy Table copies the visible rows, tab-separated, under a header that names the dispersion
/// convention.
#[test]
fn copy_table_copies_the_visible_rows() {
    let mut session = session_with(ViewPrefs::default());
    session.set_filter(Filter::Glass);
    let (text, count) = session.copy_text();
    assert_eq!(count, session.visible().len());
    assert!(count > 0, "the catalogue has glasses");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), count + 1, "a header and one line per row");
    assert!(lines[0].starts_with("Material\t"), "{}", lines[0]);
    assert!(lines[0].contains("Disp. (B\u{2013}G)"), "{}", lines[0]);
    let columns = lines[0].split('\t').count();
    for line in &lines[1..] {
        assert_eq!(line.split('\t').count(), columns, "{line}");
    }
    for name in visible_names(&session) {
        assert!(text.contains(&name), "{name}");
    }
}

/// The dispersion scale of the window is the one the copied header and figures use.
#[test]
fn copy_table_follows_the_dispersion_scale() {
    let mut session = session_with(ViewPrefs::default());
    session.set_filter(Filter::Synthetic);
    let (blue_green, _) = session.copy_text();
    session.set_scale(DispersionScale::FC);
    let (fc, _) = session.copy_text();
    assert!(
        fc.lines()
            .next()
            .is_some_and(|line| line.contains("F\u{2013}C"))
    );
    assert_ne!(blue_green, fc);
}

/// A search narrows what is copied.
#[test]
fn copy_table_copies_only_what_the_search_leaves() {
    let mut session = session_with(ViewPrefs::default());
    session.set_query("topaz");
    let (text, count) = session.copy_text();
    assert!(count >= 1);
    assert_eq!(text.lines().count(), count + 1);
    assert!(text.contains("Topaz"));
    assert!(!text.contains("Diamond"));
}
