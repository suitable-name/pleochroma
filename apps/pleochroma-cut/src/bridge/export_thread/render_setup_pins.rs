//! Identity pins for the render-setup helpers shared with the browser app through
//! `pleochroma::render_setup`: the resolved `GemMaterial` for several materials x
//! override combinations, the ICC profile bytes per `ColorSpace`, the PNG bytes
//! [`save_png`] writes for a fixed RGBA buffer, `hash_planes` for fixed plane sets, the
//! frosted-girdle classification and stone-width measurement for fixed designs, the
//! `Backdrop` settings representation, and the sample-count slider mapping.
//!
//! Every value below was recorded BEFORE the helpers moved out of this crate, through
//! the desktop's own paths (which are re-exports now), so a passing run proves the move
//! changed nothing. Hashes are 64-bit FNV-1a over a stable byte form (the raw bytes, or
//! the `Debug` dump for a struct), so they do not depend on `std`'s hasher.

use super::{icc_profile, tonemap_png::save_png};
use crate::{
    bridge::{
        frame_cache::{girdle_finish::GirdleFinishCache, stone_width::StoneWidthCache},
        render_thread::{
            MaterialOverrides, apply_material_overrides, hash_planes, resolve_material,
            resolve_material_with_override,
        },
    },
    gui::render::sample_scale,
    settings::model::Backdrop,
};
use glam::Vec3;
use pleochroma::{
    color::ColorSpace,
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
    optics::materials::GemMaterial,
};
use std::fmt::Write as _;

const SPACES: [ColorSpace; 4] = [
    ColorSpace::Srgb,
    ColorSpace::DisplayP3,
    ColorSpace::Rec2020,
    ColorSpace::AcesCg,
];

fn fnv1a(bytes: &[u8]) -> u64 {
    pleochroma_solid::mesh_cache::fnv1a_64(bytes.iter().copied())
}

fn override_combos() -> [MaterialOverrides; 6] {
    let off = MaterialOverrides {
        inclusion_sigma_s: 0.0,
        c_axis_override: None,
        alpha_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm: 0.0,
    };
    let axis = Vec3::new(0.3, 0.4, 0.866).normalize();
    [
        off,
        MaterialOverrides {
            inclusion_sigma_s: 0.35,
            ..off
        },
        MaterialOverrides {
            c_axis_override: Some(axis),
            ..off
        },
        MaterialOverrides {
            edge_rounding_radius: 0.02,
            ..off
        },
        MaterialOverrides {
            stone_width_mm: 6.5,
            ..off
        },
        MaterialOverrides {
            inclusion_sigma_s: 0.35,
            c_axis_override: Some(axis),
            alpha_axis_override: None,
            edge_rounding_radius: 0.02,
            stone_width_mm: 6.5,
        },
    ]
}

/// One `Debug` dump per (material name x override combo), hashed per combo.
fn material_dump_hashes() -> Vec<u64> {
    let materials = GemMaterial::all_materials();
    let mut custom = GemMaterial::emerald();
    custom.name = "Custom Green".to_string();
    let customs = vec![custom];
    let mut forced = GemMaterial::sapphire();
    forced.name = "Editor Override".to_string();
    let planes = StandardGemCuts::standard_round_brilliant();

    override_combos()
        .iter()
        .map(|overrides| {
            let mut dump = String::new();
            for name in [
                "Diamond",
                "sapphire",
                "Emerald",
                "custom green",
                "no such stone",
            ] {
                // `resolve_material` refuses (`None`) rather than silently substituting
                // `materials[0]` (Diamond) for an unrecognized name like "no such
                // stone" -- see its own doc comment. Dumped as a fixed marker instead
                // of a `GemMaterial` `Debug` string, so the refusal itself is still
                // part of what this hash pins, without fabricating a material to hand
                // `apply_material_overrides` overrides it was never asked to apply.
                match resolve_material(&materials, &customs, name) {
                    Some(base) => {
                        let applied = apply_material_overrides(
                            base,
                            overrides,
                            &planes,
                            &mut StoneWidthCache::new(),
                        );
                        let _ = writeln!(dump, "{applied:?}");
                    }
                    None => dump.push_str("<unresolved>\n"),
                }
            }
            let base =
                resolve_material_with_override(&materials, &customs, Some(&forced), "Diamond")
                    .expect("an explicit forced override always resolves");
            let applied =
                apply_material_overrides(base, overrides, &planes, &mut StoneWidthCache::new());
            let _ = writeln!(dump, "{applied:?}");
            fnv1a(without_zoning_field(&dump).as_bytes())
        })
        .collect()
}

/// The dump with `GemMaterial`'s `zoning` and `alpha_axis` fields projected out.
///
/// `GemMaterial` derives `Debug` and its `zoning` field is the LAST field, so every material
/// prints a trailing `, zoning: None }` that the pinned constants predate. No other dumped type
/// carries a field of that name. Removing exactly that text gives the pinned bytes. A material
/// that really carries zones prints `zoning: Some(..)`, is left alone, and fails the hash
/// comparison, which is what the pin is for. The crystal-axis `alpha_axis` field (2026-10-21)
/// is handled the same way: `alpha_axis: None` is projected out, a set second axis is not.
fn without_zoning_field(dump: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(
        dump.replace(", zoning: None", "")
            .replace(", alpha_axis: None", ""),
    )
}

