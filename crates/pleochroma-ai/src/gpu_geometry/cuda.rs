//! Native CUDA residual batches, with cached immutable network buffers.
use super::network::{Network, Plane, Probe, scratch_per_probe};
use cudarc::{
    driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::{CompileOptions, compile_ptx_with_opts},
};
use std::sync::{Arc, Mutex};

// Mirrors stone_metrics::pow2_scale_norm, including subnormal inputs.
fn arrangement_scale(value: f64) -> f64 {
    const MANTISSA: u64 = (1 << 52) - 1;
    if !(value.is_finite() && value > 0.0) {
        return 1.0;
    }
    let raw = value.to_bits();
    let (bits, lift) = if raw >> 52 == 0 {
        ((value * 18_446_744_073_709_551_616.0).to_bits(), 64_i64)
    } else {
        (raw, 0_i64)
    };
    let biased = i64::from(u16::try_from(bits >> 52).unwrap_or(1023));
    let mantissa = f64::from_bits((bits & MANTISSA) | (1023_u64 << 52));
    let mut exponent = biased - 1023 - lift;
    if mantissa >= std::f64::consts::SQRT_2 {
        exponent += 1;
    }
    let biased_out = u64::try_from((exponent + 1023).clamp(1, 2046)).unwrap_or(1023);
    f64::from_bits(biased_out << 52)
}

pub(super) enum ArrangementOutcome {
    Refuted,
    Witnessed,
    Uncertain,
    /// Plane capacity or deterministic proof-work ceiling; use the original gate.
    Capacity,
}

#[derive(Default)]
pub(super) struct ArrangementWork {
    pub tiles: u64,
    pub triples: u64,
}

pub(super) struct Runtime {
    context: Arc<CudaContext>,
    residual: CudaFunction,
    gauge: CudaFunction,
    arrangement: CudaFunction,
}

struct PrecheckFlags {
    enabled: bool,
    fixed_outline: bool,
    lock_gauge: bool,
}

impl Runtime {
    pub fn new(ordinal: usize) -> Result<Self, String> {
        let context = CudaContext::new(ordinal).map_err(|e| e.to_string())?;
        let (major, minor) = context.compute_capability().map_err(|e| e.to_string())?;
        let ptx = compile_ptx_with_opts(
            format!(
                "{}\n{}\n{}",
                include_str!("interval.cuh"),
                include_str!("residuals.cu"),
                include_str!("arrangement.cuh")
            ),
            CompileOptions {
                fmad: Some(false),
                use_fast_math: Some(false),
                prec_div: Some(true),
                prec_sqrt: Some(true),
                options: vec![format!("--gpu-architecture=compute_{major}{minor}")],
                ..CompileOptions::default()
            },
        )
        .map_err(|e| e.to_string())?;
        let module = context.load_module(ptx).map_err(|e| e.to_string())?;
        let residual = module
            .load_function("residuals")
            .map_err(|e| e.to_string())?;
        let gauge = module.load_function("gauge").map_err(|e| e.to_string())?;
        let arrangement = module
            .load_function("arrangement_proof")
            .map_err(|e| e.to_string())?;
        Ok(Self {
            context,
            residual,
            gauge,
            arrangement,
        })
    }

