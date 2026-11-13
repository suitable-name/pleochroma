//! One adaptive live pass over a set of live tiles.
//!
//! It traces the samples `[first_sample, first_sample + samples)` of
//! a scene for the pixels of the live tiles, returned as one `(pass total, odd-sample
//! total)` pair per live pixel -- the payload of `StreamEvent::LivePass`.
//!
//! # The same engines as a local pass
//!
//! A scene the GPU takes is offered to `GpuBackend::try_accumulate_geom_surface_live_cancellable`,
//! the entry point the desktop export's local adaptive pass uses. Everything else, and a GPU that
//! declines, is traced on the CPU by [`trace_live_pass_cpu`], which mirrors the desktop's
//! `export_thread::worker::core::adaptive::cpu_pass` (that one reads the desktop's `ExportCtx`
//! and cannot move here). The live tiles are claimed one at a time by scoped threads, each tile
//! is traced into a buffer of its own, and the buffers are put back in live-tile order. A pixel's
//! pair is the pass total formed from zero in ascending sample order (`add_finite_sample`) and
//! the same for the odd absolute sample indices, the arithmetic of
//! `cpu_frame::trace_live_tiles_surface`, so the result is bit-equal to it and does not depend on
//! the thread count.
//!
//! The pass is an engine of its own and never splits between CPU and GPU, like the export's.

use super::{PermitGuard, THREAD_PERMITS, effective_thread_count, resolve_facet_finishes};
use crate::assets::TrayRef;
use glam::Vec3;
use pleochroma::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            Camera, DEFAULT_FOV_DEG, EnvironmentSource, FacetFinish, add_finite_sample,
            build_plane_soa, pixel_rotations, sample_draws,
            surface::{SurfaceScene, merged_facet_finishes},
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
    },
    renderer::{
        adaptive::LiveTiles,
        gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef},
        path_cache::PathCacheFrame,
    },
    simd::PlanesSoA32,
};
use pleochroma_net::{SceneState, messages::gather_live};
use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

/// The finished tiles of a live pass: each tile's position in the live order with its pixel pairs.
type TileResults = Vec<(usize, Vec<(Vec3, Vec3)>)>;

/// Everything the tracer of one pass reads, built once per pass and shared by every thread.
struct TileTracer<'a, 'm> {
    camera: &'a Camera,
    width: u32,
    height: u32,
    stone: pleochroma::geometry::StoneGeometry<'a>,
    plane_soa: &'a PlanesSoA32,
    finishes: &'a [FacetFinish],
    prepared: &'a PreparedMaterial<'m>,
    fluorescence: &'a Fluorescence,
    max_bounces: u32,
    environment: EnvironmentSource<'a>,
    surface: SurfaceScene<'a>,
    /// The worker's path cache for this scene: a pixel it has positions for is shaded from its
    /// records.
    cache: Option<&'a PathCacheFrame<'a, 'a>>,
}

impl TileTracer<'_, '_> {
    /// The `(pass sum, odd-sample sum)` of every pixel of the rectangle `(x0, y0, x1, y1)`,
    /// row-major, for the samples `first_sample..first_sample + samples`.
    fn trace_rect(
        &self,
        (x0, y0, x1, y1): (u32, u32, u32, u32),
        first_sample: u32,
        samples: u32,
    ) -> Vec<(Vec3, Vec3)> {
        let mut out = Vec::with_capacity((x1 - x0) as usize * (y1 - y0) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = y * self.width + x;
                let rot = pixel_rotations(pixel);
                let mut pass_sum = Vec3::ZERO;
                let mut pass_odd = Vec3::ZERO;
                for local_sample in 0..samples {
                    let sample_num = first_sample.saturating_add(local_sample);
                    let cached = self
                        .cache
                        .and_then(|cache| cache.shade_sample((x, y), sample_num, &rot));
                    let sample = cached.map_or_else(
                        || {
                            let draws = sample_draws(pixel, sample_num, &rot);
                            let ray = self.camera.generate_ray(
                                x as f32,
                                y as f32,
                                self.width as f32,
                                self.height as f32,
                                draws.jitter_x,
                                draws.jitter_y,
                            );
                            trace_spectral_ray_prepared(
                                ray,
                                self.stone,
                                self.plane_soa,
                                self.finishes,
                                self.prepared,
                                self.fluorescence,
                                self.max_bounces,
                                self.environment,
                                draws.seed,
                                draws.hero_rand,
                                None,
                                self.surface,
                            )
                        },
                        |parts| parts.total(),
                    );
                    // A non-finite sample is dropped but still counted, the rule of every
                    // other backend's sums.
                    add_finite_sample(&mut pass_sum, sample);
                    if !sample_num.is_multiple_of(2) {
                        add_finite_sample(&mut pass_odd, sample);
                    }
                }
                out.push((pass_sum, pass_odd));
            }
        }
        out
    }
}

