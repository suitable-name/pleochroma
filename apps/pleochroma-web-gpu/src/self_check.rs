//! The statistical comparison of an anisotropic GPU self-check render with the CPU render of
//! the same scene.
//!
//! The Path-Traced GPU and CPU renderers draw different random samples, so their images agree
//! in expectation, not pixel by pixel. A polarization or kernel bug still gives a plausible
//! looking picture, which is why the Worker compares the two renders of a uniaxial or biaxial
//! self-check scene before its class is admitted: the total light, the share of each color
//! channel and the light of each block of the frame.
//!
//! Plain `[f32; 3]` sums and no browser types, so the whole comparison is tested natively
//! (the Worker itself is wasm-only). The tolerances are the first guess of the lane that wrote
//! this; the owner tunes them against the numbers [`Comparison::describe`] prints.

/// Blocks per side of the frame the comparison looks at (a 4 x 4 grid).
pub const BLOCKS_PER_SIDE: u32 = 4;
/// The most the GPU's total light may differ from the CPU's, relative to the CPU's.
pub const MEAN_TOLERANCE: f64 = 0.15;
/// The most one color channel's total may differ, relative to the larger of the CPU's channel
/// total and [`CHANNEL_FLOOR`] of the CPU's overall total.
pub const CHANNEL_TOLERANCE: f64 = 0.25;
/// Share of the overall CPU total that stands in for a near-black channel's own total.
pub const CHANNEL_FLOOR: f64 = 0.05;
/// The most one block's light may differ, relative to the larger of the CPU's block total and
/// [`BLOCK_FLOOR`] of an average block.
pub const BLOCK_TOLERANCE: f64 = 0.5;
/// Share of an average CPU block that stands in for a near-black block's own total.
pub const BLOCK_FLOOR: f64 = 0.25;
/// Blocks that may miss [`BLOCK_TOLERANCE`] (a block of a 16 x 16 frame holds few samples).
pub const OUTLIER_BLOCKS_ALLOWED: u32 = 2;

/// What [`compare`] measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Comparison {
    /// The GPU's total light over all pixels and channels.
    pub gpu_total: f64,
    /// The CPU's total light.
    pub cpu_total: f64,
    /// `|gpu - cpu| / cpu` of the totals.
    pub total_error: f64,
    /// The largest relative error of one color channel's total.
    pub worst_channel_error: f64,
    /// Blocks whose light is more than [`BLOCK_TOLERANCE`] off.
    pub outlier_blocks: u32,
}

impl Comparison {
    /// Whether the GPU render agrees with the CPU render.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.total_error <= MEAN_TOLERANCE
            && self.worst_channel_error <= CHANNEL_TOLERANCE
            && self.outlier_blocks <= OUTLIER_BLOCKS_ALLOWED
    }

    /// The numbers as one line, for the message that says why a check failed.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "total light {:.4} on the GPU against {:.4} on the CPU ({:.1} % off, at most {:.0} %), \
             worst color channel {:.1} % off (at most {:.0} %), {} of {} blocks off by more than {:.0} % (at most {})",
            self.gpu_total,
            self.cpu_total,
            self.total_error * 100.0,
            MEAN_TOLERANCE * 100.0,
            self.worst_channel_error * 100.0,
            CHANNEL_TOLERANCE * 100.0,
            self.outlier_blocks,
            BLOCKS_PER_SIDE * BLOCKS_PER_SIDE,
            BLOCK_TOLERANCE * 100.0,
            OUTLIER_BLOCKS_ALLOWED,
        )
    }
}

/// The light of one pixel's three channels summed in `f64`.
fn light(pixel: [f32; 3]) -> f64 {
    f64::from(pixel[0]) + f64::from(pixel[1]) + f64::from(pixel[2])
}

