//! Live export-progress thumbnail: box-downsamples the in-progress accumulation into a small
//! sRGB image, rate-limited so a fast export does not rebuild it on every batch.

use super::tonemap_png::tonemap_accumulation;
use glam::Vec3;
use pleochroma::color::ColorSpace;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::time::{Duration, Instant};

/// Preview thumbnail's long edge in pixels: large enough to read the composition, small enough
/// that the downsample stays cheap. The pass visits every source pixel, so its cost follows the
/// export's resolution.
const PREVIEW_MAX_LONG_EDGE: u32 = 360;

/// Minimum time between preview regenerations, about two per second. Nobody can see a faster
/// change, and a 4K export ticks far more often than that.
const PREVIEW_MIN_INTERVAL: Duration = Duration::from_millis(500);

/// A source of "now" for [`PreviewThrottle`], so tests can script the instants.
/// Production uses [`SystemClock`].
pub(super) trait Clock {
    /// The current instant.
    fn now(&mut self) -> Instant;
}

/// The real wall clock: [`Instant::now`], what [`PreviewThrottle`] reads in production.
pub(super) struct SystemClock;

impl Clock for SystemClock {
    fn now(&mut self) -> Instant {
        Instant::now()
    }
}

/// Rate-limits preview regeneration for one whole export.
///
/// Generic over its clock so tests can use a fake. [`PreviewThrottle::new`] uses the wall clock.
pub(super) struct PreviewThrottle<C: Clock = SystemClock> {
    last: Option<Instant>,
    clock: C,
}

impl PreviewThrottle<SystemClock> {
    pub(super) const fn new() -> Self {
        Self {
            last: None,
            clock: SystemClock,
        }
    }
}

impl<C: Clock> PreviewThrottle<C> {
    /// Builds a throttle that reads `clock`. Test-only.
    #[cfg(test)]
    const fn with_clock(clock: C) -> Self {
        Self { last: None, clock }
    }

    /// Returns a new thumbnail once `PREVIEW_MIN_INTERVAL` has passed since the last one. The
    /// first call always produces one; otherwise `None`. `accum` and `gpu_accum` are the local
    /// buffers; `remote_accum` is the remote snapshot, `None` when no remote is in play.
    pub(super) fn maybe_generate(
        &mut self,
        width: u32,
        height: u32,
        accum: &[Vec3],
        gpu_accum: &[Vec3],
        remote_accum: Option<&[Vec3]>,
        samples_done: u32,
    ) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let now = self.clock.now();
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < PREVIEW_MIN_INTERVAL)
        {
            return None;
        }
        self.last = Some(now);
        Some(generate_preview_buffer(
            width,
            height,
            accum,
            gpu_accum,
            remote_accum,
            samples_done,
        ))
    }
}

/// Builds the `SharedPixelBuffer` for the UI thread. The pixel work is in [`downsample_preview`].
fn generate_preview_buffer(
    width: u32,
    height: u32,
    accum: &[Vec3],
    gpu_accum: &[Vec3],
    remote_accum: Option<&[Vec3]>,
    samples_done: u32,
) -> SharedPixelBuffer<Rgba8Pixel> {
    let (thumb_w, thumb_h, rgba) =
        downsample_preview(width, height, accum, gpu_accum, remote_accum, samples_done);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(thumb_w, thumb_h);
    // Reinterprets the RGBA8 bytes as `Rgba8Pixel` without copying.
    let dst = buffer.make_mut_slice();
    let src: &[Rgba8Pixel] = bytemuck::cast_slice(&rgba);
    dst.copy_from_slice(src);
    buffer
}

