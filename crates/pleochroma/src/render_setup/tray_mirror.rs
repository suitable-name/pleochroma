//! Resolves the Tray Reflection of a render: the glossy plane under the stone.
//!
//! The mirror sits on the Stone Shadow's plane (at the culet, see [`tray_plane_y`]) and has the
//! Stone Shadow's scope: a studio light with a backdrop, so not As Lit, an HDR panorama, the
//! contrast view or a UV lamp. Unlike the shadow there is nothing to precompute, so resolving is
//! cheap, needs no cancellation or progress, and is available on every build.

use crate::{
    geometry::tool::StoneGeometry,
    optics::raytracer::{
        EnvironmentSource, LightingPreset,
        environment::{TrayMirror, tray_plane_y},
    },
};

/// Why a requested tray reflection is not available.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayMirrorOff {
    /// The scene has no lit backdrop: an HDR panorama, As Lit, the contrast view or a UV lamp.
    NeedsStudioBackdrop,
    /// The geometry does not bound a usable stone.
    DegenerateStone,
}

impl TrayMirrorOff {
    /// Sentence shown when the requested reflection is unavailable.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NeedsStudioBackdrop => "Tray reflection needs a studio light with a backdrop.",
            Self::DegenerateStone => "Tray reflection needs a closed, nondegenerate stone.",
        }
    }
}

/// The mirror for this stone at head-on reflectance `reflection`.
///
/// `Ok(None)` when `reflection` is 0 (or not above 0, NaN included): the reflection is off and
/// nothing about `environment` or `geom` is checked.
///
/// A value above 1 clamps to 1 (a perfect mirror). Callers attach the result with
/// [`EnvironmentSource::with_tray_mirror`]. The plane is the Stone Shadow's, so a render with
/// both has one tray.
///
/// # Errors
/// Returns the lighting restriction (an HDR panorama, the contrast view, a UV lamp, or a scene
/// without a backdrop) or the geometry restriction.
pub fn tray_mirror_for(
    reflection: f32,
    environment: EnvironmentSource<'_>,
    geom: StoneGeometry<'_>,
) -> Result<Option<TrayMirror>, TrayMirrorOff> {
    if reflection.is_nan() || reflection <= 0.0 {
        return Ok(None);
    }
    let EnvironmentSource::Studio {
        preset, backdrop, ..
    } = environment
    else {
        return Err(TrayMirrorOff::NeedsStudioBackdrop);
    };
    if preset == LightingPreset::Aset || preset.is_uv_lamp() || backdrop <= 0.0 {
        return Err(TrayMirrorOff::NeedsStudioBackdrop);
    }
    let plane_y = tray_plane_y(geom).ok_or(TrayMirrorOff::DegenerateStone)?;
    TrayMirror::new(plane_y, reflection)
        .map(Some)
        .ok_or(TrayMirrorOff::DegenerateStone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{geometry::cuts::StandardGemCuts, renderer::env_map::EnvironmentMap};

    fn studio(preset: LightingPreset, backdrop: f32) -> EnvironmentSource<'static> {
        preset.studio(1.0, 0.3, 0.9).with_backdrop(backdrop)
    }

    #[test]
    fn zero_is_off_whatever_the_scene() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let geom = StoneGeometry::planes_only(&planes);
        let map = EnvironmentMap::from_rgb(4, 2, vec![[0.5; 3]; 8]).expect("valid map");
        for reflection in [0.0, -0.5, f32::NAN] {
            assert_eq!(
                tray_mirror_for(reflection, studio(LightingPreset::LightTent, 0.23), geom),
                Ok(None)
            );
            // Not even the scope is checked while the reflection is off.
            assert_eq!(
                tray_mirror_for(reflection, EnvironmentSource::hdr(&map), geom),
                Ok(None)
            );
            assert_eq!(
                tray_mirror_for(reflection, studio(LightingPreset::Aset, 0.0), geom),
                Ok(None)
            );
        }
    }

    #[test]
    fn scope_checks_match_the_stone_shadow() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let geom = StoneGeometry::planes_only(&planes);
        let map = EnvironmentMap::from_rgb(4, 2, vec![[0.5; 3]; 8]).expect("valid map");
        let off = Err(TrayMirrorOff::NeedsStudioBackdrop);
        assert_eq!(
            tray_mirror_for(0.04, EnvironmentSource::hdr(&map), geom),
            off
        );
        assert_eq!(
            tray_mirror_for(0.04, studio(LightingPreset::Aset, 0.23), geom),
            off
        );
        assert_eq!(
            tray_mirror_for(0.04, studio(LightingPreset::UvLamp365, 0.23), geom),
            off
        );
        assert_eq!(
            tray_mirror_for(0.04, studio(LightingPreset::UvLamp395, 0.23), geom),
            off
        );
        assert_eq!(
            tray_mirror_for(0.04, studio(LightingPreset::LightTent, 0.0), geom),
            off
        );
    }

    #[test]
    fn a_degenerate_stone_is_declined() {
        assert_eq!(
            tray_mirror_for(
                0.04,
                studio(LightingPreset::LightTent, 0.23),
                StoneGeometry::planes_only(&[])
            ),
            Err(TrayMirrorOff::DegenerateStone)
        );
    }

    #[test]
    fn a_lit_scene_gets_the_shadow_plane_and_the_clamped_reflectance() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let geom = StoneGeometry::planes_only(&planes);
        let plane = tray_plane_y(geom).expect("valid stone");
        for preset in [LightingPreset::LightTent, LightingPreset::DaylightSun] {
            let mirror = tray_mirror_for(0.04, studio(preset, 0.23), geom)
                .expect("studio with backdrop")
                .expect("reflection above zero");
            assert_eq!(mirror.plane_y().to_bits(), plane.to_bits());
            assert_eq!(mirror.reflectance().to_bits(), 0.04_f32.to_bits());
        }
        let full = tray_mirror_for(7.0, studio(LightingPreset::LightTent, 0.23), geom)
            .expect("ok")
            .expect("on");
        assert_eq!(full.reflectance().to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn the_messages_are_exact() {
        assert_eq!(
            TrayMirrorOff::NeedsStudioBackdrop.message(),
            "Tray reflection needs a studio light with a backdrop."
        );
        assert_eq!(
            TrayMirrorOff::DegenerateStone.message(),
            "Tray reflection needs a closed, nondegenerate stone."
        );
    }
}
