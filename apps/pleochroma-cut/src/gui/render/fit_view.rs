//! The pose pills' "Fit" (`ViewportModel.set_view(5)`): the orbit distance that shows the
//! whole stone in the picture on screen.
//!
//! Two inputs used to make Fit frame the wrong size:
//!
//! - The radius. The solid worker's last frame (`SlintSolidSink::mesh_bounding_radius`)
//!   is only current while the solid raster is on screen. The Live view's path-traced
//!   image draws `RenderContext::active_planes` directly, and a library selection there
//!   redraws no solid frame (`resubmit_live_solid` runs in Solid mode only), so the
//!   stored radius still described whatever stone the worker drew last. Fit therefore
//!   measures the stone the tracer holds now ([`planar_bounding_radius`]) and, where the
//!   solid raster is on screen, keeps the drawn frame's radius when that is larger (the
//!   Cut slider and Cutting mode draw the partly cut stone, which can be bigger than the
//!   finished one the tracer holds).
//! - The image. The camera's field of view is the vertical one, so a picture taller than
//!   wide needs the stone further away ([`fit_distance_for_view`]).

use crate::{
    MainWindow, SolidPreviewModel, ViewportModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
};
use pleochroma::{geometry::plane::GpuFacetPlane, optics::raytracer::DEFAULT_POSE};
use pleochroma_solid::preview::{
    StoneGeometryBuf,
    camera::{fit_distance_for_view, orbit_distance_bounds, planar_bounding_radius},
};
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, Mutex, PoisonError},
};

/// The planar radius of the last plane set Fit measured.
///
/// Measuring builds the whole solid mesh, so a repeated Fit (or the harness' repeated
/// framing reads) on the same stone reuses the answer. The key is the plane set's `Arc`
/// itself, the identity `SceneIdentity` already uses: every writer of
/// `RenderContext::active_planes` installs a new `Arc`. The cache holds a clone of the
/// key, so its address cannot be reused by another allocation, and an in-place
/// `Arc::make_mut` writer copies instead of mutating the measured planes.
struct PlanarRadiusCache<T> {
    measured: Option<(Arc<T>, Option<f64>)>,
}

impl<T> PlanarRadiusCache<T> {
    const fn new() -> Self {
        Self { measured: None }
    }

    /// The cached radius when `planes` is the set measured last, otherwise `measure()`
    /// (remembered for the next call).
    fn get_or_measure(
        &mut self,
        planes: &Arc<T>,
        measure: impl FnOnce() -> Option<f64>,
    ) -> Option<f64> {
        let hit = match &self.measured {
            Some((held, radius)) if Arc::ptr_eq(held, planes) => Some(*radius),
            _ => None,
        };
        if let Some(radius) = hit {
            return radius;
        }
        let radius = measure();
        self.measured = Some((Arc::clone(planes), radius));
        radius
    }
}

thread_local! {
    /// Fit runs on the UI thread only.
    static PLANAR_RADIUS: RefCell<PlanarRadiusCache<Vec<GpuFacetPlane>>> =
        const { RefCell::new(PlanarRadiusCache::new()) };
}

/// The Fit distance for the picture on screen now.
///
/// In the Live view's path-traced mode the measured radius is also stored as the
/// frame radius, so the zoom clamp that reads it afterwards does not pull the fitted
/// camera back in.
///
/// The stone is measured at most once per plane set ([`PlanarRadiusCache`]), outside the
/// `RenderContext` lock, which the render thread shares.
pub fn fit_distance(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    frame_radius: &Mutex<f64>,
) -> f32 {
    let (planes, render_size) = {
        let ctx = RenderContext::lock(render_ctx);
        (Arc::clone(&ctx.active_planes), (ctx.width, ctx.height))
    };
    let solid = ui.global::<SolidPreviewModel>();
    let on_live_tab = ui.get_render_view_tab() == 0;
    let traced_only = on_live_tab
        && crate::gui::view_modes::LiveViewMode::from_raw(
            ui.global::<ViewportModel>().get_live_view_mode(),
        )
        .is_traced();
    // The path-traced image is letterboxed to the render resolution (`image-fit:
    // contain`), so its own aspect is what has to hold the stone.
    let traced = traced_only
        || (!on_live_tab
            && crate::gui::view_modes::EditViewMode::from_raw(solid.get_view_mode()).is_traced());
    let aspect = if traced {
        ratio(render_size.0 as f32, render_size.1 as f32)
    } else if on_live_tab {
        ratio(
            solid.get_live_viewport_width(),
            solid.get_live_viewport_height(),
        )
    } else {
        ratio(solid.get_viewport_width(), solid.get_viewport_height())
    };
    // Concave tools only remove material, so the planes alone bound the stone.
    let current = PLANAR_RADIUS.with_borrow_mut(|cache| {
        cache.get_or_measure(&planes, || {
            planar_bounding_radius(&StoneGeometryBuf {
                planes: planes.as_ref().clone(),
                ..StoneGeometryBuf::default()
            })
        })
    });
    let mut stored = frame_radius.lock().unwrap_or_else(PoisonError::into_inner);
    let radius = fit_radius(current, *stored, traced_only);
    if traced_only {
        *stored = radius;
    }
    drop(stored);
    // Cutting mode draws a different, bigger stone at every step: it fits the rough.
    let radius = radius.max(RADIUS_FLOOR.get());
    let (min, _) = orbit_distance_bounds(radius);
    // Not clamped to the far end: a very tall picture needs the stone further out than
    // the zoom range allows, and clamping would crop it again.
    fit_distance_for_view(radius, aspect).max(min)
}

