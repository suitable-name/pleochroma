//! Builds the detail pane and 3D viewport from a design in the LOCAL database.
//!
//! Builds the detail pane and 3D viewport for a design in the LOCAL database.
//!
//! `LibraryModel.selected_entry_id` is set at the click, because the card highlight, Load
//! Selected and the planner's open-in-editor read it straight after.
//!
//! [`load_diagram_detail`] numbers each load and hands it to the single worker of
//! `load_worker`, so clicking through many rows never starts many threads. Between each
//! pair of stages a load checks [`DETAIL_LOADS`]. A superseded load posts nothing:
//!
//! 1. the pane's data, read-only and metadata only (no attachment or image bytes), posted
//!    to the event loop;
//! 2. the full record, attachment blobs included, since the design file is an attachment;
//! 3. the plane resolution, which parses and solves the design file;
//! 4. the planes, posted to the event loop.
//!
//! Stage 3 cannot be cancelled once started (`resolve_catalogue_planes_with_frosting` takes
//! no cancel control), so its result is dropped if stale instead.

use super::{
    load_worker::{LoadSequence, Worker, should_continue},
    planes::{DesignFileMaterial, ReconstructedPlanesInput, apply_reconstructed_planes},
    shared::{
        catalogue_material_guess, clear_current_detail_display, format_optional_proportion,
        sides_from_rows, standard_codes,
    },
};
use crate::{
    AngleItem, DiagramDetailData, FileItem, LibraryModel, MainWindow, TiltModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{editor::CataloguePlanes, library::read_access::with_read_db},
};
use pleochroma_vault::{db::sqlite::Database, model::entry::FullDiagramMeta};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex, OnceLock},
};
use tracing::error;

/// The newest local detail load.
static DETAIL_LOADS: LoadSequence = LoadSequence::new();

/// Makes every load that is still running or waiting stale, without starting a new one.
/// For the library's source switch: a numeric entry id means a different design in
/// another library, so a load begun against the old source must never apply.
pub(in crate::gui::library) fn invalidate_detail_loads() {
    let _ = DETAIL_LOADS.begin();
}

/// Whether a resolved load should still draw its planes: it must be the newest
/// load, and the detail pane must still be showing the entry it resolved (a click on
/// a remote row moves the selection without touching [`DETAIL_LOADS`]).
fn planes_still_wanted(is_newest_load: bool, selected_entry_id: i32, entry_id: i64) -> bool {
    is_newest_load && i64::from(selected_entry_id) == entry_id
}

/// Whether the detail pane's data should still be applied: the load must be the newest and
/// the row fetched must be the one this load asked for.
const fn metadata_still_wanted(is_newest_load: bool, fetched_entry_id: i64, entry_id: i64) -> bool {
    is_newest_load && fetched_entry_id == entry_id
}

/// Everything the detail pane reads from the database, in one pass over a read-only
/// connection on the load worker. No attachment or image bytes are read: `meta` carries
/// each attachment's name and size only.
struct DetailFetch {
    meta: FullDiagramMeta,
    /// Whether the design has a diagram image (the image itself is never read here).
    has_image: bool,
    shape_options: Vec<SharedString>,
    shape_index: i32,
    /// `Database::get_preview_material`: reads only the `preview_material` column,
    /// never the two cached preview PNGs.
    cached_curve_material: Option<String>,
    /// Title of the design this one was derived from, when one is recorded.
    derived_from_title: Option<String>,
}

/// Reads `entry_id`'s detail-pane data from `db`; `Ok(None)` when the row is gone.
fn fetch_detail(db: &Database, entry_id: i64) -> anyhow::Result<Option<DetailFetch>> {
    let Some((meta, has_image)) = db.get_diagram_full_meta_lite(entry_id)? else {
        return Ok(None);
    };
    let (shape_options, shape_index) =
        crate::gui::library::local::build_shape_picker_options(db, meta.shape.as_deref());
    let cached_curve_material = db.get_preview_material(entry_id).ok().flatten();
    // `None` and errors both mean no recorded source.
    let derived_from_title = db
        .get_derived_from_title(entry_id)
        .ok()
        .flatten()
        .map(|(_, title)| title);
    Ok(Some(DetailFetch {
        meta,
        has_image,
        shape_options,
        shape_index,
        cached_curve_material,
        derived_from_title,
    }))
}

