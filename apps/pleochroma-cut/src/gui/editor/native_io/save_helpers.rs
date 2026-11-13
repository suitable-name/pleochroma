//! Shared save and export helpers.
//!
//! They are the custom-material list and snapshot, the "not a closed
//! solid" header marker, the `.asc` writer that reuses a solve, the source-row footnote, and
//! the text of a `.psdd` design file.

use crate::{bridge::render_thread::RenderContext, gui::optics::crystal_optics::library_material};
use pleochroma::{
    geometry::{meet_solver::SolvedTier, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{
    Design, built_in_refractive_index,
    native::{
        CatalogueAsc, CustomMaterialSnapshot, DesignExtras, SaveError, SaveExtras,
        catalogue_asc_extended, catalogue_asc_extended_from_solved, design_to_file,
    },
};
use pleochroma_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};

/// The registered custom catalogue materials (`RenderContext::custom_materials`), as a cheap
/// `Arc` clone. Passed to the writers, so a design on a custom material writes that material's
/// own refractive index instead of the legacy schedule RI.
pub(super) fn snapshot_custom_materials(
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Arc<Vec<GemMaterial>> {
    Arc::clone(
        &render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .custom_materials,
    )
}

/// The leading header a confirmed write stamps into the file, recording why it needed
/// confirming. Distinct from [`pleochroma_formats::asc::mark_reconstructed`]'s "RECONSTRUCTED"
/// line, which marks an angle-table placeholder rather than a design that does not close.
pub(super) const NOT_CLOSED_SOLID_MARKER: &str = "NOT A CLOSED SOLID";

/// Builds the header line itself, or `None` when `headers` already starts with one
/// (never stamped twice, mirroring `mark_reconstructed`'s own idempotence).
pub(super) fn degenerate_marker_header(headers: &[String], message: &str) -> Option<String> {
    let already_marked = headers
        .first()
        .is_some_and(|h| h.starts_with(NOT_CLOSED_SOLID_MARKER));
    (!already_marked).then(|| format!("{NOT_CLOSED_SOLID_MARKER} -- {message}"))
}

/// The `.asc` text for `design`, reusing the masts of an earlier solve when it can.
///
/// `solved` is used as is when it is present and the design has tiers: the solve is not run
/// again. Otherwise this falls back to [`catalogue_asc_extended`], which solves internally.
/// That covers a design that does not solve (`solved` is `None`, and a `MissingAnchor` fails
/// fast) and a tier-less draft, which never solves.
///
/// The caller sets `extras.custom_catalogue` from [`snapshot_custom_materials`], so a custom
/// material writes its own refractive index to the `I` line
/// (see [`SaveExtras::custom_catalogue`]).
///
/// The "not a closed solid" question is asked separately, by `decide_write_status`.
pub(super) fn catalogue_asc_reusing_solve(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    extras: &SaveExtras<'_>,
) -> Result<CatalogueAsc, SaveError> {
    match solved {
        Some(solved) if !design.tiers.is_empty() => catalogue_asc_extended_from_solved(
            design,
            solved,
            original_asc_text,
            placeholder_note,
            extras,
        ),
        _ => catalogue_asc_extended(design, original_asc_text, placeholder_note, extras),
    }
}

/// Sets (or clears) the source-catalogue-row stamp in `footnotes` before a `.asc` is written.
/// The stamp holds the recorded row id, so a re-import links back to that row rather than
/// guessing from the title. Any earlier stamp is removed first, so there is never more than
/// one. The reader is `save_imported_design` (`gui::library::local::import`), via
/// [`pleochroma_vault::local::parse_source_entry_footnote`].
///
/// Called by both Export Edited .asc and Save: a plain export has no design file beside it,
/// so the `.asc` itself must carry the stamp.
pub(super) fn stamp_source_entry_footnote(
    footnotes: &mut Vec<String>,
    source_entry_id: Option<i64>,
) {
    footnotes.retain(|f| !f.starts_with(pleochroma_vault::local::SOURCE_ENTRY_FOOTNOTE_PREFIX));
    if let Some(id) = source_entry_id {
        footnotes.push(pleochroma_vault::local::format_source_entry_footnote(id));
    }
}

/// Builds `design`'s [`CustomMaterialSnapshot`] for a save, so a design saved under a custom
/// material does not reload as Diamond. `None` when the material is unset, is a built-in
/// preset, or has no database row.
///
/// Reads the database row, not a resolved `GemMaterial`: the row holds the typed numbers
/// directly, and re-deriving them from a resolved dispersion curve would refit the Cauchy
/// fit for nothing. `crystal_system` and `optical_character` are stored as their plain text
/// (`""` for rows that predate them). [`gem_material_from_custom_snapshot`] reads them back,
/// and falls back to the sign of `birefringence_delta` for an empty or unknown name.
pub(super) fn custom_material_snapshot_for_save(
    design: &Design,
    db: &Arc<Mutex<Database>>,
) -> Option<CustomMaterialSnapshot> {
    let name = design.material.name.as_deref()?;
    if built_in_refractive_index(name).is_some() {
        return None;
    }
    let guard = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let row = guard
        .get_custom_materials()
        .ok()?
        .into_iter()
        .find(|r| r.name.eq_ignore_ascii_case(name))?;
    // The material's color pattern, when it has one that can still be read, travels as its JSON
    // text. A stored pattern this build cannot decode is left out (the plain colors still go).
    let pattern = crate::gui::color_pattern::load_pattern(&guard, &row.name)
        .ok()
        .flatten()
        .and_then(|pattern| pleochroma_cut_core::material::encode_pattern(&pattern).ok());
    drop(guard);
    // `LibraryMaterial::snapshot` (pleochroma-cut-core) builds the numbers, model and colors.
    Some(library_material(&row).snapshot().with_pattern(pattern))
}

/// The text of `design`'s self-contained `.psdd` file: every tier in full plus
/// the custom-material snapshot, history trail, `[meta]` table and attachments
/// `extras` carries, and the printed proportions of the catalogue row the design came
/// from. `draft` records that the design did not solve when it was saved.
///
/// # Errors
///
/// A ready-to-toast message: a `[meta]` value or an attachment the file format refuses
/// (a bad or repeated name, a total over the size limit). Without metadata and
/// attachments the serializer cannot fail for the fields written here.
pub(super) fn design_file_text(
    design: &Design,
    printed_proportions: Option<&ExternalProportions>,
    extras: &DesignExtras<'_>,
    draft: bool,
) -> Result<String, String> {
    let file = design_to_file(design, printed_proportions, extras).with_draft(draft);
    pleochroma_formats::native::design::to_string(&file).map_err(|e| e.to_string())
}
