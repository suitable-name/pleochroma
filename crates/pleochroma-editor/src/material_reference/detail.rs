//! [`MaterialDetail`]: everything the Material Details page shows for one material, as plain data.
//!
//! Every chart is a path string in a 0..100 viewbox (see [`super::curves`]), every number a text,
//! so the desktop, the web app and the Android app only draw it. Built-in and custom materials
//! share one shape: a field a custom material does not have reads "Not recorded".
//!
//! Colors come from the app's own color code: the per-direction colors are
//! `body_colors` over the face-up path of the tensor's unit, and the face-up color is
//! `variants::face_up_swatch` (daylight) and the same oriented evaluation under a 3200 K lamp
//! (incandescent), so the swatches equal the Stone tab's color tiles.

use std::fmt::Write as _;

use pleochroma::{
    color::{
        body_color::{BodyColor, BodyColors, Illuminant, body_colors, color_change_delta_e},
        principal_axes::{PrincipalFrame, face_up_body_color},
    },
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor, BandShape},
        dispersion::{DispersionModel, LINE_B_NM, LINE_G_NM},
        inclusions::{
            bragg::{StructureKind, structure_kind},
            catalogue::presets_for,
        },
        materials::{
            GemMaterial, MaterialLook, OpticalCharacter, optics_provenance,
            variants::{
                ColorVariant, MATERIAL_DEFAULT_LABEL, MATERIAL_DEFAULT_REFERENCE_PATH_MM,
                TensorRank, VariantProvenance, built_in_variants, face_up_swatch,
                face_up_swatch_path, material_slug, tensor_rank,
            },
        },
    },
};
use pleochroma_cut_core::{
    carat_weight,
    material::{
        GemField, MaterialLookup, MaterialOrigin, built_in_gem_data, built_in_specific_gravity,
        crystal_system_name, pattern::typical_patterns,
    },
};

use super::{
    curves::{
        AxisTick, FraunhoferGuide, LINE_D_NM, SPECTRUM_STEPS, fraunhofer_guides,
        index_samples_path, index_ticks, index_wavelength, shared_index_range, spectrum_path,
        spectrum_wavelength, wavelength_ticks,
    },
    facts::{CustomGemData, GemFacts, NOT_GRADED, NOT_RECORDED},
    rows::Origin,
    text::{DASH, dispersion_chip_text, family_line, optic_character_text, sg_basis_note},
};
use crate::lch_color::{color_family, lch_of_body_color};

const OMEGA: &str = "\u{3c9}";
const EPSILON: &str = "\u{3b5}";
const ALPHA: &str = "\u{3b1}";
const BETA: &str = "\u{3b2}";
const GAMMA: &str = "\u{3b3}";
const MINUS: &str = "\u{2212}";
const EN_DASH: &str = "\u{2013}";
const MIDDOT: &str = "\u{b7}";

/// The least CIEDE2000 between the daylight and the 3200 K color for the "Color change" chip.
pub const COLOR_CHANGE_DELTA_E: f64 = 8.0;
/// The stone width of a swatch when no design is open, millimeters.
const DEFAULT_WIDTH_MM: f64 = 7.0;
/// Headroom above the strongest absorption on the spectrum chart.
const ABSORPTION_HEADROOM: f32 = 1.05;
/// The tooltip of a missing built-in value.
const TIP_NO_SOURCE: &str = "No gemological source gives this value.";
/// The tooltip of a toughness no source grades.
const TIP_NOT_GRADED: &str = "No published toughness grade for this material.";
/// The tooltip of a custom material's empty value.
const TIP_NOT_ENTERED: &str = "You have not entered this value.";

/// What the page is built for besides the material.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DetailContext {
    /// The stone width of the colors and of the Transmission mode, millimeters: the open design's,
    /// or 7 without one (0 or less also means 7).
    pub stone_width_mm: f64,
    /// The gem data of a custom material; ignored for a built-in one.
    pub custom_gem: Option<CustomGemData>,
    /// The specific gravity of a custom material; ignored for a built-in one.
    pub custom_sg: Option<f64>,
    /// The cutter's saved color variants (those of other materials are skipped).
    pub user_variants: Vec<ColorVariant>,
    /// The finished volume of the open design in cubic millimeters at `stone_width_mm`, when one is
    /// open.
    pub design_volume: Option<f64>,
}

/// A labelled value of a card ("Hardness", "8.5 (Mohs)").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DetailRow {
    /// The row label.
    pub label: String,
    /// The value, or "Not recorded" / "Not graded".
    pub value: String,
    /// A note about the value (a source disagreement); empty when there is none.
    pub note: String,
    /// The tooltip of a missing value; empty otherwise.
    pub tooltip: String,
    /// Whether the value is a "Not recorded" / "Not graded" placeholder.
    pub missing: bool,
}

impl DetailRow {
    fn new(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.to_owned(),
            value: value.into(),
            ..Self::default()
        }
    }
}

/// A chip of the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailChip {
    /// The chip text ("RI 1.741–1.748").
    pub text: String,
    /// The tooltip; empty when there is none.
    pub tooltip: String,
    /// Whether the chip is drawn in the accent color ("Color change").
    pub highlight: bool,
}

/// A swatch with its out-of-gamut flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Swatch {
    /// The sRGB color.
    pub srgb: [u8; 3],
    /// Whether the color fell outside the sRGB gamut before it was mapped.
    pub out_of_gamut: bool,
}

impl From<BodyColor> for Swatch {
    fn from(color: BodyColor) -> Self {
        Self {
            srgb: color.srgb,
            out_of_gamut: color.out_of_gamut,
        }
    }
}

/// The header: name, family line, split swatch and chips.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailHeader {
    /// The material's name.
    pub name: String,
    /// "Chrysoberyl · Orthorhombic · biaxial (+) · built-in".
    pub family_line: String,
    /// The face-up daylight color (left half of the split swatch).
    pub swatch_daylight: [u8; 3],
    /// The face-up 3200 K color (right half).
    pub swatch_incandescent: [u8; 3],
    /// RI range, birefringence, dispersion (B–G), SG, Mohs and "Color change".
    pub chips: Vec<DetailChip>,
    /// Whether the daylight-to-3200 K color difference reaches [`COLOR_CHANGE_DELTA_E`].
    pub color_change: bool,
}

