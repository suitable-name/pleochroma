//! Off-UI-thread computation of the Tilt Performance dialog's full-axis (±90°, 1° step)
//! sweeps for all four `pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG` axes.
//!
//! One sample point (`evaluate_gem_optical_metrics`, round brilliant/diamond, release,
//! single-threaded) measures ~1.875 ms, so one full single-threaded sweep (181 points on each
//! of four axes) costs ~1.36 seconds. The sweep is threaded and skips the Fire/scintillation
//! tracing it never reads, so it finishes well under that on a multi-core machine. The
//! single-thread cost is still a visible wait on small machines, which is why:
//!
//! - This work runs on its own `std::thread::spawn` worker, never the UI thread.
//! - `AxesCacheKey`/`launched_key` short-circuit re-opening the dialog (or a `changed`
//!   re-fire with unchanged inputs) against a second ~1.4s sweep. `AxesCacheKey` keys on the
//!   fully resolved [`GemMaterial`], not its display name: a custom material can be edited
//!   (RI, birefringence, dispersion, ...) while a tier keeps its name, and a name-only key would
//!   show curves for a material the stone no longer has. [`should_recompute_axes`] is the pure
//!   dedup decision.
//! - The `generation` counter abandons a stale in-flight sweep: a superseded sweep landing
//!   after the user changed material, orbited or reopened against other geometry would be a
//!   visible glitch.
//! - `PerformanceGraphDialog.extra_axes_loading` shows for the whole sweep, which takes over
//!   a second.
//!
//! All four axes are swept the same way, axis 0 included, rather than reusing the live render
//! thread's positive-half value (`graph_brilliance` and friends, still computed every frame for
//! the Parameters panel's mini tilt chart): that value cannot supply the negative half a
//! full-axis sweep needs. One code path, one cache key and one generation counter avoid reusing
//! a value that may be mid-update, at the cost of one redundant positive-half sweep of axis 0
//! (~12% of the ~724 raytrace evaluations per run).
//!
//! The curves are scored under the tilt standard
//! ([`pleochroma_cut_core::optimize::canonical_environment`]: the grading preset at the canonical
//! light pose with the default 16-degree head shadow), never under the viewport's lighting, light
//! pose, head-shadow size or HDR map. So the curves of two designs compare, they equal the
//! library's stored curves for the same stone, and changing the view does not re-launch a sweep.
//! The cache key is therefore only the stone and its resolved material.
//!
//! The sweep honours frosting: the scene snapshot's `facet_finishes` (the frosted-girdle toggle
//! and the design's authored frosting, empty for a polished stone) go to the sweep as a
//! `FinishedStone` and into the cache key ([`stone_key`]), so the curves describe the frosted
//! stone the viewport draws and a frosting change re-launches them.

