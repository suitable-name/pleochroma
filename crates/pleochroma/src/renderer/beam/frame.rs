//! `BeamFrame`: the per-tile driver of the beam render (beam guide §14.5).
//!
//! Per tile it collects the primary beams, their reflection leaves and the beams inside the stone,
//! then shades the leaves. A tile is computed by one thread in a fixed order and written to its
//! own pixels, so the image is bit-identical for any thread count. Reflection renders end with the
//! tracer's `finish` (white balance, then `max(0)`); full beam renders stay unclamped until the
//! residual is added (beam guide §5.3).

use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Instant,
};

use glam::{DVec3, Vec3};

use super::{
    BeamOff, BeamSettings,
    aliasing::worst_node_step,
    beam::{Beam, IntervalSplit, Leaf, SeamStore, SpectralInterval, TileBuffer},
    entry::refract_entry,
    geom::Tolerances,
    interact::{InteractKit, internal_hit},
    leaf::{
        ReflectionKit, node_count, peak_weight, reflection_interpolates, reflection_leaf,
        split_retraced,
    },
    lighting::LightingScene,
    primary::{TileRect, image_polygon, primary_beams_counted, split_beam},
    propagate::{partition, project_to},
    raster::shade_tile,
    residual::KeptIndex,
    retrace::RetraceKit,
    snap::snap_partition,
    spectral::{
        check_diverged, check_interval, initial_intervals, retarget, shrink_leaf,
        strictly_constant, trace_end_nodes,
    },
    stats::{BeamStats, DropReason, TILE_ALLOWANCE},
    stone::BeamStone,
    subnodes::out_of_scope,
};
use crate::{
    optics::{
        fluorescence::Fluorescence,
        raytracer::{
            EnvironmentSource, apply_von_kries_white_balance,
            environment::environment_white_balance, surface::SurfaceScene,
            transport::PreparedMaterial,
        },
        studio_rig::StudioRig,
    },
    renderer::{
        frame_scene::FrameScene,
        path_cache::eligibility::{StoneTraits, check_eligibility},
    },
};

/// The log target of the beam tracer.
const LOG_TARGET: &str = "pleochroma::beam";

/// Beams with an image area at or below this many square pixels are not split further: the
/// interpolation error of a sub-pixel piece is below what the pixel average resolves.
pub const MIN_SPLIT_AREA_PX2: f64 = 1.0;

/// The bounces a first-surface reflection needs to be shaded: a path with `d` events is shaded iff
/// `d <= max_bounces - 1` (beam guide §5.4), and a reflection is `d = 1`.
pub const REFLECTION_MIN_BOUNCES: u32 = 2;

/// The bounces a refracted path needs to be shaded: the entry and the exit are two events, so the
/// path is shaded iff `2 <= max_bounces - 1` (beam guide §5.4).
pub const REFRACTION_MIN_BOUNCES: u32 = 3;

/// How often a beam whose partition, snapping or projection fails is bisected and tried again
/// before its region is dropped (beam guide §7.4 step 8).
pub const MAX_REPARTITIONS: u32 = 2;

/// The beam part of a frame, the kept index of its tiles and the counters (beam guide §14.5).
#[derive(Clone, Debug, PartialEq)]
pub struct BeamImage {
    /// The beam part per pixel (row-major, `width * height`), white-balanced and **not** clamped:
    /// the residual is added before the end clamp (§5.3).
    pub xyz: Vec<Vec3>,
    /// The kept index of every tile, in tile order.
    pub kept: Vec<(TileRect, KeptIndex)>,
    /// The merged counters.
    pub stats: BeamStats,
}

/// A beam on the stack of the tree walk with the number of re-partitions that led to it.
struct Work {
    /// The beam to expand.
    beam: Beam,
    /// How often its ancestors were bisected after a failed partition.
    retries: u32,
}

/// What one reflection tile produces, `None` when the render was cancelled.
type ReflectionTile = Option<(TileBuffer, BeamStats)>;

/// What one tile of the full tree produces.
type BeamTile = (TileBuffer, BeamStats, KeptIndex);

/// The beam render of one frame (beam guide §14.5).
pub struct BeamFrame<'s, 'a> {
    /// The scene the frame renders.
    scene: &'s FrameScene<'a>,
    /// The settings in force.
    settings: BeamSettings,
    /// The spectral node count `N_λ`, a multiple of 8.
    n_lambda: u32,
    /// The stone, built once.
    stone: BeamStone,
    /// The shared material and camera data of the reflection leaves.
    kit: ReflectionKit<'a>,
    /// The lighting of the fragments.
    lighting: LightingScene<'a>,
    /// The Bradford white balance of the studio environment, `None` for an HDR map.
    white_balance: Option<Vec3>,
    /// The spectral intervals the refracted trees start with (beam guide §9.2).
    intervals: Vec<SpectralInterval>,
    /// The material disperses: intervals are checked, split and their regions formed at the bin
    /// edges. `false` keeps the single all-node interval and the unchecked walk.
    checked: bool,
    /// The attempted work one tile may spend ([`TILE_ALLOWANCE`] unless a budget is given).
    allowance: u64,
}

