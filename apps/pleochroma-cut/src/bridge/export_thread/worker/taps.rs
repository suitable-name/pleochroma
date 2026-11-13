//! Optional taps on a render: where its sample indices start (`--sample-offset`) and who
//! receives its linear image (`--raw-out`, `--progress-raw`).
//!
//! A render without taps ([`RenderTaps::default`]) starts at sample 0, is not adaptive and
//! publishes nothing.
//!
//! The linear image is the accumulation buffer the tone mapper reads: per pixel the summed
//! CIE XYZ of `samples_per_pixel` samples, one frame-wide count. An adaptive render
//! ([`RenderTaps::adaptive`]) gives every pixel its own count instead: it publishes the per-pixel
//! means with their counts ([`LinearImage::counts`]). A [`RawTapSink`] turns the image into a
//! file (the command line writes `PLRAW1`); this module only hands it over.

use glam::Vec3;
use pleochroma::renderer::adaptive::AdaptiveSettings;
use std::{fmt, sync::Arc};

/// The first snapshot of a `--progress-raw` render is taken at this many samples per pixel.
pub const FIRST_SNAPSHOT_SPP: u32 = 16;

/// Whether a [`LinearImage`] is a snapshot on the way or the finished frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinearImageKind {
    /// The accumulation so far, taken at a chunk boundary of a running render.
    Snapshot,
    /// The finished image of a frame.
    Final,
}

/// A render's linear image: CIE XYZ per pixel and the samples behind it.
#[derive(Debug, Clone, Copy)]
pub struct LinearImage<'a> {
    /// Snapshot or finished frame.
    pub kind: LinearImageKind,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The samples behind every pixel's sum (one count for the whole frame). With
    /// [`Self::counts`] it is the most any pixel has.
    pub samples_per_pixel: u32,
    /// The summed CIE XYZ of each pixel, row-major from the top-left; with [`Self::counts`], the
    /// MEAN of each pixel instead (already divided by its own count).
    pub sums: &'a [Vec3],
    /// A tilt-video frame: the file stem of its PNG (`frame_0007`); `None` for a still.
    pub frame_name: Option<&'a str>,
    /// The samples behind each pixel, row-major, when they differ from pixel to pixel (adaptive
    /// sampling); `None` for the usual one frame-wide count.
    pub counts: Option<&'a [u32]>,
}

impl LinearImage<'_> {
    /// The mean CIE XYZ of each pixel (the sum over the sample count), row-major.
    ///
    /// The division is the one `tonemap_accumulation` and the `PLEOCHROMA_LINEAR_XYZ_OUT`
    /// diagnostic make: one reciprocal of the frame-wide count, multiplied in. An image with
    /// per-pixel [`Self::counts`] already holds its means and returns them as they are.
    #[must_use]
    pub fn mean_xyz(&self) -> Vec<[f32; 3]> {
        if self.counts.is_some() {
            return self.sums.iter().map(Vec3::to_array).collect();
        }
        let inv_samples = 1.0 / self.samples_per_pixel.max(1) as f32;
        self.sums
            .iter()
            .map(|sum| {
                [
                    sum.x * inv_samples,
                    sum.y * inv_samples,
                    sum.z * inv_samples,
                ]
            })
            .collect()
    }

    /// The number of samples behind each pixel, row-major: [`Self::counts`], or the frame-wide
    /// count for every pixel.
    #[must_use]
    pub fn pixel_counts(&self) -> Vec<u32> {
        self.counts.map_or_else(
            || vec![self.samples_per_pixel; self.sums.len()],
            <[u32]>::to_vec,
        )
    }
}

