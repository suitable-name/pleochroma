//! A live pass with the worker's own path cache (`PATH_CACHE_FLAG` in the request id).
//!
//! The stone pixels of the tiles the worker is given shade from cached exit records, everything
//! else is traced as in [`super::live`].
//!
//! # Whose cache
//!
//! Each worker builds the cache for the tiles it renders; nothing travels over the network. The
//! coordinator's assignment is sticky, so the same tiles come back pass after pass and the cache
//! built by the first pass serves the rest of the render.
//!
//! # What stays resident
//!
//! A `PathCacheFrame` borrows the scene of the pass it was made for, so what stays between passes
//! is the built tiles ([`Resident`]), under the cache key of the scene they were built for. A pass
//! takes the tiles when the key matches, hands them to the frame of the pass and puts them back
//! afterwards; a different scene (another render, another pose) drops them. One scene is resident
//! at a time per worker process: two renders at once evict each other's cache and each rebuilds,
//! which is slower but never wrong. Nothing is written to disk on a worker.
//!
//! # When there is no cache
//!
//! A stone the cache cannot serve (a scattering or opal-look material, frosted facets, concave
//! tiers; see `PathCacheOff`) and a scene whose estimated cache would pass
//! [`WORKER_CACHE_CAP_BYTES`] are traced exactly as an uncached pass, which is also what happens
//! when the flag is absent.

use super::{live, resolve_facet_finishes};
use crate::assets::TrayRef;
use glam::{Mat4, Vec3};
use pleochroma::{
    optics::raytracer::{
        Camera, DEFAULT_FOV_DEG, Estimator, build_plane_soa, surface::SurfaceScene,
        transport::PreparedMaterial,
    },
    renderer::{
        adaptive::LiveTiles,
        frame_scene::FrameScene,
        gpu_backend::{CacheTier, GpuAccumulate, GpuBackend, GpuPathCache, GpuSceneRef},
        path_cache::{
            BuildSettings, CacheKey, CacheOptions, PathCacheFrame, TileCache, estimate_size,
        },
    },
};
use pleochroma_net::{SceneState, messages::gather_live};
use std::sync::{Mutex, PoisonError, atomic::AtomicBool};

/// The largest cache, in bytes, a worker builds for one scene (8 GiB); a scene whose estimate is
/// larger is traced without one.
pub const WORKER_CACHE_CAP_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// The built tiles of one scene, kept between passes.
struct Resident {
    /// The scene the tiles were built for.
    key: CacheKey,
    /// The tiles built so far (empty when `refused`).
    tiles: Vec<TileCache>,
    /// The scene's estimated cache is over [`WORKER_CACHE_CAP_BYTES`]: its passes are not cached.
    refused: bool,
}

/// The resident cache, if a pass has left one.
static RESIDENT: Mutex<Option<Resident>> = Mutex::new(None);

/// Takes the resident cache when it is the one for `key`; a cache for another scene is dropped.
fn take_resident(key: &CacheKey) -> Option<Resident> {
    RESIDENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .filter(|resident| resident.key == *key)
}

/// Leaves `resident` for the next pass.
fn keep_resident(resident: Resident) {
    *RESIDENT.lock().unwrap_or_else(PoisonError::into_inner) = Some(resident);
}

/// The number of tiles in the resident cache built for `key`, for the tests; `None` when
/// nothing is resident or the resident cache belongs to another key.
#[cfg(test)]
pub fn resident_tiles(key: &CacheKey) -> Option<usize> {
    RESIDENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .filter(|resident| resident.key == *key)
        .map(|resident| resident.tiles.len())
}

/// Drops whatever is resident (the tests start from nothing).
#[cfg(test)]
pub fn clear_resident() {
    *RESIDENT.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The cache key of `scene` as a pass builds it, for the tests.
#[cfg(test)]
pub fn key_of(scene: &SceneState) -> CacheKey {
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll);
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_source(scene, hdr_map.as_deref());
    let finishes = resolve_facet_finishes(scene);
    let material = scene.traced_material();
    let frame = FrameScene {
        camera: &camera,
        width: scene.width,
        height: scene.height,
        planes: &scene.planes,
        facet_finishes: &finishes,
        material: &material,
        max_bounces: scene.max_bounces,
        environment,
    };
    CacheKey::from_scene(&frame, Mat4::IDENTITY, &BuildSettings::default())
}

