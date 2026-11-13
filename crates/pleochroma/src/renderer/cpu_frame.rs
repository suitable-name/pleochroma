//! Wasm-safe, thread-free "trace these pixels x samples" tracer over [`FrameScene`].
//!
//! The shared core both the desktop's threaded `renderer::gpu::hybrid::cpu_trace_range` and a
//! browser's per-Worker CPU tracer call.
//!
//! No threads, no [`std::time::Instant`], no filesystem access: every function here can
//! run inside a wasm32 Web Worker as well as a native thread.
//!
//! # Determinism and partitioning
//!
//! A pixel's sample stream is a pure function of `(pixel, sample_num)` alone (see
//! [`trace_pixels_interleaved`]'s own doc), so ANY partition of a frame's pixels across
//! however many callers -- one thread, one Worker, any `first_pixel`/`stride` choice --
//! produces per-pixel bit-identical summed radiance, as long as every pixel is covered
//! by exactly one partition. [`scatter_interleaved`] adds a partition's results back
//! into a full-frame buffer using the same `first_pixel`/`stride` convention, so summing
//! several partitions' scatters reproduces a single unpartitioned trace exactly.

use glam::Vec3;

use crate::{
    geometry::tool::{StoneGeometry, ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            FacetFinish, PixelRotations, add_finite_sample, pixel_rotations, sample_draws,
            surface::{SurfaceScene, merged_facet_finishes},
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
    },
    simd::PlanesSoA32,
};

use super::{
    adaptive::{AdaptiveImage, AdaptiveRun, AdaptiveSettings, AdaptiveSettingsError, LiveTiles},
    frame_scene::FrameScene,
    path_cache::PathCacheFrame,
};

/// One `(pixel, sample_num)` sample, traced through the real
/// [`trace_spectral_ray_prepared`] -- never a reimplementation of the estimator,
/// only of the per-sample seed/jitter construction around it. The sample's
/// [`SampleDraws`](crate::optics::raytracer::SampleDraws) supplies the path's seed and hero value.
///
/// Takes the batch's `plane_soa` arena (built once by the caller) rather than
/// rebuilding it from `scene.planes` on every one of the many calls a batch makes.
///
/// `rot` is `pixel`'s [`PixelRotations`], computed once per pixel by the caller: it is a
/// pure function of `pixel` alone, so passing the precomputed value to
/// [`sample_draws`] yields the same draws as recomputing it for every sample.
fn cpu_sample_xyz(
    scene: &FrameScene<'_>,
    extras: &SceneExtras<'_>,
    plane_soa: &PlanesSoA32,
    pixel: u32,
    rot: &PixelRotations,
    sample_num: u32,
) -> Vec3 {
    let width = scene.width;
    let x = pixel % width;
    let y = pixel / width;

    let draws = sample_draws(pixel, sample_num, rot);

    let ray = scene.camera.generate_ray(
        x as f32,
        y as f32,
        width as f32,
        scene.height as f32,
        draws.jitter_x,
        draws.jitter_y,
    );
    trace_spectral_ray_prepared(
        ray,
        StoneGeometry {
            planes: scene.planes,
            tools: extras.tools,
        },
        plane_soa,
        extras.facet_finishes,
        &extras.prepared,
        extras.fluorescence,
        scene.max_bounces,
        scene.environment,
        draws.seed,
        draws.hero_rand,
        None,
        extras.surface,
    )
}

/// What rides beside a [`FrameScene`] for one partition: the prepared material (the zone kernels,
/// built once per partition call, not once per sample), the tools, the fluorescence, the facet
/// finishes (already merged with the surface list, see [`merged_facet_finishes`]) and the surface
/// finish of the facets and edges.
struct SceneExtras<'a> {
    prepared: PreparedMaterial<'a>,
    tools: &'a [ToolPrimitive],
    fluorescence: &'a Fluorescence,
    facet_finishes: &'a [FacetFinish],
    surface: SurfaceScene<'a>,
}