/// Receives the linear images a render publishes.
///
/// Called on the thread that renders, between sample batches, so a slow sink slows the
/// render. A sink that fails keeps the error to itself and reports it after the render.
pub trait RawTapSink: Send + Sync + fmt::Debug {
    /// One linear image: a snapshot, or the finished frame.
    fn linear_image(&self, image: &LinearImage<'_>);
}

/// The optional taps of one render or video; the default is none.
#[derive(Debug, Clone, Default)]
pub struct RenderTaps {
    /// The absolute index of the render's first sample (0 for a render from the start).
    pub sample_offset: u32,
    /// Who receives the linear images; `None` publishes nothing.
    pub sink: Option<Arc<dyn RawTapSink>>,
    /// Also publish a snapshot at every power of two from [`FIRST_SNAPSHOT_SPP`] on.
    pub snapshots: bool,
    /// The frame being rendered (a tilt video), for [`LinearImage::frame_name`].
    pub frame_name: Option<String>,
    /// Adaptive sampling: `Some` renders every pixel only as long as its tile needs. The
    /// settings' `max_spp` is overridden by the render's own samples per pixel, which is the cap.
    /// `None` (the default) is the ordinary render. A build without the
    /// `adaptive-sampling` feature renders `Some` as `None`, with a note.
    pub adaptive: Option<AdaptiveSettings>,
    /// Adaptive sampling only: keep the frame's sums on the GPU and run the convergence check
    /// there (`GpuBackend::try_adaptive_resident`), reading back one value per live tile per
    /// pass and the whole image once. Opt in (`--gpu-convergence`); falls back to the host path
    /// wherever the GPU declines, and is ignored without [`Self::adaptive`]. `false` (the
    /// default) keeps the host path.
    pub gpu_convergence: bool,
    /// Path cache: `Some` shades eligible stone pixels from cached exit records
    /// (built per tile, kept in VRAM or RAM, persisted when the settings say so); `None` (the
    /// default) traces every sample in full.
    pub path_cache: Option<pleochroma::renderer::accel::PathCacheSettings>,
    /// Where persisted path caches live (`<app data>/render_cache/` when `None`); read only
    /// when [`Self::path_cache`] persists.
    pub cache_dir: Option<std::path::PathBuf>,
    /// The disk limit for persisted path caches in bytes; `None` uses the limit the Preferences
    /// dialog saved in the settings file (`path_cache_disk_limit_gb`, 50 GB when it has none).
    pub cache_disk_limit_bytes: Option<u64>,
    /// Beam tracing: `Some` beam-traces eligible faceted stones with the
    /// residual through the render's engines; `None` (the default) traces no beams.
    /// Without the `beam` feature the field still exists (a job file's `accel.beam` fills it),
    /// and a render whose taps carry beams renders without them, with the note of
    /// `BeamOff::Unavailable`.
    pub beam: Option<pleochroma::renderer::accel_beam::BeamSettings>,
}

impl RenderTaps {
    /// Whether the render should publish snapshots while it runs.
    #[must_use]
    pub const fn wants_snapshots(&self) -> bool {
        self.snapshots && self.sink.is_some()
    }

    /// Hands `sums` (the sums of `samples_per_pixel` samples) to the sink, if there is one.
    pub fn publish(
        &self,
        kind: LinearImageKind,
        (width, height): (u32, u32),
        samples_per_pixel: u32,
        sums: &[Vec3],
    ) {
        if let Some(sink) = &self.sink {
            sink.linear_image(&LinearImage {
                kind,
                width,
                height,
                samples_per_pixel,
                sums,
                frame_name: self.frame_name.as_deref(),
                counts: None,
            });
        }
    }

    /// Hands an adaptive render's image to the sink, if there is one: the per-pixel `means` and
    /// the `counts` behind each, with `samples_done` the samples per pixel the live tiles have.
    pub fn publish_counted(
        &self,
        kind: LinearImageKind,
        (width, height): (u32, u32),
        samples_done: u32,
        means: &[Vec3],
        counts: &[u32],
    ) {
        if let Some(sink) = &self.sink {
            sink.linear_image(&LinearImage {
                kind,
                width,
                height,
                samples_per_pixel: samples_done,
                sums: means,
                frame_name: self.frame_name.as_deref(),
                counts: Some(counts),
            });
        }
    }
}

/// When a running render takes its next snapshot: at the first chunk boundary at or after
/// each power of two from [`FIRST_SNAPSHOT_SPP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotSchedule {
    /// The next power of two a boundary has to reach.
    next: u32,
}

