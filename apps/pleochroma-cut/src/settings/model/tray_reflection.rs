//! `AppSettings::tray_reflection`: the "Tray Reflection" slider's head-on reflectance of the
//! glossy tray under the stone, its slider mapping and the one rule every loaded or edited
//! value passes through.

/// `value` limited to `0.0..=1.0`. Anything that is not above zero (NaN, `-0.0`, negatives)
/// is off and reads as `+0.0`, so an off value is always the same bits; `+inf` is `1.0`, like
/// the core tray mirror's own limit.
#[must_use]
pub const fn clamp_tray_reflection(value: f32) -> f32 {
    if value > 0.0 { value.min(1.0) } else { 0.0 }
}

/// The Tray Reflection slider's percent (`0..=100`) as a head-on reflectance, snapped to whole
/// percents and limited to `0.0..=1.0` (NaN means off).
#[must_use]
pub fn tray_reflection_from_percent(percent: f32) -> f32 {
    clamp_tray_reflection(percent.round() / 100.0)
}

/// A reflectance as the Tray Reflection slider's percent (`0.0..=100.0`).
#[must_use]
pub fn percent_from_tray_reflection(reflection: f32) -> f32 {
    clamp_tray_reflection(reflection) * 100.0
}

/// Serde helper: a stored settings file, preset or lighting row leaves out a tray reflection
/// that is off, so the text of a lighting without it stays byte-identical to the earlier
/// format.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
pub(super) const fn is_zero_reflection(value: &f32) -> bool {
    value.to_bits() == 0.0f32.to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slider's whole percents map to a reflectance and back.
    #[test]
    fn the_tray_reflection_slider_snaps_to_whole_percents() {
        assert_eq!(tray_reflection_from_percent(0.0).to_bits(), 0);
        assert_eq!(
            tray_reflection_from_percent(4.2).to_bits(),
            0.04f32.to_bits()
        );
        assert_eq!(
            tray_reflection_from_percent(250.0).to_bits(),
            1.0f32.to_bits()
        );
        assert_eq!(tray_reflection_from_percent(f32::NAN).to_bits(), 0);
        assert_eq!(
            percent_from_tray_reflection(0.5).to_bits(),
            50.0f32.to_bits()
        );
    }

    /// Off is always `+0.0`; the top end is `1.0`, also for infinity.
    #[test]
    fn the_clamp_reads_every_off_value_as_positive_zero() {
        for off in [-0.0f32, 0.0, -0.5, f32::NEG_INFINITY, f32::NAN] {
            assert_eq!(clamp_tray_reflection(off).to_bits(), 0, "{off}");
        }
        assert_eq!(clamp_tray_reflection(0.04).to_bits(), 0.04f32.to_bits());
        assert_eq!(clamp_tray_reflection(1.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(clamp_tray_reflection(7.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(
            clamp_tray_reflection(f32::INFINITY).to_bits(),
            1.0f32.to_bits()
        );
    }

    /// Only the positive-zero bit pattern counts as off for the serde skip.
    #[test]
    fn the_skip_helper_matches_positive_zero_only() {
        assert!(is_zero_reflection(&0.0));
        assert!(!is_zero_reflection(&-0.0));
        assert!(!is_zero_reflection(&0.01));
    }
}
