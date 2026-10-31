//! Upload compact material bands; evaluate all wavelengths on the native CUDA device.
use super::{ToneCuda, WAVELENGTHS, error, spectral_launch};
use crate::gpu_optics::{OpticalError, buffers};
use cudarc::driver::PushKernelArg;
use pleochroma::optics::{absorption::BandShape, materials::GemMaterial};

impl ToneCuda {
    pub(super) fn material_alphas(
        &mut self,
        materials: &[&GemMaterial],
    ) -> Result<(), OpticalError> {
        let (mut bands, mut offsets) = (Vec::<f32>::new(), Vec::<u32>::new());
        for material in materials {
            let tensor = &material.absorption;
            for axis in [
                &tensor.o_ray,
                tensor.beta_ray.as_ref().unwrap_or(&tensor.o_ray),
                &tensor.e_ray,
            ] {
                offsets.extend([(bands.len() / 4) as u32, axis.len() as u32]);
                for b in axis {
                    bands.extend([
                        b.center_nm,
                        b.width_nm,
                        b.peak,
                        if b.shape == BandShape::GaussianEnergy {
                            1.0
                        } else {
                            0.0
                        },
                    ]);
                }
            }
        }
        if bands.is_empty() {
            bands.push(0.0);
        }
        buffers::upload(&self.stream, &mut self.bands, &bands)?;
        buffers::upload(&self.stream, &mut self.band_offsets, &offsets)?;
        let count = materials.len() as u32;
        let waves = WAVELENGTHS as u32;
        let scratch = self
            .scratch
            .as_mut()
            .ok_or_else(|| error("spectral alpha scratch missing"))?;
        let mut args = self.stream.launch_builder(&self.band_kernel);
        args.arg(&self.bands)
            .arg(&self.band_offsets)
            .arg(&count)
            .arg(&waves)
            .arg(&mut scratch.alpha);
        // SAFETY: one thread owns one candidate/wavelength; offsets bound each band list.
        unsafe { args.launch(spectral_launch(count * waves)) }.map_err(error)?;
        self.launches = self.launches.saturating_add(1);
        Ok(())
    }
}