/// Traces one interleaved partition of the frame's pixels and returns one SUMMED
/// [`Vec3`] per owned pixel.
///
/// Sample indices `[sample_offset, sample_offset + spp)` are traced for pixels
/// `first_pixel, first_pixel + stride, first_pixel + 2 * stride, …` (stopping once a
/// pixel index would reach `scene.width * scene.height`), in that same ascending
/// order. A non-finite sample is dropped but still counted, like the GPU reduction it
/// merges with (see [`add_finite_sample`]).
///
/// Because a pixel's sample stream depends only on `(pixel, sample_num)` -- never on
/// `first_pixel`, `stride`, or which caller happens to own that pixel -- any partition
/// of a frame's pixels into disjoint interleaved sets, traced independently and
/// recombined with [`scatter_interleaved`], reproduces the exact same per-pixel sums a
/// single unpartitioned call over the whole frame would produce. That is what lets a
/// desktop thread pool and a browser's Web Worker pool call this same function with
/// different `first_pixel`/`stride` splits and still agree bit-for-bit with each other
/// and with a single-threaded reference trace.
#[must_use]
pub fn trace_pixels_interleaved(
    scene: &FrameScene<'_>,
    plane_soa: &PlanesSoA32,
    first_pixel: u32,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    trace_pixels_interleaved_geom(
        scene,
        &[],
        Fluorescence::none(),
        plane_soa,
        first_pixel,
        stride,
        sample_offset,
        spp,
    )
}

/// [`trace_pixels_interleaved`] for a stone with tools.
///
/// `scene.planes` stay the polyhedron and `tools` are subtracted from it; `plane_soa` is
/// the arena built from `scene.planes` alone. `FrameScene` carries no tools field of its
/// own because it is constructed by struct literal in the desktop, worker and web
/// crates, so the tools ride beside it here instead; the `fluorescence` of the material
/// rides beside the tools the same way. With empty `tools` and an empty `fluorescence`
/// the sums are [`trace_pixels_interleaved`]'s, bit for bit. A fluorescent frame, or a stone
/// with more tools than the GPU kernels trace, never reaches the GPU (see
/// `gpu_backend::scene_routes_to_gpu_with_tools`), so this CPU path is the one that traces it;
/// a concave stone within the limit is also traced by the GPU
/// (`GpuBackend::try_accumulate_geom`), with this function as its reference.
#[expect(
    clippy::too_many_arguments,
    reason = "the tools and the fluorescence ride beside the scene, as separate inputs, so \
              no caller has to build a bundle struct"
)]
#[must_use]
pub fn trace_pixels_interleaved_geom(
    scene: &FrameScene<'_>,
    tools: &[ToolPrimitive],
    fluorescence: &Fluorescence,
    plane_soa: &PlanesSoA32,
    first_pixel: u32,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    trace_pixels_interleaved_surface(
        scene,
        (tools, fluorescence, SurfaceScene::IDEAL),
        plane_soa,
        first_pixel,
        stride,
        sample_offset,
        spp,
    )
}

/// [`trace_pixels_interleaved_geom`] with the surface finish of the facets and edges.
///
/// `extras` is `(tools, fluorescence, surface)`; the surface carries the per-facet
/// [`FacetSurface`](crate::optics::raytracer::surface::FacetSurface) words, the faceting machine
/// and the abraded-edge setting. With [`SurfaceScene::IDEAL`] the sums are
/// [`trace_pixels_interleaved_geom`]'s, bit for bit. The facet finish list is merged with the
/// ground words once here, not once per sample; so are the material's zone kernels
/// ([`PreparedMaterial`]).
#[must_use]
pub fn trace_pixels_interleaved_surface(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    first_pixel: u32,
    stride: u32,
    sample_offset: u32,
    spp: u32,
) -> Vec<Vec3> {
    let num_pixels = scene.width as usize * scene.height as usize;
    if stride == 0 || num_pixels == 0 {
        return Vec::new();
    }
    let (tools, fluorescence, surface) = extras;
    let merged_finishes = merged_facet_finishes(scene.facet_finishes, surface.facet_surfaces);
    let extras = SceneExtras {
        prepared: PreparedMaterial::new(scene.material),
        tools,
        fluorescence,
        facet_finishes: &merged_finishes,
        surface,
    };

    let mut out = Vec::new();
    let mut pixel = first_pixel;
    while (pixel as usize) < num_pixels {
        let mut sum = Vec3::ZERO;
        // Per pixel, not per sample: the rotations depend on `pixel` alone (see
        // `PixelRotations`), so hoisting them leaves every sample's draws unchanged.
        let rot = pixel_rotations(pixel);
        for local_sample in 0..spp {
            let sample_num = sample_offset + local_sample;
            add_finite_sample(
                &mut sum,
                cpu_sample_xyz(scene, &extras, plane_soa, pixel, &rot, sample_num),
            );
        }
        out.push(sum);
        pixel += stride;
    }
    out
}

