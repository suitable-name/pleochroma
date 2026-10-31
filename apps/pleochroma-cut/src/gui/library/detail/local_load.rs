//! Builds the detail pane and 3D viewport from a design in the LOCAL database.
//!
//! Nothing here runs on the UI thread except applying results, and the one thing that must
//! not wait for them: `LibraryModel.selected_entry_id` is set at the click itself (the
//! selection is known then; the card highlight, Load Selected and the planner's
//! open-in-editor read it at once). [`load_diagram_detail`]
//! numbers the load and hands it to the one long-lived worker of `load_worker`:
//! one request runs at a time and at most one more waits (a newer click replaces it), so
//! clicking through many rows never piles up threads. A request has stages, and between
//! EVERY pair of them it checks [`DETAIL_LOADS`]; a superseded request returns without
//! posting anything (never as an error, never as a fallback):
//!
//! 1. the detail pane's data (read-only connection, metadata only, no attachment bytes and
//!    no image BLOB: the pane only needs to know whether an image exists), posted to the
//!    event loop, which fills the header, angle table, attachment list and shape picker;
//! 2. the full record, attachment blobs included, because the design file is an attachment;
//! 3. the plane resolution (parse the design file and run a full solve, 0.01 ms to 5.9 s);
//! 4. the planes, posted to the event loop, which draws them if still wanted.
//!
//! The solve of stage 3 has no cancel control (`resolve_catalogue_planes_with_frosting`
//! takes none), so once it has started it runs to the end and its result is then dropped
//! if stale. The bound is therefore at most one active solve plus one pending request.

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

/// Everything the detail pane reads from the database, fetched in one pass over a
/// read-only connection on the load worker, so neither the UI thread nor the shared
/// writer lock is involved and no attachment bytes or image BLOB are read: `meta` carries
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
    // `None`/an error both mean "no recorded source" (a design that was never
    // re-imported, or whose recorded source row was since deleted), matching
    // `get_derived_from_title`'s own doc comment.
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

/// Pushes the shape-picker options/current-index (`detail_header.slint`'s pencil
/// next to the "Shape:" chip, local designs only -- see
/// `library::setup_set_shape_callback`'s doc comment; recomputed on every open so
/// it always reflects this design and the library's present shape vocabulary, not
/// whatever the last-opened design left behind), the cached-curve-material
/// readout, and the derived-from note. Split out of [`apply_metadata`] purely to keep that
/// function under clippy's function-length lint.
///
/// Closes the stale-cached-curve seam: the Tilt Performance dialog's
/// "re-render with the current material?" banner
/// (`gui::tilt_profile::cached_curve_material_is_stale`, wired on
/// `MainWindow::is_cached_curve_material_stale`) needs to know what material this
/// design's cached preview/tilt-curve artifacts were actually generated under --
/// `Database::get_preview_material` is that value (set once, at first preview
/// generation, via `Database::ensure_preview_material`, and reused forever after --
/// see that method's own doc comment; a design whose file names a material has the
/// name written over instead, `Database::set_preview_material`). An empty string ("no
/// cached material on file yet") is exactly `cached_curve_material_is_stale`'s own
/// documented "never stale" case, so `unwrap_or_default` here needs no
/// special-casing.
///
/// The derived-from note is a one-line status message rather than a persistent badge: a
/// proper chip on `detail_header.slint` would need a new `DiagramDetailData` field or
/// `LibraryModel` property re-exported through `app.slint`'s `export {}` list.
///
/// The load worker keeps the cached material itself (as `ResolvedPlanes::preview_material`)
/// for `apply_catalogue_material`'s persisted-preview-material fast path, ahead of that
/// function's own refractive-index nearest-match guess -- see its doc comment.
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
        // See `pleochroma_vault::local::import_asc`: a locally-imported design's
        // `url` is a synthetic `local://<file name>` id, not a real web page --
        // gates "Open on Web"/"Copy Link" in `detail_header.slint`.
        is_local: full.url.starts_with("local://"),
        hw_ratio: format_optional_proportion(full.hw_ratio.as_deref()).into(),
        cw_ratio: format_optional_proportion(full.cw_ratio.as_deref()).into(),
        pw_ratio: format_optional_proportion(full.pw_ratio.as_deref()).into(),
        symmetry_order: full.symmetry_order.clone().unwrap_or_default().into(),
        mirror_symmetry: full.mirror_symmetry.unwrap_or(false),
    }
}

