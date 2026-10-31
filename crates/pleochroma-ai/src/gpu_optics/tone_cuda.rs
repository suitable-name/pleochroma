//! Batched native f64 Beer-Lambert exponentials with the reference serial reduction order.
//!
//! Production band evaluation, path compaction, exponentials and ordered f64 FMA sums
//! remain on CUDA. Only compact summaries and spectra return for the reference CIE
//! transform. Persistent scratch bounds each exponential tile to about 26 MiB.

use std::sync::Arc;

use cudarc::{
    driver::{CudaContext, CudaFunction, CudaSlice, CudaStream, LaunchConfig},
    nvrtc::{CompileOptions, compile_ptx_with_opts},
};

use super::OpticalError;
#[cfg(test)]
use cudarc::driver::PushKernelArg;
#[path = "tone_bands.rs"]
mod bands;
#[path = "tone_resident.rs"]
mod resident;

pub(super) const WAVELENGTHS: usize = 401;
pub(super) type ToneSpectrum = [f64; WAVELENGTHS];

const MAX_INPUTS: usize = 128;
const MAX_PATHS: usize = 4096;
const TILE_ROWS: usize = 8192;

/// One accumulator in original path/bin order. A path is [weight, D0, D1, D2].
pub(super) struct ToneInput {
    pub paths: Vec<[f64; 4]>,
    pub alpha: Vec<[f64; 3]>,
    pub principal: bool,
    pub channels: usize,
    pub scale: f64,
    /// Principal paths use the ordered f64 weight sum. Histogram weights are pre-normalized.
    pub denominator: f64,
}

struct Scratch {
    #[cfg(test)]
    paths: CudaSlice<f64>,
    #[cfg(test)]
    meta: CudaSlice<u32>,
    alpha: CudaSlice<f64>,
    output: CudaSlice<f64>,
    groups: CudaSlice<u32>,
    sums: CudaSlice<f64>,
    capacity: usize,
}

pub(super) struct ToneCuda {
    context: Arc<CudaContext>,
    stream: Arc<CudaStream>,
    #[cfg(test)]
    kernel: CudaFunction,
    #[cfg(test)]
    reduction: CudaFunction,
    resident_kernel: CudaFunction,
    resident_reduction: CudaFunction,
    band_kernel: CudaFunction,
    bands: CudaSlice<f32>,
    band_offsets: CudaSlice<u32>,
    scratch: Option<Scratch>,
    host_output: Vec<f64>,
    launches: u64,
    exponentials: u64,
}

fn error(error: impl std::fmt::Display) -> OpticalError {
    OpticalError::Runtime(format!("CUDA tone spectrum: {error}"))
}

fn spectral_launch(elements: u32) -> LaunchConfig {
    // Avoid cudarc's generic 1024-thread block for f64 transcendental work.
    // This bounds per-block register demand on both Ampere device families.
    const THREADS: u32 = 256;
    LaunchConfig {
        grid_dim: (elements.div_ceil(THREADS), 1, 1),
        block_dim: (THREADS, 1, 1),
        shared_mem_bytes: 0,
    }
}

impl ToneCuda {
    pub(super) fn new(
        context: Arc<CudaContext>,
        stream: Arc<CudaStream>,
    ) -> Result<Self, OpticalError> {
        use cudarc::driver::sys::CUdevice_attribute::{
            CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR,
            CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR,
        };
        context.bind_to_thread().map_err(error)?;
        let major = context
            .attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MAJOR)
            .map_err(error)?;
        let minor = context
            .attribute(CU_DEVICE_ATTRIBUTE_COMPUTE_CAPABILITY_MINOR)
            .map_err(error)?;
        let ptx = compile_ptx_with_opts(
            include_str!("tone_spectrum.cu"),
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
        #[cfg(test)]
        let kernel = module.load_function("tone_exponentials").map_err(error)?;
        #[cfg(test)]
        let reduction = module.load_function("tone_ordered_reduce").map_err(error)?;
        let resident_kernel = module
            .load_function("tone_resident_exponentials")
            .map_err(error)?;
        let resident_reduction = module
            .load_function("tone_resident_reduce")
            .map_err(error)?;
        let band_kernel = module.load_function("tone_band_alphas").map_err(error)?;
        let bands = stream.alloc_zeros(1).map_err(error)?;
        let band_offsets = stream.alloc_zeros(1).map_err(error)?;
        Ok(Self {
            context,
            stream,
            #[cfg(test)]
            kernel,
            #[cfg(test)]
            reduction,
            resident_kernel,
            resident_reduction,
            band_kernel,
            bands,
            band_offsets,
            scratch: None,
            host_output: Vec::new(),
            launches: 0,
            exponentials: 0,
        })
    }

