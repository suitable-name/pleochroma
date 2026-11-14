//! Renders whole beam tiles for a `BeamTileRequest`. [`render_beam_tiles`] returns one
//! `(finished XYZ, residual samples)` pixel for each pixel of the requested tiles, using this
//! machine's own engines.
//!
//! # The same entry as a local render
//!
//! The frame is built as [`super::live::reference_live_pass`] builds it and handed to
//! `cpu_frame::render_beam_hybrid_tiles_surface` with the request's `beam`, `adaptive` and
//! `sample_offset`. The display scale is 1 (the raw means), which is what the coordinator
//! merges. Tiles are independent, so a pixel of a requested tile equals the local render's.
//!
//! Only the requested tiles are rendered (`BeamRun::tiles`), so a one-tile request holds the
//! memory of one tile, not of the frame.
//!
//! # One effective setting
//!
//! Every consumer (the CPU entry, the GPU engine, the residual's wavelength classification)
//! uses the node count the CPU tree really uses (`leaf::node_count`), never the raw request
//! value. A request for 1,032 nodes is a 1,024-node render everywhere.
//!
//! # Engines
//!
//! The CPU engine is the entry's own. This module adds the GPU engine when the worker has a
//! GPU, the `gpu` feature is on and the scene goes to the GPU ([`super::scene_uses_gpu`]).

use pleochroma::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG, build_plane_soa, transport::PreparedMaterial},
    renderer::{
        accel_beam::{BeamOff, BeamSettings, DEFAULT_BEAM_MEMORY_CAP_BYTES},
        adaptive::LiveTiles,
        beam::{BeamRun, engines::EngineSet, leaf::node_count, pipeline::TiledImage},
        cpu_frame::render_beam_hybrid_tiles_surface,
        frame_scene::FrameScene,
        gpu_backend::GpuBackend,
    },
};
use pleochroma_net::messages::{BeamTilePixel, BeamTileRequest, BeamTileStats};
use std::sync::atomic::{AtomicBool, Ordering};

/// The tiles a request asked for, rendered.
#[derive(Debug, Clone, PartialEq)]
pub struct BeamTilesDone {
    /// One pixel per pixel of the requested tiles: tiles ascending, row-major inside a tile.
    pub pixels: Vec<BeamTilePixel>,
    /// The counters of the render (of the whole frame the entry rendered).
    pub stats: BeamTileStats,
}

/// How [`render_beam_tiles`] ended.
#[derive(Debug, Clone, PartialEq)]
pub enum BeamTilesOutcome {
    /// The tiles are rendered.
    Done(BeamTilesDone),
    /// `cancel` was raised first.
    Cancelled,
    /// The stone or the lighting is not beam traced; the coordinator renders these tiles itself.
    Refused(BeamOff),
    /// The render returned something that cannot be sent (a short image or a non-finite pixel).
    Failed(&'static str),
}

/// The engines this worker adds to the entry's own: the GPU engine when it may be used.
///
/// The set is empty on a CPU-only worker: the entry renders on its CPU engine then.
#[cfg(feature = "gpu")]
fn local_engines<'e>(
    gpu: &'e GpuBackend,
    (request, beam): (&BeamTileRequest, &BeamSettings),
    resident: &pleochroma::renderer::beam::tile_residual::ResidualScene<'e>,
) -> EngineSet<'e> {
    use pleochroma::renderer::{beam::engines::BeamEngine, gpu::beam::engine::GpuBeamEngine};
    let mut engines: Vec<Box<dyn BeamEngine + 'e>> = Vec::new();
    if gpu.adapter_label().is_some() && super::scene_uses_gpu(&request.scene, gpu) {
        engines.push(Box::new(
            GpuBeamEngine::new(gpu, resident, beam, &request.adaptive).with_display_scale(1.0),
        ));
    }
    EngineSet::new(engines)
}

/// The engines of a build without the `gpu` feature: none beside the entry's CPU engine.
#[cfg(not(feature = "gpu"))]
fn local_engines<'e>(
    gpu: &'e GpuBackend,
    (request, beam): (&BeamTileRequest, &BeamSettings),
    resident: &pleochroma::renderer::beam::tile_residual::ResidualScene<'e>,
) -> EngineSet<'e> {
    let _ = (gpu, request, beam, resident);
    EngineSet::new(Vec::new())
}

/// The pixels of the tile-ordered `image` of `live`'s tiles, checked to be what the request asked
/// for: the tiles of `live`, every pixel finite.
fn gather(live: &LiveTiles, image: &TiledImage) -> Result<Vec<BeamTilePixel>, &'static str> {
    let wanted: usize = live
        .indices()
        .iter()
        .map(|&tile| {
            let (x0, y0, x1, y1) = live.tile_rect(tile);
            (x1 - x0) as usize * (y1 - y0) as usize
        })
        .sum();
    if image.tiles.as_slice() != live.indices()
        || image.pixels.len() != wanted
        || image.counts.len() != wanted
    {
        return Err("the beam render returned other tiles than the request asked for");
    }
    let mut out = Vec::with_capacity(wanted);
    for (&xyz, &samples) in image.pixels.iter().zip(&image.counts) {
        if !xyz.is_finite() {
            return Err("the beam render produced a non-finite pixel");
        }
        out.push(BeamTilePixel { xyz, samples });
    }
    Ok(out)
}

