#[cfg(feature = "sqlite")]
use anyhow::Result;

/// Parses a hand-typed optional numeric field into `T`.
///
/// Fields such as `refractive_index` and `lw_ratio` on [`MetadataUpdate`] use it. `None`
/// and blank text both give `Ok(None)`, so a blank field clears the value. Other text that
/// fails to parse is an error, never coerced or dropped: a REAL or INTEGER column would
/// keep it as TEXT (a European `"1,76"`) and then sort or filter it wrongly.
///
/// # Errors
///
/// Returns an error naming `field_name` and `raw`'s text if `raw` is non-blank and does
/// not parse as `T`.
#[cfg(feature = "sqlite")]
pub(crate) fn parse_optional_numeric<T>(field_name: &str, raw: Option<&str>) -> Result<Option<T>>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(text) => text
            .parse::<T>()
            .map(Some)
            .map_err(|e| anyhow::anyhow!("{field_name}: '{text}' is not a valid number ({e})")),
    }
}

/// Fields a user might legitimately hand-correct on an already-imported design.
///
/// Used by the detail view's metadata editor (see `apps/pleochroma-cut`'s
/// `gui/library/detail` and `ui/components/detail_header.slint`).
///
/// Deliberately NOT title: that lives in `diagram_entries`, not `diagram_details`, with
/// its own setter ([`crate::db::sqlite::Database::rename_diagram_entry`]).
///
/// Every field is `Option<...>` because the underlying column is nullable and a blank
/// field in the editor is a valid edit (clears that value); there is no separate "leave
/// unchanged" state since [`crate::db::sqlite::Database::update_diagram_metadata`] is a
/// single `UPDATE` naming exactly these columns.
///
/// Deliberately excludes every other `diagram_details` column (`page_url`,
/// `diagram_image_name`/`diagram_image_data`, `competition_diagram`, `tw_ratio`,
/// `uw_ratio`, the `designer`/`source_citation` split, `pdf_file`, `gem_file`,
/// `shape_category`, `angle_settings`/`attached_files`) -- see
/// `update_diagram_metadata`'s doc for what a naive read-modify-write through
/// [`crate::model::detail::FacetingDiagramDetail`] would silently zero instead.
///
/// `designer` here is the free-text `designer_info` column, not the machine-split
/// `designer`/`source_citation` pair `FacetingDiagramDetail` also carries. There is no UI
/// for editing that split, and this update path leaves it as the import produced it.
///
/// Every numeric-looking field here (`refractive_index`, `index_gear`,
/// `symmetry_order`, `lw_ratio`/`hw_ratio`/`cw_ratio`/`pw_ratio`, `volume`) is still
/// `Option<String>` -- the editor's text field naturally produces one -- but
/// [`Database::update_diagram_metadata`] parses each through [`parse_optional_numeric`]
/// before binding it, rather than handing the raw text straight to a REAL/INTEGER
/// column. See that function's doc comment for why. `facets_count` is the one
/// exception: it stays a plain TEXT column (`"55+6"`), so it never had this problem,
/// and keeps its own tolerant parser ([`crate::model::facets::parse_facets_count`]).
///
/// [`Database::update_diagram_metadata`]: crate::db::sqlite::Database::update_diagram_metadata
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataUpdate {
    /// The free-text `designer_info` column -- see this struct's own doc comment for
    /// why this is the free-text field, not the machine-split `designer`/
    /// `source_citation` pair.
    pub designer_info: Option<String>,
    /// `diagram_details.shape`, matched by later searches with a plain `=` (see
    /// `crate::model::DEFAULT_SHAPES`'s doc comment).
    pub shape: Option<String>,
    /// Hand-typed refractive index text, parsed to `REAL` -- see [`parse_optional_numeric`].
    pub refractive_index: Option<String>,
    /// Hand-typed gear-tooth count text, parsed to `INTEGER` -- see [`parse_optional_numeric`].
    pub index_gear: Option<String>,
    /// The scraped-style `"55+6"` facet-count text; re-split into `facets`/
    /// `girdle_facets` by [`crate::model::facets::parse_facets_count`] at save time.
    pub facets_count: Option<String>,
    /// Hand-typed rotational-fold-count text, parsed to `INTEGER` -- see
    /// [`parse_optional_numeric`].
    pub symmetry_order: Option<String>,
    /// Whether the sheet additionally declares mirror-image symmetry -- already typed,
    /// no parsing needed.
    pub mirror_symmetry: Option<bool>,
    /// Hand-typed length/width ratio text, parsed to `REAL` -- see [`parse_optional_numeric`].
    pub lw_ratio: Option<String>,
    /// Hand-typed height/width ratio text, parsed to `REAL` -- see [`parse_optional_numeric`].
    pub hw_ratio: Option<String>,
    /// Hand-typed culet/width ratio text, parsed to `REAL` -- see [`parse_optional_numeric`].
    pub cw_ratio: Option<String>,
    /// Hand-typed pavilion/width ratio text, parsed to `REAL` -- see [`parse_optional_numeric`].
    pub pw_ratio: Option<String>,
    /// Hand-typed volume text, parsed to `REAL` -- see [`parse_optional_numeric`].
    pub volume: Option<String>,
}