impl SnapshotSchedule {
    /// A schedule that has taken no snapshot yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next: FIRST_SNAPSHOT_SPP,
        }
    }

    /// Whether a snapshot is due at a chunk boundary with `done` samples per pixel behind it.
    ///
    /// When it is, the schedule moves on to the first power of two above `done`, so a chunk
    /// that jumps over several powers of two takes one snapshot, not several.
    pub const fn due(&mut self, done: u32) -> bool {
        if done < self.next {
            return false;
        }
        while self.next <= done {
            let Some(next) = self.next.checked_mul(2) else {
                self.next = u32::MAX;
                break;
            };
            self.next = next;
        }
        true
    }
}

/// The most samples a claim may take, `done` samples per pixel into a render that takes
/// snapshots, so that it ends exactly on the next snapshot point: the first power of two
/// above `done`, and never below [`FIRST_SNAPSHOT_SPP`].
///
/// The local lane claims `min(its chunk size, this)`, so a GPU chunk of 1/40 of the budget
/// does not jump over the low powers of two (16 to 256 at 16k spp).
#[must_use]
pub fn snapshot_claim_cap(done: u32) -> u32 {
    let target = done
        .checked_add(1)
        .and_then(u32::checked_next_power_of_two)
        .unwrap_or(u32::MAX)
        .max(FIRST_SNAPSHOT_SPP);
    target.saturating_sub(done).max(1)
}

impl Default for SnapshotSchedule {
    fn default() -> Self {
        Self::new()
    }
}

