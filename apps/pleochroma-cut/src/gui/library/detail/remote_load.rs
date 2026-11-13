//! Builds the detail pane and 3D viewport from a design fetched off a REMOTE
//! worker's library.

use super::{
    planes::{
        ConcaveRow, DesignFileMaterial, PreparedRemoteGeometry, ReconstructedPlanesInput, WireRow,
        apply_reconstructed_planes, prepare_remote_geometry,
    },
    shared::{
        catalogue_material_guess, format_optional_proportion, sides_from_rows, standard_codes,
    },
};
use crate::{
    AngleItem, DiagramDetailData, FileItem, LibraryModel, MainWindow, TiltModel, ViewportModel,
    bridge::{
        library::source::{self as library_source, LibrarySource},
        render_thread::RenderContext,
    },
    gui::{
        render::camera_lighting::resubmit_live_solid,
        solid_preview::preview_state::SolidPreviewState,
    },
    settings::WorkerSettings,
};
use pleochroma_net::library::{DesignRecord, LibraryRequest, LibraryResponse};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::sync::{Arc, Mutex};
use tracing::error;

/// The remote counterpart of `super::local_load::load_diagram_detail`.
///
/// Fetches one design's full record over the network and applies it through
/// [`apply_design_record_to_ui`], so the detail panel and viewport look the same for either
/// source.
///
/// [`FileItem::size_str`] uses the advertised attachment size. The library protocol never
/// sends attachment bytes in a `FetchDesign` reply, so bytes are fetched only when the user
/// exports a file (see `super::attachments::export_diagram_file_via_source`).
///
/// `preview_state` is needed only once the record and its planes arrive, so it is passed on
/// to [`apply_design_record_to_ui`] from the completion closure.
///
/// # Staleness guard
///
/// `source` and `seq` drop a reply that arrives after the selection moved on or after the
/// library switched, to local or to another remote. Otherwise a stale remote id could reach
/// the local-database callbacks, which act on the library source. The caller takes `seq`
/// before the request is built, and `source` is read again when the reply lands. See
/// [`reply_is_current`].
pub(super) fn load_diagram_detail_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    preview_state: &Arc<SolidPreviewState>,
    source: Arc<Mutex<LibrarySource>>,
    seq: u64,
) {
    let render_ctx = render_ctx.clone();
    let preview_state = Arc::clone(preview_state);
    let request_source = LibrarySource::Remote(worker.clone());
    library_source::spawn_library_request_prepared(
        ui.as_weak(),
        worker,
        LibraryRequest::FetchDesign { entry_id },
        // The request thread rebuilds the stone's planes and places the concave tools, so
        // the event-loop callback below only applies them.
        |result| match result {
            Ok(LibraryResponse::Design(record)) => Some(prepare_record_geometry(record)),
            _ => None,
        },
        move |ui, result, prepared| {
            // Checked here, after the geometry was prepared: a reply superseded during that
            // work is dropped without touching the UI.
            let current_source = source
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if !reply_is_current(
                crate::gui::library::search::is_current_search(seq),
                &request_source,
                &current_source,
            ) {
                return;
            }
            match result {
                Ok(LibraryResponse::Design(record)) => {
                    apply_design_record_to_ui(ui, &render_ctx, &record, &preview_state, prepared);
                }
                Ok(LibraryResponse::NotFound) => {
                    ui.global::<LibraryModel>().set_status_message(
                        "Diagram detail not found on the remote library.".into(),
                    );
                }
                Ok(_) => {
                    ui.global::<LibraryModel>().set_status_message(
                        "Unexpected reply fetching remote diagram detail.".into(),
                    );
                }
                Err(e) => {
                    error!("Remote FetchDesign failed: {e}");
                    ui.global::<LibraryModel>()
                        .set_status_message(format!("Error loading remote detail: {e}").into());
                }
            }
        },
    );
}

/// Whether a remote-detail reply may still be applied to the UI. It may not once a newer
/// request has superseded it, or once the active library source differs from
/// `request_source`, even if `seq` is still current. This is a pure function, so both stale
/// cases are unit-tested below without a network or event loop.
#[must_use]
fn reply_is_current(
    seq_is_current: bool,
    request_source: &LibrarySource,
    current_source: &LibrarySource,
) -> bool {
    seq_is_current && request_source == current_source
}