/// Sets the shape picker options and current index, the cached curve material and the
/// derived-from note. Split out of [`apply_metadata`] to keep that function under clippy's
/// length lint.
///
/// The shape picker is local-only and recomputed on every open, so it matches this design.
/// The cached material is what `cached_curve_material_is_stale` compares against: an empty
/// string (none recorded yet) is never stale. The derived-from note is a status message.
fn push_shape_picker_and_cached_material(
    ui: &MainWindow,
    shape_options: Vec<SharedString>,
    shape_index: i32,
    cached_curve_material: Option<&str>,
    derived_from_title: Option<String>,
) {
    ui.global::<LibraryModel>()
        .set_shape_picker_options(ModelRc::new(VecModel::from(shape_options)));
    ui.global::<LibraryModel>()
        .set_shape_picker_current_index(shape_index);
    ui.global::<TiltModel>()
        .set_cached_curve_material(cached_curve_material.unwrap_or_default().into());
    if let Some(title) = derived_from_title {
        ui.global::<LibraryModel>()
            .set_status_message(format!("Derived from: {title}").into());
    }
}

/// The detail pane's header fields for `full`.
fn detail_data(full: &FullDiagramMeta, has_image: bool) -> DiagramDetailData {
    DiagramDetailData {
        id: i32::try_from(full.entry_id).unwrap_or(i32::MAX),
        title: full.title.clone().into(),
        url: full.url.clone().into(),
        designer: full.designer_info.clone().unwrap_or_default().into(),
        shape: full.shape.clone().unwrap_or_default().into(),
        gear: full.index_gear.clone().unwrap_or_default().into(),
        facets: full.facets_count.clone().unwrap_or_default().into(),
        lw_ratio: format_optional_proportion(full.lw_ratio.as_deref()).into(),
        material_guess: catalogue_material_guess(full.refractive_index.as_deref()).into(),
        ri: full.refractive_index.clone().unwrap_or_default().into(),
        volume: format_optional_proportion(full.volume.as_deref()).into(),
        competition: full.competition_diagram.clone().unwrap_or_default().into(),
        image_name: full.diagram_image_name.clone().unwrap_or_default().into(),
        has_image,
        // A locally imported design's `url` is a synthetic `local://` id, not a web page.
        // This gates "Open on Web" and "Copy Link" in `detail_header.slint`.
        is_local: full.url.starts_with("local://"),
        hw_ratio: format_optional_proportion(full.hw_ratio.as_deref()).into(),
        cw_ratio: format_optional_proportion(full.cw_ratio.as_deref()).into(),
        pw_ratio: format_optional_proportion(full.pw_ratio.as_deref()).into(),
        symmetry_order: full.symmetry_order.clone().unwrap_or_default().into(),
        mirror_symmetry: full.mirror_symmetry.unwrap_or(false),
    }
}

/// The angle table's rows for `full`, in the order of `full.angle_settings`, which is
/// sorted by `order_idx`. Sides come from each row's facet, angle and index.
fn angle_items(full: &FullDiagramMeta) -> Vec<AngleItem> {
    let sides = sides_from_rows(
        full.angle_settings
            .iter()
            .map(|a| (a.facet.as_str(), a.angle.as_str(), a.index.as_str())),
    );
    let labels: Vec<(&str, &str)> = full
        .angle_settings
        .iter()
        .map(|a| (a.facet.as_str(), a.index.as_str()))
        .collect();
    let codes = standard_codes(&labels, &sides);
    full.angle_settings
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
        .collect()
}

