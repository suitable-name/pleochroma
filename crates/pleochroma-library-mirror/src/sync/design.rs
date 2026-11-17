//! Fetches, saves and (on an update) invalidates the stale cache of exactly one
//! mirrored design.

use crate::{
    options::{MirrorContent, MirrorCounts, MirrorOptions},
    transport::LibraryTransport,
    wire::frosting_from_wire,
};
use pleochroma_net::library::{AngleSettingWire, DesignRecord, LibraryRequest, LibraryResponse};
use pleochroma_vault::{
    db::sqlite::Database,
    local::{
        DesignFileKind, design_attachment_position, inclusion_effects_of_bytes,
        is_native_design_name, native_design_attachment_position, surface_finish_of_bytes,
    },
    model::{
        angle::AngleSetting, detail::FacetingDiagramDetail, entry::FacetingDiagramEntry,
        file::AttachedFile, mirror::MirrorState, surface_finish::StoredSurfaceFinish,
    },
};
use std::sync::{Arc, Mutex, PoisonError};
use tracing::warn;

/// How [`sync_one_design`] handled exactly one design -- distinguishes an ordinary
/// save from the guards below, since only the former counts toward
/// [`MirrorCounts::new_count`]/[`MirrorCounts::updated_count`].
pub(super) enum SyncedDesign {
    /// `entry`/`detail`/mirror state were saved (new or updated).
    Saved,
    /// This design's `url` already names a local row this sync has never mirrored --
    /// see [`sync_one_design`]'s own doc comment. Nothing was written locally.
    SkippedLocalConflict,
    /// The user deleted this design while its files were being fetched: the tombstone found
    /// under the write lock wins, so nothing was written. Counted in
    /// [`MirrorCounts::skipped_deleted`].
    SkippedDeleted,
    /// The design's design file is over [`MirrorOptions::max_attachment_bytes`] and
    /// [`MirrorOptions::skip_designs_over_cap`] is set. Nothing was written and no mirror
    /// state recorded. Counted in [`MirrorCounts::designs_skipped_too_large`].
    SkippedTooLarge,
}

