use serde::{Deserialize, Serialize};

/// One tier's hand-recorded row from a design's angle-settings table.
///
/// Facet name, angle, index-wheel positions, and any notes -- the queryable, per-tier
/// child of [`crate::model::detail::FacetingDiagramDetail::angle_settings_table`], stored
/// in the `angle_settings` table (see `crate::db::sqlite::Database::save_diagram_detail`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AngleSetting {
    /// This row's `0`-based position in its design's schedule (`angle_settings.order_idx`),
    /// so the order survives the SQL round trip.
    pub order_index: u32,
    /// The facet/tier name as recorded on the design sheet (e.g. `"P1"`, `"C3"`).
    pub facet: String,
    /// The cutting angle text as recorded (e.g. `"41.000000"`), kept as a display
    /// string rather than a parsed number -- see
    /// `crate::local::reconstruct_asc_schedule` for the one path that parses it.
    pub angle: String,
    /// The index-wheel position(s) text as recorded (e.g. `"0, 24, 48, 72"` or the
    /// real catalogue's hyphen-separated `"96-08-16-..."` form).
    pub index: String,
    /// Free-text notes for this tier. The library search's free-text query matches them
    /// (see `crate::db::sqlite::search`).
    pub notes: String,
    /// The concave tool's name for this tier (e.g. `"Ball 6mm"`), or `None` for a flat
    /// tier. Stored apart from [`Self::notes`] so the library search's notes match
    /// never sees tool text.
    pub tool: Option<String>,
    /// The formatted second line of this tier's cutting-sheet row (the tool's
    /// placement, ready to display), or `None` for a flat tier. Kept as the display
    /// string, like [`Self::angle`], so the library renders it without resolving tools.
    ///
    /// Its `X`, `Y`, `Z` and `D/W` are ratios of the design's finished flat-stone width,
    /// the frame `concave_frame::CONCAVE_FRAME_VERSION` names (`pleochroma-cut-core`). A row
    /// carries no frame of its own; a frame change would need a `concave_frame` column on
    /// `diagram_details`, with rows lacking it read as this frame.
    pub tool_line: Option<String>,
}