impl<'s, 'a> BeamFrame<'s, 'a> {
    /// Builds the beam frame of `scene`, or says why the stone is not beam traced.
    ///
    /// # Errors
    ///
    /// [`BeamOff::NotEligible`] when the stone fails a path-cache eligibility rule (the scene
    /// carries no tools, fluorescence, surface roughness or depth of field, so the material and
    /// the facet finishes decide), [`BeamOff::EdgeRounding`] for a material that rounds its edges,
    /// [`BeamOff::OutOfScopeLighting`] for a UV lamp, [`BeamOff::StoneShadow`] when the
    /// environment carries a stone shadow on the paper (beam tracing has no per-landing
    /// lookup), [`BeamOff::TrayReflection`] when it carries a glossy tray (a floor bounce
    /// continues the path, which beams do not follow), and the errors of `BeamStone::new`
    /// ([`BeamOff::DegenerateStone`]).
    pub fn new(scene: &'s FrameScene<'a>, settings: &BeamSettings) -> Result<Self, BeamOff> {
        let stone = Self::accepted_stone(scene)?;
        Ok(Self::assemble(
            scene,
            settings,
            stone,
            PreparedMaterial::new(scene.material),
        ))
    }

    /// [`BeamFrame::new`] over a material the caller already prepared, so the optics tables are
    /// built once and shared with the eligibility check, the engines and the frame.
    ///
    /// `prepared` must have been built from `scene.material`. The frame is the one `new` builds;
    /// only the preparation is shared instead of repeated.
    ///
    /// # Errors
    ///
    /// The errors of [`BeamFrame::new`].
    pub fn with_prepared(
        scene: &'s FrameScene<'a>,
        settings: &BeamSettings,
        prepared: PreparedMaterial<'a>,
    ) -> Result<Self, BeamOff> {
        let stone = Self::accepted_stone(scene)?;
        Ok(Self::assemble(scene, settings, stone, prepared))
    }

    /// Whether [`BeamFrame::new`] accepts `scene`, without building the frame (no material
    /// preparation, no optics tables): the eligibility check of a caller that builds the frame
    /// later.
    ///
    /// # Errors
    ///
    /// The errors of [`BeamFrame::new`].
    pub fn accepts(scene: &FrameScene<'_>) -> Result<(), BeamOff> {
        Self::accepted_stone(scene).map(|_| ())
    }

