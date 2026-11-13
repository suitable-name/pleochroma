//! Stone Shadow resolution for a native render, at the quality the caller requests.

use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, LightingPreset,
            environment::tray_shadow::{
                TrayShadowInput, TrayShadowMap, TrayShadowOff, TrayShadowQuality,
                shared_tray_shadow_with_threads,
            },
        },
    },
};
use std::{
    num::NonZeroUsize,
    sync::{Arc, atomic::AtomicBool},
    thread,
};

/// Builds or reuses the Stone Shadow map for this stone; `Ok(None)` when `enabled` is false.
///
/// The render holds the returned map until its last ray has sampled it.
/// The build uses every available core; [`stone_shadow_for_with_threads`] caps that.
///
/// # Errors
/// Returns the lighting or geometry restriction, or cancellation during preparation.
#[expect(
    clippy::too_many_arguments,
    reason = "the render supplies its scene, quality and cancellation/progress controls independently"
)]
pub fn stone_shadow_for(
    enabled: bool,
    environment: EnvironmentSource<'_>,
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    max_bounces: u32,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Option<Arc<TrayShadowMap>>, TrayShadowOff> {
    let threads = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    stone_shadow_for_with_threads(
        enabled,
        environment,
        geom,
        material,
        max_bounces,
        quality,
        cancel,
        progress,
        threads,
    )
}

/// [`stone_shadow_for`] with a cap on the precompute's worker threads, for a caller that shares
/// the machine with a render. The thread count never changes the map's bits and is not part of
/// its cache key.
///
/// # Errors
/// Returns the lighting or geometry restriction, or cancellation during preparation.
#[expect(
    clippy::too_many_arguments,
    reason = "the render supplies its scene, quality, cancellation/progress controls and thread cap independently"
)]
pub fn stone_shadow_for_with_threads(
    enabled: bool,
    environment: EnvironmentSource<'_>,
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    max_bounces: u32,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    threads: NonZeroUsize,
) -> Result<Option<Arc<TrayShadowMap>>, TrayShadowOff> {
    if !enabled {
        return Ok(None);
    }
    let EnvironmentSource::Studio {
        preset,
        light_yaw,
        light_pitch,
        backdrop,
        ..
    } = environment
    else {
        return Err(TrayShadowOff::Hdr);
    };
    if preset == LightingPreset::Aset {
        return Err(TrayShadowOff::Aset);
    }
    if preset.is_uv_lamp() {
        return Err(TrayShadowOff::UvLamp);
    }
    if backdrop <= 0.0 {
        return Err(TrayShadowOff::AsLit);
    }
    let input = TrayShadowInput {
        geom,
        material,
        preset,
        yaw: light_yaw,
        pitch: light_pitch,
        max_bounces,
    };
    shared_tray_shadow_with_threads(&input, quality, cancel, progress, threads).map(Some)
}