/// One refractive-index curve.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexSeries {
    /// The direction: "nω", "nε", "nα", "nβ", "nγ" or "n".
    pub label: String,
    /// The index at the D line, four decimals.
    pub nd_text: String,
    /// The index at the D line.
    pub nd: f32,
    /// The curve as path data in the 0..100 viewbox.
    pub path: String,
    /// The series' palette position (0 cyan, 1 amber, 2 purple).
    pub ink: usize,
}

/// The indices of every series at one wavelength, for the hover card.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexSample {
    /// The wavelength, nanometers.
    pub wavelength_nm: f32,
    /// The horizontal position in the viewbox.
    pub x: f32,
    /// One index per series, in series order.
    pub indices: Vec<f32>,
}

/// The Refractive Index card.
#[derive(Debug, Clone, PartialEq)]
pub struct RefractiveIndexDetail {
    /// One series for an isotropic material, two for a uniaxial one, three for a biaxial one.
    pub series: Vec<IndexSeries>,
    /// The vertical gridlines with their index labels.
    pub y_ticks: Vec<AxisTick>,
    /// The horizontal gridlines: 400 to 700 nm.
    pub x_ticks: Vec<AxisTick>,
    /// The Fraunhofer lines G, F, D, C and B.
    pub guides: Vec<FraunhoferGuide>,
    /// The index at the top edge of the plot.
    pub top: f32,
    /// The index at the bottom edge of the plot.
    pub bottom: f32,
    /// "nD 1.7427 (β) · Dispersion B–G 0.0171, F–C 0.0100 · Abbe 73.9".
    pub readout: String,
    /// What kind of curves these are, and what the lines mark.
    pub model_line: String,
    /// The index at every 5 nm step.
    pub samples: Vec<IndexSample>,
}

/// The Optics card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpticsDetail {
    /// Crystal system, optic character, birefringence, critical angle, principal indices,
    /// dispersion model, default axis, pleochroism, inclusions and color pattern.
    pub rows: Vec<DetailRow>,
}

/// One absorption band of one direction.
#[derive(Debug, Clone, PartialEq)]
pub struct BandRow {
    /// The direction's label.
    pub ray: String,
    /// The band center, nanometers.
    pub center_nm: f32,
    /// "FWHM 3461 cm⁻¹" or "σ 20 nm".
    pub width_text: String,
    /// The peak absorption, in the tensor's unit.
    pub peak: f32,
    /// The peak, two decimals.
    pub peak_text: String,
    /// "Gaussian in energy" or "Gaussian in wavelength".
    pub shape: String,
}

/// One direction of the spectrum chart.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumSeries {
    /// The direction's label ("ω (ordinary)", "α along c").
    pub label: String,
    /// The direction's daylight color, for the chip swatch.
    pub swatch: [u8; 3],
    /// The series' palette position.
    pub ink: usize,
    /// Relative absorption, all directions on one scale.
    pub absorption_path: String,
    /// Transmission through a stone of the context's width along this direction.
    pub transmission_path: String,
    /// The direction's bands, strongest first.
    pub bands: Vec<BandRow>,
}

/// The Absorption Spectrum card.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumDetail {
    /// One series for a material with one color in every direction, two or three otherwise.
    pub series: Vec<SpectrumSeries>,
    /// The horizontal gridlines: 400 to 700 nm.
    pub x_ticks: Vec<AxisTick>,
    /// "18 bands (6 per direction), per unit of path at 7 mm. Strongest: …".
    pub summary: String,
    /// Where the spectrum comes from.
    pub provenance: String,
    /// Whether the spectrum, or a direction of it, is an estimate.
    pub estimated: bool,
    /// The label of the variant shown ("Material Default" or a variant's label).
    pub variant_label: String,
    /// The stone width of the transmission paths, millimeters.
    pub stone_width_mm: f64,
    /// The tooltip of the Transmission mode.
    pub transmission_tooltip: String,
}

/// One row of the color table: a direction (or the face-up color) in both lights.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorRow {
    /// The direction's label, or "Face up".
    pub label: String,
    /// The color in daylight (D65).
    pub daylight: Swatch,
    /// The color under a 3200 K lamp.
    pub incandescent: Swatch,
    /// Whether this is the face-up row.
    pub face_up: bool,
}

/// The Color by Direction and Light card.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorsDetail {
    /// One row per direction (none for a material with one color), then the face-up row.
    pub rows: Vec<ColorRow>,
    /// The CIEDE2000 between the daylight and the 3200 K color.
    pub delta_e: f64,
    /// Whether it reaches [`COLOR_CHANGE_DELTA_E`].
    pub color_change: bool,
    /// The explanation of the color change (empty without one) and of what the face-up color
    /// assumes.
    pub note: String,
}

/// One entry of the Color Variants card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantDetail {
    /// The variant's stable key.
    pub key: String,
    /// The label.
    pub label: String,
    /// The face-up daylight color at the width the variant is calibrated for.
    pub swatch: [u8; 3],
    /// The first source line.
    pub source_line: String,
    /// Whether the numbers are an estimate.
    pub estimated: bool,
    /// Whether the cutter saved it.
    pub user: bool,
    /// Whether the page shows this variant's spectrum and colors.
    pub selected: bool,
}

/// The Effects card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectsDetail {
    /// The inclusion presets of the material.
    pub inclusion_presets: Vec<String>,
    /// "Play-of-color" or "Schiller" for a material with a structure.
    pub structure: Option<String>,
    /// The sheen the material switches on ("Adularescence").
    pub sheen: Option<String>,
    /// "Milky body" or "Translucent body" for a material with a hazy body.
    pub haze_body: Option<String>,
    /// The names of the typical zoning patterns.
    pub typical_zoning: Vec<String>,
    /// The one line the card shows.
    pub summary: String,
}

/// The Durability and Cutting card (without the specific gravity, see [`WeightDetail`]).
#[derive(Debug, Clone, PartialEq)]
pub struct DurabilityDetail {
    /// The gem facts as display text.
    pub facts: GemFacts,
    /// Hardness, toughness, cleavage, parting (when recorded) and fracture.
    pub rows: Vec<DetailRow>,
    /// The Mohs range for the hardness bar.
    pub mohs_range: Option<(f32, f32)>,
    /// How far the hardness bar is filled, 0 to 1 (Mohs over 10).
    pub mohs_fill: Option<f32>,
    /// Remarks that belong to no single row: the basis of the values.
    pub notes: Vec<String>,
    /// The care notes as one paragraph.
    pub care: String,
    /// "Sources: …. Values are typical for the species." or "Sources: Entered by you; …".
    pub sources: String,
}

