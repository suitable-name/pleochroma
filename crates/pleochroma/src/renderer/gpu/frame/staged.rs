//! The staged megakernel: the megakernel's bounce loop in stages, with the rays still alive
//! at a stage end compacted into dense warps for the next stage.
//!
//! The megakernel's time follows the longest path of each warp. Measured on Synthetic
//! Moissanite at 32 bounces, 19 % of the rays are alive after bounce 8 and 2 % after bounce 32,
//! and the bounces after the eighth take 57 % of the time for a quarter of the work. Here a ray
//! alive at a stage end stores its state in a survivor buffer and its thread ends; the next
//! stage runs one thread per survivor.
//!
//! One chunk is still one submission, queued without a host round trip: stage 0 over every
//! tuple, then per stage end a one-thread kernel that turns the survivor counter into indirect
//! dispatch arguments and an indirect dispatch of the next stage. The overlapped chunk
//! pipeline, the per-pixel reduction and the readback are the megakernel's. A sample's result
//! does not depend on the stages (see `shaders/spectral_transport_staged.wgsl`), so the image
//! differs from the megakernel's only by compiler rounding between two kernels.
//!
//! On by default with the stage ends 5 and 12, except on targets where it was never measured
//! (the browser, Android and Apple systems). `PLEOCHROMA_STAGED` overrides the default (see
//! [`stage_ends_requested`]; `0` keeps the single megakernel). Every chunk of a plain
//! megakernel class runs staged then, for single requests, the batch stepper and the viewport's
//! progressive passes alike. A class with the optics tables or the tray variant
//! (the Stone Shadow, the Tray Reflection or both), the live (adaptive) passes, the path cache,
//! the beam reference kernels and the legacy wavefront keep the single megakernel, as does a
//! device that cannot bind the three extra buffers or dispatch indirectly.
//!
//! The survivor buffer follows the chunk size: records for half the chunk's tuples at the
//! first stage end, a quarter at the second, an eighth at the third, each record 352 bytes.
//! The buffer grows in steps and is never shrunk, up to [`STAGE_RAYS_MAX_BYTES`] and the
//! device's limits. A region that fills up leaves the rest of its rays in their own threads,
//! which is correct but keeps their warps running: the chunk's timing line says when the
//! regions were scaled down.

use std::{collections::HashMap, sync::OnceLock};

use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    renderer::gpu::{
        MEGAKERNEL_STORAGE_BUFFERS, compute,
        timing::{self, Slot},
    },
};

use super::{
    ChunkFrameState, GpuPipelineKind,
    dispatch::{count_u32, workgroups_for},
    material_class,
    renderer::GpuFrameRenderer,
    scene_buffers::{FrameSceneBuffers, TransportOutputs},
};

/// `shaders/spectral_transport_staged.wgsl` behind the megakernel's prelude and bounce text,
/// concatenated by `build.rs`.
const STAGED_SHADER_SRC: &str = include_str!(concat!(
    env!("OUT_DIR"),
    "/spectral_transport_staged.generated.wgsl"
));

/// The one-thread kernel between two stages.
const PREPARE_SHADER_SRC: &str = include_str!("../../shaders/stage_prepare.wgsl");

/// The staged kernel's entry point.
const STAGED_ENTRY: &str = "transport_stage_main";

/// The entry point of [`PREPARE_SHADER_SRC`].
const PREPARE_ENTRY: &str = "transport_stage_prepare";

/// Binding of the stage's `StageParams` uniform.
const STAGE_PARAMS_BINDING: u32 = 150;
/// Binding of the survivor records.
const STAGE_RAYS_BINDING: u32 = 151;
/// Binding of the survivor counters, one per stage end.
const STAGE_COUNTERS_BINDING: u32 = 152;
/// Binding of the indirect dispatch arguments (the prepare kernel only).
const STAGE_INDIRECT_BINDING: u32 = 153;

/// `u32` words per survivor record (`STAGE_WORDS` of the shader).
const STAGE_WORDS: usize = 88;

/// The most stage ends one chunk may have (one more stage than that).
const MAX_STAGE_ENDS: usize = 3;

