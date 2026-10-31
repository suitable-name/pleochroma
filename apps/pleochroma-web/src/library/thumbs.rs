//! Library thumbnails: the Solid raster of a solved design as a small PNG, and a cached PNG
//! back into a Slint image.
//!
//! The picture is the Solid view's own frame pipeline (`pleochroma_solid::preview`) at
//! [`THUMBNAIL_SIZE`] with the default camera, planned from masts that are already solved, so
//! drawing one costs a raster and never a solve. It is cached in `IndexedDB` as PNG bytes
//! (`storage::library::put_thumbnail`) and decoded when its row is drawn.

use crate::app::coalesce_now;
use pleochroma::{
    color::ColorSpace,
    geometry::meet_solver::SolvedTier,
    optics::{materials::GemMaterial, raytracer::DEFAULT_POSE},
    render_setup::encode_png_with_icc,
};
use pleochroma_cut_core::Design;
use pleochroma_solid::{
    live_update::{Clock, DEFAULT_PREVIEW_BUDGET},
    preview::{
        CameraPose, PreviewPipeline,
        view::{ReplanBasis, ReplanInputs, plan_job},
    },
};
use pleochroma_web_core::{
    library_view::THUMBNAIL_SIZE,
    settings::{DEFAULT_CAMERA_PITCH, DEFAULT_CAMERA_YAW},
};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::{collections::BTreeSet, sync::Arc};

/// `performance.now()` as the planner's budget clock.
struct PerfClock;

impl Clock for PerfClock {
    fn now_ms(&self) -> f64 {
        coalesce_now().as_secs_f64() * 1000.0
    }
}

/// The Solid view of `design` (solved as `masts`) at thumbnail size, PNG-encoded.
///
/// `None` when the masts do not belong to the design, the stone does not close, or the
/// encoder refuses.
#[must_use]
pub fn render_png(
    design: &Design,
    masts: &[SolvedTier],
    custom: &[GemMaterial],
) -> Option<Vec<u8>> {
    if masts.len() != design.tiers.len() {
        return None;
    }
    let job = plan_job(ReplanInputs {
        design: Arc::new(design.clone()),
        generation: 0,
        basis: ReplanBasis::Chain {
            last_solved: masts.to_vec(),
            dirty: BTreeSet::new(),
        },
        camera: CameraPose {
            yaw: DEFAULT_CAMERA_YAW,
            pitch: DEFAULT_CAMERA_PITCH,
            roll: 0.0,
            distance: DEFAULT_POSE.distance,
        },
        size: THUMBNAIL_SIZE,
        selected_tier: None,
        custom_materials: custom,
        view_mode: 0,
        show_preform: false,
        enlarged_panel: -1,
        tier_cutoff: -1,
    });
    let mut pipeline = PreviewPipeline::new();
    let frame = pipeline.replan(job, DEFAULT_PREVIEW_BUDGET, &PerfClock)?;
    if !frame.has_solid {
        return None;
    }
    let (width, height) = pipeline.size();
    encode_png_with_icc(pipeline.solid_rgba(), width, height, ColorSpace::Srgb).ok()
}

/// A cached PNG as a Slint image; `None` when the bytes are not a readable PNG.
#[must_use]
pub fn decode_png(png: &[u8]) -> Option<Image> {
    let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let rgba = decoded.to_rgba8();
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(rgba.width(), rgba.height());
    let bytes = buffer.make_mut_bytes();
    if bytes.len() != rgba.as_raw().len() {
        return None;
    }
    bytes.copy_from_slice(rgba.as_raw());
    Some(Image::from_rgba8(buffer))
}
