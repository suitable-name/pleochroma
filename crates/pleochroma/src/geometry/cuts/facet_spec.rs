//! [`FacetSpec`]: a UI-toolkit-agnostic row of GemCAD-style cutting
//! instructions.

/// A single row of GemCAD-style cutting instructions: one facet's angle, index
/// position(s), and any notes.
///
/// Stored as scraped: plain strings with no numeric parsing. The lenient parsers
/// `parse_angle_deg` and `parse_girdle_facet_count` are private helpers in `database_angles`.
///
/// UI-toolkit agnostic: callers convert their own row type at the call site, so this
/// crate has no UI dependency.
#[derive(Debug, Clone, Default)]
pub struct FacetSpec {
    /// The facet's name/label as scraped (e.g. `"C1"`, `"T"`, `"culet"`).
    pub facet: String,
    /// The facet's angle, as scraped (unparsed; see `parse_angle_deg`).
    pub angle: String,
    /// The facet's index position(s), as scraped (unparsed).
    pub index: String,
    /// Any free-text notes attached to this row.
    pub notes: String,
}