#[cfg(test)]
use crate::bridge::render_thread::resolve_material_with_override;
use crate::{
    ActivityModel, MainWindow, TiltModel, bridge::render_thread::RenderContext,
    gui::optics::curve_path::full_axis_curve_path, settings::SettingsPersister,
};
use pleochroma::{
    color::metrics::{FinishedStone, evaluate_all_axes_profiles_finished_threaded},
    geometry::{
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{materials::GemMaterial, raytracer::FacetFinish},
    render_setup::hash_geometry,
};
use pleochroma_cut_core::optimize::canonical_environment;
use pleochroma_solid::preview::StoneGeometryBuf;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

/// Cheap identity for "would recomputing the four full-axis sweeps produce a different
/// result": the stone and its resolved material. The lighting is the fixed tilt standard, so no
/// view setting is part of it (see this module's doc comment).
#[derive(Clone, PartialEq)]
struct AxesCacheKey {
    /// The fully resolved material (`resolve_material`'s output), not its display name:
    /// `GemMaterial`'s field-by-field `PartialEq` makes an edit to a custom material's optics
    /// under the same name a cache-key change.
    material: GemMaterial,
    planes_hash: u64,
}

/// Whether a fresh four-axis sweep for `requested` is needed, given what was last launched
/// (`current`). `true` means the ~1.36s sweep must run; `false` means the `graph_*_extra_axes`
/// data already describes exactly this material and geometry. Kept pure so it is testable.
#[must_use]
fn should_recompute_axes(current: Option<&AxesCacheKey>, requested: &AxesCacheKey) -> bool {
    current != Some(requested)
}

/// The identity of the stone a sweep describes: [`hash_geometry`] of its planes and tools,
/// extended by which facets are frosted when any are.
///
/// A polished stone (an empty finish list) keeps exactly the [`hash_geometry`] key it always
/// had; frosting a facet, or turning the frosted girdle on, changes the key, because a frosted
/// facet scatters light and so changes the curves.
#[must_use]
fn stone_key(geom: StoneGeometry<'_>, finishes: &[FacetFinish]) -> u64 {
    let geometry = hash_geometry(geom);
    if finishes.is_empty() {
        return geometry;
    }
    let mut hasher = DefaultHasher::new();
    geometry.hash(&mut hasher);
    finishes.len().hash(&mut hasher);
    for finish in finishes {
        (*finish == FacetFinish::Frosted).hash(&mut hasher);
    }
    hasher.finish()
}

/// One metric's curve for every `PROFILE_AZIMUTHS_DEG` axis, in axis order: four rows, each a
/// full 181-point ±90° profile.
pub(in crate::gui) type AxisCurveRows = Vec<[f32; 181]>;

/// Sweeps all four [`pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG`] axes, each a full
/// 181-point ±90° profile,
/// returning `(brilliance, extinction, windowing)` rows in axis order.
///
/// `stone` carries the facet finishes of the stone on screen (the frosted-girdle toggle and the
/// design's authored frosting), so the curves describe the frosted stone the viewport shows.
/// Scored under the tilt standard ([`canonical_environment`]), whatever the viewport shows.
pub(in crate::gui) fn sweep_all_axes(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
) -> (AxisCurveRows, AxisCurveRows, AxisCurveRows) {
    // The sweep itself lives in `pleochroma::color::metrics` (the browser app's Worker
    // runs the same one); this only splits its axes into the dialog's three row sets.
    let axes = evaluate_all_axes_profiles_finished_threaded(
        stone,
        material,
        canonical_environment(),
        None,
    );
    let mut brilliance_rows: AxisCurveRows = Vec::with_capacity(axes.len());
    let mut extinction_rows: AxisCurveRows = Vec::with_capacity(axes.len());
    let mut windowing_rows: AxisCurveRows = Vec::with_capacity(axes.len());
    for axis in axes {
        brilliance_rows.push(axis.brilliance);
        extinction_rows.push(axis.extinction);
        windowing_rows.push(axis.windowing);
    }
    (brilliance_rows, extinction_rows, windowing_rows)
}

/// Handles `on_request_tilt_profile_axes`: checks `launched_key` for a duplicate request, then
/// spawns the background sweep and pushes its results back to the UI thread once done (a result
/// superseded by a newer request via `generation` is dropped). See
/// [`setup_tilt_profile_callback`] for the full behaviour.
fn handle_request_tilt_profile_axes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    launched_key: &Arc<Mutex<Option<AxesCacheKey>>>,
    completed_key: &Arc<Mutex<Option<AxesCacheKey>>>,
    generation: &Arc<AtomicU64>,
) {
    // The curves describe the FINISHED gem, whatever the Cut slider says: they are printed
    // on the tilt video, which shows the finished gem (`gui::editor::finished_stone`). Asked
    // for before the context is locked below -- it takes that lock itself. With the slider
    // cut back and no current solve in the editor's cache, the stone comes from the solve
    // worker and the rest of this request runs when it lands (never a solve on this
    // thread); the request is the same one, so the dedup keys below still apply.
    let render_ctx_later = Arc::clone(render_ctx);
    let launched_key = Arc::clone(launched_key);
    let completed_key = Arc::clone(completed_key);
    let generation = Arc::clone(generation);
    crate::gui::editor::finished_stone_then(
        ui,
        render_ctx,
        |_| {},
        move |ui, finished| match axes_step(finished) {
            AxesStep::Sweep(finished) => request_axes_for_stone(
                ui,
                &render_ctx_later,
                &launched_key,
                &completed_key,
                &generation,
                finished.as_ref(),
            ),
            AxesStep::MarkStale => ui.global::<TiltModel>().set_curves_stale(true),
            AxesStep::Skip => {}
        },
    );
}