/// The attachment list's rows for `full`.
fn file_items(full: &FullDiagramMeta) -> Vec<FileItem> {
    full.attached_files
        .iter()
        .map(|f| {
            // `f.size` is the byte count (SQL `length(content)`).
            let size_kb = f.size as f64 / 1024.0;
            FileItem {
                name: f.name.clone().into(),
                url: f.url.clone().into(),
                size_str: format!("{size_kb:.1} KB").into(),
            }
        })
        .collect()
}

/// Everything one load needs, owned: the worker thread (or, if the OS refuses the thread,
/// the caller) runs it.
struct DetailRequest {
    sequence: u64,
    entry_id: i64,
    db: Arc<Mutex<Database>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    ui_weak: slint::Weak<MainWindow>,
}

/// The one worker behind every local detail load, started on the first click.
fn detail_worker() -> &'static Worker<DetailRequest> {
    static WORKER: OnceLock<Worker<DetailRequest>> = OnceLock::new();
    WORKER.get_or_init(|| Worker::new(run_request))
}

/// Loads one design from the LOCAL database into the detail pane, angle table, attachment
/// list and, once resolved, the 3D viewport. Returns at once. The stages run on the load
/// worker and are applied on the event loop, unless a later click supersedes them.
pub(super) fn load_diagram_detail(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
) {
    let sequence = DETAIL_LOADS.begin();
    // Set now rather than when stage 1 lands: the card highlight, Load Selected, the
    // planner's open-in-editor and the guide launch read it straight after the click. A
    // design that proves unreadable drops the selection again ([`post_failure`]).
    ui.global::<LibraryModel>()
        .set_selected_entry_id(i32::try_from(entry_id).unwrap_or(i32::MAX));
    let request = DetailRequest {
        sequence,
        entry_id,
        db: Arc::clone(db_mutex),
        render_ctx: Arc::clone(render_ctx),
        ui_weak: ui.as_weak(),
    };
    if let Err(request) = detail_worker().submit(sequence, request) {
        // The OS refused the worker thread (logged once): run this load inline. Its
        // results still go through the event loop, exactly as from the worker.
        run_request(request);
    }
}

/// The worker's handler: runs `request`'s stages, and turns a panic anywhere in them into
/// the error status an unreadable design gets, so the thread carries on.
fn run_request(request: DetailRequest) {
    let ui_weak = request.ui_weak.clone();
    let (sequence, entry_id) = (request.sequence, request.entry_id);
    if let Err(message) =
        crate::gui::library::local::catch_file_panic(AssertUnwindSafe(move || {
            run_stages(request);
        }))
    {
        error!("Loading the detail of diagram #{entry_id} panicked: {message}");
        post_failure(
            &ui_weak,
            sequence,
            entry_id,
            format!("Error loading detail: {message}"),
        );
    }
}

/// Reports that `entry_id` could not be loaded.
///
/// On the event loop, and only if `sequence` is still the newest load, it drops the selection
/// [`load_diagram_detail`] made at the click, clears the pane if the selection is still on
/// that row, and shows `message`. A design that is gone or unreadable never stays "selected".
fn post_failure(ui_weak: &slint::Weak<MainWindow>, sequence: u64, entry_id: i64, message: String) {
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        if !should_continue(&DETAIL_LOADS, sequence) {
            return;
        }
        if i64::from(ui.global::<LibraryModel>().get_selected_entry_id()) == entry_id {
            clear_current_detail_display(&ui);
        }
        ui.global::<LibraryModel>()
            .set_status_message(message.into());
    });
}