/// Traces one adaptive pass: samples `first_sample..first_sample + spp` for every pixel of the
/// `live` tiles, added into the full-frame buffers `sum` and `half_b`.
///
/// `extras` is as in [`trace_pixels_interleaved_surface`]. `sum` receives every sample, `half_b`
/// only the samples with an odd absolute index; pixels outside the live tiles are untouched in
/// both. Each pixel's samples are the ones [`trace_pixels_interleaved_surface`] traces for the
/// same pixel and sample indices, bit for bit, and are added in ascending sample order (a pixel's
/// pass total is formed first, then added to the buffer, as [`scatter_interleaved`] does). A
/// non-finite sample is dropped, like there.
///
/// Does nothing when the buffers are not `scene.width * scene.height` long or `live` was built
/// for another frame size.
#[expect(
    clippy::too_many_arguments,
    reason = "the scene, its extras, the live tiles, the sample range and the two accumulators \
              are separate inputs so the GPU twin tests call it with the same shape"
)]
pub fn trace_live_tiles_surface(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    live: &LiveTiles,
    first_sample: u32,
    spp: u32,
    sum: &mut [Vec3],
    half_b: &mut [Vec3],
) {
    let num_pixels = scene.width as usize * scene.height as usize;
    if num_pixels == 0
        || sum.len() != num_pixels
        || half_b.len() != num_pixels
        || live.width() != scene.width
        || live.height() != scene.height
    {
        return;
    }
    let (tools, fluorescence, surface) = extras;
    let merged_finishes = merged_facet_finishes(scene.facet_finishes, surface.facet_surfaces);
    let extras = SceneExtras {
        prepared: PreparedMaterial::new(scene.material),
        tools,
        fluorescence,
        facet_finishes: &merged_finishes,
        surface,
    };

    for &tile in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = y * scene.width + x;
                let rot = pixel_rotations(pixel);
                let mut pass_sum = Vec3::ZERO;
                let mut pass_odd = Vec3::ZERO;
                for local_sample in 0..spp {
                    let sample_num = first_sample + local_sample;
                    let sample = cpu_sample_xyz(scene, &extras, plane_soa, pixel, &rot, sample_num);
                    add_finite_sample(&mut pass_sum, sample);
                    if !sample_num.is_multiple_of(2) {
                        add_finite_sample(&mut pass_odd, sample);
                    }
                }
                sum[pixel as usize] += pass_sum;
                half_b[pixel as usize] += pass_odd;
            }
        }
    }
}

/// Renders a whole frame with adaptive sampling on the CPU.
///
/// Drives an [`AdaptiveRun`] with [`trace_live_tiles_surface`]; `extras` is as in
/// [`trace_pixels_interleaved_surface`], `sample_offset` the absolute index of the first sample
/// and `scale` the display multiplier of the convergence check (see [`AdaptiveRun::finish_pass`]).
/// The same inputs give a byte-identical image.
///
/// # Errors
///
/// Returns the rule `settings` breaks, see [`AdaptiveSettings::validate`].
pub fn render_adaptive_surface(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    settings: &AdaptiveSettings,
    sample_offset: u32,
    scale: f32,
) -> Result<AdaptiveImage, AdaptiveSettingsError> {
    let run =
        AdaptiveRun::new(*settings, scene.width, scene.height)?.with_sample_offset(sample_offset);
    Ok(run.run_with(scale, |pass, sum, half_b| {
        trace_live_tiles_surface(
            scene,
            extras,
            plane_soa,
            &pass.live,
            pass.first_sample,
            pass.spp,
            sum,
            half_b,
        );
    }))
}

