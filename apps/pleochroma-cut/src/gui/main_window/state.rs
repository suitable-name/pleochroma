//! The window's shared state: the design library, the Solid viewport's handles, the
//! persisted settings and the background render thread with its UI-thread receivers.

use super::resolve_db_path;
use crate::{
    MainWindow, RemoteWorkerModel, SolidPreviewModel, TiltModel, ViewportModel,
    bridge::render_thread::{GPU_PREPARING_STATUS, RenderContext, spawn_render_thread},
    gui::{
        custom_material_startup::initial_custom_materials_and_specific_gravity,
        editor_layout,
        optics::curve_path::tilt_curve_path,
        remote::refresh_remote_ui,
        show_toast,
        solid_preview::{
            diagram_wiring::{self, DiagramFacetOwners, DiagramHoverText, DiagramPick},
            preview_state::{
                DEFAULT_MESH_BOUNDING_RADIUS, FacetOwners, FrameGeometry, PickBuffer,
                SolidLastSolved, SolidPreviewState,
            },
        },
        solid_sink::SlintSolidSink,
        startup_settings::{
            apply_loaded_settings, refresh_lighting_preset_options, refresh_material_options,
        },
    },
    settings::{self, SettingsPersister},
};
use pleochroma::{
    color::metrics::{AsetAreaShares, GemOpticalMetrics},
    geometry::plane::GpuFacetPlane,
};
use pleochroma_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::sync::{Arc, Mutex};

/// The path tracer's or the solid worker's last-published plane arrangement, compared by
/// `Arc` pointer identity.
type ActivePlanes = Arc<Mutex<Option<Arc<Vec<GpuFacetPlane>>>>>;

/// The Solid viewport's shared handles, created together by [`create_solid_view`] and
/// handed to the callbacks that read or write them.
pub(super) struct SolidView {
    /// The last rendered frame's per-pixel facet-picking buffer.
    pub(super) pick: Arc<Mutex<Option<PickBuffer>>>,
    /// The most-recent-resolved-mast cache.
    pub(super) last_solved: SolidLastSolved,
    /// The Solid view's facet id -> hover-text table.
    pub(super) hover_text: Arc<Mutex<Vec<String>>>,
    /// The Solid view's facet id -> owning flat and concave tier tables.
    pub(super) facet_owners: Arc<Mutex<FacetOwners>>,
    /// The frame on screen's mesh geometry, camera pose and raster size.
    pub(super) geometry: Arc<Mutex<Option<FrameGeometry>>>,
    /// The solid worker's own last-published plane arrangement.
    active_planes: ActivePlanes,
    /// The path tracer's own last-published plane arrangement.
    trace_active_planes: ActivePlanes,
    /// The current solid's own bounding radius.
    pub(super) mesh_bounding_radius: Arc<Mutex<f64>>,
    /// The Edit tab's solid-inspection preview controller.
    pub(super) preview_state: Arc<SolidPreviewState>,
}

