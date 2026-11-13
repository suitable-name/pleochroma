//! Storage-buffer layouts for the megakernel and Ray-Traced GPU engines.

use super::{
    TRAY_SHADOW_BAND_WIDTH_NM, TRAY_SHADOW_BANDS, TRAY_SHADOW_FIRST_CENTER_NM, TrayShadowMap,
};
use crate::optics::raytracer::environment::TrayMirror;

/// Optional megakernel storage binding, outside all existing binding ranges.
pub const TRAY_SHADOW_BINDING: u32 = 190;

/// Packs a 16-word header followed by texel-major, band-fastest spectral ratios.
#[must_use]
pub fn pack_gpu_words(map: &TrayShadowMap) -> Vec<f32> {
    let mut words = vec![0.0; 16];
    words[..10].copy_from_slice(&[
        1.0,
        1.0,
        map.resolution as f32,
        TRAY_SHADOW_BANDS as f32,
        map.plane_y,
        map.center[0],
        map.center[1],
        map.half_extent,
        TRAY_SHADOW_FIRST_CENTER_NM,
        TRAY_SHADOW_BAND_WIDTH_NM,
    ]);
    words.extend_from_slice(map.factors());
    words
}

/// Disabled header for a variant with no active map.
#[must_use]
pub fn pack_disabled_gpu_words() -> Vec<f32> {
    vec![0.0; 16]
}

/// Header word holding the Tray Reflection's on flag (1.0 on, 0.0 off).
pub const TRAY_MIRROR_WORD_ON: usize = 10;
/// Header word holding the Tray Reflection's head-on reflectance `R0`.
pub const TRAY_MIRROR_WORD_R0: usize = 11;
/// Header word holding the Tray Reflection's plane height.
pub const TRAY_MIRROR_WORD_PLANE_Y: usize = 12;

/// The binding 190 buffer for a scene with a stone shadow, a Tray Reflection, both or neither.
///
/// The shadow part is [`pack_gpu_words`] (its flag in word 0, its texels after the header), or
/// the disabled header when `map` is `None`: a mirror-only scene uploads a header-only buffer
/// with the shadow flag off and no texels. A mirror sets word 10 to 1.0, word 11 to its head-on
/// reflectance `R0` and word 12 to its plane height; words 13 to 15 stay 0. With `mirror` `None`
/// the words are exactly those of [`pack_gpu_words`] / [`pack_disabled_gpu_words`].
#[must_use]
pub fn pack_gpu_words_with_mirror(
    map: Option<&TrayShadowMap>,
    mirror: Option<TrayMirror>,
) -> Vec<f32> {
    let mut words = map.map_or_else(pack_disabled_gpu_words, pack_gpu_words);
    if let Some(mirror) = mirror {
        words[TRAY_MIRROR_WORD_ON] = 1.0;
        words[TRAY_MIRROR_WORD_R0] = mirror.reflectance();
        words[TRAY_MIRROR_WORD_PLANE_Y] = mirror.plane_y();
    }
    words
}

/// Packs one RGB ratio per texel into the Ray-Traced environment buffer's tail.
#[must_use]
pub fn pack_whitted_rgb(map: &TrayShadowMap) -> Vec<[f32; 4]> {
    map.factors_rgb()
        .iter()
        .map(|rgb| [rgb[0], rgb[1], rgb[2], 0.0])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::raytracer::environment::tray_shadow::TrayShadowKey;

    fn map() -> TrayShadowMap {
        TrayShadowMap::from_parts(
            -0.9,
            [0.1, 0.2],
            3.0,
            1,
            vec![0.5; TRAY_SHADOW_BANDS],
            vec![[0.5; 3]],
            TrayShadowKey(7),
        )
    }

    fn mirror() -> TrayMirror {
        TrayMirror::new(-0.9, 0.04).expect("valid mirror")
    }

    /// The tray unit of the megakernel variant, which reads the header words by literal index.
    const TRAY_SHADOW_WGSL: &str =
        include_str!("../../../../renderer/shaders/transport_bounce/tray_shadow.wgsl");

    #[test]
    fn the_mirror_words_are_the_indices_the_wgsl_reads() {
        for literal in [
            format!("tray_shadow[{TRAY_MIRROR_WORD_ON}] != 0.0"),
            format!("let r0 = tray_shadow[{TRAY_MIRROR_WORD_R0}];"),
            format!("let plane_y = tray_shadow[{TRAY_MIRROR_WORD_PLANE_Y}];"),
        ] {
            assert!(
                TRAY_SHADOW_WGSL.contains(&literal),
                "`{literal}` is missing from tray_shadow.wgsl"
            );
        }
    }

    #[test]
    fn without_a_mirror_the_words_are_the_shadow_only_words() {
        let map = map();
        assert_eq!(
            pack_gpu_words_with_mirror(Some(&map), None),
            pack_gpu_words(&map)
        );
        assert_eq!(
            pack_gpu_words_with_mirror(None, None),
            pack_disabled_gpu_words()
        );
        let words = pack_gpu_words(&map);
        assert!(words[10..16].iter().all(|&w| w.to_bits() == 0));
    }

    #[test]
    fn a_mirror_only_scene_is_a_header_only_buffer_with_the_shadow_off() {
        let words = pack_gpu_words_with_mirror(None, Some(mirror()));
        assert_eq!(words.len(), 16);
        assert_eq!(words[0].to_bits(), 0.0_f32.to_bits());
        assert!(words[1..10].iter().all(|&w| w.to_bits() == 0));
        assert_eq!(words[10].to_bits(), 1.0_f32.to_bits());
        assert_eq!(words[11].to_bits(), 0.04_f32.to_bits());
        assert_eq!(words[12].to_bits(), (-0.9_f32).to_bits());
        assert!(words[13..16].iter().all(|&w| w.to_bits() == 0));
    }

    #[test]
    fn a_shadow_and_a_mirror_keep_the_shadow_words_and_texels() {
        let map = map();
        let both = pack_gpu_words_with_mirror(Some(&map), Some(mirror()));
        let shadow_only = pack_gpu_words(&map);
        assert_eq!(both.len(), shadow_only.len());
        assert_eq!(both[..10], shadow_only[..10]);
        assert_eq!(both[16..], shadow_only[16..]);
        assert_eq!(both[0].to_bits(), 1.0_f32.to_bits());
        assert_eq!(both[10].to_bits(), 1.0_f32.to_bits());
        assert_eq!(both[11].to_bits(), 0.04_f32.to_bits());
        assert_eq!(both[12].to_bits(), (-0.9_f32).to_bits());
        assert!(both[13..16].iter().all(|&w| w.to_bits() == 0));
    }
}
