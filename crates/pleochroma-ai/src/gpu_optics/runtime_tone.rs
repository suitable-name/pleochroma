//! GPU-resident face-up tone continuation shared by Fast and Full scoring.
use super::{CudaState, ToneRequest, runtime_error};
use crate::gpu_optics::{
    OpticalError,
    tone::{MAX_PATH_HALF_WIDTHS, PATH_BINS, ToneAccumulator},
};
use pleochroma::color::metrics::{FaceUpTone, ToneIlluminant};
use pleochroma_cut_core::optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW};
use std::sync::atomic::Ordering;

impl CudaState {
    pub(super) fn finish_tones(
        &mut self,
        requests: &[ToneRequest<'_>],
    ) -> Result<Vec<FaceUpTone>, OpticalError> {
        let mut inputs = Vec::with_capacity(requests.len());
        let mut configs = Vec::with_capacity(requests.len());
        for request in requests {
            let tensor = &request.material.absorption;
            let input = super::super::tone_cuda::ToneInput {
                paths: Vec::new(),
                alpha: vec![[0.0; 3]; 401],
                principal: tensor.is_pleochroic,
                channels: if tensor.is_pleochroic {
                    1
                } else if tensor.beta_ray.is_some() {
                    3
                } else {
                    1
                },
                scale: f64::from(request.material.absorption_path_scale.max(0.0)),
                denominator: 0.0,
            };
            configs.push([
                request.start as f64,
                request.end as f64,
                request.modes as f64,
                f64::from(MAX_PATH_HALF_WIDTHS * request.half_width / PATH_BINS as f32),
                input.scale,
                if input.principal { 1.0 } else { 0.0 },
            ]);
            inputs.push(input);
        }
        let summaries = self
            .postprocess
            .compact_tones(&self.buffers.output, &configs)?;
        if summaries.len() != requests.len() {
            return Err(runtime_error("native tone summary count mismatch"));
        }
        let mut counts = Vec::with_capacity(requests.len());
        for (input, summary) in inputs.iter_mut().zip(&summaries) {
            input.denominator = summary[5];
            counts.push(summary[4] as u32);
        }
        let launches = self.tone_cuda.launches();
        let exponentials = self.tone_cuda.exponentials();
        let materials: Vec<_> = requests.iter().map(|request| request.material).collect();
        let spectra = self.tone_cuda.spectra_materials(
            &inputs,
            self.postprocess.paths(),
            &counts,
            &materials,
        );
        self.counters
            .launches
            .fetch_add(self.tone_cuda.launches() - launches, Ordering::Relaxed);
        self.counters.exponentials.fetch_add(
            self.tone_cuda.exponentials() - exponentials,
            Ordering::Relaxed,
        );
        let spectra = spectra?;
        if spectra.len() != requests.len() {
            return Err(runtime_error("native resident spectrum count mismatch"));
        }
        Ok(requests
            .iter()
            .zip(summaries)
            .zip(spectra)
            .map(|((request, summary), spectrum)| {
                let illuminant = ToneIlluminant::for_environment(request.preset.studio(
                    1.0,
                    CANONICAL_LIGHT_YAW,
                    CANONICAL_LIGHT_PITCH,
                ));
                ToneAccumulator::from_gpu_summary(&summary)
                    .finish_from_spectrum(&spectrum, illuminant)
            })
            .collect())
    }
}
