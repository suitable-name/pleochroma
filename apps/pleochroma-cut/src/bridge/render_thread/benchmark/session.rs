//! Reusable live renderer state, including the production adaptive pass budget.

use super::{
    Case,
    cancellation::with_deadline,
    config::Config,
    display::Display,
    measure,
    scene::{Scene, checksum},
};
use crate::bridge::render_thread::{
    RenderContext,
    denoise::FirstHitSnapshot,
    engine::{EngineAccumulator, accumulate_engine_samples, diagnostic_hit_pixels},
    frame_helpers::{AccumulationBuffers, update_accumulation_state},
    gpu_backend::{BackendFrame, FrameOutputs, HybridPacing, ViewportGpu},
    interactive::PassBudget,
};
use anyhow::{Result, bail};
use glam::Vec3;
use pleochroma::optics::{
    fluorescence::Fluorescence,
    materials::GemMaterial,
    raytracer::{Camera, DEFAULT_FOV_DEG, surface::SurfaceScene},
};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

struct PassOutcome {
    report: Value,
    ready_at: Option<Instant>,
}

struct PassMetadata {
    report: Value,
    started: Instant,
}

pub(super) struct Session {
    gpu: ViewportGpu,
    engine: EngineAccumulator,
    hybrid: HybridPacing,
    budget: PassBudget,
    display: Display,
    accum: Vec<Vec3>,
    depth: Vec<f32>,
    normal: Vec<Vec3>,
    facet: Vec<i32>,
    signature: Vec<u32>,
    width: u32,
    height: u32,
    samples: u32,
    generation: u64,
    defaults: RenderContext,
}

impl Session {
    /// Benchmark sequences must not leave a compiler/probe competing with the next
    /// session. This wait is outside rendered-case timings; live UI teardown does
    /// not use it.
    pub(super) fn drain_preparation(&mut self) {
        self.gpu.drain_background_preparation(&mut self.hybrid);
        self.engine.drain_background_preparation();
    }

    pub fn new() -> Self {
        Self {
            gpu: ViewportGpu::acquire(),
            engine: EngineAccumulator::default(),
            hybrid: HybridPacing::new(),
            budget: PassBudget::default(),
            display: Display::new(),
            accum: Vec::new(),
            depth: Vec::new(),
            normal: Vec::new(),
            facet: Vec::new(),
            signature: Vec::new(),
            width: 0,
            height: 0,
            samples: 0,
            generation: 0,
            defaults: RenderContext::default(),
        }
    }

