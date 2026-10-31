//! Native CUDA runtime using the same primary context as Burn/CubeCL CUDA.
use super::{
    OpticalError, ScoredOptics,
    buffers::{self, Buffers},
    diagnostics::Counters,
    fan::FanGeometry,
    postprocess::Postprocess,
    prepare,
    tone_cuda::ToneCuda,
    tools,
};
use cudarc::{
    driver::{CudaContext, CudaFunction, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::{CompileOptions, compile_ptx_with_opts},
};
use pleochroma::{
    color::metrics::{FaceUpTone, FinishedStone},
    optics::{
        materials::GemMaterial,
        raytracer::{FacetFinish, LightingPreset},
    },
};
use pleochroma_cut_core::{ObjectiveComponents, optimize::ObjectiveFidelity};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct CudaState {
    context: Arc<CudaContext>,
    stream: Arc<CudaStream>,
    planar_function: CudaFunction,
    csg_function: CudaFunction,
    planar_batch_function: CudaFunction,
    csg_batch_function: CudaFunction,
    max_poses: usize,
    cells: cudarc::driver::CudaSlice<f32>,
    cell_count: usize,
    counters: Arc<Counters>,
    buffers: Buffers,
    tone_cuda: ToneCuda,
    postprocess: Postprocess,
}

struct PackedBatch {
    planes: Vec<f32>,
    tools: Vec<f32>,
    params: Vec<f32>,
    poses: Vec<f32>,
    finishes: Vec<u32>,
    descriptors: Vec<u32>,
    ranges: Vec<(usize, usize, usize)>,
    half_widths: Vec<f32>,
    ray_count: usize,
    planar_rays: usize,
    metric_slots: Vec<Option<usize>>,
}
#[derive(Clone, Copy)]
struct DeviceCandidate {
    np: i32,
    nt: i32,
    nf: i32,
    modes: usize,
}
struct PoseRun<'a> {
    fan: FanGeometry,
    cancel: &'a AtomicBool,
    device: DeviceCandidate,
    preset: LightingPreset,
}

#[path = "runtime_tone.rs"]
mod runtime_tone;

struct ToneRequest<'a> {
    material: &'a GemMaterial,
    preset: LightingPreset,
    half_width: f32,
    start: usize,
    end: usize,
    modes: usize,
}

fn runtime_error(error: impl std::fmt::Display) -> OpticalError {
    OpticalError::Runtime(error.to_string())
}

fn trace_launch(rays: u32) -> LaunchConfig {
    // cudarc's generic helper uses 1024 threads. This ray kernel retains material,
    // geometry and bounce state; smaller blocks leave room for its register footprint.
    const THREADS: u32 = 128;
    LaunchConfig {
        grid_dim: (rays.div_ceil(THREADS), 1, 1),
        block_dim: (THREADS, 1, 1),
        shared_mem_bytes: 0,
    }
}

