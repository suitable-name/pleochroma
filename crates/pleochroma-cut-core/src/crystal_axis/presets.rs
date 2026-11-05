//! The orientation presets the Stone tab offers: three geometry presets that need only the
//! design, and the color-named presets of a trichroic material.
//!
//! A color-named preset says what the cutter will SEE through the table, never which crystal
//! direction runs where: the ray along the viewing direction is the one the cutter does not
//! see, so "Best Blue Face-Up" puts the principal direction that is NOT blue through the
//! table. The names are derived from the material's own absorption: the face-up color is
//! worked out with each principal direction through the table
//! ([`pleochroma::optics::materials::face_up_swatch`]) and the three results are ranked.
//!
//! - Tanzanite: the lowest-hue result is **Best Blue Face-Up**, the highest-hue one
//!   **Violet Face-Up**.
//! - Alexandrite: the result whose color moves most between daylight and incandescent light is
//!   **Strongest Color Change**.
//! - Andalusite: the result closest to green is **Best Green Face-Up**, offered only when a
//!   direction really is green (hue within 30 degrees of green and a chroma C* of at least
//!   10); a stone none of whose directions is green gets no preset.
//!
//! Peridot and every other material get none (its pleochroism is weak, see the plan).

use std::sync::{Mutex, OnceLock};

use glam::Vec3;
use pleochroma::{
    color::{
        Illuminant,
        body_color::{delta_e_2000, lab_to_lch},
    },
    optics::{
        birefringence::alpha_axis_from_turn,
        materials::{AbsorptionUnit, GemMaterial, face_up_swatch},
    },
    render_setup::MODEL_UNIT_FACE_UP_PATH,
};

use super::{CrystalAxis, CrystalOrientation};

/// The geometry presets of the Stone tab, in the order the buttons show them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisPreset {
    /// The Crystal Axis along the table normal (tilt 0).
    ThroughTheTable,
    /// The Crystal Axis lying in the table plane (tilt 90), along the model +X axis.
    InTheTablePlane,
    /// The Crystal Axis lying in the table plane along the design's width direction (the
    /// rotating-caliper minimum width), so it is set per design.
    AlongTheWidth,
}

impl AxisPreset {
    /// Every geometry preset, in button order.
    pub const ALL: [Self; 3] = [
        Self::ThroughTheTable,
        Self::InTheTablePlane,
        Self::AlongTheWidth,
    ];

    /// The button label (Title Case).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ThroughTheTable => "Through the Table",
            Self::InTheTablePlane => "In the Table Plane",
            Self::AlongTheWidth => "Along the Width",
        }
    }

    /// One plain sentence for the button's tooltip.
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Self::ThroughTheTable => {
                "Point the crystal axis straight up through the table, so you look along it."
            }
            Self::InTheTablePlane => {
                "Lay the crystal axis flat in the plane of the table, so you look across it."
            }
            Self::AlongTheWidth => {
                "Lay the crystal axis flat in the table plane, along the width of the stone."
            }
        }
    }

    /// The `(tilt, azimuth)` in degrees this preset sets, or `None` for
    /// [`Self::AlongTheWidth`] when the design has no width direction.
    /// `width_azimuth_deg` is [`width_azimuth_deg`] of the design.
    #[must_use]
    pub fn angles(self, width_azimuth_deg: Option<f64>) -> Option<(f64, f64)> {
        match self {
            Self::ThroughTheTable => Some((0.0, 0.0)),
            Self::InTheTablePlane => Some((90.0, 0.0)),
            Self::AlongTheWidth => width_azimuth_deg.map(|azimuth| (90.0, azimuth)),
        }
    }
}

/// The azimuth, in degrees `[0, 180)`, of a design's width direction.
///
/// `width_dir = [x, z]` is (the unit vector of `pleochroma::geometry::stone_metrics::CaliperFrame::width_dir`, in the
/// model's x/z plane). The azimuth runs from `+X` toward `+Z`, like the Crystal Axis's own.
/// `None` for a vector that is not a usable direction.
#[must_use]
pub fn width_azimuth_deg(width_dir: [f64; 2]) -> Option<f64> {
    let [x, z] = width_dir;
    if !(x.is_finite() && z.is_finite()) || x.hypot(z) < 1e-12 {
        return None;
    }
    // A line: folding through the equator gives the canonical azimuth.
    Some(CrystalAxis::from_angles(90.0, z.atan2(x).to_degrees()).azimuth_deg())
}