/// Opens the on-disk design library, or -- when that fails -- a throwaway in-memory one.
///
/// Runs on the UI thread before `show()`: the `Arc<Mutex<Database>>` it returns is a
/// constructor argument of every callback group wired in `build_main_window`. Only the open
/// itself (pragmas and the create-if-missing/migration pass) runs here; the library list,
/// filter options, tags and counts load on a worker.
///
/// # Errors
///
/// Returns an error only when the in-memory fallback fails to open too.
pub(super) fn open_design_library(ui: &MainWindow) -> anyhow::Result<Arc<Mutex<Database>>> {
    // If the on-disk database cannot be opened (locked, corrupt, unwritable folder), fall back
    // to an in-memory one rather than panicking: `Database::new` runs the same schema and
    // migration chain on `:memory:`, so the library works, just unsaved. The error toast stays
    // until dismissed, because a status-bar message could be missed.
    let db_path = resolve_db_path();
    // A first start creates the library, and its per-user folder may not exist yet.
    if let Some(dir) = std::path::Path::new(&db_path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        tracing::warn!("could not create the library folder {}: {e}", dir.display());
    }
    let db = match Database::new(Some(&db_path)) {
        Ok(db_inst) => Arc::new(Mutex::new(db_inst)),
        Err(e) => {
            match Database::new(Some(":memory:")) {
                Ok(memory_db) => {
                    show_toast(
                        ui,
                        &format!(
                            "Could not open the design library at {db_path} ({e}) -- running \
                             in a temporary, unsaved library for this session."
                        ),
                        "error",
                    );
                    Arc::new(Mutex::new(memory_db))
                }
                Err(memory_err) => {
                    // Both the real database AND an in-memory SQLite connection
                    // failed to open -- SQLite itself is unusable in this process
                    // (e.g. no SQLite support compiled in, or the process is out of
                    // memory). There is no further degraded mode to fall back to, so
                    // report both errors and bail out of window construction instead
                    // of panicking.
                    return Err(anyhow::anyhow!(
                        "failed to open design library at {db_path:?} ({e}), and the \
                         in-memory fallback database also failed to open ({memory_err})"
                    ));
                }
            }
        }
    };
    Ok(db)
}

/// Creates the Solid viewport's shared handles and its preview controller, and wires the
/// Diagram mode's hover/click callbacks onto them.
pub(super) fn create_solid_view(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> SolidView {
    // The Edit tab's solid-inspection preview controller (`gui::solid_preview::preview_state`).
    // Its worker thread starts on the first `request_redraw`, so a session that never opens the
    // Solid view never pays for one.
    //
    // The last frame's facet-picking buffer and the most-recently-resolved mast. Written by
    // `SlintSolidSink::apply` and read by the editor's hover, click and edit callbacks; plain
    // shared handles, so the sink can write them from the worker's callback.
    let solid_pick: Arc<Mutex<Option<PickBuffer>>> = Arc::new(Mutex::new(None));
    let solid_last_solved: SolidLastSolved = Arc::new(Mutex::new(None));
    // Diagram mode's own pick buffer and hover and tier tables, kept apart from the Solid
    // view's (see `SlintSolidSink`).
    let diagram_pick: DiagramPick = Arc::new(Mutex::new(None));
    // The index wheel's tooth pick buffer (see `SlintSolidSink::diagram_tooth_pick`).
    let diagram_tooth_pick: DiagramPick = Arc::new(Mutex::new(None));
    // Which panel painted each diagram pixel, read by the double-click that enlarges a panel.
    let diagram_panel_pick: DiagramPick = Arc::new(Mutex::new(None));
    let diagram_hover_text: DiagramHoverText = Arc::new(Mutex::new(None));
    let diagram_facet_owners: DiagramFacetOwners = Arc::new(Mutex::new(None));
    // The Solid view's facet id -> hover-text and owning-tier tables, from every frame. Read by
    // the hover and click callbacks in `gui::editor::callbacks::tier_actions::solid_picking`
    // through `SolidPickState`.
    let solid_hover_text: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let solid_facet_owners: Arc<Mutex<FacetOwners>> = Arc::new(Mutex::new(FacetOwners::default()));
    // The path tracer's and the solid worker's last-published plane arrangements, compared by
    // `Arc` identity (see `planes_generations_match`). The render thread's frame-push closure
    // also writes `trace_active_planes`.
    let solid_active_planes: ActivePlanes = Arc::new(Mutex::new(None));
    let trace_active_planes: ActivePlanes = Arc::new(Mutex::new(None));
    // The current solid's bounding radius, written by each frame (`SlintSolidSink::apply`) and
    // read by the camera's orbit-zoom clamp and "Fit" pose. Starts at the frame fallback.
    let mesh_bounding_radius: Arc<Mutex<f64>> = Arc::new(Mutex::new(DEFAULT_MESH_BOUNDING_RADIUS));
    // The frame on screen's mesh geometry, camera pose and raster size, written with
    // `solid_pick` and read by the manipulation handles through `SolidPickState::geometry`.
    let solid_geometry: Arc<Mutex<Option<FrameGeometry>>> = Arc::new(Mutex::new(None));
    // The sink is built first and the controller takes it, so the sink learns its controller
    // afterwards (`SlintSolidSink::preview_state`).
    let sink_preview_state: Arc<Mutex<std::sync::Weak<SolidPreviewState>>> =
        Arc::new(Mutex::new(std::sync::Weak::new()));
    let solid_preview_state = SolidPreviewState::new(Arc::new(SlintSolidSink {
        preview_state: Arc::clone(&sink_preview_state),
        ui: ui.as_weak(),
        pick: Arc::clone(&solid_pick),
        last_solved: Arc::clone(&solid_last_solved),
        diagram_pick: Arc::clone(&diagram_pick),
        diagram_tooth_pick: Arc::clone(&diagram_tooth_pick),
        diagram_panel_pick: Arc::clone(&diagram_panel_pick),
        diagram_hover_text: Arc::clone(&diagram_hover_text),
        diagram_facet_owners: Arc::clone(&diagram_facet_owners),
        hover_text: Arc::clone(&solid_hover_text),
        facet_owners: Arc::clone(&solid_facet_owners),
        render_ctx: Arc::clone(render_ctx),
        solid_active_planes: Arc::clone(&solid_active_planes),
        trace_active_planes: Arc::clone(&trace_active_planes),
        mesh_bounding_radius: Arc::clone(&mesh_bounding_radius),
        geometry: Arc::clone(&solid_geometry),
    }));
    *sink_preview_state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&solid_preview_state);
    // Diagram mode's hover and click callbacks. Unlike the Solid view's, they need no
    // `gui::editor` access (see `solid_preview::diagram_wiring`); they drive
    // `SolidPreviewState::request_facet_overlay`.
    diagram_wiring::setup_diagram_hover_and_click_callbacks(
        ui,
        &diagram_pick,
        &diagram_tooth_pick,
        &diagram_hover_text,
        &diagram_facet_owners,
        &solid_preview_state,
    );
    // Double-click on a diagram panel: enlarge it, or go back to all three.
    diagram_wiring::setup_diagram_double_click_callback(ui, &diagram_panel_pick);
    SolidView {
        pick: solid_pick,
        last_solved: solid_last_solved,
        hover_text: solid_hover_text,
        facet_owners: solid_facet_owners,
        geometry: solid_geometry,
        active_planes: solid_active_planes,
        trace_active_planes,
        mesh_bounding_radius,
        preview_state: solid_preview_state,
    }
}