/// The angle table's rows for `full`. The sides come from each row's own
/// facet/angle/index label (see `sides_from_rows`'s own doc comment), computed over
/// the already order_idx-ordered `full.angle_settings` (`entries.rs`'s own
/// `ORDER BY order_idx ASC`).
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
            // `f.size` is SQL `length(content)`, the byte count the old
            // `content.len()` gave.
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

/// Starts loading one design from the LOCAL database into the detail pane, angle table,
/// attachments list and (once its planes are resolved) the 3D viewport. Returns at once:
/// every stage runs on the load worker and is applied from the event loop, unless a later
/// click superseded it first (see the module doc).
pub(super) fn load_diagram_detail(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
) {
    let sequence = DETAIL_LOADS.begin();
    // The selection is known at the click, so it is set now, not when stage 1 lands: the
    // card highlight, Load Selected, the planner's open-in-editor and the guide launch all
    // read `selected_entry_id` straight after asking for the selection, and none of them can
    // wait for a worker round trip. A design that then proves unreadable drops the
    // selection again ([`post_failure`]); the header and tables fill in a moment later.
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

/// Reports that `entry_id` could not be loaded: from the event loop, if `sequence` is still
/// the newest load, drops the selection [`load_diagram_detail`] made at the click (when it
/// is still on that row, and clears the pane with it) and shows `message`. So a design
/// that is gone or unreadable never stays "selected" with nothing behind it, as the
/// planner's open-in-editor relies on.
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

    // Stage 1: the detail pane's data, over a read-only connection of its own (the shared
    // writer lock is only the fallback for `:memory:`), without the image BLOB.
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

    // Stage 2: the design file is an attachment, so the planes resolution needs the full
    // record; its blobs are read here, off the UI thread, on a read-only connection.
    let full = with_read_db(&db, |db| db.get_diagram_full(entry_id));
    if !should_continue(&DETAIL_LOADS, sequence) {
        return;
    }

    // Stage 3: parse and solve. A design file that merely fails to read or solve is not
    // an error ([`crate::gui::editor::resolve_catalogue_planes`] falls back to the angle
    // table for it); only a panic or an unreadable record is. Uncancellable once started.
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

    // Shape picker, cached-curve-material readout and the derived-from note -- split into
    // a helper purely to keep this function under clippy's function-length lint; see that
    // helper's own doc comment.
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
    /// The planes, gear teeth and reference angle, with the material and girdle
    /// diameter the design file records -- or why resolving them panicked. A design file
    /// that merely fails to read or solve is not an error here:
    /// [`crate::gui::editor::resolve_catalogue_planes`] falls back to the angle table for
    /// it, the SAME resolution (design file first) the preview and tilt batches use, so
    /// this view's stone and a record's cached thumbnails and tilt curves never describe
    /// different geometry. It parses and solves the design file: it runs on the worker
    /// thread, never the UI thread. Paired with the ascending indices of the planes the
    /// design frosts ([`crate::gui::editor::resolve_catalogue_planes_with_frosting`]), so the
    /// viewport shows the finish the editor, the thumbnail and the Compare window show.
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

/// Redraws the Live Render tab's solid raster when it is the one showing, now that
/// the planes behind it have changed.
///
/// The selection callback's own redraw
/// (`gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`) runs
/// before the planes land, so it draws the previous design. This goes through
/// `ViewportModel.live_view_mode_changed` because its handler is the one place,
/// besides that callback, that holds the solid preview state and redraws from
/// `RenderContext::active_planes`; invoking it with the unchanged mode only repeats
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