/// Frames the built-in stand-in stone like Fit does, once at startup.
///
/// Before any design is opened the viewport shows the built-in round brilliant (radius
/// about 1.04) from the default pose distance (`DEFAULT_POSE.distance`, 2.4), which
/// is closer than the stone's own Fit distance (about 3.3) and showed it cut by the frame on
/// all sides. This sets the distance to the stand-in's Fit distance, only while
///
/// - the stand-in is still what the render context holds ([`PlanesOwner::Builtin`]), and
/// - the distance still is the default pose distance (a saved, customised zoom is the
///   cutter's own and is left alone; so is a saved zoom that happens to be the default,
///   which is the very framing this replaces).
///
/// The value is not written back to the settings file: the persisted zoom stays the
/// default, so every start frames the stand-in again until the cutter zooms. Fit's own
/// measuring ([`planar_bounding_radius`]) sizes the stone, not the solid worker's frame
/// radius, which is still the 1.5 placeholder at startup and would push the camera too far
/// out.
pub fn frame_stand_in_stone(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let planes = {
        let ctx = RenderContext::lock(render_ctx);
        if !is_unframed_stand_in(ctx.planes_owner, ctx.distance) {
            return;
        }
        Arc::clone(&ctx.active_planes)
    };
    let solid = ui.global::<SolidPreviewModel>();
    let aspect = ratio(
        solid.get_live_viewport_width(),
        solid.get_live_viewport_height(),
    );
    // Measured outside the `RenderContext` lock, which the render thread shares.
    let radius = PLANAR_RADIUS.with_borrow_mut(|cache| {
        cache.get_or_measure(&planes, || {
            planar_bounding_radius(&StoneGeometryBuf {
                planes: planes.as_ref().clone(),
                ..StoneGeometryBuf::default()
            })
        })
    });
    let Some(radius) = radius else {
        return;
    };
    let (min, _) = orbit_distance_bounds(radius);
    let distance = fit_distance_for_view(radius, aspect).max(min);
    let mut ctx = RenderContext::lock(render_ctx);
    // The lock was released while measuring: only a context nobody touched meanwhile.
    if is_unframed_stand_in(ctx.planes_owner, ctx.distance) {
        ctx.distance = distance;
        ctx.dirty = true;
    }
}

/// Whether the stand-in is on screen at the untouched default pose distance.
fn is_unframed_stand_in(owner: PlanesOwner, distance: f32) -> bool {
    owner == PlanesOwner::Builtin && (distance - DEFAULT_POSE.distance).abs() < 1e-4
}

thread_local! {
    /// A radius [`fit_distance`] never fits below (UI thread only): `0.0` normally, the rough's
    /// radius while Cutting mode is open, because its picture shows the stone after each step
    /// (the rough minus the steps so far, bigger than the finished stone Fit measures) and a
    /// Fit to the finished stone cut the early steps off at the frame.
    static RADIUS_FLOOR: Cell<f64> = const { Cell::new(0.0) };
}

/// Sets the radius [`fit_distance`] fits at least ([`RADIUS_FLOOR`]); `0.0` lifts it. Returns
/// the previous value.
pub fn set_radius_floor(radius: f64) -> f64 {
    RADIUS_FLOOR.replace(radius.max(0.0))
}

thread_local! {
    /// The distance [`auto_frame_distance`] last chose (UI thread only), so a later load can tell
    /// "still where the app put the camera" from "the cutter zoomed".
    static AUTO_FRAMED: Cell<Option<f32>> = const { Cell::new(None) };
}

/// Whether the cutter has not chosen the zoom: the camera sits at the default pose distance
/// (also what Reset Camera restores) or where [`auto_frame_distance`] last put it. Anything
/// else is a wheel zoom or a Fit, which a load leaves alone.
pub fn is_unzoomed(distance: f32) -> bool {
    (distance - DEFAULT_POSE.distance).abs() < 1e-4
        || AUTO_FRAMED
            .get()
            .is_some_and(|framed| (distance - framed).abs() < 1e-4)
}