/// Specific gravity and the weight comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct WeightDetail {
    /// "3.73 (3.68–3.78)", or "3.34" without a range; "Not recorded" without a figure.
    pub sg_text: String,
    /// The representative specific gravity.
    pub sg: Option<f64>,
    /// How much heavier (positive) or lighter than diamond a stone of the same size is, percent.
    pub vs_diamond_percent: Option<i32>,
    /// The full line: "3.73 (3.68–3.78): 6 % heavier than diamond at the same size".
    pub line: String,
    /// The basis note of a derived figure.
    pub basis_note: Option<String>,
    /// "This design: about 1.32 ct at 7 mm", with a design open.
    pub design_line: Option<String>,
}

/// A group of citations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGroup {
    /// "Optics", "Color", "Gem data" or "Specific gravity".
    pub title: String,
    /// The citations.
    pub lines: Vec<String>,
}

/// Everything the details page shows for one material.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialDetail {
    /// The material's name.
    pub name: String,
    /// Whether it is a material of the user's own.
    pub is_custom: bool,
    /// The header.
    pub header: DetailHeader,
    /// The Refractive Index card.
    pub ri: RefractiveIndexDetail,
    /// The Optics card.
    pub optics: OpticsDetail,
    /// The Absorption Spectrum card.
    pub spectrum: SpectrumDetail,
    /// The Color by Direction and Light card.
    pub colors: ColorsDetail,
    /// The Color Variants card: Material Default, the data file's variants, then the cutter's.
    pub variants: Vec<VariantDetail>,
    /// The Effects card.
    pub effects: EffectsDetail,
    /// The Durability and Cutting card.
    pub durability: DurabilityDetail,
    /// Specific gravity and weight.
    pub weight: WeightDetail,
    /// The sources, grouped.
    pub sources: Vec<SourceGroup>,
}

/// The details page of the material `name`, showing its own color (the Material Default).
///
/// `None` when `lookup` has no such material.
#[must_use]
pub fn material_detail(
    name: &str,
    lookup: &dyn MaterialLookup,
    ctx: &DetailContext,
) -> Option<MaterialDetail> {
    material_detail_for_variant(name, lookup, ctx, "")
}

/// [`material_detail`] showing the color variant with key `variant_key`: its spectrum, colors and
/// sources replace the Material Default's. An empty or unknown key shows the Material Default.
#[must_use]
pub fn material_detail_for_variant(
    name: &str,
    lookup: &dyn MaterialLookup,
    ctx: &DetailContext,
    variant_key: &str,
) -> Option<MaterialDetail> {
    let material = lookup.lookup(name)?;
    let built_in = is_built_in(&material);
    let variants = variant_list(&material, built_in, ctx);
    let selected = variants
        .iter()
        .position(|v| v.key == variant_key)
        .unwrap_or(0);
    let variant = &variants[selected];
    let width = effective_width(ctx.stone_width_mm);

    let kind = Kind::of(&material);
    let colors = colors_detail(&material, variant, width);
    let ri = refractive_index_detail(&material, kind);
    let facts = GemFacts::for_material(&material.name, !built_in, ctx.custom_gem.as_ref());
    let weight = weight_detail(&material, built_in, lookup, ctx, width);
    let durability = durability_detail(&material, built_in, facts);
    let spectrum = spectrum_detail(&material, variant, width, built_in);
    let effects = effects_detail(&material.name);
    let optics = optics_detail(&material, kind, variant, &effects);
    let header = header_detail(
        &material,
        built_in,
        &ri,
        &colors,
        &durability,
        &weight,
        &ColorPair::of(&colors),
    );
    let sources = source_groups(&material, built_in, variant, &durability, &weight);
    let variant_details = variants
        .iter()
        .enumerate()
        .map(|(i, v)| variant_detail(&material, v, i == selected))
        .collect();

    Some(MaterialDetail {
        name: material.name,
        is_custom: !built_in,
        header,
        ri,
        optics,
        spectrum,
        colors,
        variants: variant_details,
        effects,
        durability,
        weight,
        sources,
    })
}

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

/// Whether `material` is the built-in of its name (a custom material that shares a name is not).
fn is_built_in(material: &GemMaterial) -> bool {
    GemMaterial::built_in()
        .iter()
        .any(|m| m.name == material.name && m == material)
}

fn effective_width(width_mm: f64) -> f64 {
    if width_mm.is_finite() && width_mm > 0.0 {
        width_mm
    } else {
        DEFAULT_WIDTH_MM
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a stone width in millimeters is a small value; f32 is the precision the render uses"
)]
const fn width_f32(width_mm: f64) -> f32 {
    width_mm as f32
}

/// A width as text: "7" or "6.5".
fn mm_text(width_mm: f64) -> String {
    if (width_mm - width_mm.round()).abs() < 0.05 {
        format!("{width_mm:.0}")
    } else {
        format!("{width_mm:.1}")
    }
}

/// The three optics regimes the charts distinguish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Isotropic,
    Uniaxial,
    Biaxial,
}

impl Kind {
    fn of(material: &GemMaterial) -> Self {
        if material.biaxial_indicatrix(LINE_D_NM).is_some() {
            return Self::Biaxial;
        }
        match material.optical_character {
            OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative => {
                Self::Uniaxial
            }
            _ => Self::Isotropic,
        }
    }

    fn labels(self) -> Vec<String> {
        match self {
            Self::Isotropic => vec!["n".to_owned()],
            Self::Uniaxial => vec![format!("n{OMEGA}"), format!("n{EPSILON}")],
            Self::Biaxial => vec![format!("n{ALPHA}"), format!("n{BETA}"), format!("n{GAMMA}")],
        }
    }
}

