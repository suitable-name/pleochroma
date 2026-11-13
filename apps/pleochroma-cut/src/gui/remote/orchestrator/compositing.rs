//! Turning a remote accumulator's running sum into a displayable, denoised RGBA byte
//! buffer -- [`render_merged_frame`], and the two `Copy` bundles it takes. See this
//! group's own `mod.rs` doc comment.

use crate::bridge::{
    frame_cache::guide_pass::GuideCache,
    render_thread::{
        DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame, tonemap_running_average,
    },
};
use glam::Vec3;
use pleochroma::geometry::{
    plane::GpuFacetPlane,
    tool::{StoneGeometry, ToolPrimitive},
};

/// The remote accumulator's current running sum, as [`render_merged_frame`] receives it:
/// dimensions, sample count, and the merged radiance itself. `Copy` -- every field is a
/// shared reference or a scalar, matching `render_thread::gpu_backend::BackendFrame`'s
/// identical rationale.
#[derive(Clone, Copy)]
pub(super) struct AccumSnapshot<'a> {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) samples_done: u32,
    pub(super) buffer: &'a [Vec3],
}

/// The pose (`yaw`/`pitch`/`distance`) and geometry [`render_merged_frame`] needs to key
/// and, on a cache miss, regenerate the guide-buffer prepass -- see
/// [`GuideCache::ensure`]. `Copy` for the same reason as [`AccumSnapshot`].
#[derive(Clone, Copy)]
pub(super) struct PoseAndGeometry<'a> {
    pub(super) yaw: f32,
    pub(super) camera_roll: f32,
    pub(super) pitch: f32,
    pub(super) distance: f32,
    pub(super) planes: &'a [GpuFacetPlane],
    /// The concave tools cut out of `planes` (empty for a planar stone): the guides must
    /// see the notches the image shows, or the denoiser would smooth across their edges.
    pub(super) tools: &'a [ToolPrimitive],
    /// The material's `n_d` (`0.0` = none): part of the guide key, since the path
    /// signature refracts at it.
    pub(super) n_d: f32,
}

