//! Resident-path spectral dispatch: no compacted paths are copied through the host.
use super::{
    MAX_PATHS, TILE_ROWS, ToneCuda, ToneInput, ToneSpectrum, WAVELENGTHS, error, finish_sums,
    spectral_launch, validate,
};
use crate::gpu_optics::OpticalError;
use cudarc::driver::{CudaSlice, PushKernelArg};
use pleochroma::optics::materials::GemMaterial;

impl ToneCuda {
    #[cfg(test)]
    pub(in crate::gpu_optics) fn spectra_resident(
        &mut self,
        inputs: &[ToneInput],
        paths: &CudaSlice<f64>,
        counts: &[u32],
    ) -> Result<Vec<ToneSpectrum>, OpticalError> {
        self.resident_inner(inputs, paths, counts, None)
    }
    pub(in crate::gpu_optics) fn spectra_materials(
        &mut self,
        inputs: &[ToneInput],
        paths: &CudaSlice<f64>,
        counts: &[u32],
        materials: &[&GemMaterial],
    ) -> Result<Vec<ToneSpectrum>, OpticalError> {
        if materials.len() != inputs.len() {
            return Err(error("resident material count mismatch"));
        }
        self.resident_inner(inputs, paths, counts, Some(materials))
    }
    fn resident_inner(
        &mut self,
        inputs: &[ToneInput],
        paths: &CudaSlice<f64>,
        counts: &[u32],
        materials: Option<&[&GemMaterial]>,
    ) -> Result<Vec<ToneSpectrum>, OpticalError> {
        validate(inputs)?;
        if counts.len() != inputs.len()
            || counts.iter().any(|&n| n as usize > MAX_PATHS)
            || paths.len() < inputs.len() * MAX_PATHS * 5
        {
            return Err(error("invalid resident tone path layout"));
        }
        let mut groups = Vec::<u32>::new();
        let mut rows = 0u32;
        for (candidate, (input, &count)) in inputs.iter().zip(counts).enumerate() {
            if count == 0 {
                continue;
            }
            for slot in 0..input.channels {
                let channel = if input.channels == 2 && slot == 1 {
                    2
                } else {
                    slot
                };
                groups.extend([
                    rows,
                    rows + count,
                    candidate as u32,
                    slot as u32,
                    channel as u32,
                    u32::from(input.principal),
                ]);
                rows += count;
            }
        }
        let sum_count = inputs.len() * 3 * WAVELENGTHS;
        self.host_output.clear();
        self.host_output.resize(sum_count, 0.0);
        if rows > 0 {
            self.context.bind_to_thread().map_err(error)?;
            self.reserve((rows as usize).min(TILE_ROWS))?;
            let scratch = self
                .scratch
                .as_mut()
                .ok_or_else(|| error("resident spectral scratch missing"))?;
            if materials.is_none() {
                let alpha: Vec<_> = inputs
                    .iter()
                    .flat_map(|i| i.alpha.iter().flatten().copied())
                    .collect();
                self.stream
                    .memcpy_htod(&alpha, &mut scratch.alpha)
                    .map_err(error)?;
            }
            self.stream
                .memcpy_htod(&groups, &mut scratch.groups)
                .map_err(error)?;
            self.stream
                .memcpy_htod(&self.host_output, &mut scratch.sums)
                .map_err(error)?;
            if let Some(materials) = materials {
                self.material_alphas(materials)?;
            }
            for begin in (0..rows).step_by(TILE_ROWS) {
                self.resident_tile(paths, &groups, begin, (rows - begin).min(TILE_ROWS as u32))?;
            }
            let scratch = self
                .scratch
                .as_ref()
                .ok_or_else(|| error("resident spectral scratch missing"))?;
            self.stream
                .memcpy_dtoh(&scratch.sums.slice(..sum_count), &mut self.host_output)
                .map_err(error)?;
        }
        if self.host_output.iter().any(|v| !v.is_finite()) {
            return Err(error("non-finite resident spectral result"));
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
    fn resident_tile(
        &mut self,
        paths: &CudaSlice<f64>,
        groups: &[u32],
        begin: u32,
        rows: u32,
    ) -> Result<(), OpticalError> {
        let scratch = self
            .scratch
            .as_mut()
            .ok_or_else(|| error("resident spectral scratch missing"))?;
        let count = (groups.len() / 6) as u32;
        let waves = WAVELENGTHS as u32;
        let mut args = self.stream.launch_builder(&self.resident_kernel);
        args.arg(paths)
            .arg(&scratch.alpha)
            .arg(&scratch.groups)
            .arg(&count)
            .arg(&begin)
            .arg(&rows)
            .arg(&waves)
            .arg(&mut scratch.output);
        // SAFETY: groups partition the bounded resident path ranges; each tile output is unique.
        unsafe { args.launch(spectral_launch(rows * waves)) }.map_err(error)?;
        self.launches = self.launches.saturating_add(1);
        self.exponentials = self.exponentials.saturating_add(u64::from(rows * waves));
        let mut args = self.stream.launch_builder(&self.resident_reduction);
        args.arg(paths)
            .arg(&scratch.output)
            .arg(&scratch.groups)
            .arg(&count)
            .arg(&begin)
            .arg(&rows)
            .arg(&waves)
            .arg(&mut scratch.sums);
        // SAFETY: a thread owns each ordered candidate/channel/wavelength sum.
        unsafe { args.launch(spectral_launch(count * waves)) }.map_err(error)?;
        self.launches = self.launches.saturating_add(1);
        Ok(())
    }
}
