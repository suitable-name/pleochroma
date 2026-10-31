use super::section_folders::section_folder;

#[test]
fn a_section_folder_is_its_number_and_its_heading_in_dashed_lowercase() {
    assert_eq!(section_folder(0, "Getting Started"), "01-getting-started");
    assert_eq!(
        section_folder(3, "Meets, Relations and Concave Tiers"),
        "04-meets-relations-and-concave-tiers"
    );
    assert_eq!(
        section_folder(6, "History, Variants and Comparing"),
        "07-history-variants-and-comparing"
    );
    assert_eq!(
        section_folder(14, " Built from the Library! "),
        "15-built-from-the-library"
    );
}

#[test]
fn every_browser_section_has_its_own_folder() {
    use pleochroma_editor::guide::GuideCategory;
    let folders: Vec<String> = GuideCategory::ALL
        .iter()
        .enumerate()
        .map(|(index, category)| section_folder(index, category.label()))
        .collect();
    let mut sorted = folders.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(
        sorted, folders,
        "unique and sorted by name in the browser order"
    );
}