/// The element-wise sum of the CPU and GPU accumulation buffers.
#[must_use]
pub fn summed(cpu: &[Vec3], gpu: &[Vec3]) -> Vec<Vec3> {
    cpu.iter().zip(gpu).map(|(a, b)| *a + *b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// What a published image looked like: kind, samples, frame name, mean XYZ.
    type Seen = (LinearImageKind, u32, Option<String>, Vec<[f32; 3]>);

    #[derive(Debug, Default)]
    struct Recorder {
        seen: Mutex<Vec<Seen>>,
    }

    impl RawTapSink for Recorder {
        fn linear_image(&self, image: &LinearImage<'_>) {
            self.seen.lock().unwrap().push((
                image.kind,
                image.samples_per_pixel,
                image.frame_name.map(str::to_owned),
                image.mean_xyz(),
            ));
        }
    }

    #[test]
    fn no_taps_publish_nothing_and_start_at_sample_zero() {
        let taps = RenderTaps::default();
        assert_eq!(taps.sample_offset, 0);
        assert!(!taps.wants_snapshots());
        // Nothing to receive it: must not panic.
        taps.publish(LinearImageKind::Final, (1, 1), 4, &[Vec3::ONE]);
    }

    #[test]
    fn snapshots_need_both_the_flag_and_a_sink() {
        let sink: Arc<dyn RawTapSink> = Arc::new(Recorder::default());
        let flag_only = RenderTaps {
            snapshots: true,
            ..RenderTaps::default()
        };
        let sink_only = RenderTaps {
            sink: Some(Arc::clone(&sink)),
            ..RenderTaps::default()
        };
        let both = RenderTaps {
            sink: Some(sink),
            snapshots: true,
            ..RenderTaps::default()
        };
        assert!(!flag_only.wants_snapshots());
        assert!(!sink_only.wants_snapshots());
        assert!(both.wants_snapshots());
    }

    #[test]
    fn the_sink_gets_the_mean_not_the_sum_and_the_frame_name() {
        let recorder = Arc::new(Recorder::default());
        let taps = RenderTaps {
            sink: Some(recorder.clone()),
            frame_name: Some("frame_0003".to_string()),
            ..RenderTaps::default()
        };
        let sums = [Vec3::new(8.0, 4.0, 2.0), Vec3::ZERO];
        taps.publish(LinearImageKind::Final, (2, 1), 4, &sums);
        let seen = recorder.seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            &[(
                LinearImageKind::Final,
                4,
                Some("frame_0003".to_string()),
                vec![[2.0, 1.0, 0.5], [0.0, 0.0, 0.0]],
            )]
        );
    }

    #[test]
    fn an_image_with_counts_keeps_its_means_and_reports_every_pixels_own_count() {
        let means = [Vec3::new(2.0, 1.0, 0.5), Vec3::new(4.0, 2.0, 1.0)];
        let counts = [16, 64];
        let image = LinearImage {
            kind: LinearImageKind::Final,
            width: 2,
            height: 1,
            samples_per_pixel: 64,
            sums: &means,
            frame_name: None,
            counts: Some(&counts),
        };
        assert_eq!(
            image.mean_xyz(),
            vec![[2.0, 1.0, 0.5], [4.0, 2.0, 1.0]],
            "the means are not divided again"
        );
        assert_eq!(image.pixel_counts(), vec![16, 64]);

        let plain = LinearImage {
            counts: None,
            sums: &[Vec3::new(8.0, 4.0, 2.0), Vec3::ZERO],
            ..image
        };
        assert_eq!(plain.mean_xyz()[0], [0.125, 0.0625, 0.03125]);
        assert_eq!(plain.pixel_counts(), vec![64, 64]);
    }

    #[test]
    fn publish_counted_hands_the_sink_the_means_and_the_counts() {
        let recorder = Arc::new(Recorder::default());
        let taps = RenderTaps {
            sink: Some(recorder.clone()),
            ..RenderTaps::default()
        };
        let means = [Vec3::new(2.0, 1.0, 0.5), Vec3::ZERO];
        taps.publish_counted(LinearImageKind::Snapshot, (2, 1), 32, &means, &[32, 16]);
        let seen = recorder.seen.lock().unwrap();
        assert_eq!(
            seen.as_slice(),
            &[(
                LinearImageKind::Snapshot,
                32,
                None,
                vec![[2.0, 1.0, 0.5], [0.0, 0.0, 0.0]],
            )]
        );
        assert!(RenderTaps::default().adaptive.is_none(), "off by default");
    }

    #[test]
    fn the_gpu_resident_check_is_opt_in() {
        assert!(!RenderTaps::default().gpu_convergence);
    }

    #[test]
    fn snapshots_fall_on_the_first_boundary_at_or_after_each_power_of_two() {
        let mut schedule = SnapshotSchedule::new();
        let boundaries = [4, 10, 15, 16, 20, 31, 33, 40, 64, 70, 200];
        let taken: Vec<u32> = boundaries
            .into_iter()
            .filter(|&done| schedule.due(done))
            .collect();
        // 16 -> at 16; 32 -> at 33; 64 -> at 64; 128 -> at 200 (one snapshot for the jump).
        assert_eq!(taken, vec![16, 33, 64, 200]);
        assert!(!schedule.due(201), "the next power of two is 256");
        assert!(schedule.due(256));
    }

    #[test]
    fn a_boundary_past_the_top_of_the_range_cannot_loop_forever() {
        let mut schedule = SnapshotSchedule::new();
        assert!(schedule.due(u32::MAX));
        assert!(!schedule.due(u32::MAX - 1));
    }

    #[test]
    fn capped_claims_land_on_every_power_of_two() {
        // A GPU chunk of 409 spp (16384 / 40), capped: the boundaries hit 16, 32, ... 16384.
        let chunk = 409;
        let mut done = 0;
        let mut schedule = SnapshotSchedule::new();
        let mut taken = Vec::new();
        while done < 16_384 {
            done += chunk.min(snapshot_claim_cap(done)).min(16_384 - done);
            if schedule.due(done) {
                taken.push(done);
            }
        }
        assert_eq!(
            taken,
            vec![16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16_384]
        );
    }

    #[test]
    fn the_claim_cap_starts_at_the_first_snapshot_and_never_hits_zero() {
        assert_eq!(snapshot_claim_cap(0), FIRST_SNAPSHOT_SPP);
        assert_eq!(snapshot_claim_cap(5), FIRST_SNAPSHOT_SPP - 5);
        assert_eq!(snapshot_claim_cap(16), 16);
        assert_eq!(snapshot_claim_cap(100), 28);
        assert_eq!(snapshot_claim_cap(u32::MAX), 1);
        assert_eq!(snapshot_claim_cap((1 << 31) + 1), u32::MAX - (1 << 31) - 1);
    }

    #[test]
    fn summing_the_buffers_adds_pixel_by_pixel() {
        let cpu = [Vec3::new(1.0, 2.0, 3.0), Vec3::ONE];
        let gpu = [Vec3::new(0.5, 0.5, 0.5), Vec3::ZERO];
        assert_eq!(
            summed(&cpu, &gpu),
            vec![Vec3::new(1.5, 2.5, 3.5), Vec3::ONE]
        );
    }
}
