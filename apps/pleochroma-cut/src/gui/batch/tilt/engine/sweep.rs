//! The local and remote tilt-curve sweeps of one design's four axes.
//!
//! `engine.rs` keeps the resolve, save and lane bookkeeping.
//!
//! Both halves honour the design's frosting, as the render does. The local sweep takes the
//! stone's facet finishes ([`FinishedStone`]). The remote request carries the same planes in
//! `SceneState::frosted_planes`, and the worker rebuilds the finishes the way a `RENDER` of
//! that scene does, so a design's curves are the same wherever they are computed.

use crate::{
    bridge::{
        preview_render::{BATCH_TILT_LIGHTING_PRESET, PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW},
        remote::remote_render,
    },
    gui::batch::batch_queue::local_lane_count,
    settings::WorkerSettings,
};
use pleochroma::{
    color::metrics::{
        FinishedStone, PROFILE_AZIMUTHS_DEG, SweepProgress,
        evaluate_all_axes_profiles_stepped_finished_threaded,
        evaluate_full_axis_profile_at_azimuth_finished_threaded,
    },
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{DEFAULT_MAX_BOUNCES, EnvironmentSource},
    },
};
use pleochroma_net::{
    SceneState,
    messages::{AxisTiltCurves as WireAxisTiltCurves, TiltCurvesRequest, TiltCurvesResponse},
};
use pleochroma_vault::model::tilt_curves::{
    AxisTiltCurves as StorageAxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS,
    TiltPerformanceCurves,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};
use tracing::warn;

/// `max_bounces` of the [`SceneState`] a remote tilt-curve request carries.
///
/// The worker's `TILT_CURVES` handler ignores it (only the planes, material, light pose and
/// facet finishes feed the sweep) but still validates it, so it must stay within the worker's
/// `validate::MAX_BOUNCES` (128).
const REMOTE_SCENE_MAX_BOUNCES: u32 = DEFAULT_MAX_BOUNCES;

/// The environment the batch's tilt curves are scored under: [`BATCH_TILT_LIGHTING_PRESET`]
/// at the preview light pose.
const fn batch_environment() -> EnvironmentSource<'static> {
    BATCH_TILT_LIGHTING_PRESET.studio(1.0, PREVIEW_LIGHT_YAW, PREVIEW_LIGHT_PITCH)
}

/// A single fixed request id for every tilt-curve remote dispatch -- safe for the same
/// reason `bridge::preview_render::PREVIEW_REQUEST_ID` gives: each dispatch opens its
/// OWN fresh one-shot connection (see [`fetch_tilt_curves_remote`]), so nothing is ever
/// pipelined behind an unrelated request on the same socket.
const TILT_REQUEST_ID: u32 = 1;

/// Converts one wire [`WireAxisTiltCurves`] (`Vec<f32>` fields, forced by `serde`'s
/// array-length-32 ceiling -- see that type's own doc comment) into the storage
/// [`StorageAxisTiltCurves`] (`[f32; 181]` fields) a fixed-length remote reply's axis
/// must decode into.
///
/// # Errors
///
/// Returns an error string if any curve's length isn't exactly
/// `pleochroma_vault::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS` -- would mean a
/// worker on a different build with a different `TILT_CURVE_POINTS_PER_AXIS` replied
/// (the wire type's own length isn't enforced by the type system, only by every
/// production call site sending exactly that many points; see that constant's own doc
/// comment), not an ordinary failure this batch should silently swallow the same way as
/// a connection error.
fn wire_axis_to_storage(axis: WireAxisTiltCurves) -> Result<StorageAxisTiltCurves, String> {
    let to_array = |v: Vec<f32>, name: &str| -> Result<[f32; TILT_CURVE_POINTS_PER_AXIS], String> {
        v.try_into().map_err(|v: Vec<f32>| {
            format!(
                "{name} has {} points, expected {TILT_CURVE_POINTS_PER_AXIS}",
                v.len()
            )
        })
    };
    Ok(StorageAxisTiltCurves {
        brilliance_pct: to_array(axis.brilliance_pct, "brilliance_pct")?,
        extinction_pct: to_array(axis.extinction_pct, "extinction_pct")?,
        windowing_pct: to_array(axis.windowing_pct, "windowing_pct")?,
    })
}