/// The principal indices of `material` at `lambda_nm`, in series order.
fn principal_at(material: &GemMaterial, kind: Kind, lambda_nm: f32) -> Vec<f32> {
    match kind {
        Kind::Biaxial => material.biaxial_indicatrix(lambda_nm).map_or_else(
            || vec![material.dispersion.evaluate(lambda_nm)],
            |i| vec![i.n_alpha, i.n_beta, i.n_gamma],
        ),
        Kind::Uniaxial => {
            let n_o = material.dispersion.evaluate(lambda_nm);
            vec![n_o, material.extraordinary_index_at(lambda_nm, n_o)]
        }
        Kind::Isotropic => vec![material.dispersion.evaluate(lambda_nm)],
    }
}

/// The crystal direction of each of alpha, beta and gamma where the built-in entry names it.
///
/// The chrysoberyl entries document it beside their absorption bands (Farrell & Newnham 1965):
/// alpha is crystal c, beta crystal a, gamma crystal b. No other entry names its directions.
fn crystal_directions(name: &str) -> Option<[&'static str; 3]> {
    matches!(name, "Alexandrite" | "Chrysoberyl (Yellow)").then_some(["c", "a", "b"])
}

/// The direction labels of a tensor of `rank` on `material`.
fn ray_labels(material: &GemMaterial, rank: TensorRank, built_in: bool) -> Vec<String> {
    match rank {
        TensorRank::Isotropic => vec!["All directions".to_owned()],
        TensorRank::Uniaxial => vec![
            format!("{OMEGA} (ordinary)"),
            format!("{EPSILON} (along c)"),
        ],
        TensorRank::Biaxial => {
            let dirs = built_in
                .then(|| crystal_directions(&material.name))
                .flatten();
            [ALPHA, BETA, GAMMA]
                .iter()
                .enumerate()
                .map(|(i, letter)| {
                    dirs.map_or_else(
                        || (*letter).to_owned(),
                        |d| format!("{letter} along {}", d[i]),
                    )
                })
                .collect()
        }
    }
}

/// The per-direction colors of `colors`, in tensor order, for a tensor of `rank`.
fn ray_colors(colors: &BodyColors, rank: TensorRank) -> Vec<BodyColor> {
    match rank {
        TensorRank::Isotropic => vec![colors.unpolarised],
        TensorRank::Uniaxial => vec![colors.o_ray, colors.e_ray],
        TensorRank::Biaxial => vec![
            colors.o_ray,
            colors.beta_ray.unwrap_or(colors.o_ray),
            colors.e_ray,
        ],
    }
}

fn ray_bands(tensor: &AbsorptionTensor, rank: TensorRank) -> Vec<&[AbsorptionBand]> {
    let o: &[AbsorptionBand] = &tensor.o_ray;
    let e: &[AbsorptionBand] = &tensor.e_ray;
    match rank {
        TensorRank::Isotropic => vec![o],
        TensorRank::Uniaxial => vec![o, e],
        TensorRank::Biaxial => vec![o, tensor.beta_ray.as_deref().unwrap_or(o), e],
    }
}

fn absorption_at(bands: &[AbsorptionBand], lambda_nm: f32) -> f32 {
    bands
        .iter()
        .map(|band| band.evaluate(lambda_nm))
        .sum::<f32>()
        .max(0.0)
}

// ---------------------------------------------------------------------------------------------
// Variants
// ---------------------------------------------------------------------------------------------

/// The variants of the page: Material Default, the data file's, then the cutter's that the host
/// accepts.
fn variant_list(material: &GemMaterial, built_in: bool, ctx: &DetailContext) -> Vec<ColorVariant> {
    let mut list = if built_in {
        built_in_variants(&material.name)
    } else {
        Vec::new()
    };
    if list.is_empty() {
        list.push(ColorVariant {
            key: format!("{}.default", material_slug(&material.name)),
            label: MATERIAL_DEFAULT_LABEL.to_owned(),
            material: material.name.clone(),
            absorption: material.absorption.clone(),
            unit: material.absorption_unit,
            reference_path_mm: MATERIAL_DEFAULT_REFERENCE_PATH_MM,
            provenance: VariantProvenance::MaterialDefault,
        });
    }
    list.extend(
        ctx.user_variants
            .iter()
            .filter(|v| {
                v.material.eq_ignore_ascii_case(&material.name) && v.validate_for(material).is_ok()
            })
            .cloned(),
    );
    list
}

/// The first non-empty line of a variant's sources.
fn first_source_line(sources: &[String]) -> Option<&str> {
    sources
        .iter()
        .flat_map(|s| s.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
}

fn variant_detail(material: &GemMaterial, variant: &ColorVariant, selected: bool) -> VariantDetail {
    let swatch = face_up_swatch(
        &variant.absorption,
        variant.unit,
        f64::from(variant.reference_path_mm),
        material,
    )
    .srgb;
    let (source_line, estimated, user) = match &variant.provenance {
        VariantProvenance::MaterialDefault => {
            ("The material's own color.".to_owned(), false, false)
        }
        VariantProvenance::User => ("Saved by you.".to_owned(), false, true),
        VariantProvenance::BuiltIn {
            sources, estimated, ..
        } => (
            first_source_line(sources)
                .unwrap_or("No published spectrum.")
                .to_owned(),
            *estimated,
            false,
        ),
    };
    VariantDetail {
        key: variant.key.clone(),
        label: variant.label.clone(),
        swatch,
        source_line,
        estimated,
        user,
        selected,
    }
}

// ---------------------------------------------------------------------------------------------
// Refractive index
// ---------------------------------------------------------------------------------------------

const fn curve_kind_name(model: &DispersionModel) -> &'static str {
    match model {
        DispersionModel::Cauchy { .. } => "Cauchy",
        DispersionModel::Sellmeier1 { .. } | DispersionModel::Sellmeier3 { .. } => "Sellmeier",
    }
}

fn model_line(material: &GemMaterial, kind: Kind) -> String {
    let base = curve_kind_name(&material.dispersion);
    let curves = match kind {
        Kind::Isotropic => format!("{base} curve"),
        Kind::Uniaxial => match material.uniaxial_extraordinary_dispersion {
            Some(e) if curve_kind_name(&e) == base => format!("{base}, per direction"),
            Some(e) => format!(
                "{base} for {OMEGA}, {} for {EPSILON}, per direction",
                curve_kind_name(&e)
            ),
            None => format!("{base} for {OMEGA}; {EPSILON} follows at a constant offset"),
        },
        Kind::Biaxial => match material.biaxial_principal_dispersion {
            Some(pair) if pair.iter().all(|m| curve_kind_name(m) == base) => {
                format!("{base}, per direction")
            }
            Some(_) => format!("{base} for {BETA}; {ALPHA} and {GAMMA} on their own curves"),
            None => format!("{base} for {BETA}; {ALPHA} and {GAMMA} follow at constant offsets"),
        },
    };
    format!(
        "{curves}. These are the model's curves. Lines B to G mark the wavelengths the dispersion figures use."
    )
}

