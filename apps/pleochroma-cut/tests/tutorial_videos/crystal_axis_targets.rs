//! The outline targets of the Crystal Axis lessons, in the shape of `targets::TABLE`.
//! `targets::resolve` reads [`all`] after the main table: this table and the render jobs one.

use crate::targets::{Entry, Query};

/// One row for each Crystal Axis `highlight_target`. The `source` column says where the app
/// draws the outline.
pub const TABLE: &[Entry] = &[Entry {
    name: "crystal_axis_toggle",
    // The Stone tab's fold card is called "Crystal Axis" too: the label is looked up inside the
    // Solid viewport toolbar.
    queries: &[Query::InComponent("SolidViewportToolbar", "Crystal Axis")],
    source: "components/solid_viewport/toolbar.slint (Crystal Axis toggle)",
}];

/// The entries `targets::resolve` reads after its own table.
pub fn all() -> impl Iterator<Item = &'static Entry> {
    crate::jobs_targets::TABLE.iter().chain(TABLE)
}