/// What a tilt-curve request does once the finished stone's outcome is known.
#[derive(Debug, PartialEq)]
enum AxesStep {
    /// Dedup against the last launch and sweep this stone (`None`: the render context
    /// already holds the finished stone).
    Sweep(Option<StoneGeometryBuf>),
    /// There is no finished stone to sweep (the design does not solve): the curves on screen
    /// no longer describe the design, so say so. The viewport's own banner tells the cutter
    /// why.
    MarkStale,
    /// The design changed while its stone was being solved. The newer edit's own viewport
    /// push has already asked for ITS sweep; launching one for the superseded design would
    /// bump the sweep generation and make that newer sweep land as "superseded", leaving the
    /// dialog showing the curves of the design one edit ago as if they were current. Does
    /// nothing at all, so the generation counter is not touched. The same for a solve the
    /// worker displaced for a newer request: no stone came back, and the next viewport push
    /// asks again.
    Skip,
}

/// The decision of [`handle_request_tilt_profile_axes`]' continuation, pure so it can be
/// tested without a window.
fn axes_step(finished: crate::gui::editor::Finished) -> AxesStep {
    use crate::gui::editor::Withheld;
    match finished {
        Ok(stone) => AxesStep::Sweep(stone),
        Err(Withheld::Stale | Withheld::Displaced) => AxesStep::Skip,
        Err(Withheld::Unsolvable(_)) => AxesStep::MarkStale,
    }
}

/// The material the tilt sweep traces: the resolved one (`resolve_material_with_override`),
/// sized by the stone-size rule of the render's own override step
/// (`render_setup::material_for_stone`), so the curves are scored in the absorption the render
/// shows and a sized Emerald sweeps as dark as it renders. `None` when nothing resolves.
///
/// Test-only reference: the sweep itself traces the material that
/// `SceneSnapshot::capture_finished` resolved (the same override stack), and the test pins that
/// the two agree.
#[cfg(test)]
fn tilt_sweep_material(
    custom_materials: &[GemMaterial],
    material_override: Option<&GemMaterial>,
    material_name: &str,
    stone_width_mm: f32,
    planes: &[GpuFacetPlane],
) -> Option<GemMaterial> {
    let material = resolve_material_with_override(
        &GemMaterial::all_materials(),
        custom_materials,
        material_override,
        material_name,
    )?;
    Some(pleochroma::render_setup::material_for_stone(
        material,
        stone_width_mm,
        planes,
    ))
}