fn refractive_index_detail(material: &GemMaterial, kind: Kind) -> RefractiveIndexDetail {
    let labels = kind.labels();
    let per_step: Vec<Vec<f32>> = (0..=super::curves::CURVE_STEPS)
        .map(|s| principal_at(material, kind, index_wavelength(s)))
        .collect();
    let curves: Vec<Vec<f32>> = (0..labels.len())
        .map(|i| per_step.iter().map(|at| at[i.min(at.len() - 1)]).collect())
        .collect();
    let (bottom, top) = shared_index_range(&curves);
    let at_d = principal_at(material, kind, LINE_D_NM);
    let series = labels
        .iter()
        .zip(&curves)
        .enumerate()
        .map(|(i, (label, samples))| {
            let nd = at_d[i.min(at_d.len() - 1)];
            IndexSeries {
                label: label.clone(),
                nd_text: format!("{nd:.4}"),
                nd,
                path: index_samples_path(samples, bottom, top),
                ink: i,
            }
        })
        .collect();
    let samples = per_step
        .iter()
        .enumerate()
        .map(|(step, indices)| {
            let wavelength_nm = index_wavelength(u16::try_from(step).unwrap_or(0));
            IndexSample {
                wavelength_nm,
                x: super::curves::x_of_wavelength(wavelength_nm),
                indices: indices.clone(),
            }
        })
        .collect();

    let n_d = material.dispersion.n_d();
    let letter = match kind {
        Kind::Isotropic => String::new(),
        Kind::Uniaxial => format!(" ({OMEGA})"),
        Kind::Biaxial => format!(" ({BETA})"),
    };
    let mut readout = format!(
        "nD {n_d:.4}{letter} {MIDDOT} Dispersion B{EN_DASH}G {:.4}, F{EN_DASH}C {:.4}",
        material.dispersion.delta_b_g(),
        material.dispersion.delta_f_c()
    );
    if let Some(abbe) = material.dispersion.abbe_number() {
        let _ = write!(readout, " {MIDDOT} Abbe {abbe:.1}");
    }

    RefractiveIndexDetail {
        series,
        y_ticks: index_ticks(bottom, top),
        x_ticks: wavelength_ticks(),
        guides: fraunhofer_guides().to_vec(),
        top,
        bottom,
        readout,
        model_line: model_line(material, kind),
        samples,
    }
}

// ---------------------------------------------------------------------------------------------
// Colors and spectrum
// ---------------------------------------------------------------------------------------------

/// The four colors the page needs of one tensor.
struct ColorSet {
    d65: BodyColors,
    lamp: BodyColors,
    face_up_d65: BodyColor,
    face_up_lamp: BodyColor,
}

fn color_set(host: &GemMaterial, variant: &ColorVariant, width: f64) -> ColorSet {
    let path = face_up_swatch_path(variant.unit, width_f32(width));
    let lamp_light = Illuminant::Planckian(3200.0);
    let frame = variant
        .absorption
        .is_pleochroic
        .then(|| PrincipalFrame::of_material(host));
    ColorSet {
        d65: body_colors(&variant.absorption, path, Illuminant::D65),
        lamp: body_colors(&variant.absorption, path, lamp_light),
        face_up_d65: face_up_swatch(&variant.absorption, variant.unit, width, host),
        face_up_lamp: face_up_body_color(&variant.absorption, frame.as_ref(), path, lamp_light),
    }
}

/// The two face-up colors of the header.
struct ColorPair {
    daylight: [u8; 3],
    lamp: [u8; 3],
    color_change: bool,
}

impl ColorPair {
    fn of(colors: &ColorsDetail) -> Self {
        let face_up = colors.rows.last();
        Self {
            daylight: face_up.map_or([0; 3], |r| r.daylight.srgb),
            lamp: face_up.map_or([0; 3], |r| r.incandescent.srgb),
            color_change: colors.color_change,
        }
    }
}

fn colors_detail(host: &GemMaterial, variant: &ColorVariant, width: f64) -> ColorsDetail {
    let set = color_set(host, variant, width);
    let rank = tensor_rank(&variant.absorption);
    let mut rows = Vec::new();
    if rank != TensorRank::Isotropic {
        let labels = ray_labels(host, rank, is_built_in(host));
        for ((label, day), lamp) in labels
            .into_iter()
            .zip(ray_colors(&set.d65, rank))
            .zip(ray_colors(&set.lamp, rank))
        {
            rows.push(ColorRow {
                label,
                daylight: day.into(),
                incandescent: lamp.into(),
                face_up: false,
            });
        }
    }
    rows.push(ColorRow {
        label: "Face up".to_owned(),
        daylight: set.face_up_d65.into(),
        incandescent: set.face_up_lamp.into(),
        face_up: true,
    });

    let delta_e = color_change_delta_e(&set.d65, &set.lamp);
    let color_change = delta_e >= COLOR_CHANGE_DELTA_E;
    let assumption = format!(
        "The face-up color assumes the default axis and a {} mm stone; a larger stone is deeper in color.",
        mm_text(width)
    );
    let note = if color_change {
        format!(
            "Color change: {} in daylight, {} under incandescent light (3200 K). {assumption}",
            color_family(lch_of_body_color(&set.face_up_d65)).to_lowercase(),
            color_family(lch_of_body_color(&set.face_up_lamp)).to_lowercase(),
        )
    } else {
        assumption
    };
    ColorsDetail {
        rows,
        delta_e,
        color_change,
        note,
    }
}

fn band_row(ray: &str, band: &AbsorptionBand) -> BandRow {
    let (width_text, shape) = match band.shape {
        BandShape::GaussianEnergy => (
            format!("FWHM {:.0} cm\u{207b}\u{b9}", band.width_nm * 2.354_82),
            "Gaussian in energy",
        ),
        BandShape::GaussianWavelength => (
            format!("\u{3c3} {:.0} nm", band.width_nm),
            "Gaussian in wavelength",
        ),
    };
    BandRow {
        ray: ray.to_owned(),
        center_nm: band.center_nm,
        width_text,
        peak: band.peak,
        peak_text: format!("{:.2}", band.peak),
        shape: shape.to_owned(),
    }
}

