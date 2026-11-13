//! The path cache of one frame: [`PathCacheFrame`] builds the tiles on demand and shades samples.
//!
//! A frame is built only for a stone that may use the cache (see [`check_eligibility`]): `new`
//! returns the reason otherwise and the caller keeps tracing with the path tracer, which is
//! byte-identical to a render with no cache at all. Tiles are built the first time a pixel of
//! them is shaded, by the deterministic [`TileBuilder`]; the same frame therefore gives the same
//! buffers whatever order its tiles are asked for in.
//!
//! The cache serves the CPU live pass and the CPU full-frame accumulation of
//! `renderer::cpu_frame` (`trace_live_tiles_surface_cached`,
//! `trace_pixels_interleaved_surface_cached`). The caller owns the size and placement limits;
//! [`CacheOptions::budget_bytes`] only carries the number.

use std::sync::OnceLock;

use glam::Vec3;

use super::{
    eligibility::{PathCacheOff, StoneTraits, check_eligibility},
    layout::{TileCache, tile_rect},
    positions::BuildSettings,
    shade::{SampleParts, Shader},
    tile::TileBuilder,
};
use crate::{
    geometry::tool::ToolPrimitive,
    optics::{
        fluorescence::Fluorescence,
        raytracer::{PixelRotations, surface::SurfaceScene, transport::PreparedMaterial},
    },
    renderer::frame_scene::FrameScene,
    simd::PlanesSoA32,
};

/// What a [`PathCacheFrame`] is built with besides the scene.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CacheOptions {
    /// Tile size, position counts and the pruning of phase A.
    pub settings: BuildSettings,
    /// Whether the camera has depth of field (the cache is off then).
    pub depth_of_field: bool,
    /// The most bytes the cache of the frame may hold, if the caller limits it. Carried for the
    /// placement code; the frame does not enforce it.
    pub budget_bytes: Option<u64>,
}

/// The built tiles of one frame, shared by every pass.
pub struct PathCacheFrame<'s, 'a> {
    builder: TileBuilder<'s, 'a>,
    shader: Shader<'s, 'a>,
    tiles: Vec<OnceLock<TileCache>>,
    size: (u32, u32),
    budget_bytes: Option<u64>,
}

