use super::*;

const SIDE: u32 = 16;
const STRIDE: usize = 16;

/// A frame with a bright, off-centre, colored blob on a dim background: asymmetric, so a
/// mirrored copy is a different picture.
fn blob() -> Vec<[f32; 3]> {
    let mut frame = Vec::new();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = f64::from(x) - 4.0;
            let dy = f64::from(y) - 5.0;
            let glow = (-dx.mul_add(dx, dy * dy) / 18.0).exp();
            #[expect(clippy::cast_possible_truncation, reason = "a test picture")]
            let level = glow as f32;
            frame.push([
                level.mul_add(2.0, 0.2),
                level.mul_add(1.0, 0.2),
                level.mul_add(0.5, 0.2),
            ]);
        }
    }
    frame
}

/// Multiplies every pixel by a deterministic factor in `1 - spread ..= 1 + spread`.
fn noisy(frame: &[[f32; 3]], spread: f32) -> Vec<[f32; 3]> {
    let mut state: u32 = 0x1234_5678;
    frame
        .iter()
        .map(|pixel| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            #[expect(clippy::cast_precision_loss, reason = "a test noise source")]
            let unit = (state >> 8) as f32 / 16_777_216.0;
            let factor = unit.mul_add(2.0, -1.0).mul_add(spread, 1.0);
            [pixel[0] * factor, pixel[1] * factor, pixel[2] * factor]
        })
        .collect()
}

fn scaled(frame: &[[f32; 3]], factor: f32) -> Vec<[f32; 3]> {
    frame
        .iter()
        .map(|pixel| [pixel[0] * factor, pixel[1] * factor, pixel[2] * factor])
        .collect()
}

fn mirrored(frame: &[[f32; 3]]) -> Vec<[f32; 3]> {
    frame
        .chunks(STRIDE)
        .flat_map(|row| row.iter().rev().copied())
        .collect()
}

#[test]
fn a_frame_agrees_with_itself() {
    let frame = blob();
    let result = compare(&frame, &frame, SIDE, SIDE).expect("comparable");
    assert!(result.passed(), "{}", result.describe());
    assert!(result.total_error.abs() < 1e-12);
    assert_eq!(result.outlier_blocks, 0);
}

#[test]
fn sample_noise_and_a_small_bias_pass() {
    let cpu = blob();
    // Each pixel is off by up to 30 % (one pixel of four samples), the whole by 4 %.
    let gpu = scaled(&noisy(&cpu, 0.3), 1.04);
    let result = compare(&gpu, &cpu, SIDE, SIDE).expect("comparable");
    assert!(result.passed(), "{}", result.describe());
}

#[test]
fn a_dim_gpu_frame_fails_on_the_total() {
    let cpu = blob();
    let result = compare(&scaled(&cpu, 0.6), &cpu, SIDE, SIDE).expect("comparable");
    assert!(!result.passed());
    assert!(result.total_error > MEAN_TOLERANCE);
}

#[test]
fn a_lost_color_channel_fails_on_the_channels() {
    let cpu = blob();
    // The blue channel is missing and red takes its light: the total stays about the same.
    let gpu: Vec<[f32; 3]> = cpu
        .iter()
        .map(|pixel| [pixel[0] + pixel[2], pixel[1], 0.0])
        .collect();
    let result = compare(&gpu, &cpu, SIDE, SIDE).expect("comparable");
    assert!(
        result.total_error <= MEAN_TOLERANCE,
        "{}",
        result.describe()
    );
    assert!(result.worst_channel_error > CHANNEL_TOLERANCE);
    assert!(!result.passed());
}

#[test]
fn a_mirrored_picture_fails_on_the_blocks() {
    let cpu = blob();
    let result = compare(&mirrored(&cpu), &cpu, SIDE, SIDE).expect("comparable");
    // The mirrored picture has the same light and nearly the same channel shares.
    assert!(result.total_error < 1e-6);
    assert!(result.outlier_blocks > OUTLIER_BLOCKS_ALLOWED);
    assert!(!result.passed());
}

#[test]
fn a_couple_of_odd_blocks_are_allowed() {
    let cpu = blob();
    let mut gpu = cpu.clone();
    // Blank two blocks of the 4 x 4 grid (columns 12..16 of rows 12..16 and 0..4).
    for y in 12..16_usize {
        for x in (0..4_usize).chain(12..16) {
            gpu[y * STRIDE + x] = [0.0; 3];
        }
    }
    let result = compare(&gpu, &cpu, SIDE, SIDE).expect("comparable");
    assert_eq!(result.outlier_blocks, 2, "{}", result.describe());
}

#[test]
fn frames_that_cannot_be_compared_are_errors() {
    let frame = blob();
    assert!(compare(&frame, &frame[1..], SIDE, SIDE).is_err());
    assert!(compare(&[], &[], 0, 0).is_err());
    let mut nan = frame.clone();
    nan[3][1] = f32::NAN;
    assert!(compare(&nan, &frame, SIDE, SIDE).is_err());
    assert!(compare(&frame, &nan, SIDE, SIDE).is_err());
    let mut negative = frame.clone();
    negative[0][0] = -0.5;
    assert!(compare(&negative, &frame, SIDE, SIDE).is_err());
    let black = vec![[0.0_f32; 3]; frame.len()];
    assert!(compare(&frame, &black, SIDE, SIDE).is_err());
}

#[test]
fn the_description_carries_the_numbers() {
    let cpu = blob();
    let text = compare(&scaled(&cpu, 0.5), &cpu, SIDE, SIDE)
        .expect("comparable")
        .describe();
    assert!(text.contains("50.0 % off"), "{text}");
    assert!(text.contains("of 16 blocks"), "{text}");
}