    /// The prepared material the frame's reflection kit reads.
    #[must_use]
    pub const fn prepared(&self) -> &PreparedMaterial<'a> {
        self.kit.prepared()
    }

    /// The stone of `scene` when the frame accepts it, else the reason it does not.
    fn accepted_stone(scene: &FrameScene<'_>) -> Result<BeamStone, BeamOff> {
        if scene.environment.tray_shadow().is_some() {
            return Err(BeamOff::StoneShadow);
        }
        if scene.environment.tray_mirror().is_some() {
            return Err(BeamOff::TrayReflection);
        }
        check_eligibility(&StoneTraits {
            material: scene.material,
            facet_finishes: scene.facet_finishes,
            tools: &[],
            fluorescence: Fluorescence::none(),
            surface: SurfaceScene::IDEAL,
            depth_of_field: false,
        })
        .map_err(BeamOff::NotEligible)?;
        if scene.material.edge_rounding_radius > 0.0 {
            return Err(BeamOff::EdgeRounding);
        }
        if out_of_scope(&scene.environment) {
            return Err(BeamOff::OutOfScopeLighting);
        }
        BeamStone::new(scene.planes, false)
    }

    /// The frame of an accepted `scene` over its `stone` and prepared material.
    fn assemble(
        scene: &'s FrameScene<'a>,
        settings: &BeamSettings,
        stone: BeamStone,
        prepared: PreparedMaterial<'a>,
    ) -> Self {
        let n_lambda = node_count(settings);
        let size = (scene.width, scene.height);
        let (rig, white_balance) = match scene.environment {
            EnvironmentSource::Studio {
                light_yaw,
                light_pitch,
                ..
            } => (
                Some(StudioRig::new(light_yaw, light_pitch)),
                Some(environment_white_balance(scene.environment)),
            ),
            EnvironmentSource::HdrMap { .. } => (None, None),
        };
        Self {
            scene,
            settings: *settings,
            n_lambda,
            stone,
            kit: ReflectionKit::with_prepared(
                prepared,
                scene.planes,
                scene.camera,
                size,
                (&scene.environment, n_lambda),
            ),
            lighting: LightingScene {
                environment: scene.environment,
                rig,
                camera: scene.camera,
                size,
                tolerance: settings.lighting_tolerance,
                max_levels: settings.max_fragment_levels,
                n_lambda,
            },
            white_balance,
            intervals: initial_intervals(scene.material, n_lambda),
            checked: !strictly_constant(scene.material, n_lambda),
            allowance: TILE_ALLOWANCE,
        }
    }

    /// This frame with `allowance` units of attempted work per tile instead of [`TILE_ALLOWANCE`].
    #[must_use]
    pub const fn with_tile_allowance(mut self, allowance: u64) -> Self {
        self.allowance = allowance;
        self
    }

    /// The attempted work one tile may spend.
    #[must_use]
    pub const fn tile_allowance(&self) -> u64 {
        self.allowance
    }

    /// The stone the frame traces.
    #[must_use]
    pub const fn stone(&self) -> &BeamStone {
        &self.stone
    }

    /// The spectral node count in force (a multiple of 8).
    #[must_use]
    pub const fn n_lambda(&self) -> u32 {
        self.n_lambda
    }

    /// Renders the first-surface reflections of the frame on all available cores.
    ///
    /// See [`BeamFrame::render_reflections_with_threads`].
    #[must_use]
    pub fn render_reflections(
        &self,
        tile_size: u32,
        cancel: &AtomicBool,
    ) -> Option<(Vec<Vec3>, BeamStats)> {
        let threads = super::pipeline::default_threads();
        self.render_reflections_with_threads(tile_size, threads, cancel)
    }

    /// Renders the first-surface reflections of the frame with `threads` worker threads.
    ///
    /// Returns the per-pixel XYZ (row-major, `width * height`, after the white balance of the
    /// preset's illuminant and the end clamp `max(0)`, applied once per pixel as the tracer's
    /// `finish` does) and the merged statistics. Pixels the reflections miss are zero. Tiles
    /// are `tile_size` pixels square, taken row-major; each is computed by one thread in a fixed
    /// order and written to its own pixels, so the result is bit-identical for any `threads`.
    /// `cancel` is polled per beam and per tile; the result is `None` when it was set.
    #[must_use]
    pub fn render_reflections_with_threads(
        &self,
        tile_size: u32,
        threads: usize,
        cancel: &AtomicBool,
    ) -> Option<(Vec<Vec3>, BeamStats)> {
        let size = (self.scene.width, self.scene.height);
        let (columns, rows) = TileRect::grid(size, tile_size);
        let tile_count = columns as usize * rows as usize;
        let threads = threads.clamp(1, tile_count.max(1));
        let mut tiles: Vec<Option<(TileBuffer, BeamStats)>> = Vec::new();
        tiles.resize_with(tile_count, || None);
        if threads == 1 {
            for (index, slot) in tiles.iter_mut().enumerate() {
                *slot = self.render_tile(index, tile_size, cancel);
            }
        } else {
            let next = AtomicUsize::new(0);
            let done: Vec<Vec<(usize, ReflectionTile)>> = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        scope.spawn(|| {
                            let mut mine = Vec::new();
                            loop {
                                let index = next.fetch_add(1, Ordering::Relaxed);
                                if index >= tile_count || cancel.load(Ordering::Relaxed) {
                                    break;
                                }
                                mine.push((index, self.render_tile(index, tile_size, cancel)));
                            }
                            mine
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .map(|worker| match worker.join() {
                        Ok(mine) => mine,
                        Err(payload) => std::panic::resume_unwind(payload),
                    })
                    .collect()
            });
            for (index, result) in done.into_iter().flatten() {
                tiles[index] = result;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let mut image = vec![Vec3::ZERO; size.0 as usize * size.1 as usize];
        let mut stats = BeamStats::default();
        for tile in tiles {
            let (buffer, tile_stats) = tile?;
            stats.merge(&tile_stats);
            self.write_tile(&buffer, size.0, &mut image);
        }
        Some((image, stats))
    }

    /// Writes the finished pixels of `buffer` into the row-major `image` of width `width`.
    fn write_tile(&self, buffer: &TileBuffer, width: u32, image: &mut [Vec3]) {
        let (x0, y0) = buffer.origin;
        let (tile_width, tile_height) = buffer.size;
        for row in 0..tile_height {
            for column in 0..tile_width {
                let source = (row * tile_width + column) as usize;
                let target = ((y0 + row) * width + x0 + column) as usize;
                image[target] = self.finish_pixel(buffer.xyz[source]);
            }
        }
    }

    /// The tracer's `finish` for a beam pixel: the Bradford white balance of a studio environment,
    /// then `max(0)` for every environment.
    fn finish_pixel(&self, xyz: DVec3) -> Vec3 {
        let linear = xyz.as_vec3();
        let balanced = self
            .white_balance
            .map_or(linear, |scale| apply_von_kries_white_balance(linear, scale));
        balanced.max(Vec3::ZERO)
    }

    /// The beams, leaves and shaded buffer of tile `index`; `None` when cancelled.
    fn render_tile(
        &self,
        index: usize,
        tile_size: u32,
        cancel: &AtomicBool,
    ) -> Option<(TileBuffer, BeamStats)> {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let size = (self.scene.width, self.scene.height);
        let tile_index = u32::try_from(index).ok()?;
        let rect = TileRect::of_index(size, tile_size, tile_index)?;
        let mut stats = BeamStats::default();
        let mut leaves: Vec<Leaf> = Vec::new();
        if self.scene.max_bounces >= REFLECTION_MIN_BOUNCES {
            let spectrum = SpectralInterval::all(self.n_lambda);
            let primaries = primary_beams_counted(
                &self.stone,
                self.scene.camera,
                size,
                rect,
                spectrum,
                &mut stats,
            );
            for beam in primaries {
                stats.primary_beams += 1;
                self.collect_leaves(&beam, 0, &mut leaves, &mut stats, cancel)?;
            }
        }
        for leaf in &mut leaves {
            leaf.tile = tile_index;
        }
        stats.leaves += leaves.len() as u64;
        let buffer = shade_tile(&leaves, rect, &self.lighting, &mut stats);
        Some((buffer, stats))
    }

    /// Turns `beam` into reflection leaves, bisecting it while the weight interpolation check
    /// fails (beam guide §8.4) down to `max_split_depth` or [`MIN_SPLIT_AREA_PX2`].
    ///
    /// A beam that cannot be made a leaf counts as a sliver drop. A leaf that fails the check at
    /// the cap is kept. `None` when cancelled.
    fn collect_leaves(
        &self,
        beam: &Beam,
        depth: u32,
        leaves: &mut Vec<Leaf>,
        stats: &mut BeamStats,
        cancel: &AtomicBool,
    ) -> Option<()> {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        stats.record_beam(beam.bounces);
        // Leaf attempts count against the tile allowance; reflection leaves are never dropped.
        stats.record_attempt();
        let Some(leaf) = reflection_leaf(beam, &self.stone, &self.kit, &self.settings) else {
            stats.record_drop(DropReason::Sliver, 0.0);
            return Some(());
        };
        let splittable = depth < self.settings.max_split_depth
            && leaf.image.area() > MIN_SPLIT_AREA_PX2
            && !reflection_interpolates(beam, &self.stone, &self.kit, &self.settings, &leaf);
        if splittable {
            let size = (self.scene.width, self.scene.height);
            if let Some((first, second)) = split_beam(beam, &self.stone, self.scene.camera, size) {
                self.collect_leaves(&first, depth + 1, leaves, stats, cancel)?;
                return self.collect_leaves(&second, depth + 1, leaves, stats, cancel);
            }
        }
        leaves.push(leaf);
        Some(())
    }
}

impl BeamFrame<'_, '_> {
    /// Renders the full beam tree of the frame on all available cores.
    ///
    /// See [`BeamFrame::render_beams_with_threads`].
    #[must_use]
    pub fn render_beams(&self, tile_size: u32, cancel: &AtomicBool) -> Option<BeamImage> {
        let threads = super::pipeline::default_threads();
        self.render_beams_with_threads(tile_size, threads, cancel)
    }

    /// Renders the full beam tree of the frame with `threads` worker threads (beam guide §14.5).
    ///
    /// Per tile, depth first in canonical child order: the primary beams, their first-surface
    /// reflection leaves and entry refraction, then a stack of inside beams, each partitioned by
    /// exit facet (snapped, at most [`MAX_REPARTITIONS`] bisections on a failure, then a drop),
    /// projected to its next facet and expanded by `interact::internal_hit` into exit leaves and
    /// reflected beams. The leaves are shaded per tile and indexed for the residual. The beam part
    /// is white-balanced but not clamped. Tiles are `tile_size` pixels square and computed by one
    /// thread each in a fixed order, so the result is bit-identical for any `threads`. `cancel` is
    /// polled after every beam pop and per tile; the result is `None` when it was set.
    #[must_use]
    pub fn render_beams_with_threads(
        &self,
        tile_size: u32,
        threads: usize,
        cancel: &AtomicBool,
    ) -> Option<BeamImage> {
        let size = (self.scene.width, self.scene.height);
        let tiles = self.run_tiles(tile_size, threads, cancel, |index| {
            self.render_beam_tile(index, tile_size, cancel)
        })?;
        let mut xyz = vec![Vec3::ZERO; size.0 as usize * size.1 as usize];
        let mut stats = BeamStats::default();
        let mut kept = Vec::with_capacity(tiles.len());
        for (buffer, tile_stats, index) in tiles {
            stats.merge(&tile_stats);
            self.write_balanced(&buffer, size.0, &mut xyz);
            kept.push((index.tile(), index));
        }
        Some(BeamImage { xyz, kept, stats })
    }

    /// Runs `work` on every tile index, on `threads` workers taking indices from one counter, and
    /// returns the results in tile order; `None` when `cancel` was set or a tile gave nothing.
    fn run_tiles<T: Send>(
        &self,
        tile_size: u32,
        threads: usize,
        cancel: &AtomicBool,
        work: impl Fn(usize) -> Option<T> + Sync,
    ) -> Option<Vec<T>> {
        let size = (self.scene.width, self.scene.height);
        let (columns, rows) = TileRect::grid(size, tile_size);
        let tile_count = columns as usize * rows as usize;
        let threads = threads.clamp(1, tile_count.max(1));
        let mut tiles: Vec<Option<T>> = Vec::new();
        tiles.resize_with(tile_count, || None);
        if threads == 1 {
            for (index, slot) in tiles.iter_mut().enumerate() {
                *slot = work(index);
            }
        } else {
            let next = AtomicUsize::new(0);
            let done: Vec<Vec<(usize, Option<T>)>> = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads)
                    .map(|_| {
                        scope.spawn(|| {
                            let mut mine = Vec::new();
                            loop {
                                let index = next.fetch_add(1, Ordering::Relaxed);
                                if index >= tile_count || cancel.load(Ordering::Relaxed) {
                                    break;
                                }
                                mine.push((index, work(index)));
                            }
                            mine
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .map(|worker| match worker.join() {
                        Ok(mine) => mine,
                        Err(payload) => std::panic::resume_unwind(payload),
                    })
                    .collect()
            });
            for (index, result) in done.into_iter().flatten() {
                tiles[index] = result;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        tiles.into_iter().collect()
    }

    /// The white balance of a studio environment on a beam pixel, without the end clamp.
    fn balance(&self, xyz: DVec3) -> Vec3 {
        self.balanced(xyz.as_vec3())
    }

    /// The white balance of a studio environment on a beam part `xyz` (single precision, as the
    /// engines return it), without the end clamp; the identity for an HDR map.
    ///
    /// The batch pipeline balances the fragment sums with it before they become the offset of the
    /// residual, and adds the residual mean before the one end clamp per pixel (beam guide §5.3).
    #[must_use]
    pub fn balanced(&self, xyz: Vec3) -> Vec3 {
        self.white_balance
            .map_or(xyz, |scale| apply_von_kries_white_balance(xyz, scale))
    }

    /// Writes the white-balanced, unclamped pixels of `buffer` into the row-major `image` of width
    /// `width`.
    fn write_balanced(&self, buffer: &TileBuffer, width: u32, image: &mut [Vec3]) {
        let (x0, y0) = buffer.origin;
        let (tile_width, tile_height) = buffer.size;
        for row in 0..tile_height {
            for column in 0..tile_width {
                let source = (row * tile_width + column) as usize;
                let target = ((y0 + row) * width + x0 + column) as usize;
                image[target] = self.balance(buffer.xyz[source]);
            }
        }
    }

    /// The shaded buffer, counters and kept index of tile `index`; `None` when cancelled.
    fn render_beam_tile(
        &self,
        index: usize,
        tile_size: u32,
        cancel: &AtomicBool,
    ) -> Option<BeamTile> {
        let (leaves, mut stats, rect) = self.tile_leaves(index, tile_size, cancel)?;
        let buffer = shade_tile(&leaves, rect, &self.lighting, &mut stats);
        let kept = KeptIndex::from_leaves(rect, &leaves);
        Some((buffer, stats, kept))
    }

    /// The white-balanced, unclamped beam part of `leaves` on `rect`, one value per pixel of
    /// `rect` (row-major), as the full render shades them.
    ///
    /// It lets the equivalence rows split a pixel's beam value by leaf signature.
    #[must_use]
    pub fn shade_leaves(&self, leaves: &[Leaf], rect: TileRect) -> Vec<Vec3> {
        let mut stats = BeamStats::default();
        let buffer = shade_tile(leaves, rect, &self.lighting, &mut stats);
        buffer.xyz.iter().map(|&xyz| self.balance(xyz)).collect()
    }

    /// Whether tile `index` of the `tile_size`-pixel grid holds a primary beam, that is, whether
    /// [`Self::tile_leaves`] would count `primary_beams > 0` for it. Costs the clip of the front
    /// facets against the tile and no tree; `false` past the last tile.
    #[must_use]
    pub fn tile_has_primary(&self, index: usize, tile_size: u32) -> bool {
        if self.scene.max_bounces < REFLECTION_MIN_BOUNCES {
            return false;
        }
        let size = (self.scene.width, self.scene.height);
        let Some(rect) = u32::try_from(index)
            .ok()
            .and_then(|tile_index| TileRect::of_index(size, tile_size, tile_index))
        else {
            return false;
        };
        let spectrum = SpectralInterval::all(self.n_lambda);
        let mut stats = BeamStats::default();
        !primary_beams_counted(
            &self.stone,
            self.scene.camera,
            size,
            rect,
            spectrum,
            &mut stats,
        )
        .is_empty()
    }

    /// The leaves (tile index set, counters included) and the rectangle of tile `index`, before
    /// shading; `None` when cancelled or `index` is past the last tile.
    pub fn tile_leaves(
        &self,
        index: usize,
        tile_size: u32,
        cancel: &AtomicBool,
    ) -> Option<(Vec<Leaf>, BeamStats, TileRect)> {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let size = (self.scene.width, self.scene.height);
        let tile_index = u32::try_from(index).ok()?;
        let rect = TileRect::of_index(size, tile_size, tile_index)?;
        let mut stats = BeamStats::default();
        let mut leaves: Vec<Leaf> = Vec::new();
        let retrace = RetraceKit {
            camera: self.scene.camera,
            size,
            stone: &self.stone,
            material: self.scene.material,
            n_lambda: self.n_lambda,
        };
        // The recursions of the entry, the interactions and the exit leaves poll the same token.
        let kit = self
            .kit
            .interact_kit(retrace, &self.settings, rect, self.scene.max_bounces)
            .with_cancel(cancel)
            .with_allowance(self.allowance);
        self.trace_tile(rect, &kit, &mut leaves, &mut stats, cancel)?;
        if cancel.load(Ordering::Relaxed) {
            // The recursions stop silently when cancelled: a partial tile is never used.
            return None;
        }
        for leaf in &mut leaves {
            leaf.tile = tile_index;
            stats.worst_node_step = stats.worst_node_step.max(worst_node_step(leaf));
        }
        stats.leaves += leaves.len() as u64;
        Some((leaves, stats, rect))
    }

    /// The leaves of every primary beam of `rect`; `None` when cancelled.
    ///
    /// Logs one debug line per primary beam (target `pleochroma::beam`): a heartbeat inside one
    /// tile, whose tree can run long, with the beams, leaves and seconds so far.
    fn trace_tile(
        &self,
        rect: TileRect,
        kit: &InteractKit<'_, '_>,
        leaves: &mut Vec<Leaf>,
        stats: &mut BeamStats,
        cancel: &AtomicBool,
    ) -> Option<()> {
        if self.scene.max_bounces < REFLECTION_MIN_BOUNCES {
            return Some(());
        }
        let size = (self.scene.width, self.scene.height);
        let spectrum = SpectralInterval::all(self.n_lambda);
        let started = Instant::now();
        // One seam store for the whole tile, so beams that share a seam or a tile edge snap the
        // same endpoint (guide §7.4 steps 6-7). The TIR pair tag carries the branch prefix, so two
        // branches never share a crossing.
        let mut seams = SeamStore::default();
        let primaries =
            primary_beams_counted(&self.stone, self.scene.camera, size, rect, spectrum, stats);
        for primary in primaries {
            stats.primary_beams += 1;
            self.collect_leaves(&primary, 0, leaves, stats, cancel)?;
            if self.scene.max_bounces >= REFRACTION_MIN_BOUNCES {
                if self.checked {
                    self.trace_spectral(&primary, kit, &mut seams, leaves, stats, cancel)?;
                } else {
                    let children = refract_entry(&primary, kit, stats);
                    for child in &children {
                        stats.record_beam(child.bounces);
                    }
                    self.trace_inside(children, kit, &mut seams, leaves, stats, cancel)?;
                }
            }
            tracing::debug!(
                target: LOG_TARGET,
                tile_x = rect.x0,
                tile_y = rect.y0,
                primary = stats.primary_beams,
                beams = stats.beams,
                leaves = leaves.len(),
                attempted = stats.attempted_work(),
                spent = stats.allowance_spent_at(self.allowance),
                seconds = started.elapsed().as_secs_f64(),
                "beam primary traced"
            );
        }
        Some(())
    }

    /// The refracted trees of `primary` for a dispersive material (beam guide §9.2): one tree per
    /// initial interval, each traced from the entry; an interval that fails a validity check is
    /// split in two and both halves are traced again from the entry. A split attempt's leaves and
    /// counters are discarded. `None` when cancelled.
    fn trace_spectral(
        &self,
        primary: &Beam,
        kit: &InteractKit<'_, '_>,
        seams: &mut SeamStore,
        leaves: &mut Vec<Leaf>,
        stats: &mut BeamStats,
        cancel: &AtomicBool,
    ) -> Option<()> {
        let mut pending: Vec<SpectralInterval> = self.intervals.iter().rev().copied().collect();
        while let Some(interval) = pending.pop() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let beam = retarget(primary, interval);
            // The attempt's leaves and counters are discarded when its interval splits, but the
            // work it spent stays on the tile's allowance.
            let mut local = stats.carrying_attempts();
            let mut local_leaves = Vec::new();
            let children = refract_entry(&beam, kit, &mut local);
            for child in &children {
                local.record_beam(child.bounces);
            }
            let verdict = self.trace_inside_checked(
                children,
                kit,
                seams,
                (&mut local_leaves, &mut local),
                cancel,
            )?;
            match (verdict, interval.halves()) {
                (Ok(()), _) => {
                    leaves.append(&mut local_leaves);
                    stats.merge(&local);
                    stats.adopt_attempts(&local);
                }
                (Err(cause), Some((first, second))) => {
                    stats.adopt_attempts(&local);
                    stats.interval_splits[cause.slot()] += 1;
                    pending.push(second);
                    pending.push(first);
                }
                (Err(_), None) => {
                    stats.adopt_attempts(&local);
                    stats.record_drop(DropReason::BracketFailure, 0.0);
                }
            }
        }
        Some(())
    }

    /// Hands a beam still on the stack to the residual once the tile's allowance is spent: a
    /// depth-cap drop with the beam's energy.
    fn drop_spent(work: &Work, stats: &mut BeamStats) {
        let area = image_polygon(&work.beam).map_or(0.0, |image| image.area());
        stats.record_drop(DropReason::DepthCap, area * peak_weight(&work.beam));
    }

    /// [`BeamFrame::trace_inside`] for a spectral interval: before a beam is expanded its interval
    /// is checked against its end nodes, a `Diverged` drop makes the interval invalid, and the
    /// exit leaves get their kept regions at the bin edges.
    ///
    /// `None` when cancelled; `Some(Err(cause))` when a multi-node interval must split.
    fn trace_inside_checked(
        &self,
        roots: Vec<Beam>,
        kit: &InteractKit<'_, '_>,
        seams: &mut SeamStore,
        (leaves, stats): (&mut Vec<Leaf>, &mut BeamStats),
        cancel: &AtomicBool,
    ) -> Option<Result<(), IntervalSplit>> {
        let mut stack: Vec<Work> = roots
            .into_iter()
            .rev()
            .map(|beam| Work { beam, retries: 0 })
            .collect();
        while let Some(work) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if work.beam.bounces + 2 > self.scene.max_bounces {
                continue;
            }
            if stats.allowance_spent_at(self.allowance) {
                Self::drop_spent(&work, stats);
                continue;
            }
            let multi = work.beam.spectrum.len() > 1;
            if multi {
                let ends = trace_end_nodes(&work.beam, kit);
                if let Err(cause) = check_interval(&work.beam, &ends, &self.settings) {
                    return Some(Err(cause));
                }
            }
            let before = stats.clone();
            let first_new = leaves.len();
            match self.expand_beam(&work.beam, kit, seams, leaves, stats) {
                Some(children) => {
                    if multi && let Err(cause) = check_diverged(&before, stats) {
                        return Some(Err(cause));
                    }
                    let fresh = leaves.split_off(first_new);
                    if multi {
                        for leaf in fresh {
                            // A beam with many exit leaves on a dispersive stone spends tens of
                            // milliseconds here.
                            if cancel.load(Ordering::Relaxed) {
                                return None;
                            }
                            match shrink_leaf(&leaf, kit, self.settings.region_mode, stats) {
                                Ok(parts) => leaves.extend(parts),
                                Err(cause) => return Some(Err(cause)),
                            }
                        }
                    } else {
                        // A single-node leaf comes back clipped from `exit_into`, which bisects a
                        // piece the clipping refuses.
                        leaves.extend(fresh);
                    }
                    for child in &children {
                        stats.record_beam(child.bounces);
                    }
                    stack.extend(
                        children
                            .into_iter()
                            .rev()
                            .map(|beam| Work { beam, retries: 0 }),
                    );
                }
                None => Self::retry_or_drop(&work, kit, stats, &mut stack),
            }
        }
        Some(Ok(()))
    }

    /// Walks the beams inside the stone depth first, `roots` first in their order; `None` when
    /// cancelled (polled after every pop).
    fn trace_inside(
        &self,
        roots: Vec<Beam>,
        kit: &InteractKit<'_, '_>,
        seams: &mut SeamStore,
        leaves: &mut Vec<Leaf>,
        stats: &mut BeamStats,
        cancel: &AtomicBool,
    ) -> Option<()> {
        let mut stack: Vec<Work> = roots
            .into_iter()
            .rev()
            .map(|beam| Work { beam, retries: 0 })
            .collect();
        while let Some(work) = stack.pop() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            // The next event is the beam's `bounces + 1`-th; a leaf is shaded iff that is at most
            // `max_bounces - 1` (§5.4), so deeper beams contribute zero, as in the path tracer.
            if work.beam.bounces + 2 > self.scene.max_bounces {
                continue;
            }
            if stats.allowance_spent_at(self.allowance) {
                Self::drop_spent(&work, stats);
                continue;
            }
            match self.expand_beam(&work.beam, kit, seams, leaves, stats) {
                Some(children) => {
                    for child in &children {
                        stats.record_beam(child.bounces);
                    }
                    stack.extend(
                        children
                            .into_iter()
                            .rev()
                            .map(|beam| Work { beam, retries: 0 }),
                    );
                }
                None => Self::retry_or_drop(&work, kit, stats, &mut stack),
            }
        }
        Some(())
    }

    /// Partitions `beam` by exit facet, snaps and projects the pieces, and expands each at its
    /// facet: the exit leaves go to `leaves`, the reflected beams are returned in canonical order.
    ///
    /// `None` when the partition does not conserve the area, the snapping fails or a piece cannot
    /// be projected (nothing is added to `leaves` then).
    fn expand_beam(
        &self,
        beam: &Beam,
        kit: &InteractKit<'_, '_>,
        seams: &mut SeamStore,
        leaves: &mut Vec<Leaf>,
        stats: &mut BeamStats,
    ) -> Option<Vec<Beam>> {
        let split = partition(beam, &self.stone, &Tolerances::DEFAULT);
        stats.partitions += 1;
        if !split.conserved {
            stats.conservation_failures += 1;
            return None;
        }
        let pieces = snap_partition(beam, &split, kit, seams, stats)?;
        let mut projected = Vec::with_capacity(pieces.len());
        for piece in &pieces {
            projected.push(project_to(beam, piece, kit, stats)?);
        }
        if split.sliver_area > 0.0 {
            let total = beam.polygon.area();
            let share = if total > 0.0 {
                split.sliver_area / total
            } else {
                0.0
            };
            let image_area = image_polygon(beam).map_or(0.0, |image| image.area());
            stats.record_drop(DropReason::Sliver, image_area * share * peak_weight(beam));
        }
        let mut reflected = Vec::new();
        for child in &projected {
            let hit = internal_hit(child, kit, seams, stats);
            leaves.extend(hit.transmitted);
            reflected.extend(hit.reflected);
        }
        Some(reflected)
    }

    /// After a failed expansion: bisects the beam and queues both halves (at most
    /// [`MAX_REPARTITIONS`] times down a branch), or drops its region.
    fn retry_or_drop(
        work: &Work,
        kit: &InteractKit<'_, '_>,
        stats: &mut BeamStats,
        stack: &mut Vec<Work>,
    ) {
        if work.retries < MAX_REPARTITIONS
            && let Some((first, second)) = split_retraced(&work.beam, kit)
        {
            stats.repartitions += 1;
            stats.record_beam(first.bounces);
            stats.record_beam(second.bounces);
            let retries = work.retries + 1;
            stack.push(Work {
                beam: second,
                retries,
            });
            stack.push(Work {
                beam: first,
                retries,
            });
            return;
        }
        let area = image_polygon(&work.beam).map_or(0.0, |image| image.area());
        stats.record_drop(DropReason::DepthCap, area * peak_weight(&work.beam));
    }
}
