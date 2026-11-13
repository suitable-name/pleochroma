//! The false-color Delta E map of `--error-png`.

/// The colors of the map: a Delta E and the RGB it is shown as. Between two stops the color is
/// linear; from the last stop on it stays.
const STOPS: [(f32, [f32; 3]); 5] = [
    (0.0, [0.0, 0.0, 0.0]),
    (1.0, [0.0, 0.0, 255.0]),
    (2.0, [0.0, 255.0, 0.0]),
    (5.0, [255.0, 255.0, 0.0]),
    (10.0, [255.0, 0.0, 0.0]),
];

/// The color of a pixel whose Delta E is `de`: black at 0, blue at 1, green at 2, yellow at 5 and
/// red from 10, linear in between.
#[must_use]
pub(super) fn false_color(de: f32) -> [u8; 3] {
    let last = STOPS[STOPS.len() - 1];
    if de.is_nan() || de <= STOPS[0].0 {
        return channels(STOPS[0].1);
    }
    if de >= last.0 {
        return channels(last.1);
    }
    for pair in STOPS.windows(2) {
        let ((low, from), (high, to)) = (pair[0], pair[1]);
        if de <= high {
            let t = (de - low) / (high - low);
            return channels([
                (to[0] - from[0]).mul_add(t, from[0]),
                (to[1] - from[1]).mul_add(t, from[1]),
                (to[2] - from[2]).mul_add(t, from[2]),
            ]);
        }
    }
    channels(last.1)
}

/// The RGBA8 bytes of a map, one opaque pixel per Delta E.
#[must_use]
pub(super) fn to_rgba(error_map: &[f32]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(error_map.len() * 4);
    for &de in error_map {
        let [red, green, blue] = false_color(de);
        rgba.extend_from_slice(&[red, green, blue, 255]);
    }
    rgba
}

/// Rounds 0..=255 float channels to bytes.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "each channel is clamped to 0..=255 before it is rounded to a byte"
)]
fn channels(rgb: [f32; 3]) -> [u8; 3] {
    rgb.map(|value| value.clamp(0.0, 255.0).round() as u8)
}