/// The detail header's data for `record`, as the remote route shows it.
fn diagram_detail_data(record: &DesignRecord) -> DiagramDetailData {
    let is_local = record.url.starts_with("local://");
    DiagramDetailData {
        id: i32::try_from(record.entry_id).unwrap_or(i32::MAX),
        title: record.title.clone().into(),
        url: record.url.clone().into(),
        designer: record.designer_info.clone().unwrap_or_default().into(),
        shape: record.shape.clone().unwrap_or_default().into(),
        gear: record.index_gear.clone().unwrap_or_default().into(),
        facets: record.facets_count.clone().unwrap_or_default().into(),
        lw_ratio: format_optional_proportion(record.lw_ratio.as_deref()).into(),
        material_guess: catalogue_material_guess(record.refractive_index.as_deref()).into(),
        ri: record.refractive_index.clone().unwrap_or_default().into(),
        volume: format_optional_proportion(record.volume.as_deref()).into(),
        competition: record
            .competition_diagram
            .clone()
            .unwrap_or_default()
            .into(),
        image_name: record.diagram_image_name.clone().unwrap_or_default().into(),
        has_image: record.diagram_image_data.is_some(),
        is_local,
        // Formatted as on the local path, via [`format_optional_proportion`]. The chips
        // are shown for remote designs but not editable: `detail_header.slint` gates
        // editing on `detail.is_local`.
        hw_ratio: format_optional_proportion(record.hw_ratio.as_deref()).into(),
        cw_ratio: format_optional_proportion(record.cw_ratio.as_deref()).into(),
        pw_ratio: format_optional_proportion(record.pw_ratio.as_deref()).into(),
        symmetry_order: record.symmetry_order.clone().unwrap_or_default().into(),
        mirror_symmetry: record.mirror_symmetry.unwrap_or(false),
    }
}

/// `record`'s angle-table rows as borrowed wire strings, for [`prepare_remote_geometry`].
fn wire_rows(record: &DesignRecord) -> Vec<WireRow<'_>> {
    record
        .angle_settings
        .iter()
        .map(|a| {
            (
                a.facet.as_str(),
                a.angle.as_str(),
                a.index.as_str(),
                a.notes.as_str(),
                a.tool_line.as_deref(),
            )
        })
        .collect()
}

/// The stone `record` describes, rebuilt from its wire rows alone (no Slint, no render
/// context), so the request thread can run it.
fn prepare_record_geometry(record: &DesignRecord) -> PreparedRemoteGeometry {
    prepare_remote_geometry(
        record.shape.as_deref(),
        record.index_gear.as_deref(),
        &wire_rows(record),
    )
}