    pub(super) const fn launches(&self) -> u64 {
        self.launches
    }

    pub(super) const fn exponentials(&self) -> u64 {
        self.exponentials
    }

    /// All candidates share launches; each tile includes all 401 wavelengths. Tile boundaries
    /// preserve each candidate/channel's original path order and its running f64 FMA sum.
    #[cfg(test)]
    pub(super) fn spectra(
        &mut self,
        inputs: &[ToneInput],
    ) -> Result<Vec<ToneSpectrum>, OpticalError> {
        validate(inputs)?;
        let alpha: Vec<f64> = inputs
            .iter()
            .flat_map(|input| input.alpha.iter().flatten().copied())
            .collect();
        let rows: Vec<_> = inputs
            .iter()
            .enumerate()
            .flat_map(|(candidate, input)| {
                (0..input.channels).flat_map(move |channel| {
                    (0..input.paths.len()).map(move |path| Row {
                        candidate,
                        channel,
                        path,
                    })
                })
            })
            .collect();
        let sum_count = inputs.len() * 3 * WAVELENGTHS;
        self.host_output.clear();
        self.host_output.resize(sum_count, 0.0);
        if !rows.is_empty() {
            self.context.bind_to_thread().map_err(error)?;
            self.reserve(rows.len().min(TILE_ROWS))?;
            let scratch = self.scratch.as_mut().expect("allocated above");
            self.stream
                .memcpy_htod(&alpha, &mut scratch.alpha.slice_mut(..alpha.len()))
                .map_err(error)?;
            self.stream
                .memcpy_htod(&self.host_output, &mut scratch.sums.slice_mut(..sum_count))
                .map_err(error)?;
        }
        for tile in rows.chunks(TILE_ROWS) {
            self.exponentials_tile(inputs, tile)?;
        }
        if !rows.is_empty() {
            let scratch = self.scratch.as_ref().expect("allocated above");
            self.stream
                .memcpy_dtoh(&scratch.sums.slice(..sum_count), &mut self.host_output)
                .map_err(error)?;
            self.stream.synchronize().map_err(error)?;
        }
        if self.host_output.iter().any(|x| !x.is_finite()) {
            return Err(error("non-finite spectral transmittance sum"));
        }
        Ok(inputs
            .iter()
            .enumerate()
            .map(|(candidate, input)| {
                let sums = std::array::from_fn(|channel| {
                    std::array::from_fn(|wave| {
                        self.host_output[(candidate * 3 + channel) * WAVELENGTHS + wave]
                    })
                });
                finish_sums(input, &sums)
            })
            .collect())
    }

    fn reserve(&mut self, rows: usize) -> Result<(), OpticalError> {
        if self
            .scratch
            .as_ref()
            .is_some_and(|scratch| scratch.capacity >= rows)
        {
            return Ok(());
        }
        let capacity = rows.next_power_of_two().min(TILE_ROWS);
        self.scratch = Some(Scratch {
            #[cfg(test)]
            paths: self
                .stream
                .alloc_zeros::<f64>(capacity * 5)
                .map_err(error)?,
            #[cfg(test)]
            meta: self
                .stream
                .alloc_zeros::<u32>(capacity * 3)
                .map_err(error)?,
            alpha: self
                .stream
                .alloc_zeros::<f64>(MAX_INPUTS * WAVELENGTHS * 3)
                .map_err(error)?,
            output: self
                .stream
                .alloc_zeros::<f64>(capacity * WAVELENGTHS)
                .map_err(error)?,
            groups: self
                .stream
                .alloc_zeros::<u32>(MAX_INPUTS * 3 * 6)
                .map_err(error)?,
            sums: self
                .stream
                .alloc_zeros::<f64>(MAX_INPUTS * 3 * WAVELENGTHS)
                .map_err(error)?,
            capacity,
        });
        Ok(())
    }