/// [`live::trace_live_pass`] with the worker's path cache: the GPU's cached live entry when the
/// scene goes there and the backend takes it, else the CPU's cached tile tracer.
///
/// The result is the pair of every live pixel, as `trace_live_pass` returns; `None` means `cancel`
/// was raised first. A stone the cache cannot serve is traced without it.
///
/// A scene with the Stone Shadow is declined (`PathCacheOff::StoneShadow`), since the cache
/// records exits the shadow map would have to be applied to; a scene with the Tray Reflection is
/// declined likewise (`PathCacheOff::TrayReflection`). A declined scene is traced as the plain
/// [`live::trace_live_pass`] with the request's resolved `tray`, so the shadow or mirror is kept.
/// The decline is only logged.
#[must_use]
pub fn trace_live_pass_cached(
    gpu: &GpuBackend,
    scene: &SceneState,
    live: &LiveTiles,
    range: (u32, u32),
    threads: usize,
    cancel: &AtomicBool,
    tray: TrayRef<'_>,
) -> Option<Vec<(Vec3, Vec3)>> {
    if scene.estimator == Estimator::ExitCollection {
        // The cache holds branching exit records, so an exit-collection frame is traced without
        // it: every engine then runs the same estimator.
        tracing::info!(
            reason = "exit collection does not use the path cache",
            "worker: path cache declined"
        );
        return live::trace_live_pass(gpu, scene, live, range, threads, cancel, tray);
    }
    let off = if scene.stone_shadow {
        Some(pleochroma::renderer::path_cache::PathCacheOff::StoneShadow)
    } else if scene.tray_reflection > 0.0 {
        Some(pleochroma::renderer::path_cache::PathCacheOff::TrayReflection)
    } else {
        None
    };
    if let Some(off) = off {
        tracing::info!(reason = off.message(), "worker: path cache declined");
        return live::trace_live_pass(gpu, scene, live, range, threads, cancel, tray);
    }
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll);
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_source(scene, hdr_map.as_deref());
    let facet_finishes = resolve_facet_finishes(scene);
    let traced_material = scene.traced_material();
    let frame = FrameScene {
        camera: &camera,
        width: scene.width,
        height: scene.height,
        planes: &scene.planes,
        facet_finishes: &facet_finishes,
        material: &traced_material,
        max_bounces: scene.max_bounces,
        environment,
    };
    let surfaces = scene.effects.surfaces();
    let surface = scene.effects.surface_scene(&surfaces);
    let soa = build_plane_soa(&scene.planes);
    let settings = BuildSettings::default();
    let options = CacheOptions {
        settings,
        depth_of_field: false,
        budget_bytes: Some(WORKER_CACHE_CAP_BYTES),
    };
    let Ok(cache) = PathCacheFrame::new(
        &frame,
        &soa,
        (&scene.tools, &scene.fluorescence, surface),
        options,
    ) else {
        return live::trace_live_pass(gpu, scene, live, range, threads, cancel, tray);
    };

    let key = CacheKey::from_scene(&frame, Mat4::IDENTITY, &settings);
    let resident = take_resident(&key);
    let refused = resident.as_ref().map_or_else(
        || {
            let prepared = PreparedMaterial::new(frame.material).with_estimator(scene.estimator);
            let estimate = estimate_size(&frame, &prepared, &soa, &settings);
            // A scene whose estimated cache passes the worker's cap is traced without one.
            estimate.raw_bytes > WORKER_CACHE_CAP_BYTES
        },
        |resident| resident.refused,
    );
    if refused {
        keep_resident(Resident {
            key,
            tiles: Vec::new(),
            refused: true,
        });
        return live::trace_live_pass(gpu, scene, live, range, threads, cancel, tray);
    }
    if let Some(resident) = resident {
        // Every resident tile came from this same frame key, so the count is not needed.
        let _ = cache.preload(resident.tiles);
    }

    let result = trace(gpu, scene, live, range, threads, cancel, &cache, surface);
    keep_resident(Resident {
        key,
        tiles: cache.into_tiles(),
        refused: false,
    });
    result
}

