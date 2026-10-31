//! The production display functions and framebuffer pool, called synchronously.

use super::{measure, scene::checksum};
use crate::bridge::{
    pixel_buffer::FramebufferTransfer,
    render_thread::denoise::{DenoiseScratch, FirstHitSnapshot},
};
use glam::Vec3;
use pleochroma::renderer::{
    denoise::{AtrousDenoiser, AtrousParams},
    frame_denoise::denoise_and_tonemap_frame_with_params_into_cancellable,
    tonemap::{live_cpu_budget, tonemap_to_rgba_into},
};
use serde_json::{Value, json};
use std::{sync::atomic::AtomicBool, time::Instant};

pub(super) struct Display {
    denoiser: AtrousDenoiser,
    average: Vec<Vec3>,
    filtered: Vec<Vec3>,
    accum: Vec<Vec3>,
    depth: Vec<f32>,
    normal: Vec<Vec3>,
    facet: Vec<i32>,
    signature: Vec<u32>,
    dimensions: (u32, u32),
    transfer: FramebufferTransfer,
}

impl Display {
    pub const fn new() -> Self {
        Self {
            denoiser: AtrousDenoiser::new(),
            average: Vec::new(),
            filtered: Vec::new(),
            accum: Vec::new(),
            depth: Vec::new(),
            normal: Vec::new(),
            facet: Vec::new(),
            signature: Vec::new(),
            dimensions: (0, 0),
            transfer: FramebufferTransfer::new(0, 0),
        }
    }

    pub fn process(
        &mut self,
        snapshot: FirstHitSnapshot<'_>,
        denoise: bool,
        converged: bool,
        id: usize,
        pass: usize,
    ) -> (Value, Instant) {
        // Matches DisplayWork::fill: copy radiance always, guides only when consumed.
        let ((), snapshot_ms) = measure(id, pass, "display_snapshot_copy", || {
            self.accum.clear();
            self.accum.extend_from_slice(snapshot.accum_buffer);
            self.depth.clear();
            self.normal.clear();
            self.facet.clear();
            self.signature.clear();
            if denoise {
                self.depth.extend_from_slice(snapshot.first_hit_depth);
                self.normal.extend_from_slice(snapshot.first_hit_normal);
                self.facet.extend_from_slice(snapshot.first_hit_facet_id);
                self.signature
                    .extend_from_slice(snapshot.first_hit_path_sig);
            }
        });
        let display_threads = if converged {
            usize::MAX
        } else {
            live_cpu_budget::partition().1
        };
        let ((image, ready_at), display_ms) = live_cpu_budget::with_limit(display_threads, || {
            measure(id, pass, "display_denoise_tonemap_framebuffer", || {
                if self.dimensions != (snapshot.width, snapshot.height) {
                    self.dimensions = (snapshot.width, snapshot.height);
                    self.transfer = FramebufferTransfer::new(snapshot.width, snapshot.height);
                }
                let image = self.transfer.fill_with(|rgba| {
                    if denoise {
                        let _ = denoise_and_tonemap_frame_with_params_into_cancellable(
                            FirstHitSnapshot {
                                width: snapshot.width,
                                height: snapshot.height,
                                current_sample_count: snapshot.current_sample_count,
                                accum_buffer: &self.accum,
                                first_hit_depth: &self.depth,
                                first_hit_normal: &self.normal,
                                first_hit_facet_id: &self.facet,
                                first_hit_path_sig: &self.signature,
                            },
                            &mut DenoiseScratch {
                                denoiser: &mut self.denoiser,
                                avg_color_buf: &mut self.average,
                                filtered_buf: &mut self.filtered,
                            },
                            &AtrousParams::default(),
                            rgba,
                            &AtomicBool::new(false),
                        );
                    } else {
                        tonemap_to_rgba_into(
                            &self.accum,
                            1.0 / snapshot.current_sample_count.max(1) as f32,
                            rgba,
                        );
                    }
                });
                (image, Instant::now())
            })
        });
        let (validation, validation_ms) = measure(id, pass, "output_validation", || {
            json!({
                "rgba_fnv1a64": checksum(bytemuck::cast_slice(image.as_slice())),
                "finite_accumulation": self.accum.iter().all(|color| color.is_finite()),
                "finite_display_scratch": !denoise || (self.average.iter().all(|color| color.is_finite()) && self.filtered.iter().all(|color| color.is_finite())),
                "rgba_bytes": image.as_slice().len() * 4
            })
        });
        (
            json!({ "snapshot_ms": snapshot_ms, "processing_ms": display_ms, "validation_ms": validation_ms, "validation": validation }),
            ready_at,
        )
    }
}
