//! GPU-resident adaptive runs: the frame's sums stay on the GPU between passes.
//!
//! An ordinary adaptive pass ([`super::live`]) reads every live pixel's summed XYZ and
//! half-buffer B back to the host after each pass, so the host can measure the tiles. A resident
//! run keeps the two accumulators in GPU storage buffers for the whole run instead:
//!
//! - **passes** trace through the very same live megakernel, but the reduce kernel
//!   (`reduce_xyz_resident.wgsl`) adds each pixel's pass totals straight into the resident
//!   buffers at the pixel's real index, so a pass reads back nothing;
//! - **the check** runs `adaptive_tile_error.wgsl`, one thread per live tile, which reads the
//!   resident buffers and writes one noise value per live tile: the host reads back 4 bytes per
//!   live tile, then runs the unchanged decision (`AdaptiveRun::finish_pass_with_tile_errors`);
//! - **the end**: [`GpuFrameRenderer::read_resident`] reads the two full images back once.
//!
//! The renderer holds at most one resident frame, tagged with its owner's id: a second owner
//! declines instead of waiting, and a renderer replaced after a device loss holds none, so the
//! owner sees the loss as "the frame is gone" and falls back to the host path.
//!
//! The buffers are three `f32` per pixel each (12 bytes), so a frame is resident only if it fits
//! one storage binding of the device (about 11 million pixels at the 128 MiB baseline limit);
//! larger frames decline and take the host path.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::BufferUsages;

use crate::{
    color::{
        ColorSpace, HighlightRule,
        gamut::{KEEP_COLOR_KNEE, KEEP_COLOR_WHITE},
        highlight_rule,
    },
    renderer::{
        adaptive::{LiveTiles, NoiseMetric, metric::L_FLOOR},
        gpu::compute,
    },
};

use super::{GpuFrameError, renderer::GpuFrameRenderer};

/// `reduce_xyz_resident.wgsl`: the reduce that adds a pass into the resident buffers.
pub const RESIDENT_REDUCE_SHADER_SRC: &str = include_str!("../../shaders/reduce_xyz_resident.wgsl");

/// The entry point of [`RESIDENT_REDUCE_SHADER_SRC`].
pub const RESIDENT_REDUCE_ENTRY: &str = "reduce_xyz_resident_main";

/// `adaptive_tile_error.wgsl`: the per-tile noise reduction.
pub const TILE_ERROR_SHADER_SRC: &str = include_str!("../../shaders/adaptive_tile_error.wgsl");

/// The entry point of [`TILE_ERROR_SHADER_SRC`].
pub const TILE_ERROR_ENTRY: &str = "adaptive_tile_error_main";

/// Threads per workgroup of the tile-error kernel (its `@workgroup_size`).
const TILE_ERROR_WORKGROUP: usize = 64;

/// Bytes of the resident accumulator per pixel: three `f32`.
const RESIDENT_BYTES_PER_PIXEL: u64 = 12;

/// `reduce_xyz_resident.wgsl`'s uniform parameter struct (binding 0 there).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GpuReduceResidentParams {
    /// Live pixels in this chunk.
    pub(crate) num_pixels: u32,
    /// Samples per pixel in this chunk.
    pub(crate) num_samples: u32,
    /// Absolute index of the pass's first sample.
    pub(crate) sample_offset: u32,
    /// Position of the chunk's first pixel in the pass's live pixel list.
    pub(crate) first_virtual: u32,
}

