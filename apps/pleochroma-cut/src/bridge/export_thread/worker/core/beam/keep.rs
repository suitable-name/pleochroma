//! Kept beams: the persisted leaf cache of an export, a job or a tilt frame ("Keep Beams for
//! Re-Renders", `--keep-beams`; beam guide section 14.10).
//!
//! # What is kept
//!
//! With [`wants_keep`] the beam render retains every finished batch's leaves and kept regions
//! (`BeamRun::retain`) and, once the render is done, stores them in the render-cache folder under
//! the key of [`beam_cache_key`]: the lighting-independent scene (pose, design, material,
//! resolution) plus the beam settings that change leaves. The cache shares its root, its codec and
//! its disk limit with the path cache; the least recently used keys go first, whichever kind.
//!
//! # A re-render
//!
//! The next beam render of the same key loads the folder first ([`Keeper::load`]) and hands the
//! leaves to the renderer as preloaded beams ([`render_preloaded`]), so the trees of the kept
//! batches are not traced. Fragments and the full adaptive residual then run exactly as in a fresh
//! render, so the image equals the fresh one bit for bit. Leaves are not refined for a lighting
//! finer than the one they were traced under, so the lighting tolerance is part of the key: a
//! finer one traces afresh.
//!
//! # Tilt frames
//!
//! Every frame is a new pose, so a kept frame is a kept cache folder of its own. A tilt video
//! keeps them only with "Keep Path Cache for This Video" (the taps' persisting path cache); a
//! still keeps them whenever the beam settings ask.

use super::{super::super::taps::RenderTaps, BatchHook, CHECK_SCALE, Inputs, ProgressHook};
use crate::settings::store::default_render_cache_dir;
use glam::Mat4;
use pleochroma::{
    optics::tables::default_optics_tables,
    renderer::{
        accel::CacheCompression,
        accel_beam::{BeamOff, BeamSettings},
        adaptive::{AdaptiveImage, AdaptiveSettings},
        beam::{
            BeamRun,
            leaf_cache::{LeafCache, RetainedBatch, retained_bytes},
            stats::BeamStats,
        },
        cpu_frame::render_beam_hybrid_surface,
        frame_scene::FrameScene,
        path_cache::{
            BuildSettings, CacheKey, CacheKeyInputs, CacheMeta, KeyFolder, key::beam_cache_key,
            persist::unix_now,
        },
    },
};
use std::{path::PathBuf, sync::atomic::AtomicBool};

/// What a render from kept beams produced.
pub(super) struct Rerendered {
    /// The finished frame, in the contract of the renderer's entry.
    pub(super) image: AdaptiveImage,
    /// The counters (the leaves taken from the cache are counted too).
    pub(super) stats: BeamStats,
    /// Batches the cache did not hold and the render traced, to be kept as well; limited to what
    /// still fits under the cache's cap.
    pub(super) fresh: Vec<RetainedBatch>,
}

/// Renders the frame from the kept beams in `cache`: batches the cache holds skip their trees, the
/// others are traced, and everything then runs the fragments and the full adaptive residual as a
/// fresh render does (`BeamRun::preloaded`), so the image equals the fresh one bit for bit on the
/// same engines.
///
/// # Errors
///
/// The [`BeamOff`] reason the frame is not beam traced (the full render then reports it).
pub(super) fn render_preloaded(
    inputs: &Inputs<'_, '_, '_>,
    cache: &LeafCache,
    (cancel, sample_offset): (&AtomicBool, u32),
    (progress, on_batch): (ProgressHook<'_>, BatchHook<'_>),
) -> Result<Rerendered, BeamOff> {
    let room = cache.cap_bytes().saturating_sub(cache.bytes());
    let mut used = 0_u64;
    let mut fresh = Vec::new();
    let mut keep = |batch: RetainedBatch| {
        let size = retained_bytes(&batch);
        if used.saturating_add(size) <= room {
            used += size;
            fresh.push(batch);
        }
    };
    let (image, stats) = render_beam_hybrid_surface(
        inputs.frame,
        inputs.extras,
        inputs.plane_soa,
        &inputs.settings,
        &inputs.adaptive,
        inputs.engines,
        (sample_offset, CHECK_SCALE),
        cancel,
        BeamRun {
            tiles: None,
            progress,
            on_batch: Some(on_batch),
            retain: Some(&mut keep),
            preloaded: Some(cache),
        },
    )?;
    Ok(Rerendered {
        image,
        stats,
        fresh,
    })
}

