//! Pins the width at which the Material Reference table folds its less important columns: the desktop
//! page (`ui/components/material_reference/ref_table.slint`) and the web editor's page
//! (`apps/pleochroma-web/ui/editor/material_reference.slint`) fold at the same 760 px, so a material
//! looks the same in both. It only reads the two `.slint` sources as text.

use std::{fs, path::Path};

/// The width, in logical pixels, both pages fold at.
const FOLD_WIDTH: u32 = 760;

/// The text of `relative` under the `apps/` folder.
fn read_app_file(relative: &str) -> String {
    let apps = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits in apps/");
    let path = apps.join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// The pixel width in the first `property <bool> {name}: root.width {operator} <n>px` of `source`.
fn fold_width(source: &str, name: &str, operator: &str) -> Option<u32> {
    let head = format!("property <bool> {name}: root.width {operator} ");
    let start = source.find(&head)? + head.len();
    let digits: String = source[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if !source[start + digits.len()..].starts_with("px") {
        return None;
    }
    digits.parse().ok()
}

#[test]
fn the_desktop_table_folds_at_760_px() {
    let source = read_app_file("pleochroma-cut/ui/components/material_reference/ref_table.slint");
    assert_eq!(
        fold_width(&source, "mid", ">="),
        Some(FOLD_WIDTH),
        "ref_table.slint: `mid` must be `root.width >= 760px`"
    );
}

#[test]
fn the_web_table_folds_at_760_px() {
    let source = read_app_file("pleochroma-web/ui/editor/material_reference.slint");
    assert_eq!(
        fold_width(&source, "narrow", "<"),
        Some(FOLD_WIDTH),
        "web material_reference.slint: `narrow` must be `root.width < 760px`"
    );
}