/// Loads the custom gemstone materials from the library into the render context and the
/// material picker.
pub(super) fn apply_custom_materials(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // `pleochroma-vault` returns plain `CustomMaterialRow`s (it does not depend on
    // `pleochroma`), so they are converted to `GemMaterial` here.
    //
    // Synchronous: `load_settings` checks the persisted material choice against
    // `RenderContext::custom_materials`, so the rows must be in first. The library list and
    // filter options load on a worker instead (`load_filter_options_and_initial_list`).
    let custom_material_rows = db
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_custom_materials()
        .unwrap_or_default();
    let (initial_custom_mats, initial_custom_sg) =
        initial_custom_materials_and_specific_gravity(&custom_material_rows);
    // Drop the zoning rows of owners that are gone, then put the stored zones back on the
    // materials that have them, so every later `resolve_material` returns a zoned material. A
    // relative-to-stone material is scaled to the stone width (`SettingsModel.stone_width_mm`,
    // `0` = the default at start-up); `material_zones::stone_width_applied` re-scales it when
    // the width changes.
    let initial_custom_mats = {
        crate::gui::rough_colour::material_zones::register_vault(db);
        let mut materials = initial_custom_mats;
        let width_mm = f64::from(ui.global::<crate::SettingsModel>().get_stone_width_mm());
        let guard = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(error) = crate::gui::rough_colour::store::prune_orphans(&guard) {
            tracing::warn!("Rough colour: orphaned rows were not pruned: {error:#}");
        }
        let report = crate::gui::rough_colour::store::attach_zoning_reporting(
            &guard,
            &mut materials,
            width_mm,
        );
        drop(guard);
        // A damaged zone row leaves its material in the base colour: say so (the first one).
        if let Some((name, sentence)) = report.skipped.first() {
            let text = if name.is_empty() {
                sentence.clone()
            } else {
                format!(
                    "The zones of '{name}' could not be read; it renders in its base color. \
                     ({sentence})"
                )
            };
            show_toast(ui, &text, "warning");
        }
        materials
    };
    {
        let mut ctx = RenderContext::lock(render_ctx);
        ctx.custom_materials = Arc::new(initial_custom_mats.clone());
        ctx.custom_material_specific_gravity = Arc::new(initial_custom_sg);
        ctx.custom_material_gem_data = Arc::new(
            crate::gui::custom_material_startup::initial_custom_gem_data(&custom_material_rows),
        );
        ctx.custom_material_physics = Arc::new(
            crate::gui::optics::crystal_optics::custom_material_physics_from_rows(
                &custom_material_rows,
            ),
        );
        ctx.custom_material_fluorescence = Arc::new(
            crate::gui::optics::crystal_optics::custom_material_fluorescence_from_rows(
                &custom_material_rows,
            ),
        );
    }
    refresh_material_options(ui, &initial_custom_mats);
}

