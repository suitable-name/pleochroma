//! The built-in inclusion presets, keyed by material name.
//!
//! Each preset is a list of [`Family`] in the crystal frame plus the Henyey-Greenstein haze
//! asymmetry `g` of the species (used for the plain haze only while a preset is on; without a
//! preset the fixed `GemMaterial::DEFAULT_SCATTERING_G` stays).
//!
//! # Geometry (cited) and densities (aesthetic)
//!
//! * Corundum: rutile needles lie in the basal plane, parallel to the three a axes, 60 degrees
//!   apart (six-rayed star); a four-ray star means only two sets developed. GIA Gems & Gemology,
//!   Summer 2015 Gem News, "Unusual combination of inclusions in synthetic star sapphire".
//! * Chrysoberyl and alexandrite: needles usually along the crystallographic c axis,
//!   occasionally along a (Schmetzer, Bernhardt, Gilg, J. Gemmology 35(1), 2016). In the app's
//!   frame crystallographic c of chrysoberyl is the `n_alpha` direction (Handbook of Mineralogy:
//!   optical orientation X = c), so the needle runs along [`CrystalDir::AlphaAxis`]; the stone's
//!   Second Axis must be set for the direction to mean something.
//! * Tourmaline, emerald, aquamarine, morganite: growth tubes along c.
//! * Quartz: rutile needles of every orientation.
//! * Moonstone: a broad platelet family (adularescence). Precious opal and labradorite are
//!   haze only here: their color is the Bragg domain structure of [`super::bragg`].
//!
//! No published "typical `sigma_s`" exists; the strengths are aesthetic starting points anchored
//! on the haze values of `GemMaterial::recommended_scattering` (Sapphire 0.25, Ruby 0.3,
//! Tourmaline 0.25, Emerald 0.6, Quartz 0.15), as that function's doc comment says of itself.

use super::{CrystalDir, Family, FamilyKind};

/// A named preset: its families and the haze `g` of the species.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresetDef {
    /// The label shown to the user.
    pub name: &'static str,
    /// The families at density `0.35` (see `spec::density_factor`), in the crystal frame.
    pub families: &'static [Family],
    /// The Henyey-Greenstein asymmetry of the haze while this preset is on.
    pub haze_g: f32,
}

const fn needle(dir: CrystalDir, sigma: f32, p: u8, beta: f32, rho: f32) -> Family {
    Family {
        kind: FamilyKind::Needle,
        dir,
        sigma,
        p,
        lobe: beta,
        rho,
    }
}

const fn basal_trio(sigma: f32, p: u8, beta: f32, rho: f32) -> [Family; 3] {
    [
        needle(CrystalDir::Basal { turn_deg: 0.0 }, sigma, p, beta, rho),
        needle(CrystalDir::Basal { turn_deg: 60.0 }, sigma, p, beta, rho),
        needle(CrystalDir::Basal { turn_deg: 120.0 }, sigma, p, beta, rho),
    ]
}

static FINE_SILK: [Family; 3] = basal_trio(0.10, 2, 0.06, 0.5);
static RUTILE_SILK: [Family; 3] = basal_trio(0.20, 1, 0.10, 0.5);
static STAR_SILK: [Family; 3] = basal_trio(0.15, 1, 0.03, 0.3);
static CATS_EYE_SILK: [Family; 1] = [needle(CrystalDir::AlphaAxis, 0.45, 0, 0.03, 0.4)];
static GROWTH_TUBES: [Family; 1] = [needle(CrystalDir::AlongC, 0.15, 0, 0.15, 0.3)];
static RAIN: [Family; 1] = [needle(CrystalDir::AlongC, 0.12, 0, 0.20, 0.2)];
static RAIN_EMERALD: [Family; 1] = [needle(CrystalDir::AlongC, 0.20, 0, 0.20, 0.2)];
static RUTILATED: [Family; 1] = [Family {
    kind: FamilyKind::RandomNeedle,
    dir: CrystalDir::AlongC,
    sigma: 0.12,
    p: 0,
    lobe: 0.05,
    rho: 0.0,
}];
static ADULARESCENCE: [Family; 1] = [Family {
    kind: FamilyKind::Platelet,
    dir: CrystalDir::AlongC,
    sigma: 0.50,
    p: 1,
    lobe: 0.25,
    rho: 0.0,
}];

