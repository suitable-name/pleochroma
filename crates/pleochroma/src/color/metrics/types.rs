//! Shared result type and elevation-sampling grid for the metrics module.

/// GIA/AGSL-style optical gemological metrics for one camera/light pose.
#[derive(Debug, Clone, Copy)]
pub struct GemOpticalMetrics {
    /// Percentage of incident rays visibly returned to the observer.
    pub brilliance_pct: f32,
    /// Display-scaled energy-weighted F-line/C-line angular separation.
    pub fire_index: f32,
    /// Combined spatial+temporal light-return contrast, 0-100.
    pub scintillation_pct: f32,
    /// Percentage of incident rays that leaked out through the pavilion.
    pub windowing_pct: f32,
    /// Percentage of incident rays trapped, absorbed, or not visibly returned.
    pub extinction_pct: f32,
    /// Doubling of an anisotropic (birefringent) stone: the mean angular separation, in
    /// arc-minutes, of the ordinary and extraordinary images of the returned light.
    ///
    /// Pairs are ray pairs that leave through the same facet, weighted like Fire (the product
    /// of the two modes' Fresnel transmittances and exit cosines). `0.0` for an isotropic
    /// stone, when no pair qualifies, and for the neutral placeholders. Uses the d-line traces
    /// only, so the Fire-less passes report it too.
    pub doubling_arcmin: f32,
}

/// 19 tilt ELEVATION sample points (camera pitch, not tilt-from-table-up) in exact 5°
/// steps across 0° to 90°.
///
/// Another parameterisation than [`super::profile::TILT_ANGLES_DEG`]'s `-90..=+90°`
/// tilt-away-from-table-up domain: `0°` here is edge-on (camera pitch 0) and `90°` is
/// table-up (camera pitch 90), the reverse of where those poses sit there. Do not unify the
/// two domains: [`super::profile::evaluate_angular_profile_at_azimuth`] sweeps elevation at
/// one fixed azimuth and is not a coarser form of the full-axis function.
pub const PROFILE_ANGLES_DEG: [f32; 19] = [
    0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 55.0, 60.0, 65.0, 70.0, 75.0,
    80.0, 85.0, 90.0,
];