/// Fetches and saves exactly one design: `FetchDesign`, then every non-oversized
/// attachment's bytes, then one atomic local `save_design` (entry and detail together)
/// and `upsert_mirror_state`. All network I/O happens before the first local write, so a failure
/// partway through never touches the database.
///
/// # The local-row guard: never overwrite a local row
///
/// A remote design's `url` can match a local row that no mirror sync ever wrote, such as a
/// worker's own `local://` import or a hand-imported `.asc` on the same synthetic URL. Before
/// saving, [`is_unmirrored_local_row`] checks for such a row. If one exists it is left untouched,
/// and the result is [`SyncedDesign::SkippedLocalConflict`]. The check runs only when `is_new`:
/// otherwise `super::pass::run_mirror_sync` already found the mirror state, and it skips
/// tombstoned designs before calling this function.
///
/// # The tombstone re-check
///
/// The mirror state was read before the network I/O, which can take a long time. A design the
/// user deletes meanwhile has its state tombstoned by the delete, so for an update (`!is_new`) the
/// state is read again under the write lock; a tombstone there ends the design with
/// [`SyncedDesign::SkippedDeleted`] instead of re-creating it.
///
/// `Err(())` on any fetch or save failure. The caller only counts it in [`MirrorCounts::failed`];
/// the reason is not kept. The design is retried on the next sync.
///
/// `is_new` also decides whether an updated design's cached previews, tilt curves and solid
/// extents are dropped after the save, since they describe the old geometry.
pub(super) fn sync_one_design<T: LibraryTransport>(
    db: &Arc<Mutex<Database>>,
    transport: &T,
    source_id: &str,
    options: MirrorOptions,
    summary: &pleochroma_net::library::DesignSummary,
    is_new: bool,
    counts: &mut MirrorCounts,
) -> Result<SyncedDesign, ()> {
    let design = match transport.request(&LibraryRequest::FetchDesign {
        entry_id: summary.entry_id,
    }) {
        Ok(LibraryResponse::Design(d)) => *d,
        _ => return Err(()),
    };

    if options.skip_designs_over_cap && design_file_exceeds(&design, options.max_attachment_bytes) {
        return Ok(SyncedDesign::SkippedTooLarge);
    }

    let attached_files = fetch_attachments(transport, &design, options, counts)?;
    let entry = FacetingDiagramEntry {
        title: design.title.clone(),
        url: design.url.clone(),
        design_id: design.design_id.clone().unwrap_or_default(),
    };
    let detail = build_detail(
        &design,
        attached_files,
        summary.concave_tiers,
        options.content,
    );

    let db = db.lock().unwrap_or_else(PoisonError::into_inner);

    // Local-row guard: never overwrite a local row this sync has never mirrored -- see this
    // function's own doc comment.
    if is_new && is_unmirrored_local_row(&db, &design.url) {
        warn!(
            "Mirror sync: leaving local entry ({}) untouched -- it was never mirrored \
             from this or any remote (local-row guard)",
            design.url
        );
        return Ok(SyncedDesign::SkippedLocalConflict);
    }

    // Tombstone re-check: the user may have deleted this design while its files were fetched.
    if !is_new && is_tombstoned(&db, &design.url) {
        return Ok(SyncedDesign::SkippedDeleted);
    }

    // One transaction for entry and detail: a failure between the two must not leave a
    // detail-less row that the local-row guard above would then skip forever.
    let Ok(id) = db.save_design(&entry, &detail, source_id) else {
        return Err(());
    };
    // `.asc` cannot state frosting, so the remote's frosting rows are copied beside the
    // detail: replaced (the remote is authoritative), so a design that lost its frosting
    // there loses it here too. A failure is logged and does not fail the design: it reads
    // as unfrosted, and the next sync retries.
    if let Err(e) = db.set_diagram_frosting(id, &frosting_from_wire(&design.frosting)) {
        warn!(
            "Mirror sync: failed to store the frosting of entry #{id} ({}): {e}",
            design.url
        );
    }
    // The surface finish has no notation in `.asc` or on the library wire either, but a design
    // file among the attachments states it: read from the file the row now holds, and replaced
    // (the remote is authoritative) so a design that lost its finish there loses it here too. A
    // failure is logged and does not fail the design, like the frosting.
    if let Err(e) = db.set_diagram_surface_finish(id, &attached_surface_finish(&detail)) {
        warn!(
            "Mirror sync: failed to store the surface finish of entry #{id} ({}): {e}",
            design.url
        );
    }
    // ... and the inclusion and play-of-color choice, from the same file and with the same rules.
    let inclusion_effects = detail
        .attached_files
        .iter()
        .find(|file| is_native_design_name(&file.name))
        .and_then(|file| inclusion_effects_of_bytes(&file.content));
    if let Err(e) = db.set_diagram_inclusion_effects(id, inclusion_effects.as_deref()) {
        warn!(
            "Mirror sync: failed to store the inclusion effects of entry #{id} ({}): {e}",
            design.url
        );
    }
    // An updated design's detail row was just replaced, so the caches built from its old
    // geometry are stale and are dropped.
    if !is_new {
        if let Err(e) = db.delete_preview_images(id) {
            warn!(
                "Mirror sync: failed to invalidate stale preview cache for entry #{id} \
                 ({}): {e}",
                design.url
            );
        }
        if let Err(e) = db.delete_tilt_curves(id) {
            warn!(
                "Mirror sync: failed to invalidate stale tilt-curve cache for entry \
                 #{id} ({}): {e}",
                design.url
            );
        }
        if let Err(e) = db.delete_solid_extents(id) {
            warn!(
                "Mirror sync: failed to invalidate stale solid-extents cache for entry \
                 #{id} ({}): {e}",
                design.url
            );
        }
    }
    // Recorded only after the local write succeeds, so a failed design is retried next sync.
    let _ = db.upsert_mirror_state(&MirrorState {
        url: design.url.clone(),
        source_id: source_id.to_string(),
        summary_version: summary.version,
        design_version: design.version,
        deleted_locally: false,
    });
    drop(db);

    Ok(SyncedDesign::Saved)
}

/// The surface finish the first design file among `detail`'s attached files states; empty when
/// there is none, or it states none (a design file larger than the attachment limit was never
/// fetched, so it reads as none as well).
fn attached_surface_finish(detail: &FacetingDiagramDetail) -> StoredSurfaceFinish {
    detail
        .attached_files
        .iter()
        .find(|file| is_native_design_name(&file.name))
        .map(|file| surface_finish_of_bytes(&file.content))
        .unwrap_or_default()
}

/// Whether `file_name` names a design file: a native `.psdd`, or a `.asc`, `.gem` or `.gcs`.
/// [`MirrorContent::DesignFilesOnly`] keeps exactly these attachments.
fn is_design_file_name(file_name: &str) -> bool {
    is_native_design_name(file_name) || DesignFileKind::from_file_name(file_name).is_some()
}

/// Whether the design file an editor would open from `design` (the native `.psdd`, else the
/// first `.asc`, `.gem` or `.gcs`) advertises a size over `cap`. `false` for a design with no
/// design file.
fn design_file_exceeds(design: &DesignRecord, cap: u64) -> bool {
    let names = design.attachments.iter().map(|meta| meta.name.as_str());
    native_design_attachment_position(names.clone())
        .or_else(|| design_attachment_position(names).map(|(position, _)| position))
        .is_some_and(|position| design.attachments[position].size > cap)
}