/// The pass itself, with `cache` built for `scene`.
#[expect(
    clippy::too_many_arguments,
    reason = "the live pass's parameters plus the cache and the surface it was built with"
)]
fn trace(
    gpu: &GpuBackend,
    scene: &SceneState,
    live: &LiveTiles,
    range: (u32, u32),
    threads: usize,
    cancel: &AtomicBool,
    cache: &PathCacheFrame<'_, '_>,
    surface: SurfaceScene<'_>,
) -> Option<Vec<(Vec3, Vec3)>> {
    if super::scene_uses_gpu(scene, gpu) && live.tile() == cache.settings().tile {
        let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
            .with_roll(scene.camera_roll);
        let hdr_map = crate::assets::resolved_hdr_map(scene);
        let environment = crate::assets::environment_source(scene, hdr_map.as_deref());
        let facet_finishes = resolve_facet_finishes(scene);
        let traced_material = scene.traced_material();
        let gpu_scene = GpuSceneRef {
            camera: &camera,
            width: scene.width,
            height: scene.height,
            planes: &scene.planes,
            facet_finishes: &facet_finishes,
            material: &traced_material,
            max_bounces: scene.max_bounces,
            environment,
            estimator: Some(scene.estimator),
        };
        let pixels = scene.width as usize * scene.height as usize;
        let mut sum = vec![Vec3::ZERO; pixels];
        let mut half_b = vec![Vec3::ZERO; pixels];
        let mut slabs = GpuPathCache::new(cache, CacheTier::SystemRam);
        if let Some(limit) = gpu.path_cache_slab_limit() {
            slabs =
                slabs.with_slab_budget_bytes(CacheTier::SystemRam.slab_budget_bytes().min(limit));
        }
        let outcome = gpu.try_accumulate_cached_live_cancellable(
            &gpu_scene,
            &scene.tools,
            &surface,
            range.0,
            range.1,
            live,
            &slabs,
            &mut sum,
            &mut half_b,
            cancel,
        );
        match outcome {
            GpuAccumulate::Done => return Some(gather_live(live, &sum, &half_b)),
            GpuAccumulate::Cancelled => return None,
            GpuAccumulate::Declined => {}
        }
    }
    // A scene with the Stone Shadow or the Tray Reflection never reaches the cache (see
    // `trace_live_pass_cached`), so it carries neither.
    live::trace_live_pass_cpu_with(
        scene,
        live,
        range,
        threads,
        cancel,
        Some(cache),
        TrayRef::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_core::tests::tiny_scene;
    use std::sync::Mutex as StdMutex;

    /// The resident cache is one per process: tests that look at it take turns.
    static SERIAL: StdMutex<()> = StdMutex::new(());

    fn scene(width: u32, height: u32) -> SceneState {
        SceneState {
            width,
            height,
            ..tiny_scene()
        }
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    /// A request with neither the Stone Shadow nor the Tray Reflection.
    const NONE: TrayRef<'static> = TrayRef {
        shadow: None,
        mirror: None,
    };

    /// Mean luminance (Y) of the pass total over every live pixel, per sample.
    fn mean_y(pixels: &[(Vec3, Vec3)], samples: u32) -> f32 {
        let total: f32 = pixels.iter().map(|(sum, _)| sum.y).sum();
        total / pixels.len() as f32 / samples as f32
    }

    #[test]
    fn a_cached_pass_has_the_plain_passs_mean() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        clear_resident();
        let scene = scene(24, 24);
        let live = LiveTiles::all(24, 24, 8);
        let gpu = GpuBackend::disabled();
        let plain = live::trace_live_pass(&gpu, &scene, &live, (0, 256), 2, &never(), NONE)
            .expect("not cancelled");
        let cached = trace_live_pass_cached(&gpu, &scene, &live, (0, 96), 2, &never(), NONE)
            .expect("not cancelled");
        assert_eq!(plain.len(), cached.len());
        assert!(
            cached
                .iter()
                .all(|(sum, odd)| sum.is_finite() && odd.is_finite())
        );
        let (want, got) = (mean_y(&plain, 256), mean_y(&cached, 96));
        assert!(want > 0.0, "the scene has light");
        assert!(
            (got - want).abs() <= 0.10 * want,
            "cached mean {got} against the traced mean {want}"
        );
        clear_resident();
    }

    #[test]
    fn the_built_tiles_stay_resident_between_passes_and_go_with_the_scene() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        clear_resident();
        let gpu = GpuBackend::disabled();
        let first = scene(24, 24);
        let live = LiveTiles::all(24, 24, 8);
        let key = key_of(&first);
        assert_eq!(resident_tiles(&key), None, "nothing before the first pass");

        let one = trace_live_pass_cached(&gpu, &first, &live, (0, 8), 2, &never(), NONE).unwrap();
        let tiles = resident_tiles(&key).expect("the pass left its tiles");
        assert!(tiles > 0);

        // The next pass of the same render finds them and gives the same answer as a fresh
        // cache would (the tiles are deterministic).
        let two = trace_live_pass_cached(&gpu, &first, &live, (8, 8), 2, &never(), NONE).unwrap();
        assert_eq!(resident_tiles(&key), Some(tiles));
        clear_resident();
        let fresh = trace_live_pass_cached(&gpu, &first, &live, (8, 8), 2, &never(), NONE).unwrap();
        assert_eq!(two, fresh, "a resident cache shades as a built one");
        assert_ne!(one, two, "other samples, other sums");

        // Another pose is another scene: the first one's tiles are dropped.
        let moved = SceneState {
            yaw: first.yaw + 0.3,
            ..first
        };
        let _ = trace_live_pass_cached(&gpu, &moved, &live, (0, 4), 2, &never(), NONE).unwrap();
        assert_eq!(resident_tiles(&key), None);
        assert!(resident_tiles(&key_of(&moved)).is_some());
        clear_resident();
    }

    #[test]
    fn a_stone_the_cache_cannot_serve_is_traced_as_a_plain_pass() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        clear_resident();
        let mut opal = scene(16, 16);
        opal.material.name = "Precious Opal (White)".to_owned();
        let live = LiveTiles::all(16, 16, 8);
        let gpu = GpuBackend::disabled();
        let plain = live::trace_live_pass(&gpu, &opal, &live, (0, 8), 2, &never(), NONE).unwrap();
        let asked = trace_live_pass_cached(&gpu, &opal, &live, (0, 8), 2, &never(), NONE).unwrap();
        assert_eq!(plain, asked, "bit for bit");
        assert_eq!(resident_tiles(&key_of(&opal)), None, "nothing was cached");
        clear_resident();
    }

    /// A scene with the Stone Shadow declines the cache (`PathCacheOff::StoneShadow`): nothing
    /// is left resident, and the pass is still rendered WITH the shadow, as the plain pass is.
    #[test]
    fn a_shadowed_scene_declines_the_cache_but_keeps_its_shadow() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        clear_resident();
        let shadowed = SceneState {
            stone_shadow: true,
            backdrop: 1.0,
            ..scene(16, 16)
        };
        let live = LiveTiles::all(16, 16, 8);
        let gpu = GpuBackend::disabled();
        let tray = crate::assets::resolve_tray(
            &shadowed,
            Some(pleochroma_net::messages::RequestIntent::Interactive),
            &never(),
        )
        .unwrap();
        assert!(tray.shadow.is_some(), "the scene asked for a shadow");
        let plain = live::trace_live_pass(&gpu, &shadowed, &live, (0, 4), 2, &never(), tray.view())
            .unwrap();
        let asked =
            trace_live_pass_cached(&gpu, &shadowed, &live, (0, 4), 2, &never(), tray.view())
                .unwrap();
        assert_eq!(
            plain, asked,
            "bit for bit: the declined pass is the plain one"
        );
        assert_eq!(
            resident_tiles(&key_of(&shadowed)),
            None,
            "nothing was cached"
        );
        let unshadowed = SceneState {
            stone_shadow: false,
            ..shadowed
        };
        let lit =
            live::trace_live_pass(&gpu, &unshadowed, &live, (0, 4), 2, &never(), NONE).unwrap();
        assert_ne!(asked, lit, "the declined pass still carries the shadow");
        clear_resident();
    }

    /// A scene with the Tray Reflection declines the cache (`PathCacheOff::TrayReflection`) the
    /// same way: nothing is left resident, and the pass is still rendered WITH the mirror, as the
    /// plain pass is.
    #[test]
    fn a_reflective_scene_declines_the_cache_but_keeps_its_mirror() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        clear_resident();
        let reflective = SceneState {
            tray_reflection: 0.5,
            backdrop: 1.0,
            ..scene(16, 16)
        };
        let live = LiveTiles::all(16, 16, 8);
        let gpu = GpuBackend::disabled();
        let tray = crate::assets::resolve_tray(&reflective, None, &never()).unwrap();
        assert!(tray.mirror.is_some(), "the scene asked for a mirror");
        let plain =
            live::trace_live_pass(&gpu, &reflective, &live, (0, 4), 2, &never(), tray.view())
                .unwrap();
        let asked =
            trace_live_pass_cached(&gpu, &reflective, &live, (0, 4), 2, &never(), tray.view())
                .unwrap();
        assert_eq!(
            plain, asked,
            "bit for bit: the declined pass is the plain one"
        );
        assert_eq!(
            resident_tiles(&key_of(&reflective)),
            None,
            "nothing was cached"
        );
        let matte = SceneState {
            tray_reflection: 0.0,
            ..reflective
        };
        let lit = live::trace_live_pass(&gpu, &matte, &live, (0, 4), 2, &never(), NONE).unwrap();
        assert_ne!(asked, lit, "the declined pass still carries the mirror");
        clear_resident();
    }

    #[test]
    fn a_cancelled_cached_pass_returns_nothing() {
        let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        clear_resident();
        let scene = scene(16, 16);
        let live = LiveTiles::all(16, 16, 8);
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            trace_live_pass_cached(
                &GpuBackend::disabled(),
                &scene,
                &live,
                (0, 4),
                2,
                &cancelled,
                NONE
            ),
            None
        );
        clear_resident();
    }

    #[test]
    fn the_cap_is_eight_gigabytes() {
        assert_eq!(WORKER_CACHE_CAP_BYTES, 8 * 1024 * 1024 * 1024);
    }
}