/// One sample of pixel `(x, y)`: shaded from the path cache when the pixel has cached positions,
/// else traced exactly as [`cpu_sample_xyz`] does.
fn cached_sample_xyz(
    scene: &FrameScene<'_>,
    extras: &SceneExtras<'_>,
    plane_soa: &PlanesSoA32,
    cache: &PathCacheFrame<'_, '_>,
    (x, y): (u32, u32),
    sample_num: u32,
    rot: &PixelRotations,
) -> Vec3 {
    cache.shade_sample((x, y), sample_num, rot).map_or_else(
        || {
            cpu_sample_xyz(
                scene,
                extras,
                plane_soa,
                y * scene.width + x,
                rot,
                sample_num,
            )
        },
        |parts| parts.total(),
    )
}

/// [`trace_live_tiles_surface`] with an optional path cache.
///
/// With `cache` set (and built for this frame size), every pixel the cache has positions for is
/// shaded from its records (`PathCacheFrame::shade_sample`), every other pixel is traced as
/// before; the pass total and the odd-index total are formed and added exactly as there. With
/// `None`, or a cache of another frame size, this is [`trace_live_tiles_surface`] itself, bit
/// for bit.
///
/// `pass` is `(first_sample, spp)` and `buffers` the full-frame `(sum, half_b)`. A cache is only
/// built for an eligible stone (see `PathCacheFrame::new`), so an ineligible render passes
/// `None`.
pub fn trace_live_tiles_surface_cached(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    live: &LiveTiles,
    pass: (u32, u32),
    buffers: (&mut [Vec3], &mut [Vec3]),
    cache: Option<&PathCacheFrame<'_, '_>>,
) {
    let (first_sample, spp) = pass;
    let (sum, half_b) = buffers;
    let Some(cache) = cache.filter(|cache| cache.matches(scene.width, scene.height)) else {
        trace_live_tiles_surface(
            scene,
            extras,
            plane_soa,
            live,
            first_sample,
            spp,
            sum,
            half_b,
        );
        return;
    };
    let num_pixels = scene.width as usize * scene.height as usize;
    if num_pixels == 0
        || sum.len() != num_pixels
        || half_b.len() != num_pixels
        || live.width() != scene.width
        || live.height() != scene.height
    {
        return;
    }
    let (tools, fluorescence, surface) = extras;
    let merged_finishes = merged_facet_finishes(scene.facet_finishes, surface.facet_surfaces);
    let extras = SceneExtras {
        prepared: PreparedMaterial::new(scene.material),
        tools,
        fluorescence,
        facet_finishes: &merged_finishes,
        surface,
    };

    for &tile in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = y * scene.width + x;
                let rot = pixel_rotations(pixel);
                let mut pass_sum = Vec3::ZERO;
                let mut pass_odd = Vec3::ZERO;
                for local_sample in 0..spp {
                    let sample_num = first_sample + local_sample;
                    let sample = cached_sample_xyz(
                        scene,
                        &extras,
                        plane_soa,
                        cache,
                        (x, y),
                        sample_num,
                        &rot,
                    );
                    add_finite_sample(&mut pass_sum, sample);
                    if !sample_num.is_multiple_of(2) {
                        add_finite_sample(&mut pass_odd, sample);
                    }
                }
                sum[pixel as usize] += pass_sum;
                half_b[pixel as usize] += pass_odd;
            }
        }
    }
}