    #[cfg(test)]
    fn exponentials_tile(
        &mut self,
        inputs: &[ToneInput],
        rows: &[Row],
    ) -> Result<(), OpticalError> {
        let mut paths = Vec::with_capacity(rows.len() * 5);
        let mut meta = Vec::with_capacity(rows.len() * 3);
        let mut groups = Vec::<u32>::new();
        for (position, row) in rows.iter().enumerate() {
            let input = &inputs[row.candidate];
            let path = input.paths[row.path];
            paths.extend([path[0], path[1], path[2], path[3], input.scale]);
            let channel = if input.channels == 2 && row.channel == 1 {
                2
            } else {
                row.channel
            };
            meta.extend([
                u32::try_from(row.candidate).map_err(error)?,
                u32::from(input.principal),
                u32::try_from(channel).map_err(error)?,
            ]);
            let target = u32::try_from(row.candidate * 3 + row.channel).map_err(error)?;
            if groups.last().is_some_and(|&last| last == target) {
                let count_slot = groups.len() - 2;
                groups[count_slot] += 1;
            } else {
                groups.extend([u32::try_from(position).map_err(error)?, 1, target]);
            }
        }
        let scratch = self.scratch.as_mut().expect("allocated by spectra");
        self.stream
            .memcpy_htod(&paths, &mut scratch.paths.slice_mut(..paths.len()))
            .map_err(error)?;
        self.stream
            .memcpy_htod(&meta, &mut scratch.meta.slice_mut(..meta.len()))
            .map_err(error)?;
        self.stream
            .memcpy_htod(&groups, &mut scratch.groups.slice_mut(..groups.len()))
            .map_err(error)?;
        let count = rows.len() * WAVELENGTHS;
        let row_count = u32::try_from(rows.len()).map_err(error)?;
        let wavelength_count = u32::try_from(WAVELENGTHS).map_err(error)?;
        let mut launch = self.stream.launch_builder(&self.kernel);
        launch
            .arg(&scratch.paths)
            .arg(&scratch.meta)
            .arg(&scratch.alpha)
            .arg(&row_count)
            .arg(&wavelength_count)
            .arg(&mut scratch.output);
        // SAFETY: rows <= TILE_ROWS; meta candidate indices and channels were checked above.
        // Every wavelength/path writes one unique in-bounds output. Readback synchronizes it.
        unsafe { launch.launch(spectral_launch(u32::try_from(count).map_err(error)?)) }
            .map_err(error)?;
        self.launches = self.launches.saturating_add(1);
        self.exponentials = self.exponentials.saturating_add(count as u64);
        let group_count = u32::try_from(groups.len() / 3).map_err(error)?;
        let mut launch = self.stream.launch_builder(&self.reduction);
        launch
            .arg(&scratch.paths)
            .arg(&scratch.output)
            .arg(&scratch.groups)
            .arg(&group_count)
            .arg(&wavelength_count)
            .arg(&mut scratch.sums);
        // SAFETY: each group owns a distinct candidate/channel sum range. Group path ranges
        // partition this tile, and all launches share the ordered owner stream.
        unsafe { launch.launch(spectral_launch(group_count * wavelength_count)) }.map_err(error)?;
        self.launches = self.launches.saturating_add(1);
        Ok(())
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct Row {
    candidate: usize,
    channel: usize,
    path: usize,
}

fn validate(inputs: &[ToneInput]) -> Result<(), OpticalError> {
    if inputs.len() > MAX_INPUTS {
        return Err(error("more than 128 tone candidates in one batch"));
    }
    for input in inputs {
        if input.alpha.len() != WAVELENGTHS
            || input.paths.len() > MAX_PATHS
            || !(1..=3).contains(&input.channels)
            || (input.principal && input.channels != 1)
            || !input.scale.is_finite()
            || input.scale < 0.0
            || !input.denominator.is_finite()
            || input.denominator < 0.0
            || input
                .paths
                .iter()
                .flatten()
                .any(|x| !x.is_finite() || *x < 0.0)
            || input
                .alpha
                .iter()
                .flatten()
                .any(|x| !x.is_finite() || *x < 0.0)
        {
            return Err(error("invalid or oversized tone input"));
        }
    }
    Ok(())
}

fn finish_sums(input: &ToneInput, sums: &[[f64; WAVELENGTHS]; 3]) -> ToneSpectrum {
    std::array::from_fn(|wave| {
        if input.principal {
            if input.denominator > 0.0 {
                sums[0][wave] / input.denominator
            } else {
                0.0
            }
        } else {
            match input.channels {
                1 => sums[0][wave],
                2 => 2.0f64.mul_add(sums[0][wave], sums[1][wave]) / 3.0,
                _ => (sums[0][wave] + sums[1][wave] + sums[2][wave]) / 3.0,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(principal: bool, channels: usize, count: usize, seed: usize) -> ToneInput {
        let scale = 1.37;
        let weights: Vec<_> = (0..count)
            .map(|i| ((i + seed) % 31 + 1) as f32 / 997.0)
            .collect();
        let f32_sum = f64::from(weights.iter().sum::<f32>());
        let denominator = weights
            .iter()
            .fold(0.0, |sum, &weight| sum + f64::from(weight));
        let paths = weights
            .into_iter()
            .enumerate()
            .map(|(i, weight)| {
                let d0 = f64::from(0.01 + (i % 13) as f32 * 0.07);
                let d1 = f64::from(0.03 + (i % 17) as f32 * 0.05);
                let d2 = f64::from(0.02 + (i % 19) as f32 * 0.03);
                if principal {
                    [f64::from(weight), d0, d1, d2]
                } else {
                    [
                        f64::from(weight) / f32_sum,
                        (d0 + d1 + d2) * scale,
                        0.0,
                        0.0,
                    ]
                }
            })
            .collect();
        let alpha = (0..WAVELENGTHS)
            .map(|wave| {
                let t = wave as f64 / (WAVELENGTHS - 1) as f64;
                [0.3 + 0.5 * t, 0.9 - 0.2 * t, 0.1 + 1.7 * t]
            })
            .collect();
        ToneInput {
            paths,
            alpha,
            principal,
            channels,
            scale,
            denominator: if principal { denominator } else { 1.0 },
        }
    }

    fn reference(input: &ToneInput) -> ToneSpectrum {
        let sums = std::array::from_fn(|channel| {
            std::array::from_fn(|wave| {
                if channel >= input.channels {
                    return 0.0;
                }
                let alpha = input.alpha[wave];
                input.paths.iter().fold(0.0, |sum, path| {
                    let depth = if input.principal {
                        input.scale
                            * alpha[2]
                                .mul_add(path[3], alpha[1].mul_add(path[2], alpha[0] * path[1]))
                    } else {
                        let slot = if input.channels == 2 && channel == 1 {
                            2
                        } else {
                            channel
                        };
                        alpha[slot] * path[1]
                    };
                    path[0].mul_add((-depth).exp(), sum)
                })
            })
        });
        finish_sums(input, &sums)
    }

    fn assert_reference(gpu: &[ToneSpectrum], inputs: &[ToneInput]) {
        assert_eq!(gpu.len(), inputs.len());
        for (candidate, (actual, input)) in gpu.iter().zip(inputs).enumerate() {
            let expected = reference(input);
            for (wave, (&actual, expected)) in actual.iter().zip(expected).enumerate() {
                // Both paths preserve serial IEEE f64 FMA order. Libdevice exp and the
                // platform exp may differ by a few ULP, amplified by up to 4096 sums;
                // this continuous tolerance remains far below f32 tone/geometry precision.
                let tolerance = 1e-12 + 1e-12 * expected.abs();
                assert!(
                    actual.is_finite() && (actual - expected).abs() <= tolerance,
                    "candidate {candidate}, wavelength {}: CUDA {actual}, CPU {expected}",
                    wave + 380
                );
            }
        }
    }

    #[test]
    #[ignore = "requires a native CUDA device and NVRTC; explicitly exercises spectral kernels"]
    fn native_spectra_preserve_mixed_path_order_across_tiles_and_scratch_reuse() {
        let context = CudaContext::new(0).expect("CUDA device 0");
        let stream = context.new_stream().expect("CUDA tone stream");
        let mut cuda = ToneCuda::new(context, stream).expect("compile native tone kernels");
        let large = vec![
            input(true, 1, 4096, 3),
            input(false, 3, 128, 5),
            input(false, 2, 127, 7),
            input(true, 1, 4096, 11),
            input(false, 1, 96, 13),
        ];
        let rows: usize = large
            .iter()
            .map(|input| input.paths.len() * input.channels)
            .sum();
        assert!(rows > TILE_ROWS);
        assert_reference(&cuda.spectra(&large).unwrap(), &large);
        assert_eq!(cuda.launches(), 2 * rows.div_ceil(TILE_ROWS) as u64);
        assert_eq!(cuda.exponentials(), (rows * WAVELENGTHS) as u64);
        let capacity = cuda.scratch.as_ref().unwrap().capacity;
        let small = vec![input(false, 2, 3, 17), input(true, 1, 5, 19)];
        assert_reference(&cuda.spectra(&small).unwrap(), &small);
        assert_eq!(cuda.scratch.as_ref().unwrap().capacity, capacity);
        let launches = cuda.launches();
        assert!(cuda.spectra(&[]).unwrap().is_empty());
        assert_eq!(cuda.launches(), launches);
        let empty_paths = vec![input(true, 1, 0, 0), input(false, 3, 0, 0)];
        assert_reference(&cuda.spectra(&empty_paths).unwrap(), &empty_paths);
        assert_eq!(cuda.launches(), launches);
        assert_reference(&cuda.spectra(&large).unwrap(), &large);
        assert_eq!(cuda.scratch.as_ref().unwrap().capacity, capacity);
    }

    fn resident_paths(inputs: &[ToneInput]) -> Vec<f64> {
        let mut packed = vec![0.0; inputs.len() * MAX_PATHS * 5];
        for (candidate, input) in inputs.iter().enumerate() {
            for (path, row) in input.paths.iter().enumerate() {
                let offset = (candidate * MAX_PATHS + path) * 5;
                packed[offset..offset + 5].copy_from_slice(&[
                    row[0],
                    row[1],
                    row[2],
                    row[3],
                    input.scale,
                ]);
            }
        }
        packed
    }

    #[test]
    #[ignore = "requires native CUDA/NVRTC; resident multi-tile/reuse parity definition, not executed during implementation"]
    fn resident_spectra_preserve_mixed_order_across_tiles_and_smaller_reuse() {
        let context = CudaContext::new(0).expect("native CUDA");
        let stream = context.default_stream();
        let mut cuda =
            ToneCuda::new(Arc::clone(&context), Arc::clone(&stream)).expect("resident kernels");
        let cases = [
            vec![
                input(true, 1, 4096, 3),
                input(false, 3, 128, 5),
                input(true, 1, 4096, 7),
            ],
            vec![input(false, 2, 3, 11), input(true, 1, 5, 13)],
            Vec::new(),
        ];
        assert!(
            cases[0]
                .iter()
                .map(|i| i.paths.len() * i.channels)
                .sum::<usize>()
                > TILE_ROWS
        );
        for inputs in cases.iter().chain(std::iter::once(&cases[0])) {
            let mut packed = resident_paths(inputs);
            if packed.is_empty() {
                packed.push(0.0);
            }
            let device = stream.clone_htod(&packed).expect("resident fixture paths");
            let counts: Vec<_> = inputs.iter().map(|i| i.paths.len() as u32).collect();
            let actual = cuda
                .spectra_resident(inputs, &device, &counts)
                .expect("resident spectra");
            assert_reference(&actual, inputs);
        }
    }
}
