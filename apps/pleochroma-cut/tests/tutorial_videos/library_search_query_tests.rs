//! Unit tests of [`crate::library_search_query`]; included only by
//! `tests/tutorial_videos_targets.rs` (see the note in `pure_tables_tests.rs`).

use crate::library_search_query::shortening_query;

fn rows(titles: &[&str]) -> Vec<(String, String)> {
    titles
        .iter()
        .map(|title| ((*title).to_owned(), "Unknown Designer".to_owned()))
        .collect()
}

#[test]
fn a_prefix_shared_by_every_design_is_not_enough() {
    // The fixture library: all four designs start with "PC ", so "PC " lists all of them and the
    // route's wait for a shorter list could never end.
    let rows = rows(&[
        "PC 45.149 Round Trichecker-12",
        "PC 43.001A Shah (Replica)",
        "PC 42.060 Large Texas Star",
        "PC 46.019 For Fun",
    ]);
    let query = shortening_query("PC 45.149 Round Trichecker-12", &rows).expect("a query exists");
    assert_eq!(query, "PC 45");
}

#[test]
fn the_shortest_start_that_shortens_wins() {
    let rows = rows(&["Aurora", "Auburn", "Borealis"]);
    assert_eq!(shortening_query("Aurora", &rows).as_deref(), Some("Aur"));
}

#[test]
fn a_start_matching_the_designer_counts() {
    let rows = vec![
        ("Alpha".to_owned(), "Zed".to_owned()),
        ("Beta".to_owned(), "Alpha Works".to_owned()),
        ("Gamma".to_owned(), "Zed".to_owned()),
    ];
    // "Alp" lists Alpha and Beta (by designer): two of three, so it shortens.
    assert_eq!(shortening_query("Alpha", &rows).as_deref(), Some("Alp"));
}

#[test]
fn no_start_shortens_a_list_of_identical_titles() {
    let rows = rows(&["Same", "Same"]);
    assert_eq!(shortening_query("Same", &rows), None);
    assert_eq!(shortening_query("Same", &[]), None);
}

#[test]
fn a_start_never_ends_in_a_blank() {
    let rows = rows(&["ab cd", "ab xy", "zz"]);
    // "ab " would end in a blank; "ab c" is the first start that is not shared and has no
    // trailing blank, and "ab" is below three characters.
    assert_eq!(shortening_query("ab cd", &rows).as_deref(), Some("ab c"));
}