/// The stage ends of an unset or empty `PLEOCHROMA_STAGED` and of `PLEOCHROMA_STAGED=1`.
/// Speedups over the megakernel at 32 bounces, Synthetic Moissanite, Diamond and Sapphire:
/// 1.62x, 1.27x, 1.08x on an A100 and 1.60x, 1.32x, 1.03x on a Radeon 780M (RADV). Under a
/// cap of 12 the end 12 drops out and end 5 alone gives 1.37x, 1.16x, 1.14x on the A100.
/// Every staged 8-bit image in those runs was byte-equal to the megakernel's.
const DEFAULT_STAGE_ENDS: [u32; 2] = [5, 12];

/// Targets the staged kernel was never measured on: an unset `PLEOCHROMA_STAGED` keeps the single
/// megakernel there (the browser has no process variable at all).
const UNMEASURED_TARGET: bool = cfg!(any(
    target_arch = "wasm32",
    target_os = "android",
    target_vendor = "apple"
));

/// The end of the last stage: it never parks a ray (`STAGE_NEVER_PARKS` of the shader).
const STAGE_NEVER_PARKS: u32 = u32::MAX;

/// The survivor buffer's size limit, beside the device's own. Regions that do not fit are
/// scaled down together, so this bounds video memory, never the image.
const STAGE_RAYS_MAX_BYTES: usize = 512 * 1024 * 1024;

/// The fewest records a survivor region holds.
const MIN_REGION_RECORDS: usize = 64;

/// The survivor buffer grows in steps of this many records (22 MiB).
const RAYS_GROWTH_RECORDS: usize = 65_536;

/// Storage buffers the staged kernel binds: the megakernel's, the records and the counters.
const STAGED_STORAGE_BUFFERS: u32 = MEGAKERNEL_STORAGE_BUFFERS + 2;

/// Buffers the staged kernel binds in all: the megakernel's fifteen and its own three.
const STAGED_TOTAL_BUFFERS: u32 = 15 + 3;

/// What a missing staged resource means in [`GpuFrameRenderer::dispatch_staged`].
const PREPARED: &str = "staged_plan_for checked the staged resources";

/// `StageParams` of the two stage shaders, one per stage of a chunk.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct GpuStageParams {
    pub(super) mode: u32,
    pub(super) first_bounce: u32,
    pub(super) end_bounce: u32,
    pub(super) in_base: u32,
    pub(super) in_counter: u32,
    pub(super) in_capacity: u32,
    pub(super) out_base: u32,
    pub(super) out_counter: u32,
    pub(super) out_capacity: u32,
    pub(super) pad: [u32; 3],
}

/// Whether `ends` can be staged: at most [`MAX_STAGE_ENDS`] positive, strictly ascending bounce
/// counts. A list out of order would resume a ray at an earlier bounce than it parked at.
fn stage_ends_valid(ends: &[u32]) -> bool {
    ends.len() <= MAX_STAGE_ENDS
        && !ends.contains(&0)
        && ends.windows(2).all(|pair| pair[0] < pair[1])
}

/// The stage ends `value`, the content of `PLEOCHROMA_STAGED`, asks for.
///
/// Unset, empty or `1`: the default ends, 5 and 12. `0`: none (the single megakernel).
/// Otherwise a comma list of at most three strictly ascending positive bounce counts, such as
/// `8,16`: a ray still alive after that many bounces is compacted. Anything else is none.
#[must_use]
pub fn stage_ends_requested(value: Option<&str>) -> Vec<u32> {
    let text = value.map_or("", str::trim);
    match text {
        "" | "1" => return DEFAULT_STAGE_ENDS.to_vec(),
        "0" => return Vec::new(),
        _ => {}
    }
    let parsed: Option<Vec<u32>> = text
        .split(',')
        .map(|part| part.trim().parse().ok())
        .collect();
    parsed
        .filter(|ends| stage_ends_valid(ends))
        .unwrap_or_default()
}