/// Which principal direction of a biaxial stone a color-named preset puts through the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalDirection {
    /// The smallest index (the `o_ray` band set of a biaxial tensor).
    Alpha,
    /// The middle index.
    Beta,
    /// The largest index, the Crystal Axis.
    Gamma,
}

impl PrincipalDirection {
    /// The three directions, alpha first.
    pub const ALL: [Self; 3] = [Self::Alpha, Self::Beta, Self::Gamma];
}

/// The color-named presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorPresetId {
    /// Tanzanite: the bluest face-up color.
    BestBlueFaceUp,
    /// Tanzanite: the most violet face-up color.
    VioletFaceUp,
    /// Alexandrite: the face-up color that changes most between daylight and incandescent light.
    StrongestColorChange,
    /// Andalusite: the greenest face-up color, when one is green at all.
    BestGreenFaceUp,
}

impl ColorPresetId {
    /// The preset's name (Title Case), as the Stone tab and the cutting sheet show it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::BestBlueFaceUp => "Best Blue Face-Up",
            Self::VioletFaceUp => "Violet Face-Up",
            Self::StrongestColorChange => "Strongest Color Change",
            Self::BestGreenFaceUp => "Best Green Face-Up",
        }
    }
}

/// One color-named preset of a material: the orientation, and the face-up color it gives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorPreset {
    /// Which preset this is.
    pub id: ColorPresetId,
    /// The principal direction it puts through the table.
    pub through_table: PrincipalDirection,
    /// The full orientation (Crystal Axis and Second Axis turn).
    pub orientation: CrystalOrientation,
    /// The face-up color, 8-bit sRGB, under daylight (D65).
    pub swatch_srgb: [u8; 3],
}

/// The orientation that puts `direction` through the table (along `+Y`) of a biaxial stone,
/// the two other directions lying in the table plane.
///
/// Gamma through the table is the Crystal Axis along `+Y`. For alpha and beta the Crystal Axis
/// lies in the table plane (along `+X`) and the Second Axis turn is solved so that alpha runs
/// along `+Y` (alpha through the table) or along `+Z` (beta then runs along `+Y`).
#[must_use]
pub fn orientation_through_table(direction: PrincipalDirection) -> CrystalOrientation {
    match direction {
        PrincipalDirection::Gamma => CrystalOrientation::from_angles(0.0, 0.0, Some(0.0)),
        PrincipalDirection::Alpha => gamma_in_plane_with_alpha_along(Vec3::Y),
        PrincipalDirection::Beta => gamma_in_plane_with_alpha_along(Vec3::Z),
    }
}

/// The orientation with the Crystal Axis in the table plane along `+X` and the Second Axis
/// turned until alpha runs along `target` (a line, so either sign).
fn gamma_in_plane_with_alpha_along(target: Vec3) -> CrystalOrientation {
    let gamma = CrystalOrientation::from_angles(90.0, 0.0, None).gamma();
    let reference = alpha_axis_from_turn(gamma, 0.0);
    let side = gamma.cross(reference);
    // The turn that carries the reference onto `target` if a positive turn is right-handed;
    // its negative covers the other convention, and the one that lands closer wins, so this
    // does not depend on which way `alpha_axis_from_turn` rotates.
    let turn = target.dot(side).atan2(target.dot(reference)).to_degrees();
    let aligned = |turn_deg: f32| alpha_axis_from_turn(gamma, turn_deg).dot(target).abs();
    let best = if aligned(turn) >= aligned(-turn) {
        turn
    } else {
        -turn
    };
    CrystalOrientation::from_angles(90.0, 0.0, Some(f64::from(best)))
}

/// `material` with `orientation` applied to its principal frame.
fn oriented(material: &GemMaterial, orientation: &CrystalOrientation) -> GemMaterial {
    let (gamma, alpha) = orientation.to_override();
    let mut oriented = material.clone();
    oriented.c_axis = gamma;
    oriented.alpha_axis = alpha;
    oriented
}