/// The four stages of one load; each returns silently as soon as the load is stale.
fn run_stages(request: DetailRequest) {
    let DetailRequest {
        sequence,
        entry_id,
        db,
        render_ctx,
        ui_weak,
    } = request;
    if !should_continue(&DETAIL_LOADS, sequence) {
        return;
    }

    // Stage 1: the pane's data, over a read-only connection. The shared writer lock is only
    // the fallback for `:memory:`.
    let fetched = with_read_db(&db, |db| fetch_detail(db, entry_id));
    if !should_continue(&DETAIL_LOADS, sequence) {
        return;
    }
    let fetched = match fetched {
        Ok(Some(fetched)) => fetched,
        Ok(None) => {
            post_failure(
                &ui_weak,
                sequence,
                entry_id,
                "Diagram detail not found.".to_string(),
            );
            return;
        }
        Err(e) => {
            error!("Failed to fetch diagram full detail: {:?}", e);
            post_failure(
                &ui_weak,
                sequence,
                entry_id,
                format!("Error loading detail: {e}"),
            );
            return;
        }
    };
    // What the plane resolution needs from the pane's data, kept before it moves to the
    // event loop.
    let shape = fetched.meta.shape.clone();
    let index_gear = fetched.meta.index_gear.clone();
    let refractive_index = fetched.meta.refractive_index.clone();
    let preview_material = fetched.cached_curve_material.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        apply_metadata(&ui, sequence, entry_id, fetched);
    });
    if !should_continue(&DETAIL_LOADS, sequence) {
        return;
    }

    // Stage 2: the design file is an attachment, so the full record is needed. Its blobs
    // are read here, off the UI thread.
    let full = with_read_db(&db, |db| db.get_diagram_full(entry_id));
    if !should_continue(&DETAIL_LOADS, sequence) {
        return;
    }

    // Stage 3: parse and solve. A design file that fails to read or solve is not an error,
    // since [`crate::gui::editor::resolve_catalogue_planes`] falls back to the angle table.
    // Only a panic or an unreadable record is.
    let planes = match full {
        Ok(Some(full)) => crate::gui::library::local::catch_file_panic(AssertUnwindSafe(|| {
            crate::gui::editor::resolve_catalogue_planes_with_frosting(&full)
        })),
        Ok(None) => Err("the design no longer exists".to_string()),
        Err(e) => Err(format!("the design could not be read: {e}")),
    };
    if !should_continue(&DETAIL_LOADS, sequence) {
        return;
    }

    // Stage 4: the planes.
    let resolved = ResolvedPlanes {
        entry_id,
        shape,
        index_gear,
        refractive_index,
        preview_material,
        planes,
    };
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        apply_resolved_planes(&ui, &render_ctx, sequence, resolved);
    });
}

/// Fills the detail pane, angle table, attachment list and shape picker from `fetched`, on
/// the UI thread, if the load is still wanted (see [`metadata_still_wanted`]).
fn apply_metadata(ui: &MainWindow, sequence: u64, entry_id: i64, fetched: DetailFetch) {
    if !metadata_still_wanted(
        should_continue(&DETAIL_LOADS, sequence),
        fetched.meta.entry_id,
        entry_id,
    ) {
        return;
    }
    let DetailFetch {
        meta,
        has_image,
        shape_options,
        shape_index,
        cached_curve_material,
        derived_from_title,
    } = fetched;
    ui.global::<LibraryModel>()
        .set_current_detail(detail_data(&meta, has_image));
    ui.global::<LibraryModel>()
        .set_current_angles(ModelRc::new(VecModel::from(angle_items(&meta))));
    ui.global::<LibraryModel>()
        .set_current_files(ModelRc::new(VecModel::from(file_items(&meta))));
    ui.global::<LibraryModel>()
        .set_selected_entry_id(i32::try_from(entry_id).unwrap_or(i32::MAX));

    // Shape picker, cached material and derived-from note.
    push_shape_picker_and_cached_material(
        ui,
        shape_options,
        shape_index,
        cached_curve_material.as_deref(),
        derived_from_title,
    );
}

/// What the event loop needs to draw a resolved record's planes. Owned and `Send`:
/// the worker drops the record itself (attachment blobs included) and forwards only
/// this.
struct ResolvedPlanes {
    entry_id: i64,
    shape: Option<String>,
    index_gear: Option<String>,
    refractive_index: Option<String>,
    preview_material: Option<String>,
    /// The planes, gear teeth and reference angle, with the design file's material and
    /// girdle diameter, or the error message. A design file that fails to read or solve
    /// falls back to the angle table, the same resolution the preview and tilt batches use,
    /// so the viewport and the cached thumbnails show the same geometry. Paired with the
    /// ascending indices of the frosted planes.
    planes: Result<(CataloguePlanes, Vec<u32>), String>,
}