/// The process's stage ends (`PLEOCHROMA_STAGED`), read once. Unset or empty: the default ends,
/// except on an [`UNMEASURED_TARGET`] (none, so always none in the browser).
fn process_stage_ends() -> &'static [u32] {
    static ENDS: OnceLock<Vec<u32>> = OnceLock::new();
    ENDS.get_or_init(|| {
        let value = std::env::var("PLEOCHROMA_STAGED").ok();
        let text = value.as_deref().map(str::trim);
        if UNMEASURED_TARGET && text.is_none_or(str::is_empty) {
            Vec::new()
        } else {
            stage_ends_requested(text)
        }
    })
}

/// The stages of one chunk: their parameters in dispatch order and the records they need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StagePlan {
    /// One entry per stage; stage 0 traces every tuple, the others resume survivors.
    pub(super) params: Vec<GpuStageParams>,
    /// Records the survivor buffer must hold for this chunk.
    pub(super) records: usize,
    /// Whether the regions were scaled down to fit the record budget.
    pub(super) scaled: bool,
}

/// The stages of a chunk of `tuples` rays with a bounce cap of `max_bounces`.
///
/// Only the ends below the cap count. The survivors of stage end `i` get a region of
/// `tuples / 2^(i+1)` records (at least [`MIN_REGION_RECORDS`]), all regions scaled down
/// together to fit `max_records`. `None` when no end is left or nothing fits: the chunk then
/// runs the single megakernel.
#[must_use]
pub(super) fn stage_plan(
    ends: &[u32],
    max_bounces: u32,
    tuples: usize,
    max_records: usize,
) -> Option<StagePlan> {
    let ends: Vec<u32> = ends
        .iter()
        .copied()
        .filter(|end| *end < max_bounces)
        .collect();
    if ends.is_empty() || tuples == 0 || max_records == 0 {
        return None;
    }
    let mut capacities: Vec<usize> = (0..ends.len())
        .map(|i| (tuples >> (i + 1)).max(MIN_REGION_RECORDS))
        .collect();
    let wanted: usize = capacities.iter().sum();
    let scaled = wanted > max_records;
    if scaled {
        let divisor = wanted.div_ceil(max_records);
        for capacity in &mut capacities {
            *capacity /= divisor;
        }
    }
    if capacities.contains(&0) {
        return None;
    }
    let mut bases = Vec::with_capacity(capacities.len());
    let mut records = 0usize;
    for capacity in &capacities {
        bases.push(records * STAGE_WORDS);
        records += capacity;
    }
    let mut params = Vec::with_capacity(ends.len() + 1);
    for stage in 0..=ends.len() {
        let mut p = GpuStageParams {
            mode: u32::from(stage > 0),
            first_bounce: if stage == 0 { 0 } else { ends[stage - 1] },
            end_bounce: ends.get(stage).copied().unwrap_or(STAGE_NEVER_PARKS),
            in_base: 0,
            in_counter: 0,
            in_capacity: 0,
            out_base: 0,
            out_counter: 0,
            out_capacity: 0,
            pad: [0; 3],
        };
        if stage > 0 {
            p.in_base = count_u32(bases[stage - 1]);
            p.in_counter = count_u32(stage - 1);
            p.in_capacity = count_u32(capacities[stage - 1]);
        }
        if stage < ends.len() {
            p.out_base = count_u32(bases[stage]);
            p.out_counter = count_u32(stage);
            p.out_capacity = count_u32(capacities[stage]);
        }
        params.push(p);
    }
    Some(StagePlan {
        params,
        records,
        scaled,
    })
}

/// The staged kernel's pipelines and buffers. Empty (nothing compiled, nothing allocated)
/// until a chunk runs with stage ends.
#[derive(Default)]
pub(super) struct StagedState {
    /// Stage ends set by [`GpuFrameRenderer::set_stage_ends`]; `None` follows the process.
    ends_override: Option<Vec<u32>>,
    /// The staged pipeline per class value.
    pipelines: HashMap<u32, wgpu::ComputePipeline>,
    prepare: Option<wgpu::ComputePipeline>,
    rays: Option<wgpu::Buffer>,
    /// Records `rays` holds.
    rays_records: usize,
    counters: Option<wgpu::Buffer>,
    indirect: Option<wgpu::Buffer>,
    /// One `StageParams` uniform per stage, [`MAX_STAGE_ENDS`] + 1 once built.
    params: Vec<wgpu::Buffer>,
}