/// The distance that frames a newly loaded stone (`planes`, the plane set about to be claimed
/// for the render context) in the picture on screen, or `None` when the cutter's own zoom is
/// to be kept ([`is_unzoomed`]) or the planes close no solid.
///
/// A design from the library or a file used to appear at whatever distance the previous stone
/// left, and a larger stone was cut by the frame on every side (only the built-in stand-in was
/// framed, [`frame_stand_in_stone`]). The caller sets the returned distance on the render
/// context in the same lock that installs the planes and passes it to [`note_auto_framed`].
///
/// Measures the stone, which builds its solid: call it outside the `RenderContext` lock, once
/// per load, not per edit.
pub fn auto_frame_distance(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    planes: &[GpuFacetPlane],
) -> Option<f32> {
    let current = RenderContext::lock(render_ctx).distance;
    if !is_unzoomed(current) {
        return None;
    }
    let radius = planar_bounding_radius(&StoneGeometryBuf {
        planes: planes.to_vec(),
        ..StoneGeometryBuf::default()
    })?;
    let (min, _) = orbit_distance_bounds(radius);
    Some(fit_distance_for_view(radius, shown_viewport_aspect(ui)).max(min))
}

/// `width / height` of the viewport picture the cutter is looking at: the Live tab's, or the
/// Edit tab's.
fn shown_viewport_aspect(ui: &MainWindow) -> f32 {
    let solid = ui.global::<SolidPreviewModel>();
    if ui.get_render_view_tab() == 0 {
        ratio(
            solid.get_live_viewport_width(),
            solid.get_live_viewport_height(),
        )
    } else {
        ratio(solid.get_viewport_width(), solid.get_viewport_height())
    }
}

/// Records `distance` (what [`auto_frame_distance`] returned and the caller installed) as the
/// app's own framing, so the next load frames again until the cutter zooms.
pub fn note_auto_framed(distance: f32) {
    AUTO_FRAMED.set(Some(distance));
}

/// Frames the stone again for the viewport's size now, when the app placed the camera
/// itself ([`note_auto_framed`]) and the cutter has not zoomed since.
///
/// A design is framed at load for the size the viewport has at that instant. A load that
/// opens or closes a side list (the Catalog beside the viewport) frames for the size from
/// before the layout settles, and the stone then showed cut by the frame on the narrower
/// picture. The viewport's own resize signal calls this once the new size is known; a
/// camera the cutter zoomed (wheel, Fit, Reset Camera) is left alone, and so is one where the
/// size change does not change the fit.
pub fn reframe_for_viewport(ui: &MainWindow, render_ctx: &Arc<Mutex<RenderContext>>) {
    let Some(framed) = AUTO_FRAMED.get() else {
        return;
    };
    let (current, planes) = {
        let ctx = RenderContext::lock(render_ctx);
        (ctx.distance, Arc::clone(&ctx.active_planes))
    };
    if (current - framed).abs() >= 1e-4 {
        return;
    }
    // Measured outside the `RenderContext` lock, which the render thread shares.
    let radius = PLANAR_RADIUS.with_borrow_mut(|cache| {
        cache.get_or_measure(&planes, || {
            planar_bounding_radius(&StoneGeometryBuf {
                planes: planes.as_ref().clone(),
                ..StoneGeometryBuf::default()
            })
        })
    });
    let Some(radius) = radius else {
        return;
    };
    let (min, _) = orbit_distance_bounds(radius);
    let distance = fit_distance_for_view(radius, shown_viewport_aspect(ui)).max(min);
    if (distance - current).abs() < 1e-3 {
        return;
    }
    let mut ctx = RenderContext::lock(render_ctx);
    // The lock was released while measuring: only a camera nobody moved meanwhile.
    if (ctx.distance - current).abs() < 1e-4 {
        ctx.distance = distance;
        ctx.dirty = true;
        drop(ctx);
        note_auto_framed(distance);
    }
}

/// The radius to fit: the stone the tracer holds (`current`), or the drawn frame's
/// (`drawn`) where the solid raster is on screen and its stone is larger. `drawn`
/// alone when the planes do not close.
const fn fit_radius(current: Option<f64>, drawn: f64, traced_only: bool) -> f64 {
    match current {
        Some(current) if traced_only => current,
        Some(current) => current.max(drawn),
        None => drawn,
    }
}