/// Dispatches one design's full tilt-curve sweep to `worker` as a single `TILT_CURVES` request.
///
/// It blocks until the request finishes or fails. `cancel` is checked only once, up front,
/// because a `TILT_CURVES` request cannot be cancelled in flight (see `gui::batch::tilt`'s
/// module doc). Returns `None` on ANY shortfall (connection failure, missing `tilt_curves`
/// capability, a `Cancelled` or `Error` reply, or a malformed reply). The caller then
/// decides: [`Both`] requeues the design for local processing, [`RemoteOnly`] surfaces the
/// failure.
///
/// `frosted` lists the planes (ascending indices into `stone.planes`) the design frosts; the
/// scene carries them, so the worker sweeps the frosted stone.
///
/// [`Both`]: crate::settings::LiveComputeTarget::Both
/// [`RemoteOnly`]: crate::settings::LiveComputeTarget::RemoteOnly
pub(super) fn fetch_tilt_curves_remote(
    worker: &WorkerSettings,
    stone: StoneGeometry<'_>,
    frosted: &[u32],
    material: &GemMaterial,
    cancel: &AtomicBool,
) -> Option<TiltPerformanceCurves> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let scene = SceneState {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        // Ignored by the `TILT_CURVES` handler; `1` is a legal placeholder, not a render target.
        width: 1,
        height: 1,
        yaw: 0.0,
        camera_roll: 0.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        // The tilt-curve sweep is lit by a studio preset, never an HDR map.
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        estimator: pleochroma::optics::raytracer::default_estimator(),
        pitch: 0.0,
        distance: 1.0,
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        exposure: 1.0,
        max_bounces: REMOTE_SCENE_MAX_BOUNCES,
        lighting_preset: BATCH_TILT_LIGHTING_PRESET,
        material: material.clone(),
        planes: stone.planes.to_vec(),
        // The girdle toggle is a live-view setting no catalogue design stores; the design's
        // own frosting is `frosted_planes`.
        girdle_frosted: false,
        backdrop: 0.0,
        environment: pleochroma_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: stone.tools.to_vec(),
        fluorescence: Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: frosted.to_vec(),
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
    };
    let (mut stream, welcome) = remote_render::connect_and_handshake(worker).ok()?;
    if !welcome.tilt_curves {
        return None;
    }
    let request = TiltCurvesRequest {
        request_id: TILT_REQUEST_ID,
        scene,
    };
    pleochroma_net::client::send_tilt_curves_request(&mut stream, &request).ok()?;
    match pleochroma_net::client::recv_tilt_curves_response(&mut stream) {
        Ok(TiltCurvesResponse::Curves(result)) => {
            let mut axes_vec = Vec::with_capacity(result.axes.len());
            for axis in result.axes {
                match wire_axis_to_storage(axis) {
                    Ok(axis) => axes_vec.push(axis),
                    Err(e) => {
                        warn!("Remote tilt-curve reply had a malformed axis: {e}");
                        return None;
                    }
                }
            }
            let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes_vec.try_into().ok()?;
            Some(TiltPerformanceCurves { axes })
        }
        Ok(TiltCurvesResponse::Cancelled { .. } | TiltCurvesResponse::Error(_)) | Err(_) => None,
    }
}

/// Threads each local lane may use inside one design's sweep.
///
/// It is the machine's parallelism divided by the lane count, floored at `1`, so lanes times
/// inner threads never exceeds the cores.
fn inner_threads_per_lane() -> usize {
    let cores = thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    (cores / local_lane_count()).max(1)
}

/// Computes all four axis sweeps for one design locally.
///
/// `cancel` is checked before each axis, the finer-grained cancellation the local path offers.
/// Returns `None` as soon as `cancel` is seen, abandoning the design without saving anything:
/// tilt curves are all-or-nothing.
///
/// `stone` carries the design's facet finishes: a frosted facet scatters light, so a frosted
/// design's curves differ from its polished ones.
///
/// Reports no per-axis progress: with several local lanes mid-design at once, a per-axis
/// readout means nothing to the UI.
pub(super) fn compute_tilt_curves_locally(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
    cancel: &AtomicBool,
) -> Option<TiltPerformanceCurves> {
    let mut axes = Vec::with_capacity(PROFILE_AZIMUTHS_DEG.len());
    for &azimuth_deg in &PROFILE_AZIMUTHS_DEG {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (brilliance_pct, extinction_pct, windowing_pct) =
            evaluate_full_axis_profile_at_azimuth_finished_threaded(
                stone,
                material,
                azimuth_deg,
                batch_environment(),
                Some(inner_threads_per_lane()),
            );
        axes.push(StorageAxisTiltCurves {
            brilliance_pct,
            extinction_pct,
            windowing_pct,
        });
    }
    let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().ok()?;
    Some(TiltPerformanceCurves { axes })
}

/// [`compute_tilt_curves_locally`] with `on_step` called before every raytrace evaluation.
///
/// It serves the single-design progress bar. It uses the shared stepped sweep
/// (`pleochroma::color::metrics::evaluate_all_axes_profiles_stepped_finished_threaded`),
/// whose curves are bit-identical to the plain per-axis calls, and it honours `cancel`
/// between evaluations instead of between axes.
pub(super) fn compute_tilt_curves_locally_stepped(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
    cancel: &AtomicBool,
    on_step: &mut dyn FnMut(SweepProgress),
) -> Option<TiltPerformanceCurves> {
    let profiles = evaluate_all_axes_profiles_stepped_finished_threaded(
        stone,
        material,
        batch_environment(),
        None,
        &mut |progress| {
            on_step(progress);
            !cancel.load(Ordering::Relaxed)
        },
    )?;
    let axes: Vec<StorageAxisTiltCurves> = profiles
        .into_iter()
        .map(|profile| StorageAxisTiltCurves {
            brilliance_pct: profile.brilliance,
            extinction_pct: profile.extinction,
            windowing_pct: profile.windowing,
        })
        .collect();
    let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().ok()?;
    Some(TiltPerformanceCurves { axes })
}