impl GpuFrameRenderer {
    /// Sets the stage ends of every LATER frame: `Some(vec![])` forces the single megakernel,
    /// `Some(ends)` these ends, `None` follows the process default
    /// (the default ends, or what `PLEOCHROMA_STAGED` asks for; none on an unmeasured target).
    /// A list that is not at most three positive, strictly ascending bounce counts counts as
    /// empty. The chunk timing starts over, because the cost per ray changes.
    pub fn set_stage_ends(&mut self, ends: Option<Vec<u32>>) {
        self.staged.ends_override = ends.map(|ends| {
            if stage_ends_valid(&ends) {
                ends
            } else {
                Vec::new()
            }
        });
        self.last_scene_key = None;
    }

    /// The stage ends in force.
    fn stage_ends(&self) -> &[u32] {
        if let Some(ends) = &self.staged.ends_override {
            return ends;
        }
        process_stage_ends()
    }

    /// Records the survivor buffer may hold on this device.
    fn stage_max_records(&self) -> usize {
        let limits = self.ctx.device.limits();
        let device_bytes = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let bytes = STAGE_RAYS_MAX_BYTES.min(usize::try_from(device_bytes).unwrap_or(usize::MAX));
        bytes / (STAGE_WORDS * size_of::<u32>())
    }