/// [`trace_pixels_interleaved_surface`] with an optional path cache.
///
/// `partition` is `(first_pixel, stride)` and `samples` `(sample_offset, spp)`. Pixels with cached
/// positions are shaded from the cache, the others traced; the sums are formed in the same order
/// as there. With `None`, or a cache of another frame size, this is
/// [`trace_pixels_interleaved_surface`] itself, bit for bit.
#[must_use]
pub fn trace_pixels_interleaved_surface_cached(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    partition: (u32, u32),
    samples: (u32, u32),
    cache: Option<&PathCacheFrame<'_, '_>>,
) -> Vec<Vec3> {
    let (first_pixel, stride) = partition;
    let (sample_offset, spp) = samples;
    let Some(cache) = cache.filter(|cache| cache.matches(scene.width, scene.height)) else {
        return trace_pixels_interleaved_surface(
            scene,
            extras,
            plane_soa,
            first_pixel,
            stride,
            sample_offset,
            spp,
        );
    };
    let num_pixels = scene.width as usize * scene.height as usize;
    if stride == 0 || num_pixels == 0 {
        return Vec::new();
    }
    let (tools, fluorescence, surface) = extras;
    let merged_finishes = merged_facet_finishes(scene.facet_finishes, surface.facet_surfaces);
    let extras = SceneExtras {
        prepared: PreparedMaterial::new(scene.material),
        tools,
        fluorescence,
        facet_finishes: &merged_finishes,
        surface,
    };

    let mut out = Vec::new();
    let mut pixel = first_pixel;
    while (pixel as usize) < num_pixels {
        let mut sum = Vec3::ZERO;
        let rot = pixel_rotations(pixel);
        let xy = (pixel % scene.width, pixel / scene.width);
        for local_sample in 0..spp {
            let sample_num = sample_offset + local_sample;
            add_finite_sample(
                &mut sum,
                cached_sample_xyz(scene, &extras, plane_soa, cache, xy, sample_num, &rot),
            );
        }
        out.push(sum);
        pixel += stride;
    }
    out
}

/// [`render_adaptive_surface`] with an optional path cache, for the live passes of
/// [`trace_live_tiles_surface_cached`].
///
/// # Errors
///
/// Returns the rule `settings` breaks, see [`AdaptiveSettings::validate`].
pub fn render_adaptive_surface_cached(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    settings: &AdaptiveSettings,
    (sample_offset, scale): (u32, f32),
    cache: Option<&PathCacheFrame<'_, '_>>,
) -> Result<AdaptiveImage, AdaptiveSettingsError> {
    let run =
        AdaptiveRun::new(*settings, scene.width, scene.height)?.with_sample_offset(sample_offset);
    Ok(run.run_with(scale, |pass, sum, half_b| {
        trace_live_tiles_surface_cached(
            scene,
            extras,
            plane_soa,
            &pass.live,
            (pass.first_sample, pass.spp),
            (sum, half_b),
            cache,
        );
    }))
}

/// Adds one partition's `sums` back into the full-frame buffer `dst`.
///
/// `sums` must be in the same `first_pixel, first_pixel + stride, …` order
/// [`trace_pixels_interleaved`] produced them in -- this scatters them into `dst` at
/// those same pixel indices. ADDS rather than overwrites, matching every other
/// accumulation buffer in this crate (see `renderer::gpu::hybrid`'s "Merge convention"
/// doc): summing several partitions' scatters into the same zero-initialised buffer
/// reproduces a single unpartitioned trace's sums exactly.
///
/// # Panics
///
/// Panics if `sums` has more entries than `dst` has pixels starting at `first_pixel`
/// and stepping by `stride` -- i.e. if it was not produced by a
/// [`trace_pixels_interleaved`] call against a `dst`-shaped frame with the same
/// `first_pixel`/`stride`.
pub fn scatter_interleaved(dst: &mut [Vec3], first_pixel: u32, stride: u32, sums: &[Vec3]) {
    let mut pixel = first_pixel as usize;
    for &sum in sums {
        dst[pixel] += sum;
        pixel += stride as usize;
    }
}