/// The path, in the unit of the material's absorption numbers, the face-up swatch is shaded
/// over (the render's own rule for a 7 mm stone: one model unit of a per-model-unit material,
/// millimetres for a per-millimetre one over the standard girdle width).
fn swatch_path(material: &GemMaterial) -> f64 {
    match material.absorption_unit {
        AbsorptionUnit::ModelUnit => 1.0,
        AbsorptionUnit::PerMm => f64::from(MODEL_UNIT_FACE_UP_PATH) * 3.5,
    }
}

/// What one principal direction through the table looks like.
struct Choice {
    direction: PrincipalDirection,
    orientation: CrystalOrientation,
    srgb: [u8; 3],
    /// Hue angle of the daylight face-up color, degrees `[0, 360)`.
    hue_deg: f64,
    /// Chroma C* of the daylight face-up color.
    chroma: f64,
    /// CIEDE2000 between the daylight and the incandescent (3200 K) face-up color.
    change: f64,
}

fn choices(material: &GemMaterial) -> Vec<Choice> {
    let path = swatch_path(material);
    PrincipalDirection::ALL
        .into_iter()
        .map(|direction| {
            let orientation = orientation_through_table(direction);
            let stone = oriented(material, &orientation);
            let daylight = face_up_swatch(&stone, path, Illuminant::D65);
            let lamp = face_up_swatch(&stone, path, Illuminant::Planckian(3200.0));
            let [_, chroma, hue_deg] = lab_to_lch(daylight.lab);
            Choice {
                direction,
                orientation,
                srgb: daylight.srgb,
                hue_deg,
                chroma,
                change: delta_e_2000(daylight.lab, lamp.lab),
            }
        })
        .collect()
}