/// [`handle_request_tilt_profile_axes`]' second half: the dedup check and the sweep launch,
/// for the render context's stone with `finished` (the finished stone, when the Cut slider
/// has the design cut back) swapped in.
fn request_axes_for_stone(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    launched_key: &Arc<Mutex<Option<AxesCacheKey>>>,
    completed_key: &Arc<Mutex<Option<AxesCacheKey>>>,
    generation: &Arc<AtomicU64>,
    finished: Option<&StoneGeometryBuf>,
) {
    let scene =
        match crate::bridge::export_thread::SceneSnapshot::capture_finished(render_ctx, finished) {
            Ok(scene) => scene,
            Err(reason) => {
                // The curves on screen no longer describe this design, so say so instead of
                // dropping the request silently.
                tracing::warn!(%reason, "tilt curves: the scene could not be captured");
                ui.global::<TiltModel>().set_curves_stale(true);
                return;
            }
        };
    let planes = Arc::new(scene.active_planes);
    let tools = Arc::new(scene.tools);
    let facet_finishes = Arc::new(scene.facet_finishes);
    let material = scene.material;
    let key = AxesCacheKey {
        material: material.clone(),
        // `hash_geometry` is `hash_planes` for a planar stone, so its keys are unchanged; a
        // frosted stone's finishes are mixed in (`stone_key`), so frosting a facet or toggling
        // the frosted girdle re-launches the sweep.
        planes_hash: stone_key(
            StoneGeometry {
                planes: &planes,
                tools: &tools,
            },
            &facet_finishes,
        ),
    };
    // Whether the curves already on screen (`completed_key`, the last sweep that
    // actually FINISHED and pushed `graph_*` rows -- as opposed to `launched_key`
    // below, which flips as soon as a sweep merely STARTS) still describe these
    // exact inputs. Pushed unconditionally, even when the dedup check just below
    // decides nothing needs to run: a stale curve stays stale until a fresh sweep
    // actually completes, it doesn't clear itself just because nothing new was
    // launched this time.
    let stale_now = should_recompute_axes(
        completed_key
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref(),
        &key,
    );
    ui.global::<TiltModel>().set_curves_stale(stale_now);
    {
        let mut launched = launched_key.lock().unwrap();
        if !should_recompute_axes(launched.as_ref(), &key) {
            // Already computed (or currently computing) for these exact inputs.
            return;
        }
        *launched = Some(key.clone());
    }

    let my_generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
    let generation = generation.clone();
    ui.global::<TiltModel>().set_extra_axes_loading(true);
    // This ~1.36s sweep reports no completion fraction (see the module doc
    // comment), so it registers indeterminate -- `ui/models/activity.slint`'s own
    // doc comment on `start_external`/`finish_external` explains why this crosses
    // through the Slint global rather than `gui::editor::activity::
    // ActivityRegistry` directly (this module is outside `gui::editor`'s module
    // tree). Not cancellable: the sweep has no cancel probe of its own, only
    // supersession via `generation`.
    let activity_id = ui.global::<ActivityModel>().invoke_start_external(
        "tilt_curve".into(),
        "Computing tilt curves".into(),
        false,
    );
    let ui_weak_bg = ui.as_weak();

    spawn_tilt_profile_sweep(
        SweepRequest {
            planes,
            tools,
            finishes: facet_finishes,
            material,
            key,
        },
        SweepBookkeeping {
            generation,
            my_generation,
            completed_key: Arc::clone(completed_key),
            activity_id,
        },
        ui_weak_bg,
    );
}

/// The per-sweep inputs of [`spawn_tilt_profile_sweep`], bundled to keep its signature short;
/// see [`SweepBookkeeping`] for the rest.
struct SweepRequest {
    planes: Arc<Vec<GpuFacetPlane>>,
    /// The concave tools cut out of `planes`; empty for a planar stone.
    tools: Arc<Vec<ToolPrimitive>>,
    /// The per-facet finishes of the stone on screen; empty when no facet is frosted.
    finishes: Arc<Vec<FacetFinish>>,
    material: GemMaterial,
    /// The exact [`AxesCacheKey`] this sweep was launched for -- stamped into
    /// [`SweepBookkeeping::completed_key`] once the sweep actually lands, so a
    /// later staleness check (`request_axes_for_stone`'s own
    /// `stale_now`) compares against what ACTUALLY finished, not merely what was
    /// last launched.
    key: AxesCacheKey,
}

/// The generation/activity/cache-key bookkeeping [`spawn_tilt_profile_sweep`] needs
/// beyond the sweep's own inputs -- see [`SweepRequest`].
struct SweepBookkeeping {
    generation: Arc<AtomicU64>,
    my_generation: u64,
    /// Set to `Some(request.key)` once this sweep's results are actually pushed --
    /// see [`SweepRequest::key`]'s own doc comment.
    completed_key: Arc<Mutex<Option<AxesCacheKey>>>,
    /// The [`crate::ActivityModel`] id [`request_axes_for_stone`]
    /// registered for this sweep via `invoke_start_external` -- finished here
    /// regardless of whether the result is applied or dropped as stale (an
    /// abandoned sweep's own activity must not linger in the status strip
    /// forever).
    activity_id: i32,
}

