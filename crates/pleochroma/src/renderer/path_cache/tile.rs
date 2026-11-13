//! Phase A for one tile: [`TileBuilder`] and the entry point [`build_tile`].
//!
//! A tile is row-major and clipped at the frame's right and bottom edge, like the adaptive
//! sampling tiles, so both work on the same pixel sets. Tiles are independent and deterministic:
//! a caller may build them on any number of threads and gets byte-identical buffers.

use super::{
    build::TraceKit,
    layout::{TileCache, tile_count, tile_rect},
    positions::{BuildSettings, build_pixel},
    record::f32_to_f16_bits,
};
use crate::{
    optics::raytracer::transport::PreparedMaterial, renderer::frame_scene::FrameScene,
    simd::PlanesSoA32,
};

/// Builds the caches of one frame's tiles. Construct once; call [`Self::build_tile`] per tile.
pub struct TileBuilder<'s, 'a> {
    scene: &'s FrameScene<'a>,
    kit: TraceKit<'s, 'a>,
    settings: BuildSettings,
}

impl<'s, 'a> TileBuilder<'s, 'a> {
    /// Prepares the eight hero contexts of the scene's material.
    ///
    /// `plane_soa` must be the intersection arena of `scene.planes`, and `prepared` the prepared
    /// form of `scene.material`. Whether the scene may use a cache at all (non-scattering,
    /// polished, convex, no depth of field) is the caller's decision.
    #[must_use]
    pub fn new(
        scene: &'s FrameScene<'a>,
        prepared: &PreparedMaterial<'a>,
        plane_soa: &'s PlanesSoA32,
        settings: BuildSettings,
    ) -> Self {
        let settings = settings.sanitized();
        let kit = TraceKit::new(
            scene.planes,
            plane_soa,
            prepared,
            (scene.max_bounces, settings.cutoff),
            settings.dispersion_gate,
        );
        Self {
            scene,
            kit,
            settings,
        }
    }

    /// The settings in force (after sanitizing).
    #[must_use]
    pub const fn settings(&self) -> &BuildSettings {
        &self.settings
    }

    /// Number of tiles of the frame.
    #[must_use]
    pub const fn tile_count(&self) -> u32 {
        tile_count(self.scene.width, self.scene.height, self.settings.tile)
    }

    /// Phase A for the tile with this row-major index; an index past the last tile gives an
    /// empty cache.
    #[must_use]
    pub fn build_tile(&self, tile_index: u32) -> TileCache {
        let (width, height) = (self.scene.width, self.scene.height);
        let (x0, y0, x1, y1) = tile_rect(width, height, self.settings.tile, tile_index);
        let mut pixel_offsets = vec![0_u32];
        let mut position_offsets = vec![0_u32];
        let mut residual_bits = Vec::new();
        let mut records = Vec::new();
        if tile_index < self.tile_count() {
            for y in y0..y1 {
                for x in x0..x1 {
                    for position in build_pixel(&self.kit, self.scene, (x, y), &self.settings) {
                        records.extend_from_slice(&position.records);
                        position_offsets.push(u32::try_from(records.len()).unwrap_or(u32::MAX));
                        residual_bits.push(f32_to_f16_bits(position.residual));
                    }
                    pixel_offsets.push(u32::try_from(residual_bits.len()).unwrap_or(u32::MAX));
                }
            }
        }
        TileCache::from_parts(
            tile_index,
            &pixel_offsets,
            &position_offsets,
            &residual_bits,
            &records,
        )
    }
}

/// Phase A for one tile of a frame: the single entry point of the cache builders.
///
/// `plane_soa` is the intersection arena of `scene.planes` and `prepared` the prepared form of
/// `scene.material` (build both once per frame). To build many tiles, make one [`TileBuilder`]
/// instead, which shares the per-material set-up between them.
#[must_use]
pub fn build_tile<'a>(
    scene: &FrameScene<'a>,
    prepared: &PreparedMaterial<'a>,
    plane_soa: &PlanesSoA32,
    tile_index: u32,
    settings: &BuildSettings,
) -> TileCache {
    TileBuilder::new(scene, prepared, plane_soa, *settings).build_tile(tile_index)
}