/// `width / height`, or `1.0` before the picture has a size.
fn ratio(width: f32, height: f32) -> f32 {
    if width > 0.0 && height > 0.0 {
        width / height
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_POSE, PlanarRadiusCache, PlanesOwner, StoneGeometryBuf, fit_distance_for_view,
        fit_radius, is_unframed_stand_in, planar_bounding_radius, ratio,
    };
    use std::{cell::Cell, sync::Arc};

    #[test]
    fn the_same_plane_set_is_measured_once() {
        let planes = Arc::new(vec![1_u8]);
        let mut cache = PlanarRadiusCache::new();
        let calls = Cell::new(0);
        let measure = |cache: &mut PlanarRadiusCache<Vec<u8>>, planes: &Arc<Vec<u8>>| {
            cache.get_or_measure(planes, || {
                calls.set(calls.get() + 1);
                Some(1.5)
            })
        };
        assert_eq!(measure(&mut cache, &planes), Some(1.5));
        assert_eq!(measure(&mut cache, &planes), Some(1.5));
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_new_plane_set_is_measured_again_even_when_equal() {
        let first = Arc::new(vec![1_u8]);
        let second = Arc::new(vec![1_u8]);
        let mut cache = PlanarRadiusCache::new();
        assert_eq!(cache.get_or_measure(&first, || Some(1.0)), Some(1.0));
        assert_eq!(cache.get_or_measure(&second, || Some(2.0)), Some(2.0));
        // The cache now describes the second set.
        assert_eq!(
            cache.get_or_measure(&second, || panic!("cached")),
            Some(2.0)
        );
    }

    #[test]
    fn a_plane_set_that_closes_nothing_is_cached_too() {
        let planes = Arc::new(Vec::<u8>::new());
        let mut cache = PlanarRadiusCache::new();
        assert_eq!(cache.get_or_measure(&planes, || None), None);
        assert_eq!(cache.get_or_measure(&planes, || panic!("cached")), None);
    }

    #[test]
    fn the_traced_picture_fits_the_stone_the_tracer_holds() {
        // A stale, smaller frame radius (the worker drew another stone last) loses ...
        assert!((fit_radius(Some(1.4), 0.8, true) - 1.4).abs() < 1e-12);
        // ... and so does a stale, larger one.
        assert!((fit_radius(Some(1.4), 2.0, true) - 1.4).abs() < 1e-12);
    }

    #[test]
    fn the_solid_picture_keeps_the_larger_drawn_stone() {
        // The Cut slider's partly cut stone is bigger than the finished one.
        assert!((fit_radius(Some(1.4), 2.0, false) - 2.0).abs() < 1e-12);
        assert!((fit_radius(Some(1.4), 0.8, false) - 1.4).abs() < 1e-12);
    }

    #[test]
    fn planes_that_close_nothing_fall_back_to_the_drawn_frame() {
        assert!((fit_radius(None, 0.9, true) - 0.9).abs() < 1e-12);
        assert!((fit_radius(None, 0.9, false) - 0.9).abs() < 1e-12);
    }

    #[test]
    fn only_the_stand_in_at_the_default_distance_is_framed() {
        let default = DEFAULT_POSE.distance;
        assert!(is_unframed_stand_in(PlanesOwner::Builtin, default));
        // A zoom the cutter chose, or any stone but the stand-in, is left alone.
        assert!(!is_unframed_stand_in(PlanesOwner::Builtin, default + 0.5));
        assert!(!is_unframed_stand_in(
            PlanesOwner::Editor { generation: 1 },
            default
        ));
    }

    #[test]
    fn the_stand_in_fits_further_out_than_the_default_pose() {
        let planes = pleochroma::geometry::cuts::StandardGemCuts::standard_round_brilliant();
        let radius = planar_bounding_radius(&StoneGeometryBuf {
            planes,
            ..StoneGeometryBuf::default()
        })
        .expect("the built-in round brilliant closes");
        assert!(fit_distance_for_view(radius, 1.0) > DEFAULT_POSE.distance);
    }

    #[test]
    fn only_a_camera_the_app_placed_counts_as_unzoomed() {
        use super::{is_unzoomed, note_auto_framed};
        assert!(is_unzoomed(DEFAULT_POSE.distance), "the default, and Reset");
        assert!(
            !is_unzoomed(DEFAULT_POSE.distance + 0.7),
            "a wheel zoom or a Fit is the cutter's"
        );
        note_auto_framed(3.9);
        assert!(is_unzoomed(3.9), "where the last load framed the stone");
        assert!(!is_unzoomed(3.2));
        assert!(is_unzoomed(DEFAULT_POSE.distance));
    }

    #[test]
    fn an_unsized_picture_counts_as_square() {
        assert!((ratio(0.0, 400.0) - 1.0).abs() < 1e-6);
        assert!((ratio(800.0, 400.0) - 2.0).abs() < 1e-6);
    }
}