    /// A missing witness proves rejection; all other outcomes retain the gate.
    pub fn arrangement_proof(
        &self,
        net: &Network,
        device: &DeviceNetwork,
        planes: &[Plane],
        work: &mut ArrangementWork,
        check: &dyn Fn() -> Result<(), String>,
    ) -> Result<ArrangementOutcome, String> {
        let n = planes.len();
        if !(3..=256).contains(&n) {
            return Ok(ArrangementOutcome::Capacity);
        }
        if n != net.surfaces
            || net.junctions.is_empty()
            || !net.scale.is_finite()
            || net.scale <= 0.0
        {
            return Ok(ArrangementOutcome::Uncertain);
        }
        // This optional rejection proof must never monopolize the device. A
        // bypass is UNKNOWN, never a partial scan reported as a refutation.
        // Check before packing, locking, uploads or scratch allocation.
        let Some(total) = net.arrangement_triples_within_budget() else {
            return Ok(ArrangementOutcome::Capacity);
        };
        let max = planes
            .iter()
            .map(|(_, d)| d.abs())
            .fold(net.scale, f64::max);
        let scale = arrangement_scale(max);
        // These guards keep every interval intermediate finite, including the
        // inverse of a determinant whose interval excludes zero. Cases outside
        // this generous range keep the original gate.
        if !(1e-100..=1e100).contains(&scale)
            || planes.iter().any(|(normal, d)| {
                !normal.is_finite()
                    || !d.is_finite()
                    || normal.abs().max_element() > 2.0
                    || (d / scale).abs() > 2.0
            })
            || net
                .junctions
                .iter()
                .any(|j| !j.bound.is_finite() || j.bound < 0.0)
        {
            return Ok(ArrangementOutcome::Uncertain);
        }
        let original: Vec<f64> = planes
            .iter()
            .flat_map(|(v, d)| [v.x, v.y, v.z, *d])
            .collect();
        let normalized: Vec<f64> = planes
            .iter()
            .flat_map(|(v, d)| [v.x, v.y, v.z, *d / scale])
            .collect();
        let stream = &device.stream;
        // The device stream is shared with residual batches for this episode.
        let _guard = device.scratch.lock().map_err(|e| e.to_string())?;
        let raw = stream.clone_htod(&original).map_err(|e| e.to_string())?;
        let normalized = stream.clone_htod(&normalized).map_err(|e| e.to_string())?;
        let mut flags = stream
            .alloc_zeros::<u32>(net.junctions.len() + 1)
            .map_err(|e| e.to_string())?;
        let ns = n as i32;
        let nj = i32::try_from(net.junctions.len()).map_err(|e| e.to_string())?;
        for start in (0..total).step_by(32_768) {
            check()?;
            let count = (total - start).min(32_768) as u32;
            let mut launch = stream.launch_builder(&self.arrangement);
            launch
                .arg(&normalized)
                .arg(&raw)
                .arg(&device.meta)
                .arg(&device.supports)
                .arg(&device.bounds)
                .arg(&mut flags)
                .arg(&ns)
                .arg(&nj)
                .arg(&scale)
                .arg(&start)
                .arg(&count);
            // SAFETY: rank is bounded by choose(n,3); all triples index uploaded
            // planes and immutable, checked metadata; flags contains nj+1 words.
            unsafe {
                launch.launch(LaunchConfig {
                    grid_dim: (count.div_ceil(128), 1, 1),
                    block_dim: (128, 1, 1),
                    shared_mem_bytes: 0,
                })
            }
            .map_err(|e| e.to_string())?;
            work.tiles += 1;
            work.triples += u64::from(count);
        }
        check()?;
        let flags = stream.clone_dtoh(&flags).map_err(|e| e.to_string())?;
        if flags[net.junctions.len()] != 0 {
            return Ok(ArrangementOutcome::Uncertain);
        }
        if flags[..net.junctions.len()].iter().all(|&v| v != 0) {
            Ok(ArrangementOutcome::Witnessed)
        } else {
            Ok(ArrangementOutcome::Refuted)
        }
    }

    #[cfg(test)]
    pub fn arrangement_possible(
        &self,
        net: &Network,
        device: &DeviceNetwork,
        planes: &[Plane],
    ) -> Result<Option<bool>, String> {
        self.arrangement_proof(
            net,
            device,
            planes,
            &mut ArrangementWork::default(),
            &|| Ok(()),
        )
        .map(|outcome| match outcome {
            ArrangementOutcome::Refuted => Some(false),
            ArrangementOutcome::Witnessed => Some(true),
            ArrangementOutcome::Uncertain | ArrangementOutcome::Capacity => None,
        })
    }