/// Loads the persisted settings, applies them to the render context and the UI, and
/// starts the debounced background writer every settings-changing callback feeds.
pub(super) fn load_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Arc<SettingsPersister> {
    // Settings and lighting presets. A missing, unreadable or corrupt file yields the defaults.
    // The values are applied to `render_ctx` and the UI before the render thread starts, then
    // handed to the debounced background writer that every settings-changing callback feeds.
    let settings_path = settings::store::default_settings_path();
    // Copies a legacy settings file into place when none exists yet. Must run before
    // `load_with_outcome` reads `settings_path` (see `migrate_legacy_settings_if_needed`).
    settings::store::migrate_legacy_settings_if_needed(&settings_path);
    let (loaded_settings, settings_load_outcome) =
        settings::store::load_with_outcome(&settings_path);
    // A corrupt settings file is renamed aside before the defaults load (see
    // `load_with_outcome`), and this toast reports it. A warning rather than an error: the app
    // itself started fine.
    if let settings::store::SettingsLoadOutcome::Corrupt { renamed_to } = &settings_load_outcome {
        show_toast(
            ui,
            &format!(
                "Your settings file could not be read and was reset to defaults. The \
                 previous file was kept at {}.",
                renamed_to.display()
            ),
            "warning",
        );
    }
    apply_loaded_settings(ui, render_ctx, &loaded_settings);
    // The Edit sub-tab's dock width, inspector height and collapsed sections. Runs before
    // `setup_editor_layout_callbacks` (see `editor_layout::apply_editor_layout_from_settings`).
    editor_layout::apply_editor_layout_from_settings(ui, &loaded_settings.settings);
    refresh_lighting_preset_options(ui, &loaded_settings.presets);
    // Remote rendering: the "Remote coordinator" form and every transfer default,
    // restored from the last saved configuration (a legacy multi-worker list was already
    // folded into the single endpoint by `load_or_default`).
    refresh_remote_ui(ui, loaded_settings.settings.remote.as_ref());
    ui.global::<RemoteWorkerModel>()
        .set_denoise_enabled(loaded_settings.settings.denoise_enabled);
    let settings_store = Arc::new(SettingsPersister::spawn(settings_path, loaded_settings));
    // Installed before any editor callback is wired: the recent-file list and the "don't ask
    // again" choices are written from call sites without a settings handle. Every write must go
    // through the persister, because `flush()` on close overwrites the file with its snapshot.
    // See `SettingsPersister::install_for_this_thread`.
    SettingsPersister::install_for_this_thread(&settings_store);
    settings_store
}