    pub fn run_case(
        &mut self,
        config: &Config,
        case: &Case,
        scene: &Scene,
        material: &GemMaterial,
        id: usize,
        start: Instant,
    ) -> Result<Value> {
        self.generation += 1;
        let ((), reset_ms) = measure(id, 0, "accumulation_buffer_reset", || self.reset(case));
        let target = case.engine.target_samples(case.path_target);
        let cancel = AtomicBool::new(false);
        let mut passes = Vec::new();
        let mut first_ready = None;
        let stop_reason = loop {
            if self.samples >= target {
                break "converged";
            }
            if !config.full_convergence && passes.len() >= config.passes {
                break "pass_limit";
            }
            if !passes.is_empty()
                && config.case_seconds > 0.0
                && start.elapsed().as_secs_f64() >= config.case_seconds
            {
                break "soft_time_limit";
            }
            let pass = self.render_pass(config, case, scene, material, &cancel, id)?;
            let cancelled = pass.ready_at.is_none();
            if first_ready.is_none() {
                first_ready = pass
                    .ready_at
                    .map(|ready| ready.duration_since(start).as_secs_f64() * 1000.0);
            }
            passes.push(pass.report);
            if cancelled {
                break "cancelled";
            }
        };
        // Ray display no longer computes unused first-hit guides. Run the explicit
        // visibility diagnostic after all timed rendering, so it cannot delay or
        // inflate first-image latency, pass pacing or the case time limit.
        let total_case_ms = start.elapsed().as_secs_f64() * 1000.0;
        let (visible_hit_pixels, visibility_diagnostic_ms) =
            if self.samples > 0 && case.engine.is_ray() {
                let camera = Camera::new(
                    self.defaults.yaw,
                    self.defaults.pitch,
                    self.defaults.distance,
                    DEFAULT_FOV_DEG,
                )
                .with_roll(self.defaults.camera_roll);
                let frame = self.frame(scene, material, &camera, (1, 0), config.max_bounces);
                let (hits, elapsed) = measure(id, passes.len(), "visibility_diagnostic", || {
                    diagnostic_hit_pixels(&frame, &AtomicBool::new(false))
                });
                (Some(hits), elapsed)
            } else {
                (
                    if self.samples > 0 {
                        Some(self.facet.iter().filter(|&&facet| facet >= 0).count())
                    } else {
                        None
                    },
                    0.0,
                )
            };
        Ok(json!({
            "scene": scene.label, "plane_count": scene.planes.len(), "tool_count": scene.tools.len(),
            "planes_fnv1a64": checksum(bytemuck::cast_slice(&scene.planes)),
            "pose": { "yaw_radians": self.defaults.yaw, "pitch_radians": self.defaults.pitch, "roll_radians": self.defaults.camera_roll, "distance": self.defaults.distance, "fov_degrees": DEFAULT_FOV_DEG },
            "lighting": { "preset": format!("{:?}", self.defaults.lighting_preset), "yaw_radians": self.defaults.light_yaw, "pitch_radians": self.defaults.light_pitch, "exposure": self.defaults.exposure, "surface_glare": self.defaults.surface_glare, "head_shadow_degrees": self.defaults.head_shadow_deg },
            "target_samples": target, "samples_committed": self.samples,
            "visible_hit_pixels": visible_hit_pixels, "visibility_diagnostic_ms": visibility_diagnostic_ms,
            "accumulation_buffer_reset_ms": reset_ms, "first_image_ready_ms": first_ready,
            "total_case_ms": total_case_ms,
            "backend_status": self.gpu.status_message(),
            "stop_reason": stop_reason, "passes": passes
        }))
    }

    fn reset(&mut self, case: &Case) {
        let resized = self.width != case.width || self.height != case.height;
        update_accumulation_state(
            case.width,
            case.height,
            true,
            &mut AccumulationBuffers {
                accum: &mut self.accum,
                first_hit_depth: &mut self.depth,
                first_hit_normal: &mut self.normal,
                first_hit_facet_id: &mut self.facet,
            },
            &mut self.samples,
            &mut self.width,
            &mut self.height,
        );
        if resized {
            self.gpu.invalidate_guide_cache();
        }
        self.budget.reset();
    }