/// The background-thread half of [`request_axes_for_stone`]: runs the four-curve sweep off the
/// UI thread, then pushes the results back via `upgrade_in_event_loop`, dropping a result that a
/// newer request superseded (see `generation` on [`setup_tilt_profile_callback`]).
fn spawn_tilt_profile_sweep(
    request: SweepRequest,
    bookkeeping: SweepBookkeeping,
    ui_weak_bg: slint::Weak<MainWindow>,
) {
    let SweepRequest {
        planes,
        tools,
        finishes,
        material,
        key,
    } = request;
    let SweepBookkeeping {
        generation,
        my_generation,
        completed_key,
        activity_id,
    } = bookkeeping;
    std::thread::spawn(move || {
        let (brilliance_rows, extinction_rows, windowing_rows) = sweep_all_axes(
            FinishedStone::new(
                StoneGeometry {
                    planes: &planes,
                    tools: &tools,
                },
                &finishes,
            ),
            &material,
        );

        let brilliance_paths: Vec<SharedString> = brilliance_rows
            .iter()
            .map(|row| full_axis_curve_path(row).into())
            .collect();
        let extinction_paths: Vec<SharedString> = extinction_rows
            .iter()
            .map(|row| full_axis_curve_path(row).into())
            .collect();
        let windowing_paths: Vec<SharedString> = windowing_rows
            .iter()
            .map(|row| full_axis_curve_path(row).into())
            .collect();

        let to_model_rows = |rows: Vec<[f32; 181]>| -> ModelRc<ModelRc<f32>> {
            ModelRc::new(VecModel::from(
                rows.into_iter()
                    .map(|row| ModelRc::new(VecModel::from(row.to_vec())))
                    .collect::<Vec<_>>(),
            ))
        };

        let _ = ui_weak_bg.upgrade_in_event_loop(move |ui| {
            // Finished regardless of whether this result is superseded below --
            // an abandoned sweep still stops "running" from the status strip's
            // point of view.
            ui.global::<ActivityModel>()
                .invoke_finish_external(activity_id);
            if generation.load(Ordering::SeqCst) != my_generation {
                // Superseded by a newer request -- drop this stale result rather
                // than overwriting whatever the newer computation lands.
                return;
            }
            ui.global::<TiltModel>()
                .set_graph_brilliance_extra_axes(to_model_rows(brilliance_rows));
            ui.global::<TiltModel>()
                .set_graph_extinction_extra_axes(to_model_rows(extinction_rows));
            ui.global::<TiltModel>()
                .set_graph_windowing_extra_axes(to_model_rows(windowing_rows));
            ui.global::<TiltModel>()
                .set_graph_brilliance_extra_paths(ModelRc::new(VecModel::from(brilliance_paths)));
            ui.global::<TiltModel>()
                .set_graph_extinction_extra_paths(ModelRc::new(VecModel::from(extinction_paths)));
            ui.global::<TiltModel>()
                .set_graph_windowing_extra_paths(ModelRc::new(VecModel::from(windowing_paths)));
            ui.global::<TiltModel>().set_extra_axes_loading(false);
            // See `SweepRequest::key`'s own doc comment: this is what a LATER
            // sweep's own staleness check compares against.
            *completed_key
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(key);
            ui.global::<TiltModel>().set_curves_stale(false);
        });
    });
}

