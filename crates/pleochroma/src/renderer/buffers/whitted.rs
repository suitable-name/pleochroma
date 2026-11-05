//! Four-storage-buffer deterministic tracer uniform. Every field occupies vec4
//! slots, including arrays, so uniform alignment is explicit and testable.

// Available to the browser page without linking the GPU implementation.
include!(concat!(env!("OUT_DIR"), "/whitted_kernel_hash.rs"));

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuWhittedParams {
    pub counts: [u32; 4],
    pub range: [u32; 4],
    pub modes: [u32; 4],
    pub values: [f32; 4],
    pub indices: [f32; 4],
    pub axis: [f32; 4],
    pub masks: [[f32; 4]; 3],
    pub rgb_to_xyz: [[f32; 4]; 3],
    pub white_balance: [f32; 4],
    pub env_rgb: [f32; 4],
    pub key_dir: [f32; 4],
    pub fill_dir: [f32; 4],
    pub studio: [f32; 4],
    pub shadow: [f32; 4],
    pub tent: [f32; 4],
    pub extra: [f32; 4],
    pub aa: [[f32; 4]; 16],
    pub ring_dirs: [[f32; 4]; 16],
    /// [`BinnedAbsorption`](crate::renderer::whitted::BinnedAbsorption) `p` and `q`, mode-major:
    /// mode `m`, bin `b` is element `(m * 40 + b) % 4` of vec4 `(m * 40 + b) / 4`.
    pub depth_p: [[f32; 4]; 30],
    pub depth_q: [[f32; 4]; 30],
    /// Per-bin linear-sRGB weights of the illuminant white.
    pub bin_weights: [[f32; 4]; 40],
}

const _: () = {
    assert!(size_of::<GpuWhittedParams>() == 2432);
    assert!(core::mem::offset_of!(GpuWhittedParams, aa) == 320);
    assert!(core::mem::offset_of!(GpuWhittedParams, ring_dirs) == 576);
    assert!(core::mem::offset_of!(GpuWhittedParams, depth_p) == 832);
    assert!(crate::renderer::whitted::ABSORPTION_BINS == 40);
    assert!(crate::renderer::whitted::ABSORPTION_MODES == 3);
};

#[cfg(feature = "gpu")]
impl GpuWhittedParams {
    pub(crate) fn new(
        scene: &crate::renderer::frame_scene::FrameScene<'_>,
        tools: usize,
        prep: &crate::renderer::whitted::WhittedPrep<'_>,
    ) -> Self {
        use crate::renderer::whitted::{ABSORPTION_BINS, aa_offset, env::WhittedGpuEnv};
        use bytemuck::Zeroable;
        let pack = |v: glam::Vec3| [v.x, v.y, v.z, 0.0];
        let mut out = Self::zeroed();
        out.counts = [
            u32::try_from(scene.planes.len())
                .expect("plane count checked against u32::MAX in prepare"),
            u32::try_from(tools).expect("tool count checked against the GPU tool cap in prepare"),
            u32::from(
                crate::renderer::engine::RayOptions {
                    aa_taps: prep.taps,
                    fire: false,
                }
                .sanitized()
                .aa_taps,
            ),
            u32::from(prep.chain_count.min(3)),
        ];
        out.modes[2] = prep.max_depth;
        out.values[0] = prep.glare;
        out.values[1] = crate::renderer::whitted::CUTOFF;
        for (index, chain) in prep.chains.iter().enumerate() {
            out.indices[index] = chain.n;
            out.masks[index] = pack(chain.mask);
        }
        let absorption = &prep.absorption;
        out.modes[0] = u32::from(absorption.modes);
        out.axis = pack(absorption.axis);
        for mode in 0..usize::from(absorption.modes) {
            for bin in 0..ABSORPTION_BINS {
                let flat = mode * ABSORPTION_BINS + bin;
                out.depth_p[flat / 4][flat % 4] = absorption.p[mode][bin];
                out.depth_q[flat / 4][flat % 4] = absorption.q[mode][bin];
            }
        }
        out.bin_weights = absorption.weights.map(pack);
        out.rgb_to_xyz = [
            pack(prep.rgb_to_xyz.x_axis),
            pack(prep.rgb_to_xyz.y_axis),
            pack(prep.rgb_to_xyz.z_axis),
        ];
        if let Some(scale) = prep.white_balance {
            out.modes[3] = 1;
            out.white_balance = pack(scale);
        }
        out.aa = std::array::from_fn(|index| {
            let (x, y) = aa_offset(16, u32::try_from(index).expect("index below 16"));
            [x, y, 0.0, 0.0]
        });
        match prep.env.gpu_data() {
            WhittedGpuEnv::Studio {
                preset,
                exposure,
                rig,
                shadow,
                rgb,
                backdrop,
            } => {
                let params = preset.params();
                out.values[2] = backdrop;
                out.values[3] = exposure;
                out.env_rgb = pack(rgb);
                out.key_dir = pack(rig.key_dir);
                out.fill_dir = pack(rig.fill_dir);
                out.studio = [
                    params.spot_mult,
                    0.0,
                    rig.sin_light_pitch,
                    preset.model().gpu_id() as f32,
                ];
                out.shadow = [shadow[0], shadow[1], 0.0, 0.0];
                out.tent = [
                    params.tent.walls,
                    params.tent.cards,
                    params.tent.spark,
                    params.tent.ground,
                ];
                out.extra[0] = params.tent.flat;
                out.ring_dirs = rig.ring_dirs.map(pack);
            }
            WhittedGpuEnv::Aset { rgb, backdrop, .. } => {
                out.modes[1] = 1;
                out.env_rgb = pack(rgb);
                out.values[2] = backdrop;
            }
            WhittedGpuEnv::Hdr(_) => {
                out.modes[1] = 2;
            }
            #[cfg(test)]
            WhittedGpuEnv::Constant(rgb) => {
                out.modes[1] = 3;
                out.env_rgb = pack(rgb);
            }
        }
        out
    }
}