/// `adaptive_tile_error.wgsl`'s uniform parameter struct (binding 0 there), 128 bytes.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuTileErrorParams {
    /// Frame width in pixels.
    pub(crate) width: u32,
    /// Frame height in pixels.
    pub(crate) height: u32,
    /// Tile edge in pixels.
    pub(crate) tile: u32,
    /// Tiles per row.
    pub(crate) tiles_x: u32,
    /// Live tiles to measure.
    pub(crate) num_tiles: u32,
    /// Samples per live pixel.
    pub(crate) n_total: u32,
    /// How many of them had an odd absolute index.
    pub(crate) n_odd: u32,
    /// `0` relative `L*`, `1` absolute Delta E.
    pub(crate) metric: u32,
    /// `1` for `HighlightRule::KeepColor`, `0` for `Whiten`.
    pub(crate) keep_color: u32,
    /// `[has_offset, offset_base, 0]`: whether a beam part is bound (`1`), and the first word of
    /// its floats; `[0, 0, 0]` is the shipped block. Pads the integer block to 48 bytes.
    pub(crate) pad: [u32; 3],
    /// Display multiplier applied to the means before the tone-map.
    pub(crate) scale: f32,
    /// The relative metric's lightness floor.
    pub(crate) l_floor: f32,
    /// `KEEP_COLOR_KNEE`.
    pub(crate) knee: f32,
    /// `KEEP_COLOR_WHITE`.
    pub(crate) white_level: f32,
    /// First row of the sRGB XYZ-to-linear matrix (the last lane is unused).
    pub(crate) rgb_r: [f32; 4],
    /// Second row.
    pub(crate) rgb_g: [f32; 4],
    /// Third row.
    pub(crate) rgb_b: [f32; 4],
    /// The sRGB white point's `x` and `y` (the last two lanes are unused).
    pub(crate) white_xy: [f32; 4],
}

/// What one GPU convergence check measures.
#[derive(Clone, Copy, Debug)]
pub struct ResidentCheck<'a> {
    /// The live tiles to measure.
    pub live: &'a LiveTiles,
    /// Samples per pixel every live pixel holds.
    pub samples: u32,
    /// How many of them had an odd absolute index.
    pub odd_samples: u32,
    /// Display multiplier applied to the means before the tone-map.
    pub scale: f32,
    /// Which noise measure to report.
    pub metric: NoiseMetric,
}

/// The uniform block of the tile-error kernel for `check` without a beam part (the tests' view
/// of [`tile_error_params_with_offset`]).
#[cfg(test)]
#[must_use]
pub fn tile_error_params(check: &ResidentCheck<'_>) -> GpuTileErrorParams {
    tile_error_params_with_offset(check, None)
}

/// The uniform block of the tile-error kernel for `check`; with a beam part, `header_words` is
/// the length of the per-slot start list in front of the offset floats (`None`: no beam part,
/// the shipped block).
///
/// Every constant the CPU metric takes from a shared name is read from that name here (the
/// matrix and white point of `ColorSpace::Srgb`, the keep-color knee and white level, the
/// lightness floor, the process's highlight rule), so the kernel cannot drift from the CPU.
#[must_use]
pub fn tile_error_params_with_offset(
    check: &ResidentCheck<'_>,
    header_words: Option<u32>,
) -> GpuTileErrorParams {
    let space = ColorSpace::Srgb;
    let matrix = space.xyz_to_rgb_matrix();
    let (white_x, white_y) = space.white_point_xy();
    let row = |r: usize| [matrix[r][0], matrix[r][1], matrix[r][2], 0.0];
    let live = check.live;
    GpuTileErrorParams {
        width: live.width(),
        height: live.height(),
        tile: live.tile(),
        tiles_x: live.tiles_x(),
        num_tiles: u32::try_from(live.indices().len()).unwrap_or(u32::MAX),
        n_total: check.samples,
        n_odd: check.odd_samples,
        metric: match check.metric {
            NoiseMetric::RelativeLstar => 0,
            NoiseMetric::AbsoluteDeltaE => 1,
        },
        keep_color: u32::from(highlight_rule() == HighlightRule::KeepColor),
        pad: [
            u32::from(header_words.is_some()),
            header_words.unwrap_or(0),
            0,
        ],
        scale: check.scale,
        l_floor: L_FLOOR,
        knee: KEEP_COLOR_KNEE,
        white_level: KEEP_COLOR_WHITE,
        rgb_r: row(0),
        rgb_g: row(1),
        rgb_b: row(2),
        white_xy: [white_x, white_y, 0.0, 0.0],
    }
}