    /// Whether chunks of `class` may run staged on this renderer and device.
    fn staged_class(&self, class: u32) -> bool {
        let limits = self.ctx.device.limits();
        self.pipeline_kind == GpuPipelineKind::Megakernel
            && !self.stage_ends().is_empty()
            && !material_class::uses_optics_tables(class)
            && !material_class::uses_tray_variant(class)
            && limits.max_storage_buffers_per_shader_stage >= STAGED_STORAGE_BUFFERS
            && limits.max_buffers_and_acceleration_structures_per_shader_stage
                >= STAGED_TOTAL_BUFFERS
            && self
                .ctx
                .adapter
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION)
    }

    /// Compiles the staged pipeline of `class`.
    fn build_staged_pipeline(&self, class: u32) -> wgpu::ComputePipeline {
        let label = format!(
            "transport_stage_main (MATERIAL_CLASS={}, features {:#x})",
            class & material_class::KIND_MASK,
            class & material_class::FEATURE_MASK
        );
        compute::create_persisted_compute_pipeline(
            &self.ctx.device,
            &self.ctx.adapter.get_info(),
            &label,
            STAGED_SHADER_SRC,
            STAGED_ENTRY,
            &material_class::pipeline_constants(class),
        )
    }

    /// Prepares a chunk of `pixels` pixels of the frame `state` describes for the staged kernel:
    /// compiles the pipelines on first use and grows the survivor buffer to the chunk's plan.
    /// A no-op when the class does not run staged, and once everything exists and is large
    /// enough. A buffer replaced here stays alive for a chunk still in flight (its bind group
    /// holds it).
    pub(super) fn ensure_staged(&mut self, state: &ChunkFrameState<'_>, pixels: usize) {
        let class = state.pipeline_class;
        if !self.staged_class(class) {
            return;
        }
        let max_records = self.stage_max_records();
        let Some(plan) = stage_plan(
            self.stage_ends(),
            state.scene.max_bounces,
            pixels * state.spp as usize,
            max_records,
        ) else {
            return;
        };
        if !self.staged.pipelines.contains_key(&class) {
            let pipeline = self.build_staged_pipeline(class);
            self.staged.pipelines.insert(class, pipeline);
            tracing::info!(
                target: "pleochroma_cut::render_bench",
                "gpu pipeline: staged megakernel, stage ends {:?}",
                self.stage_ends()
            );
        }
        if self.staged.prepare.is_none() {
            self.staged.prepare = Some(compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &self.ctx.adapter.get_info(),
                PREPARE_ENTRY,
                PREPARE_SHADER_SRC,
                PREPARE_ENTRY,
                &[],
            ));
        }
        self.ensure_staged_buffers(plan.records, max_records);
    }

    /// Grows the survivor buffer to at least `records` (at most `max_records`) and creates the
    /// small fixed buffers on first use.
    fn ensure_staged_buffers(&mut self, records: usize, max_records: usize) {
        let device = &self.ctx.device;
        if self.staged.rays.is_none() || self.staged.rays_records < records {
            let grown =
                (records.div_ceil(RAYS_GROWTH_RECORDS) * RAYS_GROWTH_RECORDS).min(max_records);
            self.staged.rays = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("staged megakernel survivor records"),
                size: (grown * STAGE_WORDS * size_of::<u32>()) as wgpu::BufferAddress,
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));
            self.staged.rays_records = grown;
        }
        if self.staged.counters.is_none() {
            self.staged.counters = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("staged megakernel survivor counters"),
                size: ((MAX_STAGE_ENDS + 1) * size_of::<u32>()) as wgpu::BufferAddress,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if self.staged.indirect.is_none() {
            self.staged.indirect = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("staged megakernel indirect arguments"),
                size: (MAX_STAGE_ENDS * 3 * size_of::<u32>()) as wgpu::BufferAddress,
                usage: BufferUsages::STORAGE | BufferUsages::INDIRECT,
                mapped_at_creation: false,
            }));
        }
        while self.staged.params.len() < MAX_STAGE_ENDS + 1 {
            self.staged
                .params
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("staged megakernel stage params"),
                    size: size_of::<GpuStageParams>() as wgpu::BufferAddress,
                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
    }

    /// The stages a chunk of `tuples` rays runs, or `None` for the single megakernel: the class
    /// does not run staged, or [`Self::ensure_staged`] has not prepared it. The plan is made for
    /// the survivor buffer that exists, so it always fits it.
    pub(super) fn staged_plan_for(
        &self,
        state: &ChunkFrameState<'_>,
        tuples: usize,
    ) -> Option<StagePlan> {
        if !self.staged_class(state.pipeline_class)
            || !self.staged.pipelines.contains_key(&state.pipeline_class)
            || self.staged.prepare.is_none()
        {
            return None;
        }
        let plan = stage_plan(
            self.stage_ends(),
            state.scene.max_bounces,
            tuples,
            self.staged.rays_records,
        )?;
        (plan.params.len() <= self.staged.params.len()).then_some(plan)
    }

    /// Writes the parameters of every stage of `plan` and builds the bind groups: one per stage
    /// for the staged kernel, one per stage end for the prepare kernel.
    fn stage_bind_groups(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        params_buf: &wgpu::Buffer,
        outputs: &TransportOutputs,
        plan: &StagePlan,
    ) -> (Vec<wgpu::BindGroup>, Vec<wgpu::BindGroup>) {
        let staged = &self.staged;
        let pipeline = staged.pipelines.get(&state.pipeline_class).expect(PREPARED);
        let prepare = staged.prepare.as_ref().expect(PREPARED);
        let rays = staged.rays.as_ref().expect(PREPARED);
        let counters = staged.counters.as_ref().expect(PREPARED);
        let indirect = staged.indirect.as_ref().expect(PREPARED);
        let device = &self.ctx.device;
        let planes_bytes = (state.packed_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
        let finishes_bytes = (state.packed_len * size_of::<u32>()) as wgpu::BufferAddress;
        let mut stage_groups = Vec::with_capacity(plan.params.len());
        let mut prepare_groups = Vec::with_capacity(plan.params.len());
        for (index, stage) in plan.params.iter().enumerate() {
            let stage_buf = &staged.params[index];
            self.ctx
                .queue
                .write_buffer(stage_buf, 0, bytemuck::bytes_of(stage));
            stage_groups.push(compute::bind_buffers_sized(
                device,
                "staged transport bind group",
                pipeline,
                &[
                    (0, &frame_buffers.camera, None),
                    (1, params_buf, None),
                    (2, &frame_buffers.material, None),
                    (3, &frame_buffers.planes, Some(planes_bytes)),
                    (4, outputs.xyz(), None),
                    (5, outputs.radiance(), None),
                    (6, outputs.lambdas(), None),
                    (7, outputs.path_pdf(), None),
                    (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
                    (9, outputs.compat(), None),
                    (10, &frame_buffers.hdr_env.texels, None),
                    (11, &frame_buffers.hdr_env.dims, None),
                    (12, &frame_buffers.hdr_env.dist_func, None),
                    (13, &frame_buffers.hdr_env.dist_cdf, None),
                    (14, &frame_buffers.hdr_env.dist_dims, None),
                    (STAGE_PARAMS_BINDING, stage_buf, None),
                    (STAGE_RAYS_BINDING, rays, None),
                    (STAGE_COUNTERS_BINDING, counters, None),
                ],
            ));
            if index + 1 < plan.params.len() {
                prepare_groups.push(compute::bind_buffers(
                    device,
                    "stage prepare bind group",
                    prepare,
                    &[
                        (STAGE_PARAMS_BINDING, stage_buf),
                        (STAGE_COUNTERS_BINDING, counters),
                        (STAGE_INDIRECT_BINDING, indirect),
                    ],
                ));
            }
        }
        (stage_groups, prepare_groups)
    }

    /// Queues one chunk through the stages of `plan`, in one submission and without waiting:
    /// the counters are cleared, stage 0 runs over every tuple, and each further stage follows
    /// the prepare kernel that wrote its indirect arguments. Fills `outputs.xyz()` as the
    /// megakernel dispatch does.
    ///
    /// The stage buffers are shared by both chunk slots: a chunk's commands run to their end
    /// before the next chunk's begin (queue order), and `write_buffer` and `clear_buffer` are
    /// ordered with them.
    ///
    /// # Panics
    ///
    /// Panics if `plan` did not come from [`Self::staged_plan_for`] of this renderer.
    pub(super) fn dispatch_staged(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        params_buf: &wgpu::Buffer,
        outputs: &TransportOutputs,
        tuples: usize,
        plan: &StagePlan,
    ) {
        let mut mark = timing::Mark::now();
        let (stage_groups, prepare_groups) =
            self.stage_bind_groups(state, frame_buffers, params_buf, outputs, plan);
        mark.lap(Slot::TransportBind);

        let staged = &self.staged;
        let pipeline = staged.pipelines.get(&state.pipeline_class).expect(PREPARED);
        let prepare = staged.prepare.as_ref().expect(PREPARED);
        let counters = staged.counters.as_ref().expect(PREPARED);
        let indirect = staged.indirect.as_ref().expect(PREPARED);
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("pleochroma gpu staged dispatch encoder"),
            });
        encoder.clear_buffer(counters, 0, None);
        for (index, group) in stage_groups.iter().enumerate() {
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("pleochroma gpu staged transport pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                if index == 0 {
                    pass.dispatch_workgroups(workgroups_for(tuples), 1, 1);
                } else {
                    // Three words per stage end, written by the prepare pass just before.
                    let offset = ((index - 1) * 3 * size_of::<u32>()) as wgpu::BufferAddress;
                    pass.dispatch_workgroups_indirect(indirect, offset);
                }
            }
            if let Some(prepare_group) = prepare_groups.get(index) {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("pleochroma gpu stage prepare pass"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(prepare);
                pass.set_bind_group(0, prepare_group, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
        }
        let commands = encoder.finish();
        mark.lap(Slot::TransportEncode);
        self.ctx.queue.submit(Some(commands));
        mark.lap(Slot::TransportSubmit);
        if timing::enabled() {
            tracing::info!(
                target: "pleochroma_cut::render_bench",
                "gpu_timing staged_chunk tuples={tuples} stages={} first_region_records={} \
                 scaled={}",
                plan.params.len(),
                plan.params[0].out_capacity,
                plan.scaled
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::{
        GpuStageParams, STAGE_NEVER_PARKS, STAGE_WORDS, stage_ends_requested, stage_ends_valid,
        stage_plan,
    };
    use crate::{
        geometry::cuts::StandardGemCuts,
        optics::{
            materials::GemMaterial,
            raytracer::{Camera, LightingPreset},
        },
        renderer::gpu::{
            frame::{GpuFrameScene, SortedSwitch, classify_material},
            merge_tests::renderer_or_skip,
        },
    };

    #[test]
    fn the_switch_reads_off_the_default_and_ascending_lists() {
        assert_eq!(stage_ends_requested(None), vec![5, 12]);
        assert_eq!(stage_ends_requested(Some("")), vec![5, 12]);
        assert_eq!(stage_ends_requested(Some("0")), [] as [u32; 0]);
        assert_eq!(stage_ends_requested(Some("1")), vec![5, 12]);
        assert_eq!(stage_ends_requested(Some("8,16")), vec![8, 16]);
        assert_eq!(stage_ends_requested(Some("5")), vec![5]);
        assert_eq!(stage_ends_requested(Some(" 5, 12 ")), vec![5, 12]);
        assert_eq!(stage_ends_requested(Some("2,3,4")), vec![2, 3, 4]);
        assert_eq!(stage_ends_requested(Some("8,8")), [] as [u32; 0]);
        assert_eq!(stage_ends_requested(Some("12,5")), [] as [u32; 0]);
        assert_eq!(stage_ends_requested(Some("0,5")), [] as [u32; 0]);
        assert_eq!(stage_ends_requested(Some("2,3,4,5")), [] as [u32; 0]);
        assert_eq!(stage_ends_requested(Some("true")), [] as [u32; 0]);
    }

    #[test]
    fn only_short_positive_ascending_lists_are_valid() {
        assert!(stage_ends_valid(&[]));
        assert!(stage_ends_valid(&[8]));
        assert!(stage_ends_valid(&[1, 2, 3]));
        assert!(!stage_ends_valid(&[0]));
        assert!(!stage_ends_valid(&[5, 3]));
        assert!(!stage_ends_valid(&[5, 5]));
        assert!(!stage_ends_valid(&[1, 2, 3, 4]));
    }

    #[test]
    fn a_plan_chains_the_regions_and_ends_with_a_stage_that_never_parks() {
        let plan = stage_plan(&[8, 16], 32, 1000, usize::MAX).expect("two ends under the cap");
        assert_eq!(plan.records, 750);
        assert!(!plan.scaled);
        let blank = GpuStageParams {
            mode: 0,
            first_bounce: 0,
            end_bounce: 0,
            in_base: 0,
            in_counter: 0,
            in_capacity: 0,
            out_base: 0,
            out_counter: 0,
            out_capacity: 0,
            pad: [0; 3],
        };
        let second_base = u32::try_from(500 * STAGE_WORDS).expect("fits");
        assert_eq!(
            plan.params,
            vec![
                GpuStageParams {
                    end_bounce: 8,
                    out_capacity: 500,
                    ..blank
                },
                GpuStageParams {
                    mode: 1,
                    first_bounce: 8,
                    end_bounce: 16,
                    in_capacity: 500,
                    out_base: second_base,
                    out_counter: 1,
                    out_capacity: 250,
                    ..blank
                },
                GpuStageParams {
                    mode: 1,
                    first_bounce: 16,
                    end_bounce: STAGE_NEVER_PARKS,
                    in_base: second_base,
                    in_counter: 1,
                    in_capacity: 250,
                    ..blank
                },
            ]
        );
    }

    #[test]
    fn ends_at_or_over_the_cap_drop_out_and_small_budgets_scale_or_decline() {
        assert!(stage_plan(&[8, 16], 8, 1000, usize::MAX).is_none());
        assert!(stage_plan(&[], 32, 1000, usize::MAX).is_none());
        assert!(stage_plan(&[8], 32, 0, usize::MAX).is_none());
        assert!(stage_plan(&[8], 32, 1000, 0).is_none());
        let one = stage_plan(&[8, 16], 12, 1000, usize::MAX).expect("the end 8 is under 12");
        assert_eq!(one.params.len(), 2);
        assert_eq!(one.records, 500);
        // Small chunks keep a floor per region.
        let small = stage_plan(&[8, 16], 32, 100, usize::MAX).expect("plan");
        assert_eq!(small.records, 128);
        // 750 wanted, 300 allowed: every region divided by 3.
        let scaled = stage_plan(&[8, 16], 32, 1000, 300).expect("plan");
        assert!(scaled.scaled);
        assert_eq!(scaled.params[0].out_capacity, 166);
        assert_eq!(scaled.params[1].out_capacity, 83);
        assert_eq!(scaled.records, 249);
        // A plan made for a budget fits it, also just under the scaling threshold.
        for tuples in [900, 1000, 1001, 4000] {
            let plan = stage_plan(&[8, 16], 32, tuples, 700).expect("plan");
            assert!(plan.records <= 700, "{tuples}: {}", plan.records);
        }
        assert!(stage_plan(&[8, 16], 32, 1000, 1).is_none());
    }

    /// The staged kernel against the megakernel on one scene, every stage end set in turn.
    ///
    /// Panics with the numbers. A staged run repeated must be bitwise equal (a ray resumed and
    /// a ray left in its thread run the same code on exactly stored state), and it must equal
    /// the megakernel up to compiler rounding between two kernels: at most one pixel in twenty
    /// beyond 1e-3 relative, totals within 1 %.
    fn staged_twin(material_name: &str, test_name: &str) {
        let Some(mut renderer) = renderer_or_skip(test_name) else {
            return;
        };
        // The staged kernel against the megakernel whatever `PLEOCHROMA_SORTED` says: the sorted
        // wavefront would take every chunk ahead of both.
        renderer.set_sorted(SortedSwitch::Off);
        let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height, spp) = (32u32, 32u32, 3u32);
        let num_pixels = (width * height) as usize;
        let material = GemMaterial::by_name(material_name).expect("a built-in material");
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 12,
            environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        };
        let class = classify_material(&material);
        let mut render = |ends: Vec<u32>| {
            renderer.set_stage_ends(Some(ends));
            let mut accum = vec![Vec3::ZERO; num_pixels];
            renderer
                .accumulate_via_pipeline(&scene, class, 0, spp, &mut accum, None)
                .expect("the scene is GPU-supported under the studio rig");
            accum
        };
        let plain = render(Vec::new());
        let plain_total: f32 = plain.iter().map(|p| p.y).sum();
        assert!(
            plain_total > 0.0,
            "{material_name}: a black megakernel image"
        );
        // With [1, 2, 3] a ray is handed on three times, and a region that fills up leaves
        // rays in their threads, so both ways to the end of a path are compared.
        for ends in [vec![3], vec![2, 5], vec![1, 2, 3]] {
            let staged = render(ends.clone());
            let again = render(ends.clone());
            let bitwise = staged
                .iter()
                .zip(&again)
                .filter(|(a, b)| {
                    a.to_array()
                        .iter()
                        .zip(b.to_array().iter())
                        .any(|(x, y)| x.to_bits() != y.to_bits())
                })
                .count();
            assert_eq!(
                bitwise, 0,
                "{material_name}, ends {ends:?}: two staged runs differ in {bitwise} pixels"
            );
            let beyond = staged
                .iter()
                .zip(&plain)
                .filter(|(a, b)| {
                    a.to_array()
                        .iter()
                        .zip(b.to_array().iter())
                        .any(|(x, y)| (x - y).abs() > 1e-3f32.mul_add(x.abs().max(y.abs()), 1e-6))
                })
                .count();
            let total: f32 = staged.iter().map(|p| p.y).sum();
            assert!(
                beyond * 20 <= num_pixels && (total - plain_total).abs() <= 0.01 * plain_total,
                "{material_name}, ends {ends:?}: {beyond} of {num_pixels} pixels beyond rounding, \
                 totals {total} vs {plain_total}"
            );
        }
        // Without the staged pipeline the comparisons above set the megakernel against itself;
        // only a device that cannot run the staged kernel may leave it unbuilt.
        assert!(
            renderer.staged.pipelines.contains_key(&class) || !renderer.staged_class(class),
            "{material_name}: the staged kernel never ran"
        );
    }

    #[test]
    fn the_staged_kernel_agrees_with_the_megakernel_on_a_uniaxial_stone() {
        staged_twin(
            "Zircon",
            "the_staged_kernel_agrees_with_the_megakernel_on_a_uniaxial_stone",
        );
    }

    #[test]
    fn the_staged_kernel_agrees_with_the_megakernel_on_an_isotropic_stone() {
        staged_twin(
            "Diamond",
            "the_staged_kernel_agrees_with_the_megakernel_on_an_isotropic_stone",
        );
    }
}