/// The shortest angular distance between two hues, degrees `[0, 180]`.
fn hue_distance(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// The hue of green (CIELAB h*ab, degrees).
const GREEN_HUE_DEG: f64 = 140.0;

/// How far (degrees of hue) a face-up color may sit from green and still be called green.
const GREEN_TOLERANCE_DEG: f64 = 30.0;

/// The least chroma C* a face-up color needs to be called green: a nearly colorless direction
/// has a hue that means nothing.
const GREEN_MIN_CHROMA: f64 = 10.0;

/// Below this hue spread (degrees) the three face-up colors of a material are one color and
/// only the first name is offered.
const MIN_HUE_SPREAD_DEG: f64 = 3.0;

const fn preset_of(id: ColorPresetId, choice: &Choice) -> ColorPreset {
    ColorPreset {
        id,
        through_table: choice.direction,
        orientation: choice.orientation,
        swatch_srgb: choice.srgb,
    }
}

/// The color-named presets of `material`.
///
/// Tanzanite's two, Alexandrite's one, Andalusite's one when a direction is green; nothing for
/// any other material (isotropic, uniaxial, Peridot, a custom material).
/// Derived from the material's tensor, see the module documentation.
#[must_use]
pub fn color_presets(material: &GemMaterial) -> Vec<ColorPreset> {
    if material.biaxial_delta_beta_alpha.is_none() || !material.absorption.is_pleochroic {
        return Vec::new();
    }
    // The presets never read the material's own frame (every choice replaces it), so the cache
    // key is the material with the frame reset; the six spectral integrations run once per
    // distinct material instead of once per stone row, sheet or Verdict.
    let mut key = material.clone();
    key.c_axis = Vec3::Y;
    key.alpha_axis = None;
    let cache = PRESET_CACHE.get_or_init(|| Mutex::new(Vec::new()));
    if let Ok(entries) = cache.lock()
        && let Some((_, presets)) = entries.iter().find(|(cached, _)| *cached == key)
    {
        return presets.clone();
    }
    let presets = compute_color_presets(material);
    if let Ok(mut entries) = cache.lock() {
        if entries.len() >= PRESET_CACHE_LIMIT {
            entries.clear();
        }
        entries.push((key, presets.clone()));
    }
    presets
}

/// How many distinct materials the preset cache holds before it starts over.
const PRESET_CACHE_LIMIT: usize = 16;

/// Presets already worked out, keyed by the material with its frame reset.
static PRESET_CACHE: OnceLock<Mutex<PresetCacheEntries>> = OnceLock::new();

/// The cached presets, one entry per material.
type PresetCacheEntries = Vec<(GemMaterial, Vec<ColorPreset>)>;

fn compute_color_presets(material: &GemMaterial) -> Vec<ColorPreset> {
    let by_hue = |a: &&Choice, b: &&Choice| a.hue_deg.total_cmp(&b.hue_deg);
    match material.name.as_str() {
        "Tanzanite" => {
            let all = choices(material);
            let (Some(blue), Some(violet)) = (all.iter().min_by(by_hue), all.iter().max_by(by_hue))
            else {
                return Vec::new();
            };
            let mut presets = vec![preset_of(ColorPresetId::BestBlueFaceUp, blue)];
            if violet.hue_deg - blue.hue_deg >= MIN_HUE_SPREAD_DEG {
                presets.push(preset_of(ColorPresetId::VioletFaceUp, violet));
            }
            presets
        }
        "Alexandrite" => {
            let all = choices(material);
            all.iter()
                .max_by(|a, b| a.change.total_cmp(&b.change))
                .map(|change| vec![preset_of(ColorPresetId::StrongestColorChange, change)])
                .unwrap_or_default()
        }
        "Andalusite" => {
            // Only a direction that is green. The built-in (since 2026-11-03 0.65 x the Fe-Ti
            // crystal plus 0.35 x the Greener variant) has one: through alpha, beta and gamma show
            // a pale yellow-green; the Fe-Ti crystal alone had none.
            let all = choices(material);
            all.iter()
                .filter(|choice| {
                    choice.chroma >= GREEN_MIN_CHROMA
                        && hue_distance(choice.hue_deg, GREEN_HUE_DEG) <= GREEN_TOLERANCE_DEG
                })
                .min_by(|a, b| {
                    hue_distance(a.hue_deg, GREEN_HUE_DEG)
                        .total_cmp(&hue_distance(b.hue_deg, GREEN_HUE_DEG))
                })
                .map(|green| vec![preset_of(ColorPresetId::BestGreenFaceUp, green)])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// The color-named preset `id` of `material`, if it has it.
#[must_use]
pub fn color_preset(material: &GemMaterial, id: ColorPresetId) -> Option<ColorPreset> {
    color_presets(material)
        .into_iter()
        .find(|preset| preset.id == id)
}

/// The angle, in degrees `[0, 90]`, between a principal direction and the table normal.
///
/// The direction is `direction` of a stone
/// oriented as `orientation`. A stone with no Second Axis set has the
/// material's own completion of alpha (`alpha_axis_from_turn(gamma, 0)`).
#[must_use]
pub fn principal_angle_to_table_deg(
    orientation: &CrystalOrientation,
    direction: PrincipalDirection,
) -> f64 {
    let gamma = orientation.gamma();
    let alpha = orientation
        .alpha()
        .unwrap_or_else(|| alpha_axis_from_turn(gamma, 0.0));
    let beta = gamma.cross(alpha);
    let axis = match direction {
        PrincipalDirection::Alpha => alpha,
        PrincipalDirection::Beta => beta,
        PrincipalDirection::Gamma => gamma,
    };
    let along = f64::from(axis.normalize_or_zero().y.abs().clamp(0.0, 1.0));
    along.acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(name: &str) -> GemMaterial {
        GemMaterial::by_name(name).unwrap_or_else(|| panic!("{name} is a built-in material"))
    }

    #[test]
    fn the_geometry_presets_set_the_documented_angles() {
        assert_eq!(AxisPreset::ThroughTheTable.angles(None), Some((0.0, 0.0)));
        assert_eq!(AxisPreset::InTheTablePlane.angles(None), Some((90.0, 0.0)));
        assert_eq!(AxisPreset::AlongTheWidth.angles(None), None);
        assert_eq!(
            AxisPreset::AlongTheWidth.angles(Some(33.0)),
            Some((90.0, 33.0))
        );
    }

    #[test]
    fn the_width_azimuth_follows_the_convention_and_folds_into_a_half_turn() {
        // The width along +X is azimuth 0; along +Z it is 90 (azimuth runs toward +Z).
        assert_eq!(width_azimuth_deg([1.0, 0.0]), Some(0.0));
        let along_z = width_azimuth_deg([0.0, 1.0]).expect("a direction");
        assert!((along_z - 90.0).abs() < 1e-9, "{along_z}");
        // The opposite direction is the same line.
        let back = width_azimuth_deg([-1.0, 0.0]).expect("a direction");
        assert!(back.abs() < 1e-9, "{back}");
        let diagonal = width_azimuth_deg([-0.6, -0.8]).expect("a direction");
        let line = width_azimuth_deg([0.6, 0.8]).expect("a direction");
        assert!((diagonal - line).abs() < 1e-9);
        assert!((0.0..180.0).contains(&diagonal));
        // Not a direction.
        assert_eq!(width_azimuth_deg([0.0, 0.0]), None);
        assert_eq!(width_azimuth_deg([f64::NAN, 1.0]), None);
    }

    #[test]
    fn each_principal_direction_lands_along_the_table_normal() {
        for direction in PrincipalDirection::ALL {
            let orientation = orientation_through_table(direction);
            let angle = principal_angle_to_table_deg(&orientation, direction);
            assert!(
                angle < 0.1,
                "{direction:?} is {angle} degrees off the normal"
            );
        }
        // The Crystal Axis of the alpha and beta orientations lies in the table plane.
        for direction in [PrincipalDirection::Alpha, PrincipalDirection::Beta] {
            let gamma = orientation_through_table(direction).gamma();
            assert!(gamma.y.abs() < 1e-3, "{direction:?}: {gamma:?}");
        }
    }

    #[test]
    fn tanzanites_names_come_out_as_listed() {
        let presets = color_presets(&material("Tanzanite"));
        let labels: Vec<&str> = presets.iter().map(|p| p.id.label()).collect();
        assert_eq!(labels, ["Best Blue Face-Up", "Violet Face-Up"]);
        // Best Blue Face-Up: alpha (the darkest, most purplish ray) through the table, so the
        // blue and the light rays show. Violet Face-Up: gamma through the table.
        assert_eq!(presets[0].through_table, PrincipalDirection::Alpha);
        assert_eq!(presets[1].through_table, PrincipalDirection::Gamma);
        // The two really are different colors, and the blue one has the lower hue.
        assert_ne!(presets[0].swatch_srgb, presets[1].swatch_srgb);
    }

    #[test]
    fn the_other_trichroic_materials_get_one_name_each_and_the_rest_none() {
        let alexandrite = color_presets(&material("Alexandrite"));
        assert_eq!(alexandrite.len(), 1);
        assert_eq!(alexandrite[0].id, ColorPresetId::StrongestColorChange);
        // Andalusite (since 2026-11-03): alpha a brownish red-orange, beta and gamma a pale
        // yellow-green, so looking along alpha is green (swatch C* 25 h 119) and it offers one
        // "Best Green Face-Up"; the other two directions are a brownish yellow (h 88-89).
        let andalusite = color_presets(&material("Andalusite"));
        assert_eq!(andalusite.len(), 1);
        assert_eq!(andalusite[0].id, ColorPresetId::BestGreenFaceUp);
        // Peridot's pleochroism is weak, uniaxial and isotropic materials have no frame.
        for name in ["Peridot", "Sapphire", "Tourmaline", "Diamond", "Topaz"] {
            assert!(color_presets(&material(name)).is_empty(), "{name}");
        }
    }

    /// The green rule itself: a hue inside the tolerance with enough chroma counts, a hue
    /// outside it or a nearly colorless ray does not.
    #[test]
    fn a_direction_counts_as_green_only_with_a_green_hue_and_some_chroma() {
        let is_green = |hue: f64, chroma: f64| {
            chroma >= GREEN_MIN_CHROMA && hue_distance(hue, GREEN_HUE_DEG) <= GREEN_TOLERANCE_DEG
        };
        assert!(is_green(140.0, 30.0));
        assert!(is_green(112.0, 12.0), "28 degrees from green");
        assert!(!is_green(108.0, 7.0), "pale yellow");
        assert!(!is_green(111.0, 3.0), "nearly colorless");
        assert!(!is_green(41.0, 39.0), "a red-orange ray");
        assert!(!is_green(105.0, 30.0), "35 degrees from green");
    }

    #[test]
    fn a_preset_can_be_looked_up_by_id() {
        let tanzanite = material("Tanzanite");
        assert!(color_preset(&tanzanite, ColorPresetId::VioletFaceUp).is_some());
        assert!(color_preset(&tanzanite, ColorPresetId::BestGreenFaceUp).is_none());
    }
}