/// The resident accumulators of one adaptive run.
pub(super) struct ResidentFrame {
    /// Identity of the run that owns the frame.
    pub(super) owner: u64,
    /// Frame width in pixels.
    pub(super) width: u32,
    /// Frame height in pixels.
    pub(super) height: u32,
    /// Summed XYZ, three `f32` per pixel.
    pub(super) sum: wgpu::Buffer,
    /// Half-buffer B, three `f32` per pixel.
    pub(super) half_b: wgpu::Buffer,
}

/// The beam part of a batch's residual, uploaded for the check.
pub(super) struct ResidentOffset {
    /// Identity of the run that uploaded it.
    pub(super) owner: u64,
    /// The words: `header_words` tile starts, then three floats per pixel.
    pub(super) buffer: wgpu::Buffer,
    /// Length of the per-slot start list in front of the floats.
    pub(super) header_words: usize,
}

/// The tile list and the noise output of the check, grown and never shrunk.
struct ErrorBuffers {
    capacity: usize,
    tiles: wgpu::Buffer,
    noise: wgpu::Buffer,
}

/// What a [`GpuFrameRenderer`] holds for resident runs. All empty until the first one.
#[derive(Default)]
pub(super) struct ResidentState {
    /// The one resident frame, if a run holds it.
    pub(super) frame: Option<ResidentFrame>,
    /// `reduce_xyz_resident_main`.
    pub(super) reduce: Option<wgpu::ComputePipeline>,
    /// The persistent [`GpuReduceResidentParams`] uniform, rewritten before each chunk's reduce.
    pub(super) reduce_params: Option<wgpu::Buffer>,
    /// `adaptive_tile_error_main`.
    error: Option<wgpu::ComputePipeline>,
    /// The persistent [`GpuTileErrorParams`] uniform.
    error_params: Option<wgpu::Buffer>,
    /// The check's tile list and output.
    error_buffers: Option<ErrorBuffers>,
    /// The beam part of the running batch, if its run uploaded one.
    pub(super) offset: Option<ResidentOffset>,
    /// A small buffer bound at the kernel's offset binding when no beam part is used.
    pub(super) offset_dummy: Option<wgpu::Buffer>,
}

impl GpuFrameRenderer {
    /// Reserves the resident buffers of a `width` x `height` frame for the run `owner`.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::LivePassUnavailable`] when another run holds the resident frame, or the
    /// frame does not fit one storage binding of this device (the caller then takes the host
    /// path); [`GpuFrameError::DeviceLost`] on a poisoned renderer.
    pub(crate) fn begin_resident(
        &mut self,
        owner: u64,
        width: u32,
        height: u32,
    ) -> Result<(), GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        self.check_device_error()?;
        if self.live.resident.frame.is_some() {
            return Err(GpuFrameError::LivePassUnavailable(
                "another adaptive run holds the GPU-resident frame".to_string(),
            ));
        }
        let bytes = u64::from(width) * u64::from(height) * RESIDENT_BYTES_PER_PIXEL;
        let limits = self.ctx.device.limits();
        if bytes == 0
            || bytes > limits.max_storage_buffer_binding_size
            || bytes > limits.max_buffer_size
        {
            return Err(GpuFrameError::LivePassUnavailable(format!(
                "a {width} x {height} frame ({bytes} bytes per accumulator) does not fit one \
                 storage binding of this device"
            )));
        }
        self.ensure_resident_pipelines();
        let make = |label: &str| {
            self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let frame = ResidentFrame {
            owner,
            width,
            height,
            sum: make("adaptive resident sum"),
            half_b: make("adaptive resident half-buffer B"),
        };
        self.live.resident.frame = Some(frame);
        Ok(())
    }

    /// Whether `owner` holds a resident frame of this size.
    #[must_use]
    pub(crate) fn holds_resident(&self, owner: u64, width: u32, height: u32) -> bool {
        self.live
            .resident
            .frame
            .as_ref()
            .is_some_and(|f| f.owner == owner && f.width == width && f.height == height)
    }

