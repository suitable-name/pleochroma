//! Native metric classification, exact integer mode reductions and ordered tone compaction.
use super::{OpticalError, buffers, diagnostics::Counters, lighting::ExitLighting};
use cudarc::{
    driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::{CompileOptions, compile_ptx_with_opts},
};
use pleochroma::optics::raytracer::LightingPreset;
use pleochroma_cut_core::optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW};
use std::{
    collections::HashMap,
    sync::{Arc, atomic::Ordering},
};

pub(super) struct Postprocess {
    stream: Arc<CudaStream>,
    classify: CudaFunction,
    reduce: CudaFunction,
    compact: CudaFunction,
    lights: CudaSlice<f32>,
    offsets: HashMap<LightingPreset, u32>,
    metrics: CudaSlice<f32>,
    paths: CudaSlice<f64>,
    config: CudaSlice<f64>,
    metadata: CudaSlice<f64>,
    counters: Arc<Counters>,
}
fn error(e: impl std::fmt::Display) -> OpticalError {
    OpticalError::Runtime(format!("CUDA metric postprocessing: {e}"))
}
fn launch(n: u32) -> LaunchConfig {
    LaunchConfig {
        grid_dim: (n.div_ceil(128), 1, 1),
        block_dim: (128, 1, 1),
        shared_mem_bytes: 0,
    }
}
impl Postprocess {
    pub fn new(
        context: &Arc<CudaContext>,
        stream: Arc<CudaStream>,
        counters: Arc<Counters>,
    ) -> Result<Self, OpticalError> {
        use cudarc::driver::sys::CUdevice_attribute_enum::{
            CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
            CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
        };
        let major = context
            .attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR)
            .map_err(error)?;
        let minor = context
            .attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR)
            .map_err(error)?;
        let source = [
            include_str!("math.cuh"),
            include_str!("lighting.cuh"),
            include_str!("reduce.cu"),
        ]
        .join("\n");
        let ptx = compile_ptx_with_opts(
            source,
            CompileOptions {
                ftz: Some(false),
                prec_div: Some(true),
                prec_sqrt: Some(true),
                fmad: Some(false),
                use_fast_math: Some(false),
                options: vec![
                    format!("--gpu-architecture=compute_{major}{minor}"),
                    "--std=c++17".into(),
                ],
                ..CompileOptions::default()
            },
        )
        .map_err(error)?;
        let module = context.load_module(ptx).map_err(error)?;
        let (mut lights, mut offsets) = (Vec::new(), HashMap::new());
        for preset in LightingPreset::ALL {
            let light =
                ExitLighting::new(preset.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH));
            offsets.insert(preset, u32::try_from(lights.len()).map_err(error)?);
            lights.extend(light.cuda_parameters().ok_or_else(|| {
                OpticalError::Unsupported(
                    "HDR optical lighting requires a native texture sampler".into(),
                )
            })?);
        }
        Ok(Self {
            classify: module.load_function("classify_records").map_err(error)?,
            reduce: module.load_function("reduce_pose_counts").map_err(error)?,
            compact: module.load_function("compact_tone_paths").map_err(error)?,
            lights: stream.clone_htod(&lights).map_err(error)?,
            offsets,
            metrics: stream.alloc_zeros(1).map_err(error)?,
            paths: stream.alloc_zeros(1).map_err(error)?,
            config: stream.alloc_zeros(1).map_err(error)?,
            metadata: stream.alloc_zeros(1).map_err(error)?,
            stream,
            counters,
        })
    }
    pub fn light_offset(&self, preset: LightingPreset) -> Result<u32, OpticalError> {
        self.offsets
            .get(&preset)
            .copied()
            .ok_or_else(|| OpticalError::Unsupported("unknown optical lighting preset".into()))
    }
    pub fn classify_reduce(
        &mut self,
        records: &mut CudaSlice<f32>,
        poses: &CudaSlice<f32>,
        descriptors: &CudaSlice<u32>,
        count: u32,
        rays: u32,
    ) -> Result<Vec<[f32; 3]>, OpticalError> {
        if count == 0 {
            return Ok(Vec::new());
        }
        buffers::ensure(&self.stream, &mut self.metrics, count as usize * 4)?;
        let mut args = self.stream.launch_builder(&self.classify);
        args.arg(&mut *records)
            .arg(poses)
            .arg(&self.lights)
            .arg(descriptors)
            .arg(&count)
            .arg(&rays);
        // SAFETY: descriptor ranges partition rays, and all buffers remain live on this stream.
        unsafe { args.launch(launch(rays)) }.map_err(error)?;
        self.counters.launches.fetch_add(1, Ordering::Relaxed);
        let mut args = self.stream.launch_builder(&self.reduce);
        args.arg(&*records)
            .arg(descriptors)
            .arg(&count)
            .arg(&mut self.metrics);
        // SAFETY: one 128-thread block owns each descriptor and its four metric slots.
        unsafe {
            args.launch(LaunchConfig {
                grid_dim: (count, 1, 1),
                block_dim: (128, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(error)?;
        self.counters.launches.fetch_add(1, Ordering::Relaxed);
        let values = self
            .stream
            .clone_dtoh(&self.metrics.slice(..count as usize * 4))
            .map_err(error)?;
        values
            .as_chunks::<4>()
            .0
            .iter()
            .map(|row| {
                if row[3] != 0.0 || row.iter().any(|x| !x.is_finite()) {
                    Err(error("invalid native optical ray or metric"))
                } else {
                    Ok([row[0], row[1], row[2]])
                }
            })
            .collect()
    }
    pub fn compact_tones(
        &mut self,
        records: &CudaSlice<f32>,
        config: &[[f64; 6]],
    ) -> Result<Vec<[f64; 7]>, OpticalError> {
        if config.is_empty() {
            return Ok(Vec::new());
        }
        if config.len() > 128 {
            return Err(error("more than 128 face-up tone candidates"));
        }
        let packed: Vec<_> = config.iter().flatten().copied().collect();
        buffers::upload(&self.stream, &mut self.config, &packed)?;
        buffers::ensure(&self.stream, &mut self.paths, config.len() * 4096 * 5)?;
        buffers::ensure(&self.stream, &mut self.metadata, config.len() * 7)?;
        let count = u32::try_from(config.len()).map_err(error)?;
        let mut args = self.stream.launch_builder(&self.compact);
        args.arg(records)
            .arg(&self.config)
            .arg(&count)
            .arg(&mut self.paths)
            .arg(&mut self.metadata);
        // SAFETY: each candidate owns 4096 path slots and seven metadata values.
        unsafe { args.launch(launch(count)) }.map_err(error)?;
        self.counters.launches.fetch_add(1, Ordering::Relaxed);
        let values = self
            .stream
            .clone_dtoh(&self.metadata.slice(..config.len() * 7))
            .map_err(error)?;
        values
            .as_chunks::<7>()
            .0
            .iter()
            .map(|row| {
                if row[6] != 0.0 || row.iter().any(|v| !v.is_finite()) {
                    Err(error("invalid compacted tone"))
                } else {
                    Ok([row[0], row[1], row[2], row[3], row[4], row[5], row[6]])
                }
            })
            .collect()
    }
    pub fn paths(&self) -> &CudaSlice<f64> {
        &self.paths
    }
}