    pub fn prepare(&self, network: &Network) -> Result<DeviceNetwork, String> {
        let mut meta = Vec::<i32>::new();
        let mut supports = Vec::<i32>::new();
        let mut targets = vec![0.0; network.rows];
        let mut row = 0;
        for j in &network.junctions {
            let start = supports.len();
            supports.extend(
                j.support
                    .iter()
                    .map(|&i| i32::try_from(i))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?,
            );
            let [a, b, c] = j.basis;
            let values = [a, b, c, start, supports.len(), row];
            meta.extend(
                values
                    .map(i32::try_from)
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?,
            );
            targets[row..row + j.targets.len()].copy_from_slice(&j.targets);
            row += j.targets.len() + 1;
        }
        let gauge = network
            .gauge
            .iter()
            .map(|&i| i32::try_from(i))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let original: Vec<_> = network
            .junctions
            .iter()
            .flat_map(|j| [j.point.x, j.point.y, j.point.z])
            .collect();
        let bounds: Vec<_> = network.junctions.iter().map(|j| j.bound).collect();
        // Independent episode streams let concurrent rollout slots overlap.
        let stream = self.context.new_stream().map_err(|e| e.to_string())?;
        Ok(DeviceNetwork {
            meta: stream.clone_htod(&meta).map_err(|e| e.to_string())?,
            supports: stream.clone_htod(&supports).map_err(|e| e.to_string())?,
            targets: stream.clone_htod(&targets).map_err(|e| e.to_string())?,
            gauge: stream.clone_htod(&gauge).map_err(|e| e.to_string())?,
            original: stream.clone_htod(&original).map_err(|e| e.to_string())?,
            bounds: stream.clone_htod(&bounds).map_err(|e| e.to_string())?,
            stream,
            scratch: Mutex::new(None),
        })
    }