fn spectrum_detail(
    host: &GemMaterial,
    variant: &ColorVariant,
    width: f64,
    built_in: bool,
) -> SpectrumDetail {
    let tensor = &variant.absorption;
    let rank = tensor_rank(tensor);
    let labels = ray_labels(host, rank, built_in);
    let sets = ray_bands(tensor, rank);
    let path_length = face_up_swatch_path(variant.unit, width_f32(width));
    let swatches = ray_colors(&body_colors(tensor, path_length, Illuminant::D65), rank);

    let curves: Vec<Vec<f32>> = sets
        .iter()
        .map(|bands| {
            (0..=SPECTRUM_STEPS)
                .map(|s| absorption_at(bands, spectrum_wavelength(s)))
                .collect()
        })
        .collect();
    let peak_max = curves
        .iter()
        .flatten()
        .copied()
        .fold(0.0_f32, f32::max)
        .max(f32::MIN_POSITIVE);
    let scale = peak_max * ABSORPTION_HEADROOM;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the transmission is a value between 0 and 1; f32 is the chart precision"
    )]
    let path_f32 = path_length as f32;

    let mut series = Vec::new();
    let mut all_bands: Vec<BandRow> = Vec::new();
    for (i, ((label, bands), curve)) in labels.iter().zip(&sets).zip(&curves).enumerate() {
        let absorption: Vec<f32> = curve.iter().map(|a| a / scale).collect();
        let transmission: Vec<f32> = curve.iter().map(|a| 1.0 - (-a * path_f32).exp()).collect();
        let mut rows: Vec<BandRow> = bands.iter().map(|b| band_row(label, b)).collect();
        rows.sort_by(|a, b| b.peak.total_cmp(&a.peak));
        all_bands.extend(rows.iter().cloned());
        series.push(SpectrumSeries {
            label: label.clone(),
            swatch: swatches[i.min(swatches.len() - 1)].srgb,
            ink: i,
            absorption_path: spectrum_path(&absorption),
            transmission_path: spectrum_path(&transmission),
            bands: rows,
        });
    }

    let unit_text = match variant.unit {
        pleochroma::optics::materials::AbsorptionUnit::ModelUnit => "per unit of path at 7 mm",
        pleochroma::optics::materials::AbsorptionUnit::PerMm => "per mm of path",
    };
    let summary = if all_bands.is_empty() {
        "No absorption bands: this material is colorless.".to_owned()
    } else {
        let per = sets.first().map_or(0, |s| s.len());
        let equal = sets.iter().all(|s| s.len() == per);
        let count = if equal && sets.len() > 1 {
            format!("{} bands ({per} per direction)", all_bands.len())
        } else {
            format!("{} bands", all_bands.len())
        };
        all_bands.sort_by(|a, b| b.peak.total_cmp(&a.peak));
        let strongest: Vec<String> = all_bands
            .iter()
            .take(4)
            .map(|b| {
                format!(
                    "{} {:.1} nm ({}, {})",
                    b.ray, b.center_nm, b.width_text, b.peak_text
                )
            })
            .collect();
        format!("{count}, {unit_text}. Strongest: {}.", strongest.join(", "))
    };

    let (provenance, estimated) = match &variant.provenance {
        VariantProvenance::MaterialDefault => optics_provenance(&host.name)
            .filter(|_| built_in)
            .map_or((String::new(), false), |p| {
                (p.absorption.to_owned(), p.absorption_estimated)
            }),
        VariantProvenance::BuiltIn {
            sources, estimated, ..
        } => (
            first_source_line(sources).unwrap_or_default().to_owned(),
            *estimated,
        ),
        VariantProvenance::User => ("Saved by you.".to_owned(), false),
    };

    SpectrumDetail {
        series,
        x_ticks: wavelength_ticks(),
        summary,
        provenance,
        estimated,
        variant_label: variant.label.clone(),
        stone_width_mm: width,
        transmission_tooltip: format!(
            "Light that passes through a stone {} mm wide, along each direction.",
            mm_text(width)
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Optics, effects
// ---------------------------------------------------------------------------------------------

fn effects_detail(name: &str) -> EffectsDetail {
    let inclusion_presets: Vec<String> = presets_for(name)
        .iter()
        .map(|p| p.name.to_owned())
        .collect();
    let look = MaterialLook::of(name);
    let structure = structure_kind(name).map(|k| match k {
        StructureKind::Opal => "Play-of-color".to_owned(),
        StructureKind::Labradorite => "Schiller".to_owned(),
    });
    let sheen = look.inclusion_preset.map(str::to_owned);
    let haze_body = look.haze_sigma_s.map(|_| match structure_kind(name) {
        Some(StructureKind::Opal) => "Milky body".to_owned(),
        Some(StructureKind::Labradorite) => "Translucent body".to_owned(),
        None => "Hazy body".to_owned(),
    });
    let typical_zoning: Vec<String> = typical_patterns(name)
        .iter()
        .map(|p| p.label.to_owned())
        .collect();

    let mut parts: Vec<String> = Vec::new();
    if !inclusion_presets.is_empty() {
        parts.push(format!("Inclusions: {}", inclusion_presets.join(", ")));
    }
    parts.extend(structure.clone());
    if let Some(sheen) = &sheen
        && !inclusion_presets.contains(sheen)
    {
        parts.push(sheen.clone());
    }
    parts.extend(haze_body.clone());
    if !typical_zoning.is_empty() {
        parts.push(format!("Typical zoning: {}", typical_zoning.join(", ")));
    }
    let summary = if parts.is_empty() {
        "No inclusion presets, special looks or typical zoning.".to_owned()
    } else {
        parts.join(&format!(" {MIDDOT} "))
    };
    EffectsDetail {
        inclusion_presets,
        structure,
        sheen,
        haze_body,
        typical_zoning,
        summary,
    }
}

fn optics_detail(
    material: &GemMaterial,
    kind: Kind,
    variant: &ColorVariant,
    effects: &EffectsDetail,
) -> OpticsDetail {
    let at_d = principal_at(material, kind, LINE_D_NM);
    let birefringence = match kind {
        Kind::Isotropic => "None".to_owned(),
        Kind::Uniaxial => {
            let (n_o, n_e) = (at_d[0], at_d[1]);
            if n_e >= n_o {
                format!("{:.4} (n{EPSILON} {MINUS} n{OMEGA})", n_e - n_o)
            } else {
                format!("{:.4} (n{OMEGA} {MINUS} n{EPSILON})", n_o - n_e)
            }
        }
        Kind::Biaxial => format!("{:.4} (n{GAMMA} {MINUS} n{ALPHA})", at_d[2] - at_d[0]),
    };
    let n_d = material.dispersion.n_d();
    let critical = if n_d > 1.0 {
        format!("{:.1}\u{b0}", (1.0 / n_d).asin().to_degrees())
    } else {
        DASH.to_owned()
    };
    let principal = at_d
        .iter()
        .map(|n| format!("{n:.4}"))
        .collect::<Vec<_>>()
        .join(" / ");
    let model = {
        let line = model_line(material, kind);
        line.split(". ").next().unwrap_or_default().to_owned()
    };
    let default_axis = if kind == Kind::Isotropic {
        "Not applicable (isotropic)".to_owned()
    } else {
        let c = material.c_axis.try_normalize().unwrap_or(glam::Vec3::Y);
        let tilt = c.y.abs().clamp(0.0, 1.0).acos().to_degrees();
        if tilt < 1.0 {
            "Through the table".to_owned()
        } else if tilt > 89.0 {
            "In the table plane".to_owned()
        } else {
            format!("Tilted {tilt:.0}\u{b0} from the table normal")
        }
    };
    let pleochroism = match tensor_rank(&variant.absorption) {
        TensorRank::Isotropic => "None",
        TensorRank::Uniaxial => "Dichroic (two colors)",
        TensorRank::Biaxial => "Trichroic (three colors)",
    };
    let list_or = |items: &[String], none: &str| {
        if items.is_empty() {
            none.to_owned()
        } else {
            items.join(", ")
        }
    };
    OpticsDetail {
        rows: vec![
            DetailRow::new(
                "Crystal system",
                crystal_system_name(material.crystal_system),
            ),
            DetailRow::new(
                "Optic character",
                optic_character_text(material.optical_character),
            ),
            DetailRow::new("Birefringence", birefringence),
            DetailRow::new("Critical angle", critical),
            DetailRow::new("Principal indices", principal),
            DetailRow::new("Dispersion model", model),
            DetailRow::new("Default axis", default_axis),
            DetailRow::new("Pleochroism", pleochroism),
            DetailRow::new("Inclusions", list_or(&effects.inclusion_presets, "None")),
            DetailRow::new(
                "Color pattern",
                list_or(&effects.typical_zoning, "No typical zoning"),
            ),
        ],
    }
}

// ---------------------------------------------------------------------------------------------
// Durability, weight, header, sources
// ---------------------------------------------------------------------------------------------

fn missing_row(label: &str, value: &str, tooltip: &str) -> DetailRow {
    DetailRow {
        label: label.to_owned(),
        value: value.to_owned(),
        tooltip: tooltip.to_owned(),
        missing: true,
        ..DetailRow::default()
    }
}

fn durability_detail(material: &GemMaterial, built_in: bool, facts: GemFacts) -> DurabilityDetail {
    let data = built_in
        .then(|| built_in_gem_data(&material.name))
        .flatten();
    let tip = if built_in {
        TIP_NO_SOURCE
    } else {
        TIP_NOT_ENTERED
    };
    let field_note = |field: GemField| -> String {
        data.map(|d| {
            d.notes
                .iter()
                .filter(|n| n.field == field)
                .map(|n| n.text)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
    };
    let with_note = |mut row: DetailRow, field: GemField| {
        row.note = field_note(field);
        row
    };

    let mut rows = Vec::new();
    rows.push(with_note(
        facts.hardness_line().map_or_else(
            || missing_row("Hardness", NOT_RECORDED, tip),
            |v| DetailRow::new("Hardness", v),
        ),
        GemField::Hardness,
    ));
    rows.push(with_note(
        facts.toughness.as_deref().map_or_else(
            || {
                if built_in && data.is_some() {
                    missing_row("Toughness", NOT_GRADED, TIP_NOT_GRADED)
                } else {
                    missing_row("Toughness", NOT_RECORDED, tip)
                }
            },
            |v| DetailRow::new("Toughness", v),
        ),
        GemField::Toughness,
    ));
    rows.push(with_note(
        facts.cleavage.as_deref().map_or_else(
            || missing_row("Cleavage", NOT_RECORDED, tip),
            |v| DetailRow::new("Cleavage", v),
        ),
        GemField::Cleavage,
    ));
    if let Some(parting) = facts.parting.as_deref() {
        rows.push(with_note(
            DetailRow::new("Parting", parting),
            GemField::Parting,
        ));
    }
    rows.push(with_note(
        facts.fracture.as_deref().map_or_else(
            || missing_row("Fracture", NOT_RECORDED, tip),
            |v| DetailRow::new("Fracture", v),
        ),
        GemField::Fracture,
    ));

    let row_notes: Vec<&str> = data
        .map(|d| d.notes.iter().map(|n| n.text).collect())
        .unwrap_or_default();
    let notes: Vec<String> = facts
        .notes
        .iter()
        .filter(|n| !row_notes.contains(&n.as_str()))
        .cloned()
        .collect();
    let sources = if facts.sources.is_empty() {
        String::new()
    } else if facts.user_entered {
        format!("Sources: {}.", facts.sources.join("; "))
    } else {
        format!(
            "Sources: {}. Values are typical for the species.",
            facts.sources.join("; ")
        )
    };
    DurabilityDetail {
        mohs_range: facts.hardness_range,
        mohs_fill: facts
            .hardness_range
            .map(|(_, hi)| (hi / 10.0).clamp(0.0, 1.0)),
        care: facts.care.join(" "),
        facts,
        rows,
        notes,
        sources,
    }
}

fn weight_detail(
    material: &GemMaterial,
    built_in: bool,
    lookup: &dyn MaterialLookup,
    ctx: &DetailContext,
    width: f64,
) -> WeightDetail {
    let (sg, range, basis_note) = if built_in {
        built_in_specific_gravity(&material.name).map_or((None, None, None), |s| {
            (
                Some(s.representative),
                Some(s.range),
                sg_basis_note(&material.name),
            )
        })
    } else {
        let sg = ctx
            .custom_sg
            .or_else(|| lookup.specific_gravity(&material.name));
        (sg, sg.map(|s| (s, s)), None)
    };
    let Some(sg) = sg else {
        return WeightDetail {
            sg_text: NOT_RECORDED.to_owned(),
            sg: None,
            vs_diamond_percent: None,
            line: NOT_RECORDED.to_owned(),
            basis_note,
            design_line: None,
        };
    };
    let sg_text = match range {
        Some((lo, hi)) if (hi - lo).abs() > 1e-9 => {
            format!("{sg:.2} ({lo:.2}{EN_DASH}{hi:.2})")
        }
        _ => format!("{sg:.2}"),
    };
    let diamond = built_in_specific_gravity("Diamond").map_or(3.52, |d| d.representative);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a percentage of a specific-gravity ratio is a small number"
    )]
    let percent = ((sg / diamond - 1.0) * 100.0).round() as i32;
    let comparison = match percent {
        0 => "the same weight as diamond at the same size".to_owned(),
        p if p > 0 => format!("{p} % heavier than diamond at the same size"),
        p => format!("{} % lighter than diamond at the same size", -p),
    };
    let design_line = ctx.design_volume.filter(|v| *v > 0.0).map(|volume| {
        format!(
            "This design: about {:.2} ct at {} mm",
            carat_weight(volume, sg),
            mm_text(width)
        )
    });
    WeightDetail {
        line: format!("{sg_text}: {comparison}"),
        sg_text,
        sg: Some(sg),
        vs_diamond_percent: Some(percent),
        basis_note,
        design_line,
    }
}

fn header_detail(
    material: &GemMaterial,
    built_in: bool,
    ri: &RefractiveIndexDetail,
    colors: &ColorsDetail,
    durability: &DurabilityDetail,
    weight: &WeightDetail,
    pair: &ColorPair,
) -> DetailHeader {
    let data = built_in
        .then(|| built_in_gem_data(&material.name))
        .flatten();
    let origin = match data.map(|d| d.origin) {
        Some(MaterialOrigin::Synthetic) => Origin::Synthetic,
        Some(MaterialOrigin::Glass) => Origin::Glass,
        Some(MaterialOrigin::Natural) => Origin::Natural,
        None => Origin::Custom,
    };
    let mut family = family_line(material, origin);
    if let Some(d) = data
        && !d.species.eq_ignore_ascii_case(&material.name)
    {
        family = format!("{} {MIDDOT} {family}", d.species);
    }

    let lowest = ri.series.iter().map(|s| s.nd).fold(f32::INFINITY, f32::min);
    let highest = ri
        .series
        .iter()
        .map(|s| s.nd)
        .fold(f32::NEG_INFINITY, f32::max);
    let ri_text = if (highest - lowest).abs() < 0.0005 {
        format!("RI {lowest:.3}")
    } else {
        format!("RI {lowest:.3}{EN_DASH}{highest:.3}")
    };
    let biref = if material.optical_character == OpticalCharacter::Isotropic {
        DASH.to_owned()
    } else {
        format!("{:.3}", material.birefringence_delta.abs())
    };
    let chip = |text: String, tooltip: &str| DetailChip {
        text,
        tooltip: tooltip.to_owned(),
        highlight: false,
    };
    let mut chips = vec![
        chip(
            ri_text,
            "Principal refractive indices at the D line (the model's curve).",
        ),
        chip(format!("Biref. {biref}"), ""),
        chip(
            format!("Disp. {} (B{EN_DASH}G)", dispersion_chip_text(material)),
            &format!(
                "Dispersion: n at {LINE_G_NM} nm minus n at {LINE_B_NM} nm, from this material's index curve."
            ),
        ),
    ];
    if weight.sg.is_some() {
        chips.push(chip(format!("SG {:.2}", weight.sg.unwrap_or_default()), ""));
    }
    if let Some(hardness) = durability.facts.hardness.as_deref() {
        chips.push(chip(format!("Mohs {hardness}"), ""));
    }
    if pair.color_change {
        chips.push(DetailChip {
            text: "Color change".to_owned(),
            tooltip: colors.note.clone(),
            highlight: true,
        });
    }
    DetailHeader {
        name: material.name.clone(),
        family_line: family,
        swatch_daylight: pair.daylight,
        swatch_incandescent: pair.lamp,
        chips,
        color_change: pair.color_change,
    }
}

fn source_groups(
    material: &GemMaterial,
    built_in: bool,
    variant: &ColorVariant,
    durability: &DurabilityDetail,
    weight: &WeightDetail,
) -> Vec<SourceGroup> {
    let mut groups = Vec::new();
    let provenance = optics_provenance(&material.name).filter(|_| built_in);
    if let Some(p) = provenance {
        groups.push(SourceGroup {
            title: "Optics".to_owned(),
            lines: vec![format!("Refractive index: {}", p.dispersion)],
        });
    }
    let color_lines: Vec<String> = match &variant.provenance {
        VariantProvenance::MaterialDefault => provenance
            .map(|p| {
                let mut line = format!("Absorption: {}", p.absorption);
                if p.absorption_estimated {
                    line.push_str(" (partly estimated)");
                }
                vec![line]
            })
            .unwrap_or_default(),
        VariantProvenance::BuiltIn {
            sources, estimated, ..
        } => {
            let mut lines: Vec<String> = sources.clone();
            if *estimated {
                lines.push("Estimated, not a fit to a measured spectrum.".to_owned());
            }
            lines
        }
        VariantProvenance::User => vec!["Saved by you.".to_owned()],
    };
    if !color_lines.is_empty() {
        groups.push(SourceGroup {
            title: "Color".to_owned(),
            lines: color_lines,
        });
    }
    if !durability.facts.sources.is_empty() {
        groups.push(SourceGroup {
            title: "Gem data".to_owned(),
            lines: durability.facts.sources.clone(),
        });
    }
    if let Some(note) = &weight.basis_note {
        groups.push(SourceGroup {
            title: "Specific gravity".to_owned(),
            lines: vec![note.clone()],
        });
    }
    groups
}