/// Applies a fetched design to the detail pane and the viewport. `prepared` is the geometry
/// [`prepare_record_geometry`] built on the request thread. When it is `None`, the concave
/// rows are rebuilt here instead.
fn apply_design_record_to_ui(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    record: &DesignRecord,
    preview_state: &Arc<SolidPreviewState>,
    prepared: Option<PreparedRemoteGeometry>,
) {
    ui.global::<LibraryModel>()
        .set_current_detail(diagram_detail_data(record));
    // An absent `preview_material` means nothing is cached yet, so the Tilt Performance
    // dialog's "re-render with the current material?" offer stays hidden.
    ui.global::<TiltModel>()
        .set_cached_curve_material(record.preview_material.clone().unwrap_or_default().into());
    // Provenance is local-catalogue only, so remote designs get no "Derived from" note.

    // The local path derives the sides the same way (see `angle_items` in `local_load.rs`):
    // `sides_from_rows` is shared, so both paths agree on a design's sides.
    let sides = sides_from_rows(
        record
            .angle_settings
            .iter()
            .map(|a| (a.facet.as_str(), a.angle.as_str(), a.index.as_str())),
    );
    let labels: Vec<(&str, &str)> = record
        .angle_settings
        .iter()
        .map(|a| (a.facet.as_str(), a.index.as_str()))
        .collect();
    let codes = standard_codes(&labels, &sides);
    let angle_items: Vec<AngleItem> = record
        .angle_settings
        .iter()
        .zip(sides)
        .zip(codes)
        .map(|((a, side), code)| AngleItem {
            order_idx: a.order_index.cast_signed(),
            side,
            code: code.into(),
            facet: a.facet.clone().into(),
            angle: a.angle.trim_start_matches('-').to_string().into(),
            index_val: a.index.clone().into(),
            notes: a.notes.clone().into(),
            second_line: a.tool_line.clone().unwrap_or_default().into(),
        })
        .collect();
    ui.global::<LibraryModel>()
        .set_current_angles(ModelRc::new(VecModel::from(angle_items.clone())));

    let file_items: Vec<FileItem> = record
        .attachments
        .iter()
        .map(|f| {
            let size_kb = f.size as f64 / 1024.0;
            FileItem {
                name: f.name.clone().into(),
                url: f.url.clone().into(),
                size_str: format!("{size_kb:.1} KB").into(),
            }
        })
        .collect();
    ui.global::<LibraryModel>()
        .set_current_files(ModelRc::new(VecModel::from(file_items)));
    ui.global::<LibraryModel>()
        .set_selected_entry_id(i32::try_from(record.entry_id).unwrap_or(i32::MAX));

    // With prepared geometry, the planes, gear and tools are applied as they are and no
    // concave rows are passed, so `apply_reconstructed_planes` has nothing to rebuild.
    // Without it, the concave rows are passed in and rebuilt there.
    let (real_design, tools, tool_placements, concave_rows) = match prepared {
        Some(geometry) => (
            Some((
                geometry.planes,
                geometry.gear_teeth,
                geometry.gear_reference_angle,
            )),
            geometry.tools,
            geometry.tool_placements,
            Vec::new(),
        ),
        None => (
            None,
            Vec::new(),
            Vec::new(),
            ConcaveRow::from_wire(wire_rows(record)),
        ),
    };
    apply_reconstructed_planes(
        ui,
        render_ctx,
        record.entry_id,
        ReconstructedPlanesInput {
            shape: record.shape.as_deref(),
            index_gear: record.index_gear.as_deref(),
            angle_items: &angle_items,
            refractive_index: record.refractive_index.as_deref(),
            // The same value as `TiltModel.cached_curve_material` set above.
            preview_material: record.preview_material.as_deref(),
            // A remote record carries no attachment bytes, so no real `Design` can be
            // built here. The placeholder reconstruction (prepared above) is used instead.
            real_design,
            // No real planes, so nothing to name as frosted.
            frosted: Vec::new(),
            // No design file, so the persisted material and the refractive-index rules
            // apply at the default girdle width.
            design_material: DesignFileMaterial::default(),
            tools,
            tool_placements,
            concave_rows,
        },
    );

    // Redraws the solid, as the local selection callback does
    // (`gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`). Here the
    // planes arrive asynchronously, so this is where the redraw must happen. It runs only on
    // the Live Render tab in Solid mode, so the previous design's raster is not left up.
    if ui.get_render_view_tab() == 0 && ui.global::<ViewportModel>().get_live_view_mode() == 0 {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resubmit_live_solid(ui, &ctx, preview_state);
    }
}

#[cfg(test)]
mod reply_is_current_tests {
    use super::{LibrarySource, reply_is_current};
    use crate::settings::WorkerSettings;

    fn worker(address: &str) -> WorkerSettings {
        WorkerSettings {
            address: address.to_string(),
            ..WorkerSettings::default()
        }
    }

    #[test]
    fn current_seq_and_matching_remote_source_is_current() {
        let source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(reply_is_current(true, &source, &source));
    }

    #[test]
    fn a_superseded_seq_is_never_current_even_with_a_matching_source() {
        let source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(!reply_is_current(false, &source, &source));
    }

    #[test]
    fn switching_to_local_before_the_reply_lands_is_never_current() {
        let request_source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(!reply_is_current(
            true,
            &request_source,
            &LibrarySource::Local
        ));
    }

    #[test]
    fn switching_to_a_different_remote_worker_before_the_reply_lands_is_never_current() {
        let request_source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        let current_source = LibrarySource::Remote(worker("10.0.0.6:9443"));
        assert!(!reply_is_current(true, &request_source, &current_source));
    }

    #[test]
    fn a_stale_seq_and_a_moved_source_together_are_still_never_current() {
        let request_source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(!reply_is_current(
            false,
            &request_source,
            &LibrarySource::Local
        ));
    }
}
