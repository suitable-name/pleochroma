//! The folder of each tutorial section under the video output folder: the section's place in the
//! browser order and its heading in lowercase words joined by dashes
//! (`03-arranging-and-repeating-tiers`), so the folders sorted by name follow the browser.

/// The folder name of the section at `index` (0-based, in the browser order) with heading
/// `label`: two digits from 01, a dash, and the heading's letters and digits in lowercase with
/// every other run of characters as one dash.
#[must_use]
pub fn section_folder(index: usize, label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    format!("{:02}-{slug}", index + 1)
}