/// The hashes of [`material_dump_hashes`] (through [`without_zoning_field`]).
const MATERIAL_DUMP_HASHES: [u64; 6] = [
    13_049_985_679_120_865_014,
    11_575_449_901_402_516_878,
    2_951_040_454_685_933_178,
    12_399_276_284_270_091_646,
    9_376_709_567_068_427_923,
    1_008_369_690_435_516_343,
];

#[test]
fn resolved_material_dumps_are_pinned() {
    // Re-recorded 2026-09-28: `crates/pleochroma::optics::materials` changed under
    // this pin since it was last recorded (an unrelated, legitimate fix elsewhere in
    // the workspace, not a `render_setup`/desktop-side regression) -- this pin only
    // ever guards the desktop's own re-export paths against silently changing
    // material resolution, not upstream `pleochroma` material physics, so it is
    // re-baselined to the crate's current, real output rather than papering over a
    // stale assertion.
    //
    // Re-pinned 2026-10-08 for the absorption unit tag (`GemMaterial.absorption_unit` is in
    // every `Debug` dump, so all six combos move) and the `(W / 7) / K` ModelUnit scale,
    // K = 2.52 (`MODEL_UNIT_FACE_UP_PATH`).
    //
    // Re-pinned 2026-10-22 (render physics campaign, owner's run): every `GemMaterial` dump
    // gained the per-axis curve field and the inclusion set, and Sapphire, Ruby and Synthetic
    // Moissanite carry their measured extraordinary-index curves.
    //
    // Re-pinned 2026-10-27 (owner's Linux run): `MODEL_UNIT_FACE_UP_PATH` moved from 2.52 to the
    // measured K = 2.83, so every `ModelUnit` material's `absorption_path_scale = (W / 7) / K`
    // (printed in each `Debug` dump) changed and all six combos move. The Windows run confirms
    // the same values next.
    assert_eq!(material_dump_hashes(), MATERIAL_DUMP_HASHES);
}

#[test]
fn icc_profile_bytes_are_pinned() {
    let got: Vec<(usize, u64)> = SPACES
        .iter()
        .map(|&space| {
            let bytes = icc_profile::build(space);
            (bytes.len(), fnv1a(&bytes))
        })
        .collect();
    // The profile's copyright text names the crate (`pleochroma::color::space`), so a crate
    // rename changes these bytes and needs a re-pin.
    assert_eq!(
        got,
        [
            (2576, 15_151_355_189_953_627_717),
            (2584, 3_399_358_691_760_220_469),
            (2580, 9_952_526_404_351_897_998),
            (2580, 4_523_072_377_019_198_032),
        ]
    );
}