const fn preset(name: &'static str, families: &'static [Family], haze_g: f32) -> PresetDef {
    PresetDef {
        name,
        families,
        haze_g,
    }
}

static CORUNDUM: [PresetDef; 3] = [
    preset("Fine Silk", &FINE_SILK, 0.5),
    preset("Rutile Silk", &RUTILE_SILK, 0.5),
    preset("Star Silk", &STAR_SILK, 0.5),
];
static CHRYSOBERYL: [PresetDef; 1] = [preset("Cat's-Eye Silk", &CATS_EYE_SILK, 0.5)];
static TOURMALINE: [PresetDef; 1] = [preset("Growth Tubes", &GROWTH_TUBES, 0.4)];
static EMERALD: [PresetDef; 1] = [preset("Rain", &RAIN_EMERALD, 0.35)];
static BERYL: [PresetDef; 1] = [preset("Rain", &RAIN, 0.35)];
static QUARTZ: [PresetDef; 1] = [preset("Rutilated", &RUTILATED, 0.3)];
static MOONSTONE: [PresetDef; 1] = [preset("Adularescence", &ADULARESCENCE, 0.2)];

/// The materials that have an explicit entry with no preset: haze only, as today.
const HAZE_ONLY: &[&str] = &[
    "Diamond",
    "Zircon",
    "Topaz",
    "Spinel",
    "Tanzanite",
    "Amethyst",
    "Citrine",
    "Ametrine",
    "Peridot",
    "Pyrope Garnet",
    "Almandine Garnet",
    "Spessartine Garnet",
    "Color-Change Garnet (Pyrope-Spessartine)",
    "Grossular Garnet (Tsavorite)",
    "Andradite Garnet (Demantoid)",
    "Benitoite",
    "Andalusite",
    "Synthetic Moissanite",
    "Cubic Zirconia",
    "YAG",
    "GGG",
    "Glass (N-BK7)",
    "Glass (F2)",
    "Opal",
    "Rutile",
    // Their color is the domain structure of `bragg`, not an oriented family.
    "Precious Opal (White)",
    "Precious Opal (Black)",
    "Labradorite",
];

/// One material per catalogue arm that holds presets.
///
/// A caller can walk every preset name without knowing the arms. Kept beside [`catalogue_arm`]; the test
/// `preset_materials_cover_every_arm_with_presets` fails when an arm is added and not listed.
pub const PRESET_MATERIALS: [&str; 7] = [
    "Sapphire",
    "Chrysoberyl (Yellow)",
    "Tourmaline",
    "Emerald",
    "Aquamarine",
    "Quartz",
    "Moonstone",
];

/// The explicit catalogue arm of a material: its presets (an empty slice for a material that
/// is haze only), or `None` for a name with no arm at all.
///
/// The `None` case is what `tests::every_builtin_has_an_explicit_catalogue_arm` pins (the
/// `recommended_scattering_arm` pattern): a catch-all would hide a built-in that was forgotten.
#[must_use]
pub fn catalogue_arm(material_name: &str) -> Option<&'static [PresetDef]> {
    match material_name {
        "Sapphire" | "Ruby" => Some(&CORUNDUM),
        "Chrysoberyl (Yellow)" | "Alexandrite" => Some(&CHRYSOBERYL),
        "Tourmaline" => Some(&TOURMALINE),
        "Emerald" => Some(&EMERALD),
        "Aquamarine" | "Morganite" => Some(&BERYL),
        "Quartz" => Some(&QUARTZ),
        "Moonstone" => Some(&MOONSTONE),
        name if HAZE_ONLY.contains(&name) => Some(&[]),
        _ => None,
    }
}

/// The presets offered for `material_name` (empty for haze only and for an unknown name).
#[must_use]
pub fn presets_for(material_name: &str) -> &'static [PresetDef] {
    catalogue_arm(material_name).unwrap_or(&[])
}

/// The preset called `preset_name` (ignoring ASCII case) for `material_name`.
#[must_use]
pub fn find_preset(material_name: &str, preset_name: &str) -> Option<&'static PresetDef> {
    presets_for(material_name)
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(preset_name))
}