    /// Frees the resident frame if `owner` holds it.
    pub(crate) fn release_resident(&mut self, owner: u64) {
        if self
            .live
            .resident
            .frame
            .as_ref()
            .is_some_and(|f| f.owner == owner)
        {
            self.live.resident.frame = None;
        }
        if self
            .live
            .resident
            .offset
            .as_ref()
            .is_some_and(|o| o.owner == owner)
        {
            self.live.resident.offset = None;
        }
    }

    /// Builds the resident reduce and tile-error pipelines and their uniforms, if not built yet.
    fn ensure_resident_pipelines(&mut self) {
        let info = self.ctx.adapter.get_info();
        let state = &mut self.live.resident;
        if state.reduce.is_none() {
            state.reduce = Some(compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &info,
                RESIDENT_REDUCE_ENTRY,
                RESIDENT_REDUCE_SHADER_SRC,
                RESIDENT_REDUCE_ENTRY,
                &[],
            ));
        }
        if state.error.is_none() {
            state.error = Some(compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &info,
                TILE_ERROR_ENTRY,
                TILE_ERROR_SHADER_SRC,
                TILE_ERROR_ENTRY,
                &[],
            ));
        }
        let uniform = |label: &str, size: usize| {
            self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as wgpu::BufferAddress,
                usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        if state.reduce_params.is_none() {
            state.reduce_params = Some(uniform(
                "adaptive resident reduce params",
                size_of::<GpuReduceResidentParams>(),
            ));
        }
        if state.offset_dummy.is_none() {
            state.offset_dummy = Some(self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("adaptive tile error offset placeholder"),
                size: 16,
                usage: BufferUsages::STORAGE,
                mapped_at_creation: false,
            }));
        }
        if state.error_params.is_none() {
            state.error_params = Some(uniform(
                "adaptive tile error params",
                size_of::<GpuTileErrorParams>(),
            ));
        }
    }

    /// Measures the live tiles of `check` on the GPU and reads back one noise value per tile.
    ///
    /// The values are in the order of `check.live.indices()` and in the unit of
    /// `check.metric`; 4 bytes per live tile cross to the host. A tile whose halves cannot be
    /// compared reads back as a huge finite value (it never converges).
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::LivePassUnavailable`] when `owner` does not hold a resident frame of the
    /// tiles' size (a lost device, a wrong run); [`GpuFrameError::DeviceLost`] when the readback
    /// fails or the renderer is poisoned.
    pub(crate) fn resident_tile_noise(
        &mut self,
        owner: u64,
        check: &ResidentCheck<'_>,
    ) -> Result<Vec<f32>, GpuFrameError> {
        self.measure_resident_tiles(owner, check, None)
    }

    /// [`Self::resident_tile_noise`] with the beam part uploaded by
    /// [`Self::upload_resident_offset`] added inside the metric (`offset + mean_a` against
    /// `offset + mean_b`, beam guide section 12.4).
    ///
    /// `starts` has one entry per tile of `check.live`, in the same order: the position, in
    /// pixels, of that tile's first pixel in the uploaded offset, or `u32::MAX` (`NO_OFFSET` in
    /// the kernel) for a tile the offset does not cover; it reads back as a huge finite value
    /// and never converges.
    ///
    /// # Errors
    ///
    /// As [`Self::resident_tile_noise`], plus [`GpuFrameError::LivePassUnavailable`] when `owner`
    /// uploaded no offset or `starts` is not one entry per live tile.
    pub(crate) fn resident_tile_noise_with_offset(
        &mut self,
        owner: u64,
        check: &ResidentCheck<'_>,
        starts: &[u32],
    ) -> Result<Vec<f32>, GpuFrameError> {
        self.measure_resident_tiles(owner, check, Some(starts))
    }

    /// Uploads the beam part of a batch, once, for the run `owner`: `offset` is one XYZ per pixel
    /// in the batch layout (the batch's tiles in order, each row-major). Room is reserved for the
    /// per-tile start list of up to `max_tiles` live tiles, which every check rewrites.
    ///
    /// A second upload replaces the first. The offset is freed with the resident frame.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::LivePassUnavailable`] when `owner` holds no resident frame or the offset
    /// does not fit one storage binding of this device.
    pub(crate) fn upload_resident_offset(
        &mut self,
        owner: u64,
        offset: &[Vec3],
        max_tiles: usize,
    ) -> Result<(), GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        if self
            .live
            .resident
            .frame
            .as_ref()
            .is_none_or(|f| f.owner != owner)
        {
            return Err(GpuFrameError::LivePassUnavailable(
                "the GPU-resident frame is gone".to_string(),
            ));
        }
        let words = max_tiles + offset.len() * 3;
        let bytes = (words.max(4) * size_of::<u32>()) as u64;
        let limits = self.ctx.device.limits();
        if bytes > limits.max_storage_buffer_binding_size || bytes > limits.max_buffer_size {
            return Err(GpuFrameError::LivePassUnavailable(format!(
                "a beam part of {} pixels does not fit one storage binding of this device",
                offset.len()
            )));
        }
        let buffer = self.ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("adaptive tile error beam offset"),
            size: bytes,
            usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let data: Vec<u32> = offset
            .iter()
            .flat_map(|v| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()])
            .collect();
        if !data.is_empty() {
            self.ctx.queue.write_buffer(
                &buffer,
                (max_tiles * size_of::<u32>()) as u64,
                bytemuck::cast_slice(&data),
            );
        }
        self.live.resident.offset = Some(ResidentOffset {
            owner,
            buffer,
            header_words: max_tiles,
        });
        Ok(())
    }

    /// The body of [`Self::resident_tile_noise`] and [`Self::resident_tile_noise_with_offset`]:
    /// `starts` is `None` for the shipped metric.
    fn measure_resident_tiles(
        &mut self,
        owner: u64,
        check: &ResidentCheck<'_>,
        starts: Option<&[u32]>,
    ) -> Result<Vec<f32>, GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        self.check_device_error()?;
        let tiles = check.live.indices();
        if tiles.is_empty() {
            return Ok(Vec::new());
        }
        if !self.holds_resident(owner, check.live.width(), check.live.height()) {
            return Err(GpuFrameError::LivePassUnavailable(
                "the GPU-resident frame is gone".to_string(),
            ));
        }
        self.ensure_error_buffers(tiles.len());

        let state = &self.live.resident;
        let offset = state.offset.as_ref().filter(|o| o.owner == owner);
        // The beam part, when asked for: the per-slot start list goes into the header of the
        // uploaded offset, and the kernel reads the floats behind it.
        let header_words = match starts {
            None => None,
            Some(starts) => {
                let fits = offset.is_some_and(|o| starts.len() <= o.header_words);
                if !fits || starts.len() != tiles.len() {
                    return Err(GpuFrameError::LivePassUnavailable(
                        "the beam part was not uploaded for these tiles".to_string(),
                    ));
                }
                offset.map(|o| u32::try_from(o.header_words).unwrap_or(u32::MAX))
            }
        };
        let params = tile_error_params_with_offset(check, header_words);
        let (Some(frame), Some(pipeline), Some(params_buf), Some(buffers), Some(dummy)) = (
            state.frame.as_ref(),
            state.error.as_ref(),
            state.error_params.as_ref(),
            state.error_buffers.as_ref(),
            state.offset_dummy.as_ref(),
        ) else {
            return Err(GpuFrameError::LivePassUnavailable(
                "the tile-error pipeline was not prepared".to_string(),
            ));
        };
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        queue.write_buffer(params_buf, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&buffers.tiles, 0, bytemuck::cast_slice(tiles));
        let offset_buffer = match (starts, offset) {
            (Some(starts), Some(uploaded)) => {
                queue.write_buffer(&uploaded.buffer, 0, bytemuck::cast_slice(starts));
                &uploaded.buffer
            }
            _ => dummy,
        };
        let bind_group = compute::bind_buffers(
            device,
            "adaptive tile error bind group",
            pipeline,
            &[
                (0, params_buf),
                (1, &buffers.tiles),
                (2, &frame.sum),
                (3, &frame.half_b),
                (4, &buffers.noise),
                (5, offset_buffer),
            ],
        );
        let workgroups = u32::try_from(tiles.len().div_ceil(TILE_ERROR_WORKGROUP))
            .map_err(|_| GpuFrameError::LivePassUnavailable("too many live tiles".to_string()))?;
        let _ = compute::dispatch(device, queue, pipeline, &bind_group, (workgroups, 1, 1));
        self.check_device_error()?;

        let (staging, copy_index) = compute::copy_to_staging::<f32>(
            device,
            queue,
            &buffers.noise,
            tiles.len(),
            "adaptive tile error readback staging",
        );
        let rx = compute::begin_map_read(&staging);
        compute::finish_map_read::<f32>(device, &staging, copy_index, &rx)
            .map_err(|e| GpuFrameError::DeviceLost(self.ctx.failure_with_symptom(&e.to_string())))
    }

    /// Grows the tile list and noise output to `tiles` entries.
    fn ensure_error_buffers(&mut self, tiles: usize) {
        let state = &mut self.live.resident;
        if state
            .error_buffers
            .as_ref()
            .is_some_and(|b| b.capacity >= tiles)
        {
            return;
        }
        let bytes = (tiles * size_of::<u32>()) as wgpu::BufferAddress;
        let device = &self.ctx.device;
        state.error_buffers = Some(ErrorBuffers {
            capacity: tiles,
            tiles: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("adaptive tile error tile list"),
                size: bytes,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            noise: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("adaptive tile error output"),
                size: bytes,
                usage: BufferUsages::STORAGE | BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
        });
    }

    /// Reads the summed XYZ and half-buffer B of the run `owner` back, once, at the end.
    ///
    /// # Errors
    ///
    /// [`GpuFrameError::LivePassUnavailable`] when `owner` holds no resident frame;
    /// [`GpuFrameError::DeviceLost`] when the readback fails.
    pub(crate) fn read_resident(
        &self,
        owner: u64,
    ) -> Result<(Vec<Vec3>, Vec<Vec3>), GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        self.check_device_error()?;
        let Some(frame) = self
            .live
            .resident
            .frame
            .as_ref()
            .filter(|f| f.owner == owner)
        else {
            return Err(GpuFrameError::LivePassUnavailable(
                "the GPU-resident frame is gone".to_string(),
            ));
        };
        let floats = frame.width as usize * frame.height as usize * 3;
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let mut images = Vec::with_capacity(2);
        for (buffer, label) in [
            (&frame.sum, "adaptive resident sum readback staging"),
            (
                &frame.half_b,
                "adaptive resident half-buffer B readback staging",
            ),
        ] {
            let (staging, copy_index) =
                compute::copy_to_staging::<f32>(device, queue, buffer, floats, label);
            let rx = compute::begin_map_read(&staging);
            let data = compute::finish_map_read::<f32>(device, &staging, copy_index, &rx).map_err(
                |e| GpuFrameError::DeviceLost(self.ctx.failure_with_symptom(&e.to_string())),
            )?;
            images.push(
                data.as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| Vec3::new(c[0], c[1], c[2]))
                    .collect::<Vec<Vec3>>(),
            );
        }
        let half_b = images.pop().unwrap_or_default();
        let sum = images.pop().unwrap_or_default();
        Ok((sum, half_b))
    }

    /// [`Self::read_resident`] for the pixel rows `rows.0..rows.1` only (full width, row-major):
    /// a beam batch reads back the band its tiles cover instead of the whole frame.
    ///
    /// # Errors
    ///
    /// As [`Self::read_resident`], plus [`GpuFrameError::LivePassUnavailable`] when the rows are
    /// empty or reach past the frame.
    pub(crate) fn read_resident_rows(
        &self,
        owner: u64,
        rows: (u32, u32),
    ) -> Result<(Vec<Vec3>, Vec<Vec3>), GpuFrameError> {
        if self.poisoned {
            return Err(GpuFrameError::DeviceLost(
                "GPU renderer poisoned by a previous failed chunk readback".to_string(),
            ));
        }
        self.check_device_error()?;
        let Some(frame) = self
            .live
            .resident
            .frame
            .as_ref()
            .filter(|f| f.owner == owner)
        else {
            return Err(GpuFrameError::LivePassUnavailable(
                "the GPU-resident frame is gone".to_string(),
            ));
        };
        let (start, end) = rows;
        if start >= end || end > frame.height {
            return Err(GpuFrameError::LivePassUnavailable(
                "the rows to read lie outside the resident frame".to_string(),
            ));
        }
        let row_bytes = u64::from(frame.width) * RESIDENT_BYTES_PER_PIXEL;
        let first_byte = u64::from(rows.0) * row_bytes;
        let byte_len = u64::from(rows.1 - rows.0) * row_bytes;
        let device = &self.ctx.device;
        let queue = &self.ctx.queue;
        let mut images = Vec::with_capacity(2);
        for (buffer, label) in [
            (&frame.sum, "adaptive resident band readback staging (sum)"),
            (
                &frame.half_b,
                "adaptive resident band readback staging (half-buffer B)",
            ),
        ] {
            let staging = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: byte_len,
                usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("adaptive resident band readback encoder"),
            });
            encoder.copy_buffer_to_buffer(buffer, first_byte, &staging, 0, byte_len);
            let copy_index = queue.submit(Some(encoder.finish()));
            let rx = compute::begin_map_read(&staging);
            let data = compute::finish_map_read::<f32>(device, &staging, copy_index, &rx).map_err(
                |e| GpuFrameError::DeviceLost(self.ctx.failure_with_symptom(&e.to_string())),
            )?;
            images.push(
                data.as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| Vec3::new(c[0], c[1], c[2]))
                    .collect::<Vec<Vec3>>(),
            );
        }
        let half_b = images.pop().unwrap_or_default();
        let sum = images.pop().unwrap_or_default();
        Ok((sum, half_b))
    }
}