/// Turns a remote accumulator's running sum into a displayable RGBA byte buffer.
///
/// The denoise is the same single À-Trous pass the local path runs on its readback. It
/// runs once on the whole merged buffer, never per contribution, because denoising is
/// nonlinear and `buffer` already holds the sum of every `FRAME` received so far.
///
/// The depth, normal and facet-id guides come from `guide_cache`. A remote payload does not
/// carry them, so they are built locally by a primary-ray prepass over the current pose and
/// geometry. Caching on pose and geometry keeps repeated `FRAME` events cheap (see
/// `bridge::frame_cache::guide_pass`).
///
/// No Slint types, so the unit tests run without a window, socket or worker. The full
/// denoise takes seconds at 4K, so only `super::generation`'s background thread calls this,
/// never the UI thread. That thread seeds a throwaway [`GuideCache`] via [`GuideCache::adopt`]
/// so the internal `ensure` is a cache hit rather than a synchronous regenerate.
pub(super) fn render_merged_frame(
    accum: AccumSnapshot<'_>,
    denoise_enabled: bool,
    pose: PoseAndGeometry<'_>,
    guide_cache: &mut GuideCache,
    scratch: &mut DenoiseScratch<'_>,
) -> Vec<u8> {
    if !denoise_enabled {
        return tonemap_running_average(
            accum.width,
            accum.height,
            accum.samples_done,
            accum.buffer,
        );
    }
    let camera = pleochroma::optics::raytracer::Camera::new(
        pose.yaw,
        pose.pitch,
        pose.distance,
        pleochroma::optics::raytracer::DEFAULT_FOV_DEG,
    )
    .with_roll(pose.camera_roll);
    let stone = StoneGeometry {
        planes: pose.planes,
        tools: pose.tools,
    };
    let key = GuideCache::key_for_camera(accum.width, accum.height, &camera, stone, pose.n_d);
    let never_cancel = std::sync::atomic::AtomicBool::new(false);
    let Some(guides) = guide_cache.ensure_geom_cancellable(key, stone, &never_cancel) else {
        return tonemap_running_average(
            accum.width,
            accum.height,
            accum.samples_done,
            accum.buffer,
        );
    };
    denoise_and_tonemap_frame(
        FirstHitSnapshot {
            width: accum.width,
            height: accum.height,
            current_sample_count: accum.samples_done,
            accum_buffer: accum.buffer,
            first_hit_depth: &guides.depth,
            first_hit_normal: &guides.normal,
            first_hit_facet_id: &guides.facet_id,
            first_hit_path_sig: &guides.path_sig,
        },
        scratch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::frame_cache::guide_pass::generate_guide_buffers;
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::raytracer::{Camera, DEFAULT_FOV_DEG},
        renderer::denoise::AtrousDenoiser,
    };

    /// A deliberately non-uniform synthetic accumulation buffer (mirrors
    /// `render_thread`'s own `denoise_and_tonemap_frame` tests) so a bug that skips
    /// filtering, or filters the wrong data, would visibly change the output.
    fn synthetic_noisy_buffer(width: u32, height: u32) -> Vec<Vec3> {
        (0..(width * height) as usize)
            .map(|i| {
                let x = (i % width as usize) as f32;
                Vec3::new(1.0 + x, 0.5 * x, 0.1f32.mul_add(-x, 2.0))
            })
            .collect()
    }

    #[test]
    fn render_merged_frame_skips_denoising_when_disabled() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();

        let bytes = render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: 4,
                buffer: &buffer,
            },
            false,
            PoseAndGeometry {
                yaw: 0.60,
                camera_roll: 0.0,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
                tools: &[],
                n_d: 0.0,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        assert_eq!(bytes, tonemap_running_average(width, height, 4, &buffer));
        assert_eq!(
            guide_cache.generation(),
            0,
            "denoising disabled must never trigger the guide prepass at all -- the \
             user's toggle governs whether the (cheap, but not free) prepass runs, \
             exactly like it already governs the local path"
        );
    }

    #[test]
    fn render_merged_frame_reuses_guides_across_repeated_calls_with_an_unchanged_pose() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();

        // Three redraws, as if three `FRAME` events arrived from the same
        // in-progress remote render at an unchanged camera pose.
        for samples in [1u32, 2, 3] {
            render_merged_frame(
                AccumSnapshot {
                    width,
                    height,
                    samples_done: samples,
                    buffer: &buffer,
                },
                true,
                PoseAndGeometry {
                    yaw: 0.60,
                    camera_roll: 0.0,
                    pitch: 0.45,
                    distance: 2.4,
                    planes: &planes,
                    tools: &[],
                    n_d: 0.0,
                },
                &mut guide_cache,
                &mut DenoiseScratch {
                    denoiser: &mut denoiser,
                    avg_color_buf: &mut avg,
                    filtered_buf: &mut filtered,
                },
            );
        }

        assert_eq!(
            guide_cache.generation(),
            1,
            "repeated redraws of the same in-progress remote render at an unchanged \
             pose must reuse the cached guide buffers, not regenerate them per frame"
        );
    }

    #[test]
    fn render_merged_frame_regenerates_guides_when_the_pose_changes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();

        render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: 1,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw: 0.60,
                camera_roll: 0.0,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
                tools: &[],
                n_d: 0.0,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );
        render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: 1,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw: 0.90,
                camera_roll: 0.0,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
                tools: &[],
                n_d: 0.0,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        assert_eq!(
            guide_cache.generation(),
            2,
            "a camera-pose change (a fresh drag settling into a new remote request) \
             must invalidate the cached guides -- reusing a stale pose's guides would \
             misalign depth/normal/facet-id against the new image's geometry"
        );
    }

    /// `render_merged_frame` takes one buffer and its true merged `samples_done`, which
    /// drives the denoiser's convergence taper (see `renderer::denoise`). At a converged
    /// count the output must be bit-identical to a plain tonemap, as on the local path. A
    /// denoise of a small per-contribution slice could never reach that identity.
    #[test]
    fn render_merged_frame_at_a_converged_sample_count_matches_a_plain_tonemap() {
        const HIGH_SAMPLE_COUNT: u32 = 50_000; // taper(50000) << taper_identity_epsilon

        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);

        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();
        let high_total_bytes = render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done: HIGH_SAMPLE_COUNT,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw: 0.60,
                camera_roll: 0.0,
                pitch: 0.45,
                distance: 2.4,
                planes: &planes,
                tools: &[],
                n_d: 0.0,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        assert_eq!(
            high_total_bytes,
            tonemap_running_average(width, height, HIGH_SAMPLE_COUNT, &buffer),
            "at a converged TRUE total, denoising the merged buffer must be an exact \
             no-op, just like the local path's readback"
        );
    }

    /// At a low (unconverged) sample count, `render_merged_frame` must match a direct
    /// `denoise_and_tonemap_frame` fed the guides from `generate_guide_buffers` for the same
    /// pose and geometry. The check compares output, not that the image "looks filtered":
    /// on a tiny crop the per-facet edge stop can leave every pixel unchanged (see
    /// `renderer::denoise`).
    #[test]
    fn render_merged_frame_at_a_low_sample_count_matches_denoise_and_tonemap_frame_directly() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let (width, height) = (6u32, 5u32);
        let buffer = synthetic_noisy_buffer(width, height);
        let (yaw, pitch, distance) = (0.60, 0.45, 2.4);
        let samples_done = 1;
        let mut guide_cache = GuideCache::new();
        let mut denoiser = AtrousDenoiser::new();
        let mut avg = Vec::new();
        let mut filtered = Vec::new();
        let actual = render_merged_frame(
            AccumSnapshot {
                width,
                height,
                samples_done,
                buffer: &buffer,
            },
            true,
            PoseAndGeometry {
                yaw,
                camera_roll: 0.0,
                pitch,
                distance,
                planes: &planes,
                tools: &[],
                n_d: 0.0,
            },
            &mut guide_cache,
            &mut DenoiseScratch {
                denoiser: &mut denoiser,
                avg_color_buf: &mut avg,
                filtered_buf: &mut filtered,
            },
        );

        // Independently reproduce what `render_merged_frame` should have done: generate
        // the guides for the same pose/geometry and denoise directly, with fresh
        // scratch state so nothing is shared with the call above.
        let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG);
        let guides = generate_guide_buffers(width, height, &camera, &planes);
        let mut expected_denoiser = AtrousDenoiser::new();
        let mut expected_avg = Vec::new();
        let mut expected_filtered = Vec::new();
        let expected = denoise_and_tonemap_frame(
            FirstHitSnapshot {
                width,
                height,
                current_sample_count: samples_done,
                accum_buffer: &buffer,
                first_hit_depth: &guides.depth,
                first_hit_normal: &guides.normal,
                first_hit_facet_id: &guides.facet_id,
                first_hit_path_sig: &guides.path_sig,
            },
            &mut DenoiseScratch {
                denoiser: &mut expected_denoiser,
                avg_color_buf: &mut expected_avg,
                filtered_buf: &mut expected_filtered,
            },
        );

        assert_eq!(
            actual, expected,
            "render_merged_frame must denoise via the exact same guides/mechanism a \
             direct generate_guide_buffers + denoise_and_tonemap_frame call would \
             produce for the identical pose and geometry"
        );
    }
}