/// Fetches each attachment whose size is within `options.max_attachment_bytes` (and, with
/// [`MirrorContent::DesignFilesOnly`], that is a design file), and tallies skips, fetches and
/// bytes into `counts`.
fn fetch_attachments<T: LibraryTransport>(
    transport: &T,
    design: &DesignRecord,
    options: MirrorOptions,
    counts: &mut MirrorCounts,
) -> Result<Vec<AttachedFile>, ()> {
    let mut attached_files = Vec::with_capacity(design.attachments.len());
    for meta in &design.attachments {
        if options.content == MirrorContent::DesignFilesOnly && !is_design_file_name(&meta.name) {
            counts.attachments_skipped_by_content += 1;
            continue;
        }
        if meta.size > options.max_attachment_bytes {
            counts.attachments_skipped_too_large += 1;
            continue;
        }
        match transport.request(&LibraryRequest::FetchAttachment {
            attachment_id: meta.id,
        }) {
            Ok(LibraryResponse::Attachment { name, content }) => {
                counts.attachment_bytes_fetched += content.len() as u64;
                counts.attachments_fetched += 1;
                attached_files.push(AttachedFile {
                    name,
                    url: meta.url.clone(),
                    content,
                });
            }
            Ok(LibraryResponse::NotFound) => {
                // Vanished server-side between FetchDesign and FetchAttachment -- save
                // the rest of the design without this one file rather than failing it
                // outright.
            }
            _ => return Err(()),
        }
    }
    Ok(attached_files)
}

/// Builds the [`FacetingDiagramDetail`] that `save_diagram_detail` fully replaces the row with.
/// With [`MirrorContent::DesignFilesOnly`] the diagram image is left out.
fn build_detail(
    design: &DesignRecord,
    attached_files: Vec<AttachedFile>,
    concave_tiers: u32,
    content: MirrorContent,
) -> FacetingDiagramDetail {
    let angle_settings_table: Vec<AngleSetting> =
        design.angle_settings.iter().map(to_angle_setting).collect();
    // The wire carries no placement count; a concave row's index text lists them.
    let concave_facets = angle_settings_table
        .iter()
        .filter(|a| a.tool.is_some())
        .map(|a| {
            let placements = a.index.split(',').filter(|i| !i.trim().is_empty()).count();
            u32::try_from(placements).unwrap_or(u32::MAX)
        })
        .sum();
    let keep_image = content == MirrorContent::Everything;
    FacetingDiagramDetail {
        page_url: design.page_url.clone(),
        diagram_image_name: design.diagram_image_name.clone().filter(|_| keep_image),
        diagram_image_data: design.diagram_image_data.clone().filter(|_| keep_image),
        angle_settings_table,
        attached_files,
        competition_diagram: design.competition_diagram.clone(),
        lw_ratio: design.lw_ratio.clone(),
        refractive_index: design.refractive_index.clone(),
        index_gear: design.index_gear.clone(),
        volume: design.volume.clone(),
        facets_count: design.facets_count.clone(),
        shape: design.shape.clone(),
        designer_info: design.designer_info.clone(),
        // These eight fields must be mapped: `save_diagram_detail` replaces the whole detail
        // row, so an unmapped field would be blanked on every sync.
        hw_ratio: design.hw_ratio.clone(),
        tw_ratio: design.tw_ratio.clone(),
        uw_ratio: design.uw_ratio.clone(),
        pw_ratio: design.pw_ratio.clone(),
        cw_ratio: design.cw_ratio.clone(),
        symmetry_order: design.symmetry_order.clone(),
        mirror_symmetry: design.mirror_symmetry,
        designer: design.designer.clone(),
        // Same reason as the fields above. Every `FacetingDiagramDetail` field is set here.
        source_citation: design.source_citation.clone(),
        pdf_file: design.pdf_file.clone(),
        gem_file: design.gem_file.clone(),
        shape_category: design.shape_category.clone(),
        concave_tiers,
        concave_facets,
    }
}

/// Whether `url` names a local row that no mirror sync wrote (no `library_mirror_state` entry).
/// A brand-new URL and a previously mirrored row both return `false`.
fn is_unmirrored_local_row(db: &Database, url: &str) -> bool {
    let Ok(Some(_)) = db.diagram_entry_id_for_url(url) else {
        return false;
    };
    db.get_mirror_state(url).ok().flatten().is_none()
}

/// Whether `url`'s mirror state is a tombstone (the user deleted the design locally). An
/// unreadable or missing state is not one.
fn is_tombstoned(db: &Database, url: &str) -> bool {
    matches!(db.get_mirror_state(url), Ok(Some(state)) if state.deleted_locally)
}

fn to_angle_setting(a: &AngleSettingWire) -> AngleSetting {
    AngleSetting {
        order_index: a.order_index,
        facet: a.facet.clone(),
        angle: a.angle.clone(),
        index: a.index.clone(),
        notes: a.notes.clone(),
        // The wire keeps the formatted tool line only; its first column is the tool code.
        tool: a
            .tool_line
            .as_deref()
            .and_then(|line| line.split_whitespace().next())
            .map(str::to_owned),
        tool_line: a.tool_line.clone(),
    }
}