/// The pass on the CPU: one `(pass sum, odd-sample sum)` pair per live pixel, in the order of
/// [`pleochroma_net::messages::gather_live`] (tiles ascending, row-major inside a tile).
///
/// `threads` is the tracer thread count (`0` for all cores), taken from the process-wide permit
/// pool. Returns `None` when `cancel` was raised before the pass finished.
///
/// `tray` is the request's resolved Stone Shadow and Tray Reflection (`shadow` `Some` exactly when
/// `scene.stone_shadow` is set, `mirror` exactly when `scene.tray_reflection > 0.0`): a live pass
/// belongs to the desktop's adaptive export, so the caller resolved the `Final` map
/// (`crate::assets::resolve_tray` with no intent) and refused the request if it could not.
#[must_use]
pub fn trace_live_pass_cpu(
    scene: &SceneState,
    live: &LiveTiles,
    range: (u32, u32),
    threads: usize,
    cancel: &AtomicBool,
    tray: TrayRef<'_>,
) -> Option<Vec<(Vec3, Vec3)>> {
    trace_live_pass_cpu_with(scene, live, range, threads, cancel, None, tray)
}

/// [`trace_live_pass_cpu`] with a path cache: every pixel `cache` has positions for is shaded from
/// its records (the cache must have been built for `scene`), the others are traced. `None` is
/// [`trace_live_pass_cpu`] itself. `tray` is the request's resolved Stone Shadow and Tray
/// Reflection as there.
#[must_use]
pub fn trace_live_pass_cpu_with(
    scene: &SceneState,
    live: &LiveTiles,
    (first_sample, samples): (u32, u32),
    threads: usize,
    cancel: &AtomicBool,
    cache: Option<&PathCacheFrame<'_, '_>>,
    tray: TrayRef<'_>,
) -> Option<Vec<(Vec3, Vec3)>> {
    let cache = cache.filter(|cache| cache.matches(scene.width, scene.height));
    let tiles = live.indices();
    if tiles.is_empty() || samples == 0 {
        return Some(Vec::new());
    }
    let requested = effective_thread_count(threads).max(1).min(tiles.len());
    let num_threads = THREAD_PERMITS.acquire(requested, cancel)?;
    let _permits = PermitGuard(num_threads);

    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll);
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_for(scene, hdr_map.as_deref(), tray);
    let stone = super::stone_geometry(scene);
    let surfaces = scene.effects.surfaces();
    let resolved = resolve_facet_finishes(scene);
    let finishes = merged_facet_finishes(&resolved, &surfaces);
    let traced_material = scene.traced_material();
    let prepared = PreparedMaterial::new(&traced_material).with_estimator(scene.estimator);
    let plane_soa = build_plane_soa(stone.planes);
    let tracer = TileTracer {
        camera: &camera,
        width: scene.width,
        height: scene.height,
        stone,
        plane_soa: &plane_soa,
        finishes: &finishes,
        prepared: &prepared,
        fluorescence: &scene.fluorescence,
        max_bounces: scene.max_bounces,
        environment,
        surface: scene.effects.surface_scene(&surfaces),
        cache,
    };

    let next_tile = AtomicUsize::new(0);
    let tile_results: Mutex<TileResults> = Mutex::new(Vec::new());
    thread::scope(|s| {
        for _ in 0..num_threads {
            s.spawn(|| {
                let mut mine = Vec::new();
                while !cancel.load(Ordering::Relaxed) {
                    let position = next_tile.fetch_add(1, Ordering::Relaxed);
                    let Some(&tile) = tiles.get(position) else {
                        break;
                    };
                    let rect = live.tile_rect(tile);
                    mine.push((position, tracer.trace_rect(rect, first_sample, samples)));
                }
                tile_results
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend(mine);
            });
        }
    });
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    // Live-tile order, whatever order the threads finished in.
    let mut tile_results = tile_results
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner);
    tile_results.sort_unstable_by_key(|(position, _)| *position);
    Some(
        tile_results
            .into_iter()
            .flat_map(|(_, pixels)| pixels)
            .collect(),
    )
}