/// Wires `MainWindow::request_tilt_profile_axes` to a background computation of all four
/// full-axis (±90°, 1° step) tilt-elevation sweeps, pushing the results into
/// `graph_*_extra_axes`/`graph_*_extra_paths` (and `extra_axes_loading` around the computation)
/// once done.
pub(in crate::gui) fn setup_tilt_profile_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    // `Some(key)` once a request for exactly these inputs has been launched (whether
    // or not it has finished yet) -- deduplicates re-opening the dialog against the
    // same geometry/material/light without a second ~1.36s background sweep.
    let launched_key: Arc<Mutex<Option<AxesCacheKey>>> = Arc::new(Mutex::new(None));
    // Cloned out before `on_request_tilt_profile_axes` moves the original in below --
    // `on_rerender_curve_with_current_material` needs its own handle to force a fresh
    // sweep regardless of what the dedup check thinks.
    let launched_key_for_rerender = Arc::clone(&launched_key);
    // `Some(key)` once a sweep for exactly these inputs has actually LANDED (as
    // opposed to `launched_key`, which flips the instant one merely starts) --
    // see `TiltModel.curves_stale`'s own staleness check in
    // `request_axes_for_stone`.
    let completed_key: Arc<Mutex<Option<AxesCacheKey>>> = Arc::new(Mutex::new(None));
    // Bumped on every newly-launched computation; a background thread checks its own
    // snapshot against the latest value before applying results, so a stale
    // computation is silently dropped instead of overwriting fresher data.
    let generation = Arc::new(AtomicU64::new(0));

    let ui_weak = ui.as_weak();
    // `render_ctx` is cloned into its OWN binding for the closure below, rather
    // than shadowing the outer `render_ctx`: shadowing it would move the outer
    // binding into the closure, leaving nothing for `setup_video_export_callback`
    // below to borrow.
    let render_ctx_axes = render_ctx.clone();
    ui.global::<TiltModel>()
        .on_request_tilt_profile_axes(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            handle_request_tilt_profile_axes(
                &ui,
                &render_ctx_axes,
                &launched_key,
                &completed_key,
                &generation,
            );
        });

    // The decision-logic callback `performance_graph_dialog.slint`'s `cache_is_stale`
    // property calls into. `cached` arrives from `MainWindow::cached_curve_material`,
    // pushed by `gui::detail::load_diagram_detail`/`_remote` on every design selection.
    ui.global::<TiltModel>().on_is_cached_curve_material_stale(
        |cached: SharedString, current: SharedString| {
            let cached = if cached.is_empty() {
                None
            } else {
                Some(cached.as_str())
            };
            cached_curve_material_is_stale(cached, &current)
        },
    );

    // Re-renders the tilt curve fresh against the current material -- the "discard the
    // stale cached curve, sweep a fresh one" action the stale-cache banner promises.
    // `invoke_request_tilt_profile_axes` alone dedupes against `launched_key` exactly
    // like a dialog-open re-fire does, so clicking this while the live inputs still
    // hash to the last-launched `AxesCacheKey` would silently do nothing. This button
    // means "redo it now regardless," so it clears `launched_key` first, forcing the
    // next call to treat this as a brand-new request.
    let ui_weak_rerender = ui.as_weak();
    ui.global::<TiltModel>()
        .on_rerender_curve_with_current_material(move || {
            if let Some(ui) = ui_weak_rerender.upgrade() {
                *launched_key_for_rerender.lock().unwrap() = None;
                ui.global::<TiltModel>().invoke_request_tilt_profile_axes();
            }
        });

    // The tilt-video export's own wiring is registered from here rather than
    // `gui::mod::run_gui` directly, piggy-backing on the fact that
    // `setup_tilt_profile_callback` is already invoked once from there at
    // startup -- see `video_export`'s own module doc comment.
    super::video_export::setup_video_export_callback(ui, render_ctx, settings_store);
}

/// Whether the tilt-curve/preview data currently cached for this design was rendered
/// under a different `GemMaterial` than the one the viewport currently has loaded, and
/// the dialog should therefore offer to re-render fresh rather than silently show a
/// stale-colored cached curve/image.
///
/// `cached_material_name` is `None` whenever there is no cached curve to compare
/// against at all (e.g. no preview generated yet, or browsing a remote library whose
/// wire protocol carries no preview-material field), in which case this always
/// returns `false`: "no cached data" is not "stale cached data". Names compare ignoring ASCII
/// case, as `resolve_material` does.
///
/// The caller (`gui::detail::load_diagram_detail`) sources `cached_material_name` from
/// `Database::get_preview_images(entry_id).material`.
#[must_use]
pub fn cached_curve_material_is_stale(
    cached_material_name: Option<&str>,
    current_material_name: &str,
) -> bool {
    cached_material_name.is_some_and(|cached| !cached.eq_ignore_ascii_case(current_material_name))
}

#[cfg(test)]
#[path = "tilt_profile_tests.rs"]
mod tests;