impl<'s, 'a> PathCacheFrame<'s, 'a> {
    /// Builds the cache of a frame, or says why the stone may not use one.
    ///
    /// `plane_soa` is the intersection arena of `scene.planes`; `extras` is the same
    /// `(tools, fluorescence, surface)` the CPU passes take, which the eligibility rules read.
    ///
    /// # Errors
    ///
    /// Returns the first eligibility rule the stone breaks ([`PathCacheOff::message`] is the text
    /// to show).
    pub fn new(
        scene: &'s FrameScene<'a>,
        plane_soa: &'s PlanesSoA32,
        extras: (&[ToolPrimitive], &Fluorescence, SurfaceScene<'_>),
        options: CacheOptions,
    ) -> Result<Self, PathCacheOff> {
        if scene.environment.tray_shadow().is_some() {
            return Err(PathCacheOff::StoneShadow);
        }
        if scene.environment.tray_mirror().is_some() {
            return Err(PathCacheOff::TrayReflection);
        }
        // A record's three optical-depth coefficients describe one absorption tensor; a zoned
        // stone has one per zone (`record`).
        if scene.material.zoning.is_some() {
            return Err(PathCacheOff::ZonedColor);
        }
        let (tools, fluorescence, surface) = extras;
        check_eligibility(&StoneTraits {
            material: scene.material,
            facet_finishes: scene.facet_finishes,
            tools,
            fluorescence,
            surface,
            depth_of_field: options.depth_of_field,
        })?;
        let prepared = PreparedMaterial::new(scene.material);
        let builder = TileBuilder::new(scene, &prepared, plane_soa, options.settings);
        let tiles = (0..builder.tile_count()).map(|_| OnceLock::new()).collect();
        Ok(Self {
            builder,
            shader: Shader::new(scene, plane_soa, prepared),
            tiles,
            size: (scene.width, scene.height),
            budget_bytes: options.budget_bytes,
        })
    }

    /// `true` when the cache was built for a frame of `width` x `height` pixels.
    #[must_use]
    pub const fn matches(&self, width: u32, height: u32) -> bool {
        self.size.0 == width && self.size.1 == height
    }

    /// The byte limit the caller gave, if any (not enforced here).
    #[must_use]
    pub const fn budget_bytes(&self) -> Option<u64> {
        self.budget_bytes
    }

    /// The phase A settings in force.
    #[must_use]
    pub const fn settings(&self) -> &BuildSettings {
        self.builder.settings()
    }

    /// Number of tiles of the frame.
    #[must_use]
    pub const fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Number of tiles built so far.
    #[must_use]
    pub fn built_tiles(&self) -> usize {
        self.tiles
            .iter()
            .filter(|tile| tile.get().is_some())
            .count()
    }

    /// Bytes of the tiles built so far.
    #[must_use]
    pub fn built_bytes(&self) -> usize {
        self.tiles
            .iter()
            .filter_map(OnceLock::get)
            .map(TileCache::byte_len)
            .sum()
    }

    /// The row-major index of the tile containing pixel `(x, y)`.
    #[must_use]
    pub const fn tile_of(&self, (x, y): (u32, u32)) -> u32 {
        let tile = self.builder.settings().tile;
        y / tile * self.size.0.div_ceil(tile) + x / tile
    }

    /// The cache of a tile, built on first use; `None` for an index past the last tile.
    #[must_use]
    pub fn tile(&self, index: u32) -> Option<&TileCache> {
        self.tiles
            .get(index as usize)
            .map(|slot| slot.get_or_init(|| self.builder.build_tile(index)))
    }

    /// Puts tiles read from a persisted cache in place of building them, and returns how many
    /// were taken.
    ///
    /// A tile whose index is past the last tile, or that was built already, is dropped. The
    /// caller vouches that the tiles belong to this frame (the cache key names them), so the
    /// loaded frame shades exactly like a built one.
    #[must_use]
    pub fn preload(&self, tiles: Vec<TileCache>) -> usize {
        let mut taken = 0;
        for tile in tiles {
            let index = tile.header().tile_index as usize;
            if let Some(slot) = self.tiles.get(index)
                && slot.set(tile).is_ok()
            {
                taken += 1;
            }
        }
        taken
    }

    /// The tiles built (or preloaded) so far, in tile order, taken out of a frame that is done
    /// with them.
    ///
    /// A frame borrows its scene, so a cache that outlives one pass (the viewport's) keeps the
    /// tiles and hands them back with [`Self::preload`] to the frame of the next pass.
    #[must_use]
    pub fn into_tiles(self) -> Vec<TileCache> {
        self.tiles
            .into_iter()
            .filter_map(OnceLock::into_inner)
            .collect()
    }

    /// Shades sample `sample_num` of pixel `(x, y)` from the cache.
    ///
    /// `None` when the pixel has no cached positions (it is not on the stone, or off the frame):
    /// the caller traces it.
    #[must_use]
    pub fn shade_sample(
        &self,
        (x, y): (u32, u32),
        sample_num: u32,
        rot: &PixelRotations,
    ) -> Option<SampleParts> {
        if x >= self.size.0 || y >= self.size.1 {
            return None;
        }
        let index = self.tile_of((x, y));
        let tile = self.tile(index)?;
        let tile_size = self.builder.settings().tile;
        let (x0, y0, x1, _) = tile_rect(self.size.0, self.size.1, tile_size, index);
        let local = (y - y0) as usize * (x1 - x0) as usize + (x - x0) as usize;
        self.shader
            .shade_sample(tile, local, (x, y), sample_num, rot)
    }

    /// Completes the dropped energy of sample `sample_num` of pixel `(x, y)`, shaded at its
    /// cached position `position` (the index within the pixel).
    ///
    /// The GPU's phase B decides which samples trace their residual and reports them; the host
    /// finishes them here with the ordinary path tracer and the cover test. Zero for a pixel or
    /// position the cache does not have.
    #[must_use]
    pub fn complete_residual(
        &self,
        (x, y): (u32, u32),
        (sample_num, position): (u32, usize),
        rot: &PixelRotations,
    ) -> Vec3 {
        if x >= self.size.0 || y >= self.size.1 {
            return Vec3::ZERO;
        }
        let index = self.tile_of((x, y));
        let Some(tile) = self.tile(index) else {
            return Vec3::ZERO;
        };
        let tile_size = self.builder.settings().tile;
        let (x0, y0, x1, _) = tile_rect(self.size.0, self.size.1, tile_size, index);
        let local = (y - y0) as usize * (x1 - x0) as usize + (x - x0) as usize;
        self.shader
            .complete_residual_of_sample(tile, local, (x, y), (sample_num, position), rot)
    }
}