/// Why two frames cannot be compared at all, `None` when they can.
fn unusable(gpu: &[[f32; 3]], cpu: &[[f32; 3]], width: u32, height: u32) -> Option<String> {
    let pixels = u64::from(width) * u64::from(height);
    if pixels == 0 {
        return Some("the self-check frame is empty".to_owned());
    }
    let held = |frame: &[[f32; 3]]| u64::try_from(frame.len()).unwrap_or(u64::MAX);
    if held(gpu) != pixels || held(cpu) != pixels {
        return Some(format!(
            "the self-check frames hold {} (GPU) and {} (CPU) pixels, {pixels} expected",
            gpu.len(),
            cpu.len()
        ));
    }
    let bad = |frame: &[[f32; 3]]| {
        frame
            .iter()
            .any(|pixel| pixel.iter().any(|v| !v.is_finite() || *v < 0.0))
    };
    if bad(gpu) {
        return Some("the GPU self-check frame has a non-finite or negative value".to_owned());
    }
    if bad(cpu) {
        return Some("the CPU self-check frame has a non-finite or negative value".to_owned());
    }
    None
}

/// Per-channel totals of `frame`.
fn channel_totals(frame: &[[f32; 3]]) -> [f64; 3] {
    let mut totals = [0.0_f64; 3];
    for pixel in frame {
        for (total, value) in totals.iter_mut().zip(pixel) {
            *total += f64::from(*value);
        }
    }
    totals
}

/// Light of block (`bx`, `by`) of a `width` x `height` frame cut into
/// [`BLOCKS_PER_SIDE`] x [`BLOCKS_PER_SIDE`] blocks (the last blocks take the remainder).
fn block_light(frame: &[[f32; 3]], (width, height): (u32, u32), (bx, by): (u32, u32)) -> f64 {
    let edge = |index: u32, size: u32| {
        (
            u64::from(index) * u64::from(size) / u64::from(BLOCKS_PER_SIDE),
            u64::from(index + 1) * u64::from(size) / u64::from(BLOCKS_PER_SIDE),
        )
    };
    let (x0, x1) = edge(bx, width);
    let (y0, y1) = edge(by, height);
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let index = usize::try_from(y * u64::from(width) + x).unwrap_or(usize::MAX);
            if let Some(pixel) = frame.get(index) {
                sum += light(*pixel);
            }
        }
    }
    sum
}

/// Compares the GPU frame's `gpu` sums with the CPU frame's `cpu` sums (both the sums of the
/// same number of samples per pixel, `width` x `height` pixels, row by row).
///
/// # Errors
///
/// A text when the frames cannot be compared: empty, of different sizes, holding a NaN,
/// infinite or negative value, or a CPU frame without any light.
pub fn compare(
    gpu: &[[f32; 3]],
    cpu: &[[f32; 3]],
    width: u32,
    height: u32,
) -> Result<Comparison, String> {
    if let Some(reason) = unusable(gpu, cpu, width, height) {
        return Err(reason);
    }
    let (gpu_channels, cpu_channels) = (channel_totals(gpu), channel_totals(cpu));
    let gpu_total: f64 = gpu_channels.iter().sum();
    let cpu_total: f64 = cpu_channels.iter().sum();
    if cpu_total <= 0.0 {
        return Err(
            "the CPU self-check frame has no light, so there is nothing to compare".to_owned(),
        );
    }
    let total_error = (gpu_total - cpu_total).abs() / cpu_total;

    let channel_floor = cpu_total * CHANNEL_FLOOR;
    let worst_channel_error = gpu_channels
        .iter()
        .zip(&cpu_channels)
        .map(|(g, c)| (g - c).abs() / c.max(channel_floor))
        .fold(0.0_f64, f64::max);

    let average_block = cpu_total / f64::from(BLOCKS_PER_SIDE * BLOCKS_PER_SIDE);
    let block_floor = average_block * BLOCK_FLOOR;
    let mut outlier_blocks = 0;
    for by in 0..BLOCKS_PER_SIDE {
        for bx in 0..BLOCKS_PER_SIDE {
            let g = block_light(gpu, (width, height), (bx, by));
            let c = block_light(cpu, (width, height), (bx, by));
            if (g - c).abs() / c.max(block_floor) > BLOCK_TOLERANCE {
                outlier_blocks += 1;
            }
        }
    }
    Ok(Comparison {
        gpu_total,
        cpu_total,
        total_error,
        worst_channel_error,
        outlier_blocks,
    })
}

#[cfg(test)]
mod tests;