/// Draws `resolved`'s planes, on the UI thread, if they are still wanted (see
/// [`planes_still_wanted`]).
fn apply_resolved_planes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    sequence: u64,
    resolved: ResolvedPlanes,
) {
    let selected = ui.global::<LibraryModel>().get_selected_entry_id();
    if !planes_still_wanted(
        DETAIL_LOADS.is_current(sequence),
        selected,
        resolved.entry_id,
    ) {
        return;
    }
    let (catalogue, frosted) = match resolved.planes {
        Ok(resolved_planes) => resolved_planes,
        Err(message) => {
            error!(
                "Resolving the planes of diagram #{} failed: {message}",
                resolved.entry_id
            );
            ui.global::<LibraryModel>().set_status_message(
                format!("Could not build the 3D view of this design: {message}").into(),
            );
            return;
        }
    };
    // The design file's material and size, for the viewport to draw the stone as the
    // editor and the exports do (`planes::apply_catalogue_material`).
    let design_material = DesignFileMaterial::from(&catalogue);
    let (tools, tool_placements) = (catalogue.render_tools, catalogue.render_placements);
    let real_design = (
        catalogue.planes,
        catalogue.gear_teeth,
        catalogue.gear_reference_angle,
    );
    apply_reconstructed_planes(
        ui,
        render_ctx,
        resolved.entry_id,
        ReconstructedPlanesInput {
            shape: resolved.shape.as_deref(),
            index_gear: resolved.index_gear.as_deref(),
            // Unread: `real_design` is always `Some` here, and the angle table is
            // only consulted when it is `None`.
            angle_items: &[],
            refractive_index: resolved.refractive_index.as_deref(),
            preview_material: resolved.preview_material.as_deref(),
            real_design: Some(real_design),
            design_material,
            // The stone with its concave tools, as the Compare window draws it.
            tools,
            tool_placements,
            concave_rows: Vec::new(),
            frosted,
        },
    );
    request_live_solid_redraw(ui);
}

/// Redraws the Live Render solid when that tab is showing, since the planes behind it
/// have changed.
///
/// The selection callback's redraw runs before the planes land, so it shows the previous
/// design. This goes through `ViewportModel.live_view_mode_changed`, whose handler redraws
/// from `RenderContext::active_planes`. Invoking it with the unchanged mode only repeats
/// that redraw.
fn request_live_solid_redraw(ui: &MainWindow) {
    let viewport = ui.global::<ViewportModel>();
    let mode = viewport.get_live_view_mode();
    if ui.get_render_view_tab() == 0 && mode == 0 {
        viewport.invoke_live_view_mode_changed(mode);
    }
}

#[cfg(test)]
mod tests {
    use super::{metadata_still_wanted, planes_still_wanted};
    use crate::gui::library::detail::load_worker::{LoadSequence, should_continue};

    #[test]
    fn planes_are_drawn_only_for_the_newest_load_of_the_selected_entry() {
        assert!(planes_still_wanted(true, 7, 7));
        assert!(
            !planes_still_wanted(false, 7, 7),
            "a superseded load never draws"
        );
        assert!(
            !planes_still_wanted(true, 8, 7),
            "the selection moved to another row"
        );
    }

    #[test]
    fn the_detail_pane_data_is_applied_only_for_the_newest_load_of_the_asked_row() {
        assert!(metadata_still_wanted(true, 7, 7));
        assert!(
            !metadata_still_wanted(false, 7, 7),
            "a superseded load never applies its pane data"
        );
        assert!(
            !metadata_still_wanted(true, 8, 7),
            "the fetched row is not the one asked for"
        );
    }

    #[test]
    fn a_superseded_load_fails_the_metadata_stage_guard() {
        let loads = LoadSequence::new();
        let first = loads.begin();
        assert!(metadata_still_wanted(should_continue(&loads, first), 7, 7));
        let _second = loads.begin();
        assert!(!metadata_still_wanted(should_continue(&loads, first), 7, 7));
    }
}