/// The Zones (ASET-style) bar parts of `shares` in percent: red, green, blue and dark (Leak plus
/// Lost), the order `ViewportModel.aset_shares` carries.
fn zone_parts(shares: &AsetAreaShares) -> [f32; 4] {
    [
        shares.red_pct,
        shares.green_pct,
        shares.blue_pct,
        shares.dark_pct(),
    ]
}

/// The line under the zones legend that splits the Dark share, as the card prints it:
/// `Dark = Leak 9 % + Lost 12 %`.
fn zone_split_text(shares: &AsetAreaShares) -> String {
    format!(
        "Dark = Leak {:.0} % + Lost {:.0} %",
        shares.leak_pct, shares.lost_pct
    )
}

/// The Zones (ASET-style) line of the copied figures text:
/// `Zones (ASET-style): Red 44.0% · Green 19.0% · Blue 16.1% · Dark 20.9% (Leak 9.0%, Lost 11.9%)`.
fn zone_copy_line(shares: &AsetAreaShares) -> String {
    format!(
        "Zones (ASET-style): Red {:.1}% \u{b7} Green {:.1}% \u{b7} Blue {:.1}% \u{b7} Dark {:.1}% (Leak {:.1}%, Lost {:.1}%)",
        shares.red_pct,
        shares.green_pct,
        shares.blue_pct,
        shares.dark_pct(),
        shares.leak_pct,
        shares.lost_pct
    )
}

/// Pushes the Contrast figure and the Zones (ASET-style) shares to `ViewportModel`; `None` (not
/// measured) hides the Contrast row and the zones block. The shares model is replaced only when a
/// value changed, because every displayed frame carries the same figures again.
fn push_zone_figures(ui: &MainWindow, shares: Option<&AsetAreaShares>) {
    let viewport = ui.global::<ViewportModel>();
    let Some(shares) = shares else {
        viewport.set_contrast(-1.0);
        if viewport.get_aset_shares().row_count() != 0 {
            viewport.set_aset_shares(ModelRc::default());
            viewport.set_aset_split_text(slint::SharedString::new());
            viewport.set_aset_copy_text(slint::SharedString::new());
        }
        return;
    };
    viewport.set_contrast(shares.contrast);
    let parts = zone_parts(shares);
    let split = slint::SharedString::from(zone_split_text(shares));
    let copy = slint::SharedString::from(zone_copy_line(shares));
    let current = viewport.get_aset_shares();
    let unchanged = current.row_count() == parts.len()
        && viewport.get_aset_split_text() == split
        && viewport.get_aset_copy_text() == copy
        && parts.iter().enumerate().all(|(index, part)| {
            current
                .row_data(index)
                .is_some_and(|shown| shown.to_bits() == part.to_bits())
        });
    if !unchanged {
        viewport.set_aset_shares(ModelRc::new(VecModel::from(parts.to_vec())));
        viewport.set_aset_split_text(split);
        viewport.set_aset_copy_text(copy);
    }
}

/// The three 19-point tilt graphs (brilliance, extinction, windowing) one metrics push carries.
type TiltGraphs = ([f32; 19], [f32; 19], [f32; 19]);

/// Whether two graph triples are bit-for-bit equal (so `-0.0` and NaN payloads count as
/// changes, never as false matches).
fn same_graph_bits(a: &TiltGraphs, b: &TiltGraphs) -> bool {
    let bits_equal = |x: &[f32; 19], y: &[f32; 19]| {
        x.iter()
            .zip(y)
            .all(|(left, right)| left.to_bits() == right.to_bits())
    };
    bits_equal(&a.0, &b.0) && bits_equal(&a.1, &b.1) && bits_equal(&a.2, &b.2)
}

