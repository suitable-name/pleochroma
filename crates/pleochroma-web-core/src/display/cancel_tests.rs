//! The cancellable denoise: a raised flag stops it and stores nothing; an unset flag
//! changes no byte.

use super::*;
use pleochroma::geometry::cuts::StandardGemCuts;

const N_D: f32 = 2.417;
const WIDTH: u32 = 24;
const HEIGHT: u32 = 16;

fn parts() -> (Camera, Vec<GpuFacetPlane>, Vec<Vec3>) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let sum = (0..(WIDTH * HEIGHT) as usize)
        .map(|i| {
            Vec3::new(
                ((i * 37) % 101) as f32 * 0.03,
                ((i * 53) % 97) as f32 * 0.02,
                ((i * 71) % 89) as f32 * 0.025,
            )
        })
        .collect();
    (Camera::new(0.60, 0.45, 2.4, 42.0), planes, sum)
}

fn frame<'a>(camera: &'a Camera, planes: &'a [GpuFacetPlane], sum: &'a [Vec3]) -> DenoiseFrame<'a> {
    DenoiseFrame {
        guide_key: 7,
        width: WIDTH,
        height: HEIGHT,
        camera,
        planes,
        tools: &[],
        n_d: N_D,
        sample_count: 3,
        sum,
    }
}

/// A flag raised before the call returns `None` and leaves no guides behind (neither a
/// partial set nor a stale one), for both denoiser flavours.
#[test]
fn a_raised_flag_returns_none_and_stores_no_guides() {
    let (camera, planes, sum) = parts();
    let frame = frame(&camera, &planes, &sum);
    for mut denoiser in [Denoiser::new(), Denoiser::single_thread()] {
        let cancel = AtomicBool::new(true);
        assert!(
            denoiser
                .denoised_rgba_cancellable(&frame, &cancel)
                .is_none()
        );
        assert!(
            !denoiser.has_guides_for(7),
            "no guides from a cancelled call"
        );
        assert!(
            denoiser
                .denoised_mean_cancellable(&frame, &cancel)
                .is_none()
        );
        assert!(!denoiser.has_guides_for(7));
    }
}

/// With an unset flag the cancellable call is bit-equal to the plain one, for both
/// denoiser flavours (and a cancelled call leaves the denoiser usable).
#[test]
fn an_unset_flag_matches_the_plain_denoise_bit_for_bit() {
    let (camera, planes, sum) = parts();
    let frame = frame(&camera, &planes, &sum);
    for build in [Denoiser::new, Denoiser::single_thread] {
        let mut plain = build();
        let expected = plain.denoised_rgba(&frame);
        let expected_mean = plain.denoised_mean(&frame);

        let mut denoiser = build();
        assert!(
            denoiser
                .denoised_rgba_cancellable(&frame, &AtomicBool::new(true))
                .is_none()
        );
        let actual = denoiser
            .denoised_rgba_cancellable(&frame, &AtomicBool::new(false))
            .expect("an unset flag never cancels");
        assert_eq!(actual, expected);
        assert!(denoiser.has_guides_for(7));
        let actual_mean = denoiser
            .denoised_mean_cancellable(&frame, &AtomicBool::new(false))
            .expect("an unset flag never cancels");
        assert_eq!(actual_mean, expected_mean);
    }
}