    fn render_pass(
        &mut self,
        config: &Config,
        case: &Case,
        scene: &Scene,
        material: &GemMaterial,
        cancel: &AtomicBool,
        id: usize,
    ) -> Result<PassOutcome> {
        let index = self.samples;
        let samples = self.budget.samples(
            false,
            index == 0,
            case.engine.target_samples(case.path_target) - index,
        );
        let pass_start = Instant::now();
        let camera = Camera::new(
            self.defaults.yaw,
            self.defaults.pitch,
            self.defaults.distance,
            DEFAULT_FOV_DEG,
        )
        .with_roll(self.defaults.camera_roll);
        let sample_offset = index + if case.engine.is_ray() { 0 } else { config.seed };
        let frame = self.frame(
            scene,
            material,
            &camera,
            (samples, sample_offset),
            config.max_bounces,
        );
        let pass_index = usize::try_from(index)?;
        self.gpu.set_path_sig_wanted(case.denoise);
        let (mut staging, staging_ms) = measure(id, pass_index, "pass_staging_reset", || {
            self.budget.begin(self.accum.len())
        });
        let after_ms = if self.generation == 1 && index == 0 {
            config.cancel_after_ms
        } else {
            None
        };
        let ((completed, cancel_drain_ms), trace_ms) =
            measure(id, pass_index, "trace_including_prepare_and_guides", || {
                with_deadline(cancel, after_ms, || {
                    accumulate_engine_samples(
                        &mut self.engine,
                        case.engine,
                        self.generation,
                        None,
                        &mut self.gpu,
                        &frame,
                        &mut staging,
                        &mut self.hybrid,
                        config.backend,
                        cancel,
                    )
                })
            });
        let observation = serde_json::to_value(self.engine.benchmark_observation())?;
        let completed = match completed {
            Ok(completed) => completed,
            Err(reason) => bail!("engine refused scene: {}", reason.message()),
        };
        let cancelled = !completed || cancel.load(Ordering::Acquire);
        #[cfg_attr(
            not(feature = "gpu"),
            expect(unused_mut, reason = "only the gpu build adds fields")
        )]
        let mut report = json!({ "sample_offset": frame.sample_offset, "requested_samples": samples,
            "samples_committed": 0, "cancelled": cancelled, "cancel_request_to_return_ms": cancel_drain_ms,
            "staging_reset_ms": staging_ms, "trace_ms": trace_ms, "backend": observation });
        // Present only with PLEOCHROMA_GPU_TIMING set and a GPU request behind this pass.
        #[cfg(feature = "gpu")]
        if let Some(timing) = pleochroma::renderer::gpu::timing::take_last_pass_summary() {
            report["gpu_timing"] = serde_json::to_value(timing)?;
        }
        if cancelled {
            return Ok(PassOutcome {
                report,
                ready_at: None,
            });
        }
        self.finish_pass(
            case,
            &frame,
            id,
            PassMetadata {
                report,
                started: pass_start,
            },
            cancel,
        )
    }

    fn frame<'a>(
        &self,
        scene: &'a Scene,
        material: &'a GemMaterial,
        camera: &'a Camera,
        samples: (u32, u32),
        max_bounces: u32,
    ) -> BackendFrame<'a> {
        let environment = self
            .defaults
            .lighting_preset
            .studio(
                self.defaults.exposure,
                self.defaults.light_yaw,
                self.defaults.light_pitch,
            )
            .with_backdrop(self.defaults.backdrop.resolve(
                self.defaults.lighting_preset,
                self.defaults.light_yaw,
                self.defaults.light_pitch,
            ))
            .with_surface_glare(self.defaults.surface_glare)
            .with_head_shadow(self.defaults.head_shadow_deg);
        BackendFrame {
            width: self.width,
            height: self.height,
            yaw: self.defaults.yaw,
            pitch: self.defaults.pitch,
            distance: self.defaults.distance,
            camera,
            planes: &scene.planes,
            tools: &scene.tools,
            fluorescence: Fluorescence::none(),
            facet_finishes: &[],
            surface: SurfaceScene::IDEAL,
            material,
            max_bounces,
            environment,
            spp: samples.0,
            sample_offset: samples.1,
        }
    }

    fn finish_pass(
        &mut self,
        case: &Case,
        frame: &BackendFrame<'_>,
        id: usize,
        metadata: PassMetadata,
        cancel: &AtomicBool,
    ) -> Result<PassOutcome> {
        let PassMetadata {
            mut report,
            started,
        } = metadata;
        let pass_index = usize::try_from(self.samples)?;
        let (signature_ok, signature_ms) = measure(id, pass_index, "path_signature_sync", || {
            self.gpu
                .sync_path_sig(frame, &mut self.signature, case.denoise, cancel)
        });
        if !signature_ok {
            bail!("unexpected cancellation while syncing signatures");
        }
        let ((), commit_ms) = measure(id, pass_index, "pass_commit", || {
            self.budget.commit(
                &mut FrameOutputs {
                    accum: &mut self.accum,
                    depth: &mut self.depth,
                    normal: &mut self.normal,
                    facet_id: &mut self.facet,
                },
                frame.spp,
                started.elapsed(),
            );
            self.samples += frame.spp;
        });
        let pass_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (display, ready_at) = self.display.process(
            FirstHitSnapshot {
                width: case.width,
                height: case.height,
                current_sample_count: self.samples,
                accum_buffer: &self.accum,
                first_hit_depth: &self.depth,
                first_hit_normal: &self.normal,
                first_hit_facet_id: &self.facet,
                first_hit_path_sig: &self.signature,
            },
            case.denoise,
            self.samples >= case.engine.target_samples(case.path_target),
            id,
            pass_index,
        );
        report["samples_committed"] = json!(frame.spp);
        report["total_samples_committed"] = json!(self.samples);
        report["signature_ms"] = json!(signature_ms);
        report["commit_ms"] = json!(commit_ms);
        report["pass_through_commit_ms"] = json!(pass_ms);
        report["display"] = display;
        Ok(PassOutcome {
            report,
            ready_at: Some(ready_at),
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Also drain on a report-write error or another early exit.
        self.drain_preparation();
    }
}