impl CudaState {
    pub(super) fn new(
        ordinal: usize,
        max_poses: usize,
        counters: Arc<Counters>,
    ) -> Result<Self, OpticalError> {
        // CudaContext::new retains cuDevicePrimaryCtxRetain, as cubecl-cuda0.11 does.
        let context =
            CudaContext::new(ordinal).map_err(|e| OpticalError::Unavailable(e.to_string()))?;
        let stream = context.new_stream().map_err(runtime_error)?;
        use cudarc::driver::sys::CUdevice_attribute_enum::{
            CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
            CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
        };
        let major = context
            .attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR)
            .map_err(runtime_error)?;
        let minor = context
            .attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR)
            .map_err(runtime_error)?;
        let source = [
            include_str!("math.cuh"),
            include_str!("geometry.cuh"),
            include_str!("frosted.cuh"),
            include_str!("trace.cu"),
        ]
        .join("\n");
        let ptx = compile_ptx_with_opts(
            source,
            CompileOptions {
                ftz: Some(false),
                prec_sqrt: Some(true),
                prec_div: Some(true),
                fmad: Some(false),
                use_fast_math: Some(false),
                options: vec![
                    format!("--gpu-architecture=compute_{major}{minor}"),
                    "--std=c++17".into(),
                ],
                ..CompileOptions::default()
            },
        )
        .map_err(|e| OpticalError::Unavailable(format!("NVRTC analytical optical kernel: {e}")))?;
        let module = context.load_module(ptx).map_err(runtime_error)?;
        let planar_function = module
            .load_function("optical_trace_planar")
            .map_err(runtime_error)?;
        let csg_function = module
            .load_function("optical_trace_csg")
            .map_err(runtime_error)?;
        let planar_batch_function = module
            .load_function("optical_trace_batch_planar")
            .map_err(runtime_error)?;
        let csg_batch_function = module
            .load_function("optical_trace_batch_csg")
            .map_err(runtime_error)?;
        let cells = prepare::cells();
        let cell_count = cells.len() / 2;
        let capacity = super::capacity::size(&context, max_poses, cell_count)?;
        let max_poses = capacity.max_pose_tile;
        let _ = counters.capacity.set(capacity);
        let cells = stream.clone_htod(&cells).map_err(runtime_error)?;
        let buffers = Buffers::new(&stream)?;
        let tone_cuda = ToneCuda::new(Arc::clone(&context), Arc::clone(&stream))?;
        let postprocess = Postprocess::new(&context, Arc::clone(&stream), Arc::clone(&counters))?;
        Ok(Self {
            context,
            stream,
            planar_function,
            csg_function,
            planar_batch_function,
            csg_batch_function,
            max_poses,
            cells,
            cell_count,
            counters,
            buffers,
            tone_cuda,
            postprocess,
        })
    }

    /// Batch distinct Fast candidates across candidate/ray dimensions.
    pub(super) fn evaluate_fast_batch(
        &mut self,
        candidates: &[super::worker::Candidate],
    ) -> Result<Vec<ScoredOptics>, OpticalError> {
        let batch = self.pack_fast_batch(candidates)?;
        let metrics = self.trace_fast_batch(&batch)?;
        self.reduce_fast_batch(candidates, &batch, &metrics)
    }

    fn pack_fast_batch(
        &self,
        candidates: &[super::worker::Candidate],
    ) -> Result<PackedBatch, OpticalError> {
        let (mut planes, mut tools, mut params, mut poses) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut finishes, mut descriptors) = (Vec::new(), Vec::new());
        let mut ranges = vec![(0usize, 0usize, 1usize); candidates.len()];
        let mut metric_slots = vec![None; candidates.len()];
        let mut half_widths = vec![1.0f32; candidates.len()];
        let mut ray_count = 0usize;
        let mut planar_rays = 0usize;
        let indices = (0..candidates.len())
            .filter(|&i| candidates[i].tools.is_empty())
            .chain((0..candidates.len()).filter(|&i| !candidates[i].tools.is_empty()));
        for i in indices {
            let c = &candidates[i];
            if c.planes.is_empty() {
                continue;
            }
            let stone = c.stone();
            let fan = c.fan;
            let material = prepare::material_params(&c.material);
            let modes = if material[0] == 0.0 { 1 } else { 2 };
            let end = ray_count + self.cell_count * 5 * modes;
            let nf = if c.finishes.contains(&FacetFinish::Frosted) {
                c.finishes.len()
            } else {
                0
            };
            metric_slots[i] = Some(descriptors.len() / 12);
            let descriptor = [
                ray_count,
                end,
                planes.len(),
                c.planes.len(),
                tools.len(),
                c.tools.len(),
                finishes.len(),
                nf,
                params.len(),
                poses.len(),
                self.postprocess.light_offset(c.preset)? as usize,
                modes,
            ];
            for field in descriptor {
                descriptors.push(u32::try_from(field).map_err(runtime_error)?);
            }
            planes.extend(
                c.planes
                    .iter()
                    .flat_map(|p| [p.normal[0], p.normal[1], p.normal[2], p.d]),
            );
            tools.extend(tools::pack(&c.tools));
            finishes.extend(prepare::finishes(stone));
            params.extend(material);
            poses.extend(prepare::pose_data(
                &[(0.0, 90.0f32.to_radians())],
                fan,
                true,
            ));
            ranges[i] = (ray_count, end, modes);
            half_widths[i] = fan.half_width();
            ray_count = end;
            if c.tools.is_empty() {
                planar_rays = end;
            }
        }
        Ok(PackedBatch {
            planes,
            tools,
            params,
            poses,
            finishes,
            descriptors,
            ranges,
            half_widths,
            ray_count,
            planar_rays,
            metric_slots,
        })
    }

    fn trace_fast_batch(&mut self, batch: &PackedBatch) -> Result<Vec<[f32; 3]>, OpticalError> {
        self.context.bind_to_thread().map_err(runtime_error)?;
        let ray_count = batch.ray_count;
        let planar_rays = batch.planar_rays;
        if ray_count != 0 {
            buffers::upload(&self.stream, &mut self.buffers.planes, &batch.planes)?;
            buffers::upload(&self.stream, &mut self.buffers.tools, &batch.tools)?;
            buffers::upload(&self.stream, &mut self.buffers.finishes, &batch.finishes)?;
            buffers::upload(&self.stream, &mut self.buffers.material, &batch.params)?;
            buffers::upload(&self.stream, &mut self.buffers.poses, &batch.poses)?;
            buffers::upload(
                &self.stream,
                &mut self.buffers.descriptors,
                &batch.descriptors,
            )?;
            buffers::ensure(&self.stream, &mut self.buffers.output, ray_count * 12)?;
            let count = u32::try_from(batch.descriptors.len() / 12).map_err(runtime_error)?;
            let cell_count = i32::try_from(self.cell_count).map_err(runtime_error)?;
            for (function, start, len) in [
                (&self.planar_batch_function, 0, planar_rays),
                (
                    &self.csg_batch_function,
                    planar_rays,
                    ray_count - planar_rays,
                ),
            ] {
                if len == 0 {
                    continue;
                }
                let begin = u32::try_from(start).map_err(runtime_error)?;
                let rays = u32::try_from(len).map_err(runtime_error)?;
                let mut launch = self.stream.launch_builder(function);
                launch
                    .arg(&self.buffers.planes)
                    .arg(&self.buffers.tools)
                    .arg(&self.buffers.finishes)
                    .arg(&self.buffers.material)
                    .arg(&self.buffers.poses)
                    .arg(&self.buffers.descriptors)
                    .arg(&count)
                    .arg(&begin)
                    .arg(&rays)
                    .arg(&self.cells)
                    .arg(&cell_count)
                    .arg(&mut self.buffers.output);
                // SAFETY: descriptors are range-checked u32 offsets into live packed buffers,
                // each ray writes its unique 12-float slot; GPU postprocessing follows on the same stream.
                unsafe { launch.launch(trace_launch(rays)) }.map_err(runtime_error)?;
                self.counters.launches.fetch_add(1, Ordering::Relaxed);
                self.counters
                    .rays
                    .fetch_add(u64::from(rays), Ordering::Relaxed);
            }
            return self.postprocess.classify_reduce(
                &mut self.buffers.output,
                &self.buffers.poses,
                &self.buffers.descriptors,
                count,
                ray_count as u32,
            );
        }
        Ok(Vec::new())
    }

    fn reduce_fast_batch(
        &mut self,
        candidates: &[super::worker::Candidate],
        batch: &PackedBatch,
        metrics: &[[f32; 3]],
    ) -> Result<Vec<ScoredOptics>, OpticalError> {
        let mut results = Vec::with_capacity(candidates.len());
        let mut tones = Vec::new();
        let mut tone_slots = Vec::new();
        for (i, candidate) in candidates.iter().enumerate() {
            let Some(slot) = batch.metric_slots[i] else {
                results.push(empty_score());
                continue;
            };
            let row = metrics
                .get(slot)
                .ok_or_else(|| runtime_error("native metric result missing"))?;
            results.push(ScoredOptics {
                components: components(*row),
                tone: FaceUpTone::NONE,
                poses: 1,
            });
            let (start, end, modes) = batch.ranges[i];
            tones.push(ToneRequest {
                material: &candidate.material,
                preset: candidate.preset,
                half_width: batch.half_widths[i],
                start,
                end,
                modes,
            });
            tone_slots.push(i);
        }
        let finished = self.finish_tones(&tones)?;
        if finished.len() != tone_slots.len() {
            return Err(runtime_error("native tone result count mismatch"));
        }
        for (slot, tone) in tone_slots.into_iter().zip(finished) {
            results[slot].tone = tone;
        }
        Ok(results)
    }

    pub(super) fn evaluate(
        &mut self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
        fidelity: ObjectiveFidelity,
        preset: LightingPreset,
        cancel: &AtomicBool,
        fan: FanGeometry,
    ) -> Result<ScoredOptics, OpticalError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(OpticalError::Cancelled);
        }
        if stone.geom.planes.is_empty() {
            return Ok(empty_score());
        }
        let device = self.upload_candidate(stone, material)?;
        let poses = prepare::poses(fidelity);
        let (rows, tone) = self.evaluate_poses(
            material,
            &poses,
            PoseRun {
                fan,
                cancel,
                device,
                preset,
            },
        )?;
        let components = if matches!(fidelity, ObjectiveFidelity::Fast) {
            components(rows[0])
        } else {
            let (mut b, mut e, mut w) = (0.0f64, 0.0f64, 0.0f64);
            for row in &rows[1..] {
                b += f64::from(row[0]);
                e += f64::from(row[1]);
                w += f64::from(row[2]);
            }
            let n = (rows.len() - 1) as f64;
            ObjectiveComponents {
                tilt_brilliance_pct: (b / n) as f32,
                extinction_pct: (e / n) as f32,
                windowing_pct: (w / n) as f32,
            }
        };
        Ok(ScoredOptics {
            components,
            tone,
            poses: rows.len(),
        })
    }

    fn upload_candidate(
        &mut self,
        stone: FinishedStone<'_>,
        material: &GemMaterial,
    ) -> Result<DeviceCandidate, OpticalError> {
        self.context.bind_to_thread().map_err(runtime_error)?;
        let packed_planes: Vec<f32> = stone
            .geom
            .planes
            .iter()
            .flat_map(|p| [p.normal[0], p.normal[1], p.normal[2], p.d])
            .collect();
        buffers::upload(&self.stream, &mut self.buffers.planes, &packed_planes)?;
        buffers::upload(
            &self.stream,
            &mut self.buffers.tools,
            &tools::pack(stone.geom.tools),
        )?;
        let params = prepare::material_params(material);
        let modes = if params[0] == 0.0 { 1 } else { 2 };
        buffers::upload(&self.stream, &mut self.buffers.material, &params)?;
        buffers::upload(
            &self.stream,
            &mut self.buffers.finishes,
            &prepare::finishes(stone),
        )?;
        let np = i32::try_from(stone.geom.planes.len()).map_err(runtime_error)?;
        let nt = i32::try_from(stone.geom.tools.len()).map_err(runtime_error)?;
        let nf = if stone.finishes.contains(&FacetFinish::Frosted) {
            i32::try_from(stone.finishes.len()).map_err(runtime_error)?
        } else {
            0
        };
        Ok(DeviceCandidate { np, nt, nf, modes })
    }

    fn evaluate_poses(
        &mut self,
        material: &GemMaterial,
        poses: &[(f32, f32)],
        run: PoseRun<'_>,
    ) -> Result<(Vec<[f32; 3]>, FaceUpTone), OpticalError> {
        let mut results = Vec::with_capacity(poses.len());
        let mut tone = FaceUpTone::NONE;
        for (tile_index, tile) in poses.chunks(self.max_poses).enumerate() {
            if run.cancel.load(Ordering::Relaxed) {
                return Err(OpticalError::Cancelled);
            }
            let first = tile_index == 0;
            let rows = self.trace_pose_tile(tile, run.device, run.fan, first, run.preset)?;
            if first {
                let request = ToneRequest {
                    material,
                    preset: run.preset,
                    half_width: run.fan.half_width(),
                    start: 0,
                    end: self.cell_count * 5 * run.device.modes,
                    modes: run.device.modes,
                };
                tone = self
                    .finish_tones(&[request])?
                    .into_iter()
                    .next()
                    .ok_or_else(|| runtime_error("native face-up tone missing"))?;
            }
            results.extend(rows);
            if run.cancel.load(Ordering::Relaxed) {
                return Err(OpticalError::Cancelled);
            }
        }
        Ok((results, tone))
    }

    fn trace_pose_tile(
        &mut self,
        tile: &[(f32, f32)],
        device: DeviceCandidate,
        fan: FanGeometry,
        first: bool,
        preset: LightingPreset,
    ) -> Result<Vec<[f32; 3]>, OpticalError> {
        let DeviceCandidate { np, nt, nf, modes } = device;
        let per_pose = self.cell_count * 5 * modes * 12;
        buffers::ensure(
            &self.stream,
            &mut self.buffers.output,
            per_pose * tile.len(),
        )?;
        buffers::upload(
            &self.stream,
            &mut self.buffers.poses,
            &prepare::pose_data(tile, fan, first),
        )?;
        let count = i32::try_from(tile.len()).map_err(runtime_error)?;
        let cell_count = i32::try_from(self.cell_count).map_err(runtime_error)?;
        let rays = u32::try_from(per_pose / 12 * tile.len()).map_err(runtime_error)?;
        let function = if nt == 0 {
            &self.planar_function
        } else {
            &self.csg_function
        };
        let mut launch = self.stream.launch_builder(function);
        launch
            .arg(&self.buffers.planes)
            .arg(&np)
            .arg(&self.buffers.tools)
            .arg(&nt)
            .arg(&self.buffers.finishes)
            .arg(&nf)
            .arg(&self.buffers.material)
            .arg(&self.buffers.poses)
            .arg(&count)
            .arg(&self.cells)
            .arg(&cell_count)
            .arg(&mut self.buffers.output);
        // SAFETY: all buffers remain live through ordered native postprocessing below;
        // kernel bounds every thread by count*cell_count*5*modes and writes 12 floats.
        unsafe { launch.launch(trace_launch(rays)) }.map_err(runtime_error)?;
        self.counters.launches.fetch_add(1, Ordering::Relaxed);
        self.counters
            .rays
            .fetch_add(u64::from(rays), Ordering::Relaxed);
        let mut descriptors = Vec::with_capacity(tile.len() * 12);
        let rays_per_pose = per_pose / 12;
        for pose in 0..tile.len() {
            descriptors.extend(
                [
                    pose * rays_per_pose,
                    (pose + 1) * rays_per_pose,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    pose * 12,
                    self.postprocess.light_offset(preset)? as usize,
                    modes,
                ]
                .map(|v| v as u32),
            );
        }
        buffers::upload(&self.stream, &mut self.buffers.descriptors, &descriptors)?;
        self.postprocess.classify_reduce(
            &mut self.buffers.output,
            &self.buffers.poses,
            &self.buffers.descriptors,
            tile.len() as u32,
            rays,
        )
    }
}

fn components(row: [f32; 3]) -> ObjectiveComponents {
    ObjectiveComponents {
        tilt_brilliance_pct: row[0],
        extinction_pct: row[1],
        windowing_pct: row[2],
    }
}
fn empty_score() -> ScoredOptics {
    ScoredOptics {
        components: ObjectiveComponents {
            windowing_pct: 5.0,
            extinction_pct: 5.0,
            tilt_brilliance_pct: 85.0,
        },
        tone: FaceUpTone::NONE,
        poses: 0,
    }
}