#[cfg(test)]
mod tests {
    use naga::{
        TypeInner,
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    use super::{
        GpuReduceResidentParams, GpuTileErrorParams, RESIDENT_REDUCE_ENTRY,
        RESIDENT_REDUCE_SHADER_SRC, ResidentCheck, TILE_ERROR_ENTRY, TILE_ERROR_SHADER_SRC,
        tile_error_params, tile_error_params_with_offset,
    };
    use crate::{
        color::{ColorSpace, HighlightRule, gamut::KEEP_COLOR_KNEE, highlight_rule},
        renderer::adaptive::{LiveTiles, NoiseMetric, metric::L_FLOOR},
    };

    fn validated(name: &str, source: &str) -> naga::Module {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name} does not parse: {}", e.emit_to_string(source)));
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name} does not validate: {}", e.emit_to_string(source)));
        module
    }

    /// The byte size naga computes for the struct `name` of `module`.
    fn struct_span(module: &naga::Module, name: &str) -> Option<u32> {
        module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(n), TypeInner::Struct { span, .. }) if n == name => Some(*span),
                _ => None,
            })
    }

    fn has_entry(module: &naga::Module, name: &str) -> bool {
        module.entry_points.iter().any(|e| e.name == name)
    }

    #[test]
    fn both_resident_kernels_validate_and_have_their_entry_points() {
        let reduce = validated("reduce_xyz_resident.wgsl", RESIDENT_REDUCE_SHADER_SRC);
        assert!(has_entry(&reduce, RESIDENT_REDUCE_ENTRY));
        let error = validated("adaptive_tile_error.wgsl", TILE_ERROR_SHADER_SRC);
        assert!(has_entry(&error, TILE_ERROR_ENTRY));
    }

    #[test]
    fn the_uniform_structs_have_the_size_the_shaders_lay_out() {
        let reduce = validated("reduce_xyz_resident.wgsl", RESIDENT_REDUCE_SHADER_SRC);
        assert_eq!(
            struct_span(&reduce, "GpuReduceResidentParams"),
            u32::try_from(size_of::<GpuReduceResidentParams>()).ok()
        );
        let error = validated("adaptive_tile_error.wgsl", TILE_ERROR_SHADER_SRC);
        assert_eq!(
            struct_span(&error, "TileErrorParams"),
            u32::try_from(size_of::<GpuTileErrorParams>()).ok()
        );
        assert_eq!(size_of::<GpuTileErrorParams>(), 128);
    }

    #[test]
    fn the_tile_error_uniform_carries_the_cpu_constants() {
        let live = LiveTiles::from_indices(40, 36, 16, vec![1, 5, 8]);
        let check = ResidentCheck {
            live: &live,
            samples: 48,
            odd_samples: 24,
            scale: 0.5,
            metric: NoiseMetric::AbsoluteDeltaE,
        };
        let p = tile_error_params(&check);
        assert_eq!((p.width, p.height, p.tile, p.tiles_x), (40, 36, 16, 3));
        assert_eq!((p.num_tiles, p.n_total, p.n_odd, p.metric), (3, 48, 24, 1));
        assert_eq!(
            p.keep_color,
            u32::from(highlight_rule() == HighlightRule::KeepColor)
        );
        assert_eq!(p.scale.to_bits(), 0.5_f32.to_bits());
        assert_eq!(p.l_floor.to_bits(), L_FLOOR.to_bits());
        assert_eq!(p.knee.to_bits(), KEEP_COLOR_KNEE.to_bits());
        let m = ColorSpace::Srgb.xyz_to_rgb_matrix();
        assert_eq!(p.rgb_g[..3], m[1][..]);
        let relative = tile_error_params(&ResidentCheck {
            metric: NoiseMetric::RelativeLstar,
            ..check
        });
        assert_eq!(relative.metric, 0);
    }

    #[test]
    fn the_tile_error_kernel_reads_a_beam_part_at_binding_five() {
        let error = validated("adaptive_tile_error.wgsl", TILE_ERROR_SHADER_SRC);
        let bound: Vec<(String, u32)> = error
            .global_variables
            .iter()
            .filter_map(|(_, var)| {
                let binding = var.binding.as_ref()?;
                (binding.group == 0)
                    .then(|| (var.name.clone().unwrap_or_default(), binding.binding))
            })
            .collect();
        assert!(
            bound.contains(&("tile_offset".to_string(), 5)),
            "bindings: {bound:?}"
        );
        // The shipped bindings keep their numbers.
        for (name, binding) in [
            ("params", 0),
            ("tile_list", 1),
            ("resident_sum", 2),
            ("resident_half_b", 3),
            ("tile_noise_out", 4),
        ] {
            assert!(
                bound.contains(&(name.to_string(), binding)),
                "{name} moved: {bound:?}"
            );
        }
    }

    #[test]
    fn without_a_beam_part_the_uniform_block_is_the_shipped_one() {
        let live = LiveTiles::from_indices(40, 36, 16, vec![1, 5, 8]);
        let check = ResidentCheck {
            live: &live,
            samples: 48,
            odd_samples: 24,
            scale: 0.5,
            metric: NoiseMetric::RelativeLstar,
        };
        let shipped = tile_error_params(&check);
        assert_eq!(shipped.pad, [0; 3]);
        assert_eq!(tile_error_params_with_offset(&check, None), shipped);
        let beam = tile_error_params_with_offset(&check, Some(7));
        assert_eq!(beam.pad, [1, 7, 0]);
        assert_eq!(
            GpuTileErrorParams {
                pad: [0; 3],
                ..beam
            },
            shipped,
            "a beam part changes nothing but the pad words"
        );
    }
}