/// The most memory a worker lets the beam producer's look-ahead and kept-index packing use,
/// whatever a request asks for.
pub const WORKER_BEAM_MEMORY_CAP: u64 = DEFAULT_BEAM_MEMORY_CAP_BYTES;

/// The settings a worker renders `requested` with.
///
/// The node count is the one the CPU tree uses, the memory cap is the smaller of the client's
/// and [`WORKER_BEAM_MEMORY_CAP`], and no leaf cache is kept. These are the worker's own
/// resources, not the client's.
fn effective_settings(requested: &BeamSettings) -> BeamSettings {
    BeamSettings {
        spectral_nodes: node_count(requested),
        memory_cap_bytes: requested.memory_cap_bytes.min(WORKER_BEAM_MEMORY_CAP),
        leaf_cache_bytes: 0,
        keep_leaves: false,
        ..*requested
    }
}

/// Renders the tiles `live` of `request` (see the module doc comment).
///
/// `live` must be what `validate::validate_beam_tiles` returned for `request`. Polls `cancel`
/// through the entry; a raised token gives [`BeamTilesOutcome::Cancelled`].
#[must_use]
pub fn render_beam_tiles(
    gpu: &GpuBackend,
    request: &BeamTileRequest,
    live: &LiveTiles,
    cancel: &AtomicBool,
) -> BeamTilesOutcome {
    let scene = &request.scene;
    if scene.stone_shadow {
        return BeamTilesOutcome::Refused(BeamOff::StoneShadow);
    }
    if scene.tray_reflection > 0.0 {
        return BeamTilesOutcome::Refused(BeamOff::TrayReflection);
    }
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll)
        .with_pan(scene.camera_pan);
    let hdr_map = crate::assets::resolved_hdr_map(scene);
    let environment = crate::assets::environment_source(scene, hdr_map.as_deref());
    let finishes = super::resolve_facet_finishes(scene);
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
    // Beam tiles always trace with branching: the beam engine has no exit collection, and its
    // residual must use the same estimator.
    let prepared = PreparedMaterial::new(&material)
        .with_estimator(pleochroma::optics::raytracer::Estimator::Branching);
    // The effective settings: the node count the CPU tree uses, for the CPU entry, the GPU
    // engine and the residual alike.
    let beam = effective_settings(&request.beam);
    let resident = pleochroma::renderer::beam::tile_residual::ResidualScene {
        frame: &frame,
        extras: (
            &scene.tools,
            &scene.fluorescence,
            scene.effects.surface_scene(&surfaces),
        ),
        plane_soa: &plane_soa,
        prepared: &prepared,
        n_lambda: beam.spectral_nodes,
        sample_offset: request.sample_offset,
        residual_min_spp: beam.residual_min_spp,
    };
    let engines = local_engines(gpu, (request, &beam), &resident);
    let mut progress = |_: pleochroma::renderer::beam::BeamProgress| {};
    let rendered = render_beam_hybrid_tiles_surface(
        &frame,
        (
            &scene.tools,
            &scene.fluorescence,
            scene.effects.surface_scene(&surfaces),
        ),
        &plane_soa,
        &beam,
        &request.adaptive,
        &engines,
        (request.sample_offset, 1.0),
        cancel,
        BeamRun {
            tiles: Some(live),
            ..BeamRun::new(&mut progress)
        },
    );
    if cancel.load(Ordering::Relaxed) {
        return BeamTilesOutcome::Cancelled;
    }
    let (image, stats) = match rendered {
        Ok(done) => done,
        Err(off) => return BeamTilesOutcome::Refused(off),
    };
    let pixels = match gather(live, &image) {
        Ok(pixels) => pixels,
        Err(why) => return BeamTilesOutcome::Failed(why),
    };
    let residual_samples = pixels.iter().map(|p| u64::from(p.samples)).sum();
    BeamTilesOutcome::Done(BeamTilesDone {
        pixels,
        stats: BeamTileStats {
            beams: stats.beams,
            leaves: stats.leaves,
            fragments: stats.fragments,
            drops: stats.drops.iter().sum(),
            residual_samples,
        },
    })
}

/// The effective settings never take the client's memory cap above the worker's, nor its leaf
/// cache or kept leaves.
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
#[test]
fn the_effective_memory_cap_is_the_minimum() {
    let greedy = BeamSettings {
        memory_cap_bytes: u64::MAX,
        leaf_cache_bytes: u64::MAX,
        keep_leaves: true,
        ..BeamSettings::default()
    };
    let effective = effective_settings(&greedy);
    assert_eq!(effective.memory_cap_bytes, WORKER_BEAM_MEMORY_CAP);
    assert_eq!(effective.leaf_cache_bytes, 0);
    assert!(!effective.keep_leaves);
    let modest = BeamSettings {
        memory_cap_bytes: 1 << 20,
        ..BeamSettings::default()
    };
    assert_eq!(effective_settings(&modest).memory_cap_bytes, 1 << 20);
}

/// The unrestricted local render of the whole frame of `request`: the reference the e2e tests
/// compare a worker's tiles with.
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
pub fn reference_beam_tiles(request: &BeamTileRequest, live: &LiveTiles) -> BeamTilesOutcome {
    let never = AtomicBool::new(false);
    render_beam_tiles(&GpuBackend::disabled(), request, live, &never)
}