#[test]
fn save_png_bytes_are_pinned() {
    let (width, height) = (7u32, 5u32);
    let rgba: Vec<u8> = (0..width * height * 4)
        .map(|i| (i * 37 + 11).to_le_bytes()[0])
        .collect();
    let dir = std::env::temp_dir().join(format!(
        "pleochroma-cut-render-setup-pins-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let got: Vec<(usize, u64)> = SPACES
        .iter()
        .map(|&space| {
            let path = dir.join(format!("{space:?}.png"));
            save_png(&path, width, height, &rgba, space).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            (bytes.len(), fnv1a(&bytes))
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        got,
        [
            // Re-pinned 2026-10-29 with the ICC profiles above (the crate's new name in the
            // embedded profile's copyright text; the sRGB file embeds none and keeps its bytes).
            (213, 10_826_987_011_992_901_810),
            (2597, 4_883_349_284_706_428_323),
            (2611, 12_790_413_013_770_100_845),
            (2613, 14_413_600_939_343_022_441),
        ]
    );
}

fn fixed_plane_sets() -> [Vec<GpuFacetPlane>; 3] {
    [
        StandardGemCuts::standard_round_brilliant(),
        StandardGemCuts::emerald_cut(),
        Vec::new(),
    ]
}

#[test]
fn hash_planes_outputs_are_pinned() {
    let got: Vec<u64> = fixed_plane_sets().iter().map(|p| hash_planes(p)).collect();
    assert_eq!(
        got,
        [
            9_156_382_085_212_162_079,
            12_772_034_658_704_493_466,
            8_556_445_246_977_061_536,
        ]
    );
}

#[test]
fn girdle_finish_and_stone_width_outputs_are_pinned() {
    let designs = [
        StandardGemCuts::standard_round_brilliant(),
        StandardGemCuts::emerald_cut(),
    ];
    let mut girdle = GirdleFinishCache::new();
    let mut width = StoneWidthCache::new();
    let got: Vec<(usize, u64, Option<u64>)> = designs
        .iter()
        .map(|planes| {
            let finishes = girdle.ensure(planes);
            let finish_hash = fnv1a(format!("{finishes:?}").as_bytes());
            (
                finishes.len(),
                finish_hash,
                width.ensure(planes).map(f64::to_bits),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                74,
                13_475_232_019_886_019_249,
                Some(4_611_686_018_452_307_957)
            ),
            (
                34,
                13_461_213_193_500_309_065,
                Some(4_609_884_578_683_813_888)
            ),
        ]
    );
}

/// The backdrop pins after the lit-tray model (2026-10-20): the albedo of each choice (the
/// tray radiance itself follows the lighting, so it is no longer one number per choice),
/// the pill indices (Black appended as 3), the fallback and the default (White), and the
/// on-disk TOML names. The old test pinned the card levels `0.0 / 0.23 / 8.0`, the
/// index fallback Grey and the default Grey; those moved on purpose, and saved settings
/// (which always write the name) are untouched.
#[test]
fn backdrop_albedos_indices_default_and_settings_form_are_pinned() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Wrapper {
        backdrop: Backdrop,
    }
    let all = [
        Backdrop::AsLit,
        Backdrop::Grey,
        Backdrop::White,
        Backdrop::Black,
    ];
    let albedo_bits: Vec<u32> = all
        .into_iter()
        .map(|b| {
            pleochroma::render_setup::Backdrop::from_index(b.index())
                .albedo()
                .to_bits()
        })
        .collect();
    assert_eq!(
        albedo_bits,
        [
            0.0_f32.to_bits(),
            0.18_f32.to_bits(),
            0.90_f32.to_bits(),
            0.02_f32.to_bits()
        ]
    );
    let indices: Vec<i32> = all.into_iter().map(Backdrop::index).collect();
    assert_eq!(indices, [0, 1, 2, 3]);
    for index in -1..5 {
        let back = Backdrop::from_index(index);
        assert_eq!(
            back.index(),
            if (0..=3).contains(&index) { index } else { 2 }
        );
    }
    assert_eq!(Backdrop::default(), Backdrop::White);
    // As Lit resolves to exactly 0.0 under every preset (the preset's own ground).
    assert_eq!(
        Backdrop::AsLit
            .resolve(
                pleochroma::optics::raytracer::LightingPreset::default(),
                0.8,
                1.2
            )
            .to_bits(),
        0.0_f32.to_bits()
    );
    for backdrop in all {
        let text = toml::to_string(&Wrapper { backdrop }).unwrap();
        assert_eq!(text, format!("backdrop = \"{backdrop:?}\"\n"));
        let back: Wrapper = toml::from_str(&text).unwrap();
        assert_eq!(back, Wrapper { backdrop });
    }
}

/// Prints the resolved tray radiance (bits and value) of every choice under the preview
/// rig/pose and the live default rig/pose, for the owner to read off after the first run and
/// pin. Ignored: the values are measurements of the tray model
/// (`pleochroma::optics::raytracer::tray_radiance`), not decisions. Run with
/// `cargo test -p pleochroma-cut backdrop_resolved_trays_print -- --ignored --nocapture`.
#[test]
#[ignore = "prints the resolved tray radiance per choice for the owner to pin"]
fn backdrop_resolved_trays_print() {
    use pleochroma::optics::raytracer::LightingPreset;
    let poses = [
        (
            "preview WhiteTray 0.85/0.95",
            LightingPreset::WhiteTray,
            0.85,
            0.95,
        ),
        (
            "preview-pose IsoHemisphere",
            LightingPreset::IsoHemisphere,
            0.85,
            0.95,
        ),
        (
            "live default LightTent 48/72 deg",
            LightingPreset::default(),
            48.0_f32.to_radians(),
            72.0_f32.to_radians(),
        ),
    ];
    for (name, preset, yaw, pitch) in poses {
        for backdrop in [
            Backdrop::AsLit,
            Backdrop::Grey,
            Backdrop::White,
            Backdrop::Black,
        ] {
            let tray = backdrop.resolve(preset, yaw, pitch);
            println!("{name} {backdrop:?}: {tray} bits={}", tray.to_bits());
        }
    }
}

#[test]
fn sample_scale_mapping_is_pinned() {
    let counts: Vec<u32> = (0..16).map(sample_scale::exponent_to_count).collect();
    assert_eq!(
        counts,
        [
            8, 8, 8, 8, 16, 32, 64, 128, 256, 512, 1024, 1024, 1024, 1024, 1024, 1024
        ]
    );
    let exps: Vec<u32> = [0, 1, 8, 9, 300, 511, 512, 1024, 4096, u32::MAX]
        .into_iter()
        .map(sample_scale::count_to_exponent)
        .collect();
    assert_eq!(exps, [3, 3, 3, 3, 8, 8, 9, 10, 10, 10]);
    assert_eq!(sample_scale::exponent_to_count_bounded(9, 7, 13), 512);
    assert_eq!(sample_scale::count_to_exponent_bounded(100_000, 7, 13), 13);
    assert_eq!(
        (sample_scale::MIN_EXPONENT, sample_scale::MAX_EXPONENT),
        (3, 10)
    );
}