/// The beam-traced hybrid render of a frame (see `renderer::beam`).
///
/// Beams for the stone, the residual through the engines, batches pipelined; the same output
/// contract as [`render_adaptive_surface`] (`scale` is applied to the final pixels the same way).
/// Polls `cancel` throughout and reports two-phase progress.
///
/// `run` carries the tile restriction (a remote worker renders only its tiles: the tree, the
/// fragments and the residual run for those tiles alone), the progress callback, and the
/// `on_batch` / `retain` hooks (`beam::BeamRun`).
///
/// # Errors
///
/// [`crate::renderer::accel_beam::BeamOff`] when the stone or the lighting is not beam traced;
/// the caller then renders without beams (byte-identical to a render without them). That is also
/// the answer for an `adaptive` that is not valid and for a stone with tools.
///
/// Returns the same [`AdaptiveImage`] as [`render_adaptive_surface`]: `pixels` hold the final
/// pixels (white-balanced beam part plus the residual mean, clamped at zero once), `counts` the
/// residual samples per pixel, `sum` and `half_b` the residual sums with the beam part added once
/// per sample. A cancelled render returns what it finished; the caller checks the token. `engines`
/// may be empty: the render then runs on its own CPU engine, which is also where a batch goes when
/// every engine given has declined. See `beam::pipeline` for the batching, the failover and the
/// progress rules.
#[cfg(feature = "beam")]
#[expect(
    clippy::too_many_arguments,
    reason = "the adaptive entry's arguments plus beams, engines, cancel and progress"
)]
pub fn render_beam_hybrid_surface(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    beam: &crate::renderer::accel_beam::BeamSettings,
    adaptive: &AdaptiveSettings,
    engines: &crate::renderer::beam::engines::EngineSet<'_>,
    (sample_offset, scale): (u32, f32),
    cancel: &std::sync::atomic::AtomicBool,
    run: crate::renderer::beam::BeamRun<'_>,
) -> Result<
    (AdaptiveImage, crate::renderer::beam::stats::BeamStats),
    crate::renderer::accel_beam::BeamOff,
> {
    crate::renderer::beam::pipeline::render_hybrid(
        &crate::renderer::beam::pipeline::HybridInput {
            scene,
            extras,
            plane_soa,
            beam,
            adaptive,
            engines,
            sample_offset,
            scale,
        },
        cancel,
        run,
    )
}

/// The beam-traced hybrid render of the tiles of `run.tiles`, returned as a tiled image rather
/// than a frame-sized one.
///
/// Everything is as for [`render_beam_hybrid_surface`] (the same arguments, the same pixels), but
/// the returned `TiledImage` holds only the rendered tiles, ascending and row-major inside each
/// tile, so a remote worker asked for one tile holds the memory of one tile. Without `run.tiles`
/// it holds every tile of the frame.
///
/// # Errors
///
/// [`crate::renderer::accel_beam::BeamOff`] as for [`render_beam_hybrid_surface`].
#[cfg(feature = "beam")]
#[expect(
    clippy::too_many_arguments,
    reason = "the adaptive entry's arguments plus beams, engines, cancel and progress"
)]
pub fn render_beam_hybrid_tiles_surface(
    scene: &FrameScene<'_>,
    extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
    plane_soa: &PlanesSoA32,
    beam: &crate::renderer::accel_beam::BeamSettings,
    adaptive: &AdaptiveSettings,
    engines: &crate::renderer::beam::engines::EngineSet<'_>,
    (sample_offset, scale): (u32, f32),
    cancel: &std::sync::atomic::AtomicBool,
    run: crate::renderer::beam::BeamRun<'_>,
) -> Result<
    (
        crate::renderer::beam::pipeline::TiledImage,
        crate::renderer::beam::stats::BeamStats,
    ),
    crate::renderer::accel_beam::BeamOff,
> {
    crate::renderer::beam::pipeline::render_hybrid_tiles(
        &crate::renderer::beam::pipeline::HybridInput {
            scene,
            extras,
            plane_soa,
            beam,
            adaptive,
            engines,
            sample_offset,
            scale,
        },
        cancel,
        run,
    )
}