/// Box-downsamples the combined accumulation into a small sRGB thumbnail, returning
/// `(width, height, rgba_bytes)`. Each source pixel is folded straight into its bucket, with no
/// full-resolution intermediate.
///
/// # Combining the hybrid buffers
///
/// Each engine keeps its own buffer, and `run_export` sums them only at the end. The preview
/// sums all of them per pixel, so a mid-export thumbnail is not missing an engine's share.
///
/// # Normalising by samples actually done
///
/// Source pixels are sums, not averages. Normalisation happens in [`tonemap_accumulation`]
/// with `samples_done`. Using the export's target would make the preview nearly black for most
/// of the export.
///
/// # sRGB only
///
/// The thumbnail always uses the sRGB path, whatever the export's `ColorSpace`. Slint displays
/// sRGB bytes, and a preview cannot carry an ICC profile.
fn downsample_preview(
    width: u32,
    height: u32,
    accum: &[Vec3],
    gpu_accum: &[Vec3],
    remote_accum: Option<&[Vec3]>,
    samples_done: u32,
) -> (u32, u32, Vec<u8>) {
    debug_assert_eq!(accum.len(), gpu_accum.len());
    debug_assert_eq!(accum.len(), (width as usize) * (height as usize));
    debug_assert!(remote_accum.is_none_or(|r| r.len() == accum.len()));

    let long_edge = width.max(height).max(1);
    let scale = (f64::from(PREVIEW_MAX_LONG_EDGE) / f64::from(long_edge)).min(1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "scale is in (0, 1], so the rounded value is within 0..=width"
    )]
    let thumb_w = ((f64::from(width) * scale).round() as u32).max(1);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "scale is in (0, 1], so the rounded value is within 0..=height"
    )]
    let thumb_h = ((f64::from(height) * scale).round() as u32).max(1);

    let width_usize = width as usize;
    let height_usize = height as usize;
    let thumb_w_usize = thumb_w as usize;
    let thumb_rows = thumb_h as usize;

    let mut sum = vec![Vec3::ZERO; thumb_w_usize * thumb_rows];
    let mut count = vec![0u32; sum.len()];

    // Source-to-bucket mapping by floored division. It covers every bucket because `scale`
    // never upsamples, so no `count` stays zero.
    //
    // The column mapping depends only on `x`, so it is a lookup table. Recomputing it per pixel
    // cost ~15 ms per preview at 4K.
    let col_bucket: Vec<usize> = (0..width_usize)
        .map(|x| x * thumb_w_usize / width_usize)
        .collect();

    for y in 0..height_usize {
        let ty = y * thumb_rows / height_usize;
        let row = y * width_usize;
        let dst_row = ty * thumb_w_usize;
        for (x, &tx) in col_bucket.iter().enumerate() {
            let src = row + x;
            let dst = dst_row + tx;
            sum[dst] += accum[src] + gpu_accum[src] + remote_accum.map_or(Vec3::ZERO, |r| r[src]);
            count[dst] += 1;
        }
    }

    let thumb_accum: Vec<Vec3> = sum
        .iter()
        .zip(&count)
        .map(|(s, c)| *s / (*c).max(1) as f32)
        .collect();

    let rgba = tonemap_accumulation(
        thumb_w,
        thumb_h,
        samples_done.max(1),
        &thumb_accum,
        ColorSpace::Srgb,
    );
    (thumb_w, thumb_h, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A uniform accum buffer (every pixel identical) must downsample to a uniform
    /// thumbnail of the expected dimensions -- the simplest possible correctness check
    /// on the bucket mapping and averaging.
    #[test]
    fn downsample_preview_of_a_uniform_buffer_is_uniform_and_correctly_sized() {
        let width = 37;
        let height = 21;
        let accum = vec![Vec3::new(1.0, 0.5, 0.25); (width * height) as usize];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];

        let (thumb_w, thumb_h, rgba) =
            downsample_preview(width, height, &accum, &gpu_accum, None, 4);

        assert!(thumb_w <= width && thumb_h <= height);
        assert_eq!(rgba.len(), (thumb_w * thumb_h * 4) as usize);
        let (chunks, _) = rgba.as_chunks::<4>();
        let first = chunks[0];
        for px in chunks {
            assert_eq!(
                px, &first,
                "a uniform source buffer must downsample uniformly"
            );
        }
    }

    /// Long-edge cap: an image already smaller than `PREVIEW_MAX_LONG_EDGE` must not be
    /// upsampled -- the preview is a downsample-only operation.
    #[test]
    fn downsample_preview_never_upsamples_a_small_export() {
        let accum = vec![Vec3::ONE; 8 * 8];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];
        let (thumb_w, thumb_h, _) = downsample_preview(8, 8, &accum, &gpu_accum, None, 1);
        assert_eq!((thumb_w, thumb_h), (8, 8));
    }

    /// The GPU's contribution must not be dropped: an all-zero CPU `accum` with a
    /// nonzero `gpu_accum` must still produce a nonzero (non-black) preview.
    #[test]
    fn downsample_preview_includes_the_gpu_buffers_contribution() {
        let accum = vec![Vec3::ZERO; 4 * 4];
        let gpu_accum = vec![Vec3::new(2.0, 2.0, 2.0); accum.len()];
        let (_, _, rgba) = downsample_preview(4, 4, &accum, &gpu_accum, None, 1);
        assert!(
            rgba.iter().any(|&b| b > 0),
            "a nonzero gpu_accum-only buffer must not downsample to an all-black preview"
        );
    }

    /// The remote engine's live snapshot must not be dropped either -- same shape as
    /// the GPU check above, but for `remote_accum`.
    #[test]
    fn downsample_preview_includes_the_remote_buffers_contribution() {
        let accum = vec![Vec3::ZERO; 4 * 4];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];
        let remote_accum = vec![Vec3::new(3.0, 3.0, 3.0); accum.len()];
        let (_, _, rgba) = downsample_preview(4, 4, &accum, &gpu_accum, Some(&remote_accum), 1);
        assert!(
            rgba.iter().any(|&b| b > 0),
            "a nonzero remote_accum-only buffer must not downsample to an all-black preview"
        );
    }

    /// A [`Clock`] that returns a fixed sequence of instants, one per call.
    struct FakeClock {
        times: std::vec::IntoIter<Instant>,
    }

    impl FakeClock {
        fn new(times: Vec<Instant>) -> Self {
            Self {
                times: times.into_iter(),
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&mut self) -> Instant {
            self.times
                .next()
                .expect("FakeClock ran out of scheduled instants")
        }
    }

    /// The first call emits, a call before `PREVIEW_MIN_INTERVAL` is withheld, and a call after
    /// it emits again.
    #[test]
    fn preview_throttle_emits_first_then_withholds_until_the_interval_elapses() {
        let accum = vec![Vec3::ONE; 4 * 4];
        let gpu_accum = vec![Vec3::ZERO; accum.len()];
        let base = Instant::now();
        let mut throttle = PreviewThrottle::with_clock(FakeClock::new(vec![
            base,
            base + PREVIEW_MIN_INTERVAL / 2,
            base + PREVIEW_MIN_INTERVAL + Duration::from_millis(1),
        ]));

        assert!(
            throttle
                .maybe_generate(4, 4, &accum, &gpu_accum, None, 1)
                .is_some(),
            "the first call must always produce a preview"
        );
        assert!(
            throttle
                .maybe_generate(4, 4, &accum, &gpu_accum, None, 1)
                .is_none(),
            "a call made before the interval elapses must be rate-limited"
        );
        assert!(
            throttle
                .maybe_generate(4, 4, &accum, &gpu_accum, None, 1)
                .is_some(),
            "a call made after the interval elapses must emit again"
        );
    }
}