/// Sets the three tilt graphs and their curve paths on `TiltModel`, only when they differ
/// (bitwise) from `last_graphs`. Each set rebuilds three `VecModel`s and three path strings,
/// and the suspended-tab tick and every displayed frame carry the same graphs again.
fn push_tilt_graphs(ui: &MainWindow, last_graphs: &Mutex<Option<TiltGraphs>>, graphs: TiltGraphs) {
    let changed = {
        let mut last = last_graphs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = last
            .as_ref()
            .is_none_or(|previous| !same_graph_bits(previous, &graphs));
        if changed {
            *last = Some(graphs);
        }
        changed
    };
    if !changed {
        return;
    }
    let (graph_brilliance, graph_extinction, graph_windowing) = graphs;
    ui.global::<TiltModel>()
        .set_graph_brilliance(slint::ModelRc::new(slint::VecModel::from(
            graph_brilliance.to_vec(),
        )));
    ui.global::<TiltModel>()
        .set_graph_extinction(slint::ModelRc::new(slint::VecModel::from(
            graph_extinction.to_vec(),
        )));
    ui.global::<TiltModel>()
        .set_graph_windowing(slint::ModelRc::new(slint::VecModel::from(
            graph_windowing.to_vec(),
        )));
    // Tilt-curve `Path::commands` strings for the performance graph dialog's line chart (see
    // `curve_path::tilt_curve_path`'s doc comment).
    ui.global::<TiltModel>()
        .set_graph_brilliance_path(tilt_curve_path(&graph_brilliance).into());
    ui.global::<TiltModel>()
        .set_graph_extinction_path(tilt_curve_path(&graph_extinction).into());
    ui.global::<TiltModel>()
        .set_graph_windowing_path(tilt_curve_path(&graph_windowing).into());
}

/// Spawns the background spectral ray tracer and the UI-thread closures that receive its
/// frames, metrics and status texts.
pub(super) fn spawn_viewport_render_thread(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid: &SolidView,
) {
    crate::bridge::optical_overlay::setup(ui, render_ctx);
    // The GPU kernels compile in the background from now on, Ray-Traced first; the viewport
    // renders on the backends that build warms.
    crate::bridge::shader_cache::start(ui, render_ctx);
    // Spawn Background Multi-Threaded Physically Based Spectral Raytracer
    let ui_weak_render = ui.as_weak();
    let render_ctx_frame_push = render_ctx.clone();
    let trace_active_planes_push = Arc::clone(&solid.trace_active_planes);
    let solid_active_planes_push = Arc::clone(&solid.active_planes);
    // The tilt graphs last set on the UI (see the metrics closure below). An `Arc<Mutex<_>>`
    // because the closure must be `Send + Clone`; it is only ever locked on the UI thread.
    let last_graphs: Arc<Mutex<Option<TiltGraphs>>> = Arc::default();
    spawn_render_thread(
        ui_weak_render,
        render_ctx.clone(),
        move |ui: &MainWindow,
              img: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
              scene_generation: u64| {
            // Every frame is shown, even one whose scene generation has since moved on: during a
            // drag the generation changes faster than frames are traced, so dropping stale
            // frames would freeze the picture. The overlay keeps its own staleness gate.
            crate::bridge::optical_overlay::displayed(
                ui,
                scene_generation,
                img.width(),
                img.height(),
            );
            // The view's background takes the border color of the frame, so no bars show.
            crate::gui::render::backdrop_color::push_backdrop(
                ui,
                img.as_bytes(),
                img.width(),
                img.height(),
            );
            ui.global::<ViewportModel>()
                .set_render_image(crate::bridge::pixel_buffer::rgba_image(img));
            ui.global::<ViewportModel>().set_has_render(true);
            // Remember the planes this frame was traced with (by `Arc` identity) and update
            // `SolidPreviewModel.trace_matches_solid` against the solid worker's planes. Runs on
            // the UI thread, like the other callbacks here that lock `render_ctx`.
            let planes_now = render_ctx_frame_push
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .active_planes
                .clone();
            *trace_active_planes_push
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(planes_now);
            ui.global::<SolidPreviewModel>().set_trace_matches_solid(
                crate::gui::solid_sink::planes_generations_match(
                    &trace_active_planes_push,
                    &solid_active_planes_push,
                ),
            );
        },
        move |ui: &MainWindow,
              metrics: GemOpticalMetrics,
              graph_brilliance: [f32; 19],
              graph_extinction: [f32; 19],
              graph_windowing: [f32; 19],
              cam_pitch_deg: f32| {
            ui.global::<ViewportModel>()
                .set_brilliance_pct(metrics.brilliance_pct);
            ui.global::<ViewportModel>()
                .set_fire_index(metrics.fire_index);
            ui.global::<ViewportModel>()
                .set_scintillation_pct(metrics.scintillation_pct);
            ui.global::<ViewportModel>()
                .set_windowing_pct(metrics.windowing_pct);
            ui.global::<ViewportModel>()
                .set_extinction_pct(metrics.extinction_pct);
            push_zone_figures(ui, metrics.aset.as_ref());
            crate::gui::render::crystal_axis_ui::push_doubling(ui, metrics.doubling_arcmin);
            push_tilt_graphs(
                ui,
                &last_graphs,
                (graph_brilliance, graph_extinction, graph_windowing),
            );
            ui.global::<TiltModel>().set_cam_pitch_deg(cam_pitch_deg);
        },
        // Pushes `ViewportGpu`'s self-healing status text
        // (empty while healthy) to `ViewportModel.gpu_status_text` -- see
        // `spawn_render_thread`'s own `update_gpu_status` doc comment. Preparing the GPU
        // renderer is no fault, so it stays out of the graphics-status banner; the render stage
        // says the trace is being prepared.
        |ui: &MainWindow, text: slint::SharedString| {
            let shown = if text.as_str() == GPU_PREPARING_STATUS {
                slint::SharedString::new()
            } else {
                text
            };
            ui.global::<ViewportModel>().set_gpu_status_text(shown);
        },
        // Pushes `RenderContext::material_unresolved`'s cutter-facing reason (empty
        // once resolved) to `ViewportModel.trace_refusal`.
        |ui: &MainWindow, text: slint::SharedString| {
            ui.global::<ViewportModel>().set_trace_refusal(text);
        },
        // Pushes the non-blocking trace note (the Stone Shadow's and Tray Reflection's soft
        // notes) to `ViewportModel.trace_note` -- see `spawn_render_thread`'s `update_trace_note`.
        |ui: &MainWindow, text: slint::SharedString| {
            ui.global::<ViewportModel>().set_trace_note(text);
        },
    );
}