    pub fn batch(
        &self,
        net: &Network,
        device: &DeviceNetwork,
        batch: &[Vec<Plane>],
        limits: Option<&[f64]>,
        fixed_outline: bool,
        lock_gauge: bool,
    ) -> Result<Vec<Probe>, String> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }
        // Bounded scratch: <= 32 MiB total plane, residual and point buffers.
        let per = scratch_per_probe(net);
        if batch.len() > 64
            || per
                .checked_mul(batch.len())
                .is_none_or(|n| n > 4 * 1024 * 1024)
        {
            return Err("geometry CUDA batch exceeds 32 MiB scratch budget".into());
        }
        if batch.iter().any(|planes| planes.len() != net.surfaces) {
            return Err("changed plane provenance".into());
        }
        if limits.is_some_and(|limits| limits.len() != batch.len()) {
            return Err("geometry precheck limit count differs from the batch".into());
        }
        let input: Vec<f64> = batch
            .iter()
            .flat_map(|planes| planes.iter().flat_map(|(n, d)| [n.x, n.y, n.z, *d]))
            .collect();
        let stream = &device.stream;
        let mut scratch = device.scratch.lock().map_err(|e| e.to_string())?;
        if scratch.as_ref().is_none_or(|s| s.capacity < batch.len()) {
            // Drop the previous bounded allocation before growing; there are never
            // two retained scratch sets, and a run's episodes own separate locks.
            *scratch = None;
            *scratch = Some(Scratch {
                capacity: batch.len(),
                planes: stream
                    .alloc_zeros::<f64>(input.len())
                    .map_err(|e| e.to_string())?,
                output: stream
                    .alloc_zeros::<f64>(batch.len() * (net.rows + 1))
                    .map_err(|e| e.to_string())?,
                points: stream
                    .alloc_zeros::<f64>(batch.len() * net.junctions.len() * 7)
                    .map_err(|e| e.to_string())?,
                displacements: stream
                    .alloc_zeros::<f64>(batch.len())
                    .map_err(|e| e.to_string())?,
            });
        }
        let scratch = scratch.as_mut().ok_or("missing CUDA scratch")?;
        stream
            .memcpy_htod(&input, &mut scratch.planes.slice_mut(..input.len()))
            .map_err(|e| e.to_string())?;
        if let Some(limits) = limits {
            stream
                .memcpy_htod(limits, &mut scratch.displacements.slice_mut(..limits.len()))
                .map_err(|e| e.to_string())?;
        }
        self.launch(
            net,
            device,
            scratch,
            batch.len(),
            PrecheckFlags {
                enabled: limits.is_some(),
                fixed_outline,
                lock_gauge,
            },
        )?;
        let result = stream
            .clone_dtoh(&scratch.output.slice(..batch.len() * (net.rows + 1)))
            .map_err(|e| e.to_string())?;
        Ok(result
            .chunks_exact(net.rows + 1)
            .map(|result| {
                let rows = result[..net.rows].to_vec();
                Probe {
                    rows: rows.iter().all(|r| r.is_finite()).then_some(rows),
                    possible: result[net.rows] != 0.0,
                }
            })
            .collect())
    }
    fn launch(
        &self,
        net: &Network,
        device: &DeviceNetwork,
        scratch: &mut Scratch,
        batch_len: usize,
        flags: PrecheckFlags,
    ) -> Result<(), String> {
        let stream = &device.stream;
        let planes = &scratch.planes;
        let output = &mut scratch.output;
        let points = &mut scratch.points;
        let count = i32::try_from(batch_len).map_err(|e| e.to_string())?;
        let nj = i32::try_from(net.junctions.len()).map_err(|e| e.to_string())?;
        let ns = i32::try_from(net.surfaces).map_err(|e| e.to_string())?;
        let rows = i32::try_from(net.rows).map_err(|e| e.to_string())?;
        let ng = i32::try_from(net.gauge.len()).map_err(|e| e.to_string())?;
        let precheck = i32::from(flags.enabled);
        let fixed_outline = i32::from(flags.fixed_outline);
        let lock_gauge = i32::from(flags.lock_gauge);
        let tasks = u32::try_from(batch_len * net.junctions.len()).map_err(|e| e.to_string())?;
        let mut launch = stream.launch_builder(&self.residual);
        launch
            .arg(planes)
            .arg(&device.meta)
            .arg(&device.supports)
            .arg(&device.targets)
            .arg(&device.original)
            .arg(&device.bounds)
            .arg(&scratch.displacements)
            .arg(&mut *output)
            .arg(&mut *points)
            .arg(&count)
            .arg(&nj)
            .arg(&ns)
            .arg(&rows)
            .arg(&net.scale)
            .arg(&precheck)
            .arg(&fixed_outline);
        // SAFETY: checked dimensions bound every index in residuals.cu; buffers stay
        // alive through stream-synchronized readback. All scalar ABI types match C.
        unsafe {
            launch.launch(LaunchConfig {
                grid_dim: (tasks.div_ceil(128), 1, 1),
                block_dim: (128, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(|e| e.to_string())?;
        let mut launch = stream.launch_builder(&self.gauge);
        launch
            .arg(&*points)
            .arg(&device.gauge)
            .arg(&mut *output)
            .arg(&count)
            .arg(&nj)
            .arg(&ng)
            .arg(&rows)
            .arg(&net.radial)
            .arg(&net.height)
            .arg(&net.scale)
            .arg(&precheck)
            .arg(&lock_gauge);
        // SAFETY: the first launch populated every point; gauge indices were checked
        // while preparing the immutable host network, and cudarc orders both launches.
        unsafe {
            launch.launch(LaunchConfig {
                grid_dim: ((batch_len as u32).div_ceil(64), 1, 1),
                block_dim: (64, 1, 1),
                shared_mem_bytes: 0,
            })
        }
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

pub(super) struct DeviceNetwork {
    meta: CudaSlice<i32>,
    supports: CudaSlice<i32>,
    targets: CudaSlice<f64>,
    gauge: CudaSlice<i32>,
    original: CudaSlice<f64>,
    bounds: CudaSlice<f64>,
    stream: Arc<CudaStream>,
    scratch: Mutex<Option<Scratch>>,
}

struct Scratch {
    capacity: usize,
    planes: CudaSlice<f64>,
    output: CudaSlice<f64>,
    points: CudaSlice<f64>,
    displacements: CudaSlice<f64>,
}