/// The pass on the GPU when `scene` goes there and the backend takes it, else on the CPU.
///
/// The result is the pair of every live pixel, as [`trace_live_pass_cpu`] returns. `None` means
/// `cancel` was raised first. `tray` is the request's resolved Stone Shadow and Tray Reflection as
/// there.
#[must_use]
pub fn trace_live_pass(
    gpu: &GpuBackend,
    scene: &SceneState,
    live: &LiveTiles,
    range: (u32, u32),
    threads: usize,
    cancel: &AtomicBool,
    tray: TrayRef<'_>,
) -> Option<Vec<(Vec3, Vec3)>> {
    super::note_tray_route(scene, gpu);
    if super::scene_uses_gpu(scene, gpu) {
        let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
            .with_roll(scene.camera_roll);
        let hdr_map = crate::assets::resolved_hdr_map(scene);
        let environment = crate::assets::environment_for(scene, hdr_map.as_deref(), tray);
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
        let surfaces = scene.effects.surfaces();
        let frame = scene.width as usize * scene.height as usize;
        let mut sum = vec![Vec3::ZERO; frame];
        let mut half_b = vec![Vec3::ZERO; frame];
        let outcome = gpu.try_accumulate_geom_surface_live_cancellable(
            &gpu_scene,
            &scene.tools,
            &scene.effects.surface_scene(&surfaces),
            range.0,
            range.1,
            live,
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
    trace_live_pass_cpu(scene, live, range, threads, cancel, tray)
}

/// The reference for the tests: the `pleochroma` crate's own single-threaded live pass
/// (`cpu_frame::trace_live_tiles_surface`) over `scene`, as one pair per live pixel in payload
/// order. Independent of everything above, so an equality with [`trace_live_pass_cpu`] checks it.
#[cfg(test)]
pub fn reference_live_pass(
    scene: &SceneState,
    live: &LiveTiles,
    (first_sample, samples): (u32, u32),
) -> Vec<(Vec3, Vec3)> {
    use pleochroma::renderer::{cpu_frame::trace_live_tiles_surface, frame_scene::FrameScene};
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
    let surfaces = scene.effects.surfaces();
    let plane_soa = build_plane_soa(&scene.planes);
    let pixels = (scene.width * scene.height) as usize;
    let (mut sum, mut half_b) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    trace_live_tiles_surface(
        &frame,
        (
            &scene.tools,
            &scene.fluorescence,
            scene.effects.surface_scene(&surfaces),
        ),
        &plane_soa,
        live,
        first_sample,
        samples,
        &mut sum,
        &mut half_b,
    );
    gather_live(live, &sum, &half_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_core::tests::tiny_scene;

    fn scene(width: u32, height: u32) -> SceneState {
        SceneState {
            width,
            height,
            ..tiny_scene()
        }
    }

    fn reference(
        scene: &SceneState,
        live: &LiveTiles,
        first_sample: u32,
        samples: u32,
    ) -> Vec<(Vec3, Vec3)> {
        reference_live_pass(scene, live, (first_sample, samples))
    }

    #[test]
    fn the_threaded_cpu_pass_equals_the_reference_live_pass_bit_for_bit() {
        let scene = scene(24, 16);
        // Tiles 0, 2 and 5 of a 3 x 2 grid, an odd first sample so the half buffer takes the
        // first sample of the pass.
        let live = LiveTiles::from_indices(24, 16, 8, vec![0, 2, 5]);
        let never = AtomicBool::new(false);
        let got = trace_live_pass_cpu(&scene, &live, (5, 6), 3, &never, TrayRef::default())
            .expect("not cancelled");
        assert_eq!(got.len(), 3 * 64);
        assert_eq!(got, reference(&scene, &live, 5, 6), "bit for bit");
        assert!(
            got.iter().any(|(sum, _)| *sum != Vec3::ZERO),
            "something was traced"
        );
        // One thread agrees with three.
        let single = trace_live_pass_cpu(&scene, &live, (5, 6), 1, &never, TrayRef::default())
            .expect("not cancelled");
        assert_eq!(single, got);
    }

    #[test]
    fn the_half_buffer_holds_exactly_the_odd_absolute_samples() {
        let scene = scene(16, 8);
        let live = LiveTiles::from_indices(16, 8, 8, vec![0, 1]);
        let never = AtomicBool::new(false);
        // Samples 4 and 5: only 5 is odd.
        let none = TrayRef::default();
        let two = trace_live_pass_cpu(&scene, &live, (4, 2), 2, &never, none).unwrap();
        let five = trace_live_pass_cpu(&scene, &live, (5, 1), 2, &never, none).unwrap();
        for ((_, odd), (single, _)) in two.iter().zip(&five) {
            assert_eq!(odd, single, "B is sample 5 alone");
        }
        // An even-only pass has an empty half buffer.
        let even = trace_live_pass_cpu(&scene, &live, (4, 1), 2, &never, none).unwrap();
        assert!(even.iter().all(|(_, odd)| *odd == Vec3::ZERO));
    }

    #[test]
    fn a_cancelled_pass_returns_nothing() {
        let scene = scene(16, 8);
        let live = LiveTiles::all(16, 8, 8);
        let cancelled = AtomicBool::new(true);
        let none = TrayRef::default();
        assert_eq!(
            trace_live_pass_cpu(&scene, &live, (0, 4), 2, &cancelled, none),
            None
        );
        let gpu = GpuBackend::disabled();
        assert_eq!(
            trace_live_pass(&gpu, &scene, &live, (0, 4), 2, &cancelled, none),
            None
        );
    }

    #[test]
    fn a_disabled_gpu_falls_back_to_the_cpu_pass() {
        let scene = scene(16, 8);
        let live = LiveTiles::from_indices(16, 8, 8, vec![1]);
        let never = AtomicBool::new(false);
        let gpu = GpuBackend::disabled();
        let got =
            trace_live_pass(&gpu, &scene, &live, (0, 4), 2, &never, TrayRef::default()).unwrap();
        assert_eq!(got, reference(&scene, &live, 0, 4));
    }
}