/// Whether the render keeps its beams: the settings ask for it and, for a tilt frame (taps with a
/// frame name), the video keeps its path cache.
#[must_use]
pub(super) fn wants_keep(settings: &BeamSettings, taps: &RenderTaps) -> bool {
    settings.keep_leaves
        && (taps.frame_name.is_none() || taps.path_cache.is_some_and(|cache| cache.persist))
}

/// Where and under which key one render keeps its beams.
pub(super) struct Keeper {
    /// The render-cache folder.
    root: PathBuf,
    /// The folder name.
    key: CacheKey,
    /// The description written beside the leaves.
    meta: CacheMeta,
    /// The disk limit in bytes.
    limit: u64,
    /// The retention cap in bytes (`BeamSettings::leaf_cache_bytes`).
    cap: u64,
    /// The frame size and the tile edge the kept rectangles must lie on.
    grid: ((u32, u32), u32),
}

impl Keeper {
    /// The keeper of this render, or `None` when the render does not keep its beams.
    pub(super) fn new(
        frame: &FrameScene<'_>,
        settings: &BeamSettings,
        adaptive: &AdaptiveSettings,
        taps: &RenderTaps,
    ) -> Option<Self> {
        if !wants_keep(settings, taps) {
            return None;
        }
        let build = BuildSettings {
            tile: adaptive.tile,
            ..BuildSettings::default()
        };
        // Leaves are traced with `PreparedMaterial::new`, which reads the process default of the
        // optics tables. The key takes the same mode, so leaves built with the tables on are never
        // loaded for a render with them off.
        let inputs = CacheKeyInputs::from_scene(frame, Mat4::IDENTITY, &build)
            .with_optics_tables(default_optics_tables());
        let key = beam_cache_key(&inputs, settings, adaptive.tile);
        let compression = taps
            .path_cache
            .map_or(CacheCompression::Zstd, |cache| cache.compression);
        Some(Self {
            root: taps
                .cache_dir
                .clone()
                .unwrap_or_else(default_render_cache_dir),
            meta: CacheMeta::describe(&inputs, &key, compression, unix_now()),
            key,
            limit: taps
                .cache_disk_limit_bytes
                .unwrap_or_else(crate::bridge::export_thread::cache_run::saved_limit_bytes),
            cap: settings.leaf_cache_bytes,
            grid: ((frame.width, frame.height), adaptive.tile),
        })
    }

    /// The kept leaves of this render, if a finished folder holds some. A corrupt folder is
    /// deleted by the loader and counts as none.
    pub(super) fn load(&self) -> Option<LeafCache> {
        let folder = match KeyFolder::open(&self.root, &self.key) {
            Ok(folder) => folder,
            Err(reason) => {
                tracing::debug!(%reason, "kept beams: building new ones");
                return None;
            }
        };
        let (size, tile) = self.grid;
        match LeafCache::load_for_frame(&folder, self.cap, size, tile) {
            Ok(Some(cache)) if !cache.is_empty() => {
                tracing::info!(key = %self.key, "kept beams: loaded from disk");
                Some(cache)
            }
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(%error, "kept beams: could not read the folder");
                None
            }
        }
    }

    /// Stores `cache` under the key and enforces the disk limit. Problems are logged, never an
    /// error for the render; an empty cache stores nothing.
    pub(super) fn store(&self, cache: &LeafCache) {
        if cache.is_empty() {
            return;
        }
        let folder = match KeyFolder::create(&self.root, &self.key, self.meta.clone()) {
            Ok(folder) => folder,
            Err(error) => {
                tracing::warn!(%error, "kept beams: could not create the folder");
                return;
            }
        };
        match cache.store_with_limit(&folder, self.limit) {
            Ok(evicted) => {
                tracing::info!(key = %self.key, evicted = evicted.len(), "kept beams: stored");
            }
            Err(error) => {
                tracing::warn!(%error, "kept beams: could not store the leaves, dropping the folder");
                folder.delete();
            }
        }
    }
}