#[cfg(test)]
mod graph_tests {
    use super::*;

    fn shares(leak: f32, lost: f32) -> AsetAreaShares {
        AsetAreaShares {
            red_pct: 44.0,
            green_pct: 19.0,
            blue_pct: 16.0,
            leak_pct: leak,
            lost_pct: lost,
            contrast: 27.0,
        }
    }

    #[test]
    fn the_zone_bar_parts_end_with_the_dark_share() {
        let parts = zone_parts(&shares(9.0, 12.0));
        assert_eq!(parts, [44.0, 19.0, 16.0, 21.0]);
    }

    #[test]
    fn the_copy_line_carries_every_zone_with_one_decimal() {
        assert_eq!(
            zone_copy_line(&shares(9.04, 11.86)),
            "Zones (ASET-style): Red 44.0% \u{b7} Green 19.0% \u{b7} Blue 16.0% \u{b7} Dark 20.9% (Leak 9.0%, Lost 11.9%)"
        );
    }

    #[test]
    fn the_dark_split_line_reads_as_the_card_prints_it() {
        assert_eq!(
            zone_split_text(&shares(9.04, 11.86)),
            "Dark = Leak 9 % + Lost 12 %"
        );
    }

    #[test]
    fn graph_triples_compare_bitwise() {
        let a: TiltGraphs = ([1.0; 19], [2.0; 19], [3.0; 19]);
        assert!(same_graph_bits(&a, &a));
        let mut b = a;
        b.2[18] = 3.000_000_5;
        assert!(!same_graph_bits(&a, &b));
        let mut zero = a;
        zero.0[0] = 0.0;
        let mut negative_zero = a;
        negative_zero.0[0] = -0.0;
        assert!(!same_graph_bits(&zero, &negative_zero));
    }
}
