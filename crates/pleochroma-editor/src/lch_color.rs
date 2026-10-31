//! The pure half of the path-aware L*C*h body-colour editor.
//!
//! The editor lives in Design settings > Body Color > "Custom...": solving a typed Tone /
//! Saturation / Hue for the design's reference path, the swatches of that colour at other sizes, the reachability badge, seeding the sliders
//! from a stored colour, and the one edit that stores the result.
//!
//! The solve is [`solve_body_color`] (seven-band, optical-density space: the same pick is
//! deeper in a larger stone). The stored form is the band rows
//! (`MaterialSelection::body_color_bands_override`, amplitudes per millimetre) NEXT TO the
//! nearest legacy triple (`body_color_override`), so an older build still shows a close
//! colour. The path scale is left to the renderer: with the design's girdle diameter set
//! (`Design::girdle_diameter_mm`) it scales model units to millimetres itself
//! (`render_setup::apply_material_overrides`).
//!
//! A design's color is stored per axis: one L*C*h row for an isotropic host, "Along the Axis" and
//! "Across the Axis" for a uniaxial one, "Alpha", "Beta" and "Gamma" for a biaxial one
//! ([`axis_rows`]). Each row is solved with the same seven-band solver; the rows together are
//! one [`VariantChoice`] ([`custom_choice`]) whose tensor has the host's rank, so a pleochroic
//! stone never becomes isotropic by being recolored.
//!
//! No Slint here, so the rules are unit-testable without a window.

use std::sync::atomic::AtomicBool;

use pleochroma::{
    color::body_color::{
        BodyColor, Illuminant, body_color_swatches, body_colors, lab_to_lch, srgb_to_lab,
    },
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor, BandShape, body_color_bands},
        chromophore::{BodyColorTarget, solve_body_color},
        materials::{
            AbsorptionUnit, GemMaterial,
            variants::{TensorRank, face_up_swatch_path, host_rank},
        },
    },
};
use pleochroma_cut_core::{
    MaterialSelection,
    material::{
        CUSTOM_VARIANT_KEY, CUSTOM_VARIANT_LABEL, VariantChoice,
        color::{fantasy_lab, nearest_legacy_rgb},
    },
};

/// The fixed paths (millimetres) every solved colour is shown at, before the stone's own.
pub const SWATCH_PATHS_MM: [f64; 3] = [3.0, 5.0, 10.0];

/// The colour a typed (or picked) L*C*h solved to, for one path.
#[derive(Debug, Clone, PartialEq)]
pub struct LchSolve {
    /// The typed target `[L*, C*, h]`.
    pub lch: [f64; 3],
    /// The reference path (mm) the colour was solved for: the stone's own.
    pub path_mm: f64,
    /// `Delta E 2000` of the solved colour from the target.
    pub delta_e: f64,
    /// Whether the solver reached the target (within its tolerance).
    pub reachable: bool,
    /// The band rows `[centre_nm, width_nm, amplitude_per_mm]` (zero bands dropped; empty
    /// for a colourless solve).
    pub bands: Vec<[f32; 3]>,
    /// The nearest legacy triple of the solved colour at the reference path.
    pub triple: [f32; 3],
    /// sRGB (0..1) of the solved colour at each of [`SWATCH_PATHS_MM`], then the stone's own path.
    pub swatches: Vec<[f32; 3]>,
}

/// Solves `lch` for `path_mm`; `None` when `cancel` was set (a newer request superseded it).
#[must_use]
pub fn solve_lch(lch: [f64; 3], path_mm: f64, cancel: &AtomicBool) -> Option<LchSolve> {
    let target = BodyColorTarget { lch, path_mm };
    let solution = solve_body_color(&target, cancel).ok()?;
    let bands = body_color_bands(solution.amplitudes_per_mm)
        .iter()
        .map(|band| [band.center_nm, band.width_nm, band.peak])
        .collect();
    let swatches = swatches_for(&solution.amplitudes_per_mm, path_mm);
    let at_stone = swatches.last().copied().unwrap_or([1.0; 3]);
    let triple = nearest_legacy_rgb(srgb_to_lab(at_stone.map(f64::from)));
    Some(LchSolve {
        lch,
        path_mm,
        delta_e: solution.delta_e,
        reachable: solution.reachable,
        bands,
        triple,
        swatches,
    })
}

/// The colour of `amplitudes` (per mm) at 3 mm, 5 mm, 10 mm and `path_mm`.
fn swatches_for(amplitudes: &[f32; 7], path_mm: f64) -> Vec<[f32; 3]> {
    let mut paths = SWATCH_PATHS_MM.to_vec();
    paths.push(path_mm);
    body_color_swatches(amplitudes, &paths)
}

/// An sRGB swatch (0..1) as the bytes the UI paints.
#[must_use]
pub fn swatch_bytes(rgb: [f32; 3]) -> [u8; 3] {
    rgb.map(channel_byte)
}

/// One sRGB channel (0..1, clamped) as a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the channel is clamped to 0..=1, so the rounded byte is in 0..=255"
)]
fn channel_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The captions under the swatches: the fixed paths, then "This stone" with its own path.
#[must_use]
pub fn swatch_labels(path_mm: f64) -> Vec<String> {
    let mut labels: Vec<String> = SWATCH_PATHS_MM
        .iter()
        .map(|mm| format!("{mm:.0} mm"))
        .collect();
    labels.push(format!("This stone {path_mm:.1} mm"));
    labels
}

/// The badge under the swatches.
#[must_use]
pub fn badge_text(delta_e: f64, reachable: bool) -> String {
    if reachable {
        format!("Reachable (Delta E {delta_e:.1})")
    } else {
        format!(
            "Not fully reachable with the seven-band color model: the closest color is Delta E {delta_e:.1} away. Try less saturation or a lighter tone."
        )
    }
}

/// The reference path as a multiple of the girdle width. A copy of the desktop's
/// `physics_state::GIRDLE_PATH_FACTOR` (the physics colour editor's convention); a desktop
/// test pins the two together.
pub const GIRDLE_PATH_FACTOR: f64 = 1.5;
/// The reference path (mm) of a design with no girdle diameter; a copy of the desktop's
/// `physics_state::DEFAULT_PATH_MM`.
pub const DEFAULT_PATH_MM: f64 = 5.0;

/// The reference path (mm) of a design whose girdle is `girdle_diameter_mm` wide: the same
/// number the physics colour editor uses (`default_reference_path_mm`).
#[must_use]
pub fn reference_path_mm(girdle_diameter_mm: Option<f64>) -> f64 {
    girdle_diameter_mm
        .filter(|mm| *mm > 0.0)
        .map_or(DEFAULT_PATH_MM, |mm| mm * GIRDLE_PATH_FACTOR)
}

/// The stone width (mm) a solver path stands for.
///
/// It is `solver_path_mm / GIRDLE_PATH_FACTOR`, the inverse of [`reference_path_mm`] for a design
/// with a girdle. For callers that only hold the path (the web editor); the desktop knows the
/// girdle and uses [`stone_width_mm`].
#[must_use]
pub const fn stone_width_for_path(solver_path_mm: f64) -> f64 {
    solver_path_mm / GIRDLE_PATH_FACTOR
}

/// The width of the stone (mm) a design with `girdle_diameter_mm` is drawn at.
///
/// It is the girdle, or the 7 mm default (`PHYSICS_DEFAULT_STONE_WIDTH_MM`) while none is set,
/// which is also the width the render uses for it. This is the width a Custom Color stores as its
/// `reference_path_mm`.
#[must_use]
pub fn stone_width_mm(girdle_diameter_mm: Option<f64>) -> f64 {
    girdle_diameter_mm
        .filter(|mm| *mm > 0.0)
        .unwrap_or_else(|| f64::from(pleochroma::render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM))
}

/// `[L*, C*, h]` the editor opens on for a design's stored colour at `path_mm`.
///
/// From the bands when it has them, else from the triple; `None` for a design with the material's own
/// colour (the caller then keeps the editor's default).
///
/// The bands are decoded with the real codec (`GemMaterial::with_body_color_bands`) and the
/// color is evaluated from that spectrum, so the seed comes from the stored spectrum itself,
/// whatever its shape (a historical wavelength-domain row, an off-basis centre, duplicate
/// rows that add up). The solve that follows is a refit and is stored only on an edit
/// (desktop/web guards).
#[must_use]
pub fn lch_from_material(material: &MaterialSelection, path_mm: f64) -> Option<[f64; 3]> {
    if let Some(rows) = material
        .body_color_bands_override
        .as_deref()
        .filter(|rows| !rows.is_empty())
    {
        let gem = GemMaterial::new_custom("seed", 1.5, 0.0, 0.0, [0.0; 3])
            .with_body_color_bands(rows, 1.0);
        let srgb = body_colors(&gem.absorption, path_mm, Illuminant::D65)
            .unpolarised
            .srgb
            .map(|byte| f64::from(byte) / 255.0);
        return Some(lab_to_lch(srgb_to_lab(srgb)));
    }
    material
        .body_color_override
        .map(|triple| lab_to_lch(fantasy_lab(triple)))
}

/// The slider range of Saturation (C*): the picked colour is clamped into it.
pub const MAX_CHROMA: f64 = 130.0;

/// The L*C*h of a picked screen colour (sRGB 0..1), clamped into the sliders' ranges.
#[must_use]
pub fn lch_from_srgb(srgb: [f64; 3]) -> [f64; 3] {
    let [l, c, h] = lab_to_lch(srgb_to_lab(srgb));
    [
        l.clamp(0.0, 100.0),
        c.clamp(0.0, MAX_CHROMA),
        h.rem_euclid(360.0),
    ]
}

/// The selection with `solve` stored as the design's body colour (bands plus the nearest
/// triple), or `None` when the design already has exactly that (no undo step is added).
///
/// A plain color replaces the stone's pattern (the pattern would otherwise hide it).
#[must_use]
pub fn recoloured_with_solve(
    material: &MaterialSelection,
    solve: &LchSolve,
) -> Option<MaterialSelection> {
    let bands = (!solve.bands.is_empty()).then(|| solve.bands.clone());
    let next = material
        .clone()
        .with_color_pattern(None)
        .with_body_color_bands(Some(solve.triple), bands, None);
    (next != *material).then_some(next)
}

/// One color row of the Custom editor: a direction of the crystal the cutter sets a color for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxisRow {
    /// The row's name in Title Case ("Along the Axis").
    pub label: &'static str,
    /// A short name for a swatch caption ("Along").
    pub short: &'static str,
    /// One plain sentence for the tooltip: which direction this row colors.
    pub hint: &'static str,
}

/// The one row of an isotropic host.
const ISOTROPIC_ROWS: [AxisRow; 1] = [AxisRow {
    label: "Color",
    short: "Color",
    hint: "The one color of this stone. An isotropic material shows the same color in every direction.",
}];

/// The two rows of a uniaxial host: the ordinary and the extraordinary ray.
const UNIAXIAL_ROWS: [AxisRow; 2] = [
    AxisRow {
        label: "Along the Axis",
        short: "Along",
        hint: "The color the stone shows when you look along its crystal axis (the ordinary ray).",
    },
    AxisRow {
        label: "Across the Axis",
        short: "Across",
        hint: "The color the stone shows when you look across its crystal axis (the extraordinary ray).",
    },
];

/// The three rows of a biaxial host: the principal directions alpha, beta and gamma.
const BIAXIAL_ROWS: [AxisRow; 3] = [
    AxisRow {
        label: "Alpha",
        short: "Alpha",
        hint: "The color along the alpha direction of the crystal (the lowest of its three refractive indices).",
    },
    AxisRow {
        label: "Beta",
        short: "Beta",
        hint: "The color along the beta direction of the crystal (the middle refractive index).",
    },
    AxisRow {
        label: "Gamma",
        short: "Gamma",
        hint: "The color along the gamma direction of the crystal (the highest of its three refractive indices).",
    },
];

/// The rows the Custom editor shows for a host of `rank`: one, two or three. The order is the
/// tensor's own (`o_ray`, then `beta_ray` when there is one, then `e_ray`).
#[must_use]
pub const fn axis_rows(rank: TensorRank) -> &'static [AxisRow] {
    match rank {
        TensorRank::Isotropic => &ISOTROPIC_ROWS,
        TensorRank::Uniaxial => &UNIAXIAL_ROWS,
        TensorRank::Biaxial => &BIAXIAL_ROWS,
    }
}

/// The `[L*, C*, h]` of a body color (its gamut-mapped sRGB bytes), inside the sliders' ranges.
#[must_use]
pub fn lch_of_body_color(color: &BodyColor) -> [f64; 3] {
    lch_from_srgb(color.srgb.map(|byte| f64::from(byte) / 255.0))
}

/// A coarse name for a color: "Blue", "Yellow Green", "Colorless". Used in tooltips and the
/// row captions, so a cutter reads the direction colors of a pleochroic stone without a chart.
#[must_use]
pub fn color_family(lch: [f64; 3]) -> &'static str {
    let [lightness, chroma, hue] = lch;
    if chroma < 6.0 {
        return if lightness > 90.0 {
            "Colorless"
        } else if lightness < 25.0 {
            "Black"
        } else {
            "Grey"
        };
    }
    match hue.rem_euclid(360.0) {
        h if h < 20.0 => "Pink",
        h if h < 50.0 => "Red",
        h if h < 75.0 => "Orange",
        h if h < 105.0 => "Yellow",
        h if h < 130.0 => "Yellow Green",
        h if h < 170.0 => "Green",
        h if h < 210.0 => "Teal",
        h if h < 285.0 => "Blue",
        h if h < 325.0 => "Violet",
        _ => "Pink",
    }
}

/// The optical path (in the unit of `unit`'s amplitudes) the editor reads a tensor at: a
/// per-model-unit tensor at its face-up path for a stone `stone_width_mm` wide
/// (`face_up_swatch_path`, about one model unit), a per-millimetre one at the solver's own path.
fn seed_path(unit: AbsorptionUnit, stone_width_mm: f64, solver_path_mm: f64) -> f64 {
    match unit {
        AbsorptionUnit::ModelUnit => face_up_swatch_path(unit, to_f32_width(stone_width_mm)),
        AbsorptionUnit::PerMm => solver_path_mm,
    }
}

/// A stone width as the `f32` the face-up path takes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a stone width in millimetres is a small value; f32 is the precision the render uses"
)]
const fn to_f32_width(stone_width_mm: f64) -> f32 {
    stone_width_mm as f32
}

/// `[L*, C*, h]` of each direction of `tensor`, one entry per row of `rank` ([`axis_rows`]).
///
/// The directions are the ordinary ray, the `beta_ray` (a biaxial host only; a tensor without one
/// gives its ordinary ray, the `alpha == beta` case) and the extraordinary ray.
///
/// A `ModelUnit` tensor (every built-in host and its Material Default) is read at the face-up path
/// of a stone `stone_width_mm` wide, a `PerMm` one at `solver_path_mm`, so a stored Custom Color
/// seeds exactly what it solved and a built-in tensor is not read at five to ten times the
/// optical density the render gives it.
#[must_use]
pub fn axis_lch_from_tensor(
    tensor: &AbsorptionTensor,
    unit: AbsorptionUnit,
    rank: TensorRank,
    stone_width_mm: f64,
    solver_path_mm: f64,
) -> Vec<[f64; 3]> {
    let path_mm = seed_path(unit, stone_width_mm, solver_path_mm);
    let colors = body_colors(tensor, path_mm, Illuminant::D65);
    match rank {
        TensorRank::Isotropic => vec![lch_of_body_color(&colors.o_ray)],
        TensorRank::Uniaxial => vec![
            lch_of_body_color(&colors.o_ray),
            lch_of_body_color(&colors.e_ray),
        ],
        TensorRank::Biaxial => vec![
            lch_of_body_color(&colors.o_ray),
            lch_of_body_color(colors.beta_ray.as_ref().unwrap_or(&colors.o_ray)),
            lch_of_body_color(&colors.e_ray),
        ],
    }
}

/// The rows the Custom editor opens on for a design's stone: one `[L*, C*, h]` per row of the
/// host's rank.
///
/// * A color variant (a picked variant or an earlier Custom color): the color of each of its
///   directions, read in the variant's own unit ([`axis_lch_from_tensor`]).
/// * A legacy single color (the nine presets, the old L*C*h editor): that one color on every
///   row, which is what "Convert" starts from.
/// * Otherwise the host's own tensor, direction by direction in the host's own unit, so a small
///   hue change keeps the host's pleochroism and the editor never opens black.
///
/// `stone_width_mm` is the stone's width (`stone_width_mm`, the render's width for the view
/// target) and `solver_path_mm` the editor's solver path ([`reference_path_mm`]).
#[must_use]
pub fn seed_axis_lch(
    material: &MaterialSelection,
    host: &GemMaterial,
    stone_width_mm: f64,
    solver_path_mm: f64,
) -> Vec<[f64; 3]> {
    let rank = host_rank(host);
    if let Some(choice) = &material.color_variant_override {
        return axis_lch_from_tensor(
            &choice.absorption,
            choice.unit,
            rank,
            stone_width_mm,
            solver_path_mm,
        );
    }
    if let Some(lch) = lch_from_material(material, solver_path_mm) {
        return vec![lch; axis_rows(rank).len()];
    }
    axis_lch_from_tensor(
        &host.absorption,
        host.absorption_unit,
        rank,
        stone_width_mm,
        solver_path_mm,
    )
}

/// Solves every row of the Custom editor: `lch[i]` for row `i`, at `path_mm`.
///
/// A row whose target and path equal an earlier solve in `previous` (same index) is not solved
/// again, so moving one slider re-solves one row. `None` when `cancel` was set (a newer request
/// superseded this one).
#[must_use]
pub fn solve_axes(
    lch: &[[f64; 3]],
    previous: &[Option<LchSolve>],
    path_mm: f64,
    cancel: &AtomicBool,
) -> Option<Vec<LchSolve>> {
    lch.iter()
        .enumerate()
        .map(|(row, target)| {
            let kept = previous.get(row).and_then(Option::as_ref).filter(|solve| {
                solve.lch == *target && solve.path_mm.to_bits() == path_mm.to_bits()
            });
            kept.map_or_else(
                || solve_lch(*target, path_mm, cancel),
                |solve| Some(solve.clone()),
            )
        })
        .collect()
}

/// Peak (per millimetre) of the stand-in band of a colorless row next to a colored one: far below
/// anything visible (0.0001 per mm over 10 mm is a 0.1 % dip), yet it keeps every band set of
/// the tensor non-empty, as a variant requires (`VariantProblem::Empty`).
const NEGLIGIBLE_PEAK_PER_MM: f32 = 1.0e-4;

/// The bands of a solve as absorption bands: the solver's rows are energy-domain Gaussians
/// (`[centre_nm, sigma_cm1, amplitude_per_mm]`).
fn bands_of(solve: &LchSolve) -> Vec<AbsorptionBand> {
    solve
        .bands
        .iter()
        .map(|row| AbsorptionBand {
            center_nm: row[0],
            width_nm: row[1],
            peak: row[2],
            shape: BandShape::GaussianEnergy,
        })
        .collect()
}

/// The tensor of the Custom editor's rows: `solves[i]` is row `i` of `rank` ([`axis_rows`]).
///
/// The tensor has the rank of the host: isotropic for [`TensorRank::Isotropic`], `uniaxial` for
/// two rows, `biaxial` for three. `None` when the number of solves does not match the rank. A
/// colorless row beside a colored one gets a negligible stand-in band, so no band set is empty
/// while another is not.
#[must_use]
pub fn tensor_from_solves(rank: TensorRank, solves: &[LchSolve]) -> Option<AbsorptionTensor> {
    if solves.len() != axis_rows(rank).len() {
        return None;
    }
    let mut sets: Vec<Vec<AbsorptionBand>> = solves.iter().map(bands_of).collect();
    if sets.iter().any(|set| !set.is_empty()) {
        for set in &mut sets {
            if set.is_empty() {
                set.push(AbsorptionBand {
                    center_nm: 550.0,
                    width_nm: 1000.0,
                    peak: NEGLIGIBLE_PEAK_PER_MM,
                    shape: BandShape::GaussianEnergy,
                });
            }
        }
    }
    let mut sets = sets.into_iter();
    match (rank, sets.next(), sets.next(), sets.next()) {
        (TensorRank::Isotropic, Some(one), None, None) => Some(AbsorptionTensor::isotropic(one)),
        (TensorRank::Uniaxial, Some(o), Some(e), None) => Some(AbsorptionTensor::uniaxial(o, e)),
        (TensorRank::Biaxial, Some(alpha), Some(beta), Some(gamma)) => {
            Some(AbsorptionTensor::biaxial(alpha, beta, gamma))
        }
        _ => None,
    }
}

/// The Custom editor's rows as the [`VariantChoice`] a design stores.
///
/// Key [`CUSTOM_VARIANT_KEY`], label [`CUSTOM_VARIANT_LABEL`], the tensor of
/// [`tensor_from_solves`] (so a pleochroic host gets a pleochroic tensor), per millimetre.
/// `stone_width_mm` is the width of the stone the colors were solved for; it is stored as the
/// choice's `reference_path_mm`, which means a stone width for every variant (a path is the
/// solver's own number, [`reference_path_mm`]). `None` when `solves` does not match `rank`.
#[must_use]
pub fn custom_choice(
    rank: TensorRank,
    solves: &[LchSolve],
    stone_width_mm: f64,
) -> Option<VariantChoice> {
    let absorption = tensor_from_solves(rank, solves)?;
    Some(VariantChoice {
        key: CUSTOM_VARIANT_KEY.to_owned(),
        label: CUSTOM_VARIANT_LABEL.to_owned(),
        absorption,
        unit: AbsorptionUnit::PerMm,
        reference_path_mm: to_f32_width(stone_width_mm),
    })
}

/// The face-up color of `tensor` at `path_mm`, then the color of each direction (ordinary ray,
/// `beta_ray` if any, extraordinary ray), as sRGB `0..=1`: the swatches of the per-axis editor.
#[must_use]
pub fn direction_swatches(tensor: &AbsorptionTensor, path_mm: f64) -> Vec<[f32; 3]> {
    let colors = body_colors(tensor, path_mm, Illuminant::D65);
    let rgb = |color: &BodyColor| color.srgb.map(|byte| f32::from(byte) / 255.0);
    let mut swatches = vec![rgb(&colors.unpolarised), rgb(&colors.o_ray)];
    if let Some(beta) = &colors.beta_ray {
        swatches.push(rgb(beta));
    }
    swatches.push(rgb(&colors.e_ray));
    swatches
}

/// The selection with `choice` stored as the design's color, or `None` when the design already
/// has exactly that (no undo step is added).
///
/// A plain color replaces the stone's pattern (the pattern would otherwise hide it); the legacy
/// colors are dropped with it (see [`MaterialSelection::with_color_variant`]).
#[must_use]
pub fn recoloured_with_choice(
    material: &MaterialSelection,
    choice: VariantChoice,
) -> Option<MaterialSelection> {
    let next = material
        .clone()
        .with_color_pattern(None)
        .with_color_variant(Some(choice));
    (next != *material).then_some(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::{
        color::body_color::lch_to_lab,
        optics::absorption::{AbsorptionBand, AbsorptionTensor},
    };

    const BLUE: [f64; 3] = [50.0, 30.0, 265.0];

    fn solved(path_mm: f64) -> LchSolve {
        solve_lch(BLUE, path_mm, &AtomicBool::new(false)).expect("not cancelled")
    }

    fn lightness(srgb: [f32; 3]) -> f64 {
        srgb_to_lab(srgb.map(f64::from))[0]
    }

    #[test]
    fn a_solve_carries_bands_triple_and_four_swatches() {
        let solve = solved(5.0);
        assert!(!solve.bands.is_empty(), "a blue is not colourless");
        assert_eq!(solve.swatches.len(), SWATCH_PATHS_MM.len() + 1);
        assert_eq!(swatch_labels(5.0).len(), solve.swatches.len());
        assert!(solve.triple.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn the_same_colour_is_deeper_in_a_larger_stone() {
        let solve = solved(5.0);
        let [at3, at5, at10, _] = [
            solve.swatches[0],
            solve.swatches[1],
            solve.swatches[2],
            solve.swatches[3],
        ];
        assert!(lightness(at10) < lightness(at5) && lightness(at5) < lightness(at3));
    }

    #[test]
    fn the_swatch_at_the_stones_own_path_is_the_solved_colour() {
        let solve = solved(5.0);
        let at_five = solve.swatches[1];
        let at_stone = solve.swatches[3];
        for (a, b) in at_five.iter().zip(at_stone) {
            assert!((a - b).abs() < 1e-4, "{at_five:?} vs {at_stone:?}");
        }
        assert!(solve.reachable, "the typed blue is reachable at 5 mm");
    }

    #[test]
    fn a_cancelled_solve_returns_none() {
        assert!(solve_lch(BLUE, 5.0, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn applying_stores_bands_and_triple_and_a_repeat_is_not_an_edit() {
        let solve = solved(5.0);
        let plain = MaterialSelection::none();
        let stored = recoloured_with_solve(&plain, &solve).expect("a new colour is an edit");
        assert_eq!(stored.body_color_override, Some(solve.triple));
        assert_eq!(stored.body_color_bands_override, Some(solve.bands.clone()));
        assert!(recoloured_with_solve(&stored, &solve).is_none());
    }

    #[test]
    fn a_colourless_solve_stores_no_bands() {
        let mut solve = solved(5.0);
        solve.bands.clear();
        let stored = recoloured_with_solve(&MaterialSelection::none(), &solve);
        assert_eq!(stored.and_then(|m| m.body_color_bands_override), None);
    }

    #[test]
    fn reopening_seeds_the_sliders_from_the_stored_colour() {
        let solve = solved(5.0);
        let stored = recoloured_with_solve(&MaterialSelection::none(), &solve).expect("edit");
        let [l, c, h] = lch_from_material(&stored, 5.0).expect("has a colour");
        assert!((l - BLUE[0]).abs() < 6.0, "L* {l}");
        assert!((c - BLUE[1]).abs() < 10.0, "C* {c}");
        let hue_gap = ((h - BLUE[2] + 180.0).rem_euclid(360.0) - 180.0).abs();
        assert!(hue_gap < 15.0, "h {h}");
        assert_eq!(lch_from_material(&MaterialSelection::none(), 5.0), None);
    }

    /// The `L*a*b*` of a tensor's unpolarised body color at `path_mm` under D65.
    fn lab_of(tensor: &AbsorptionTensor, path_mm: f64) -> [f64; 3] {
        let srgb = body_colors(tensor, path_mm, Illuminant::D65)
            .unpolarised
            .srgb
            .map(|byte| f64::from(byte) / 255.0);
        srgb_to_lab(srgb)
    }

    fn with_rows(rows: &[[f32; 3]]) -> MaterialSelection {
        MaterialSelection::none().with_body_color_bands(None, Some(rows.to_vec()), None)
    }

    fn lab_distance(a: [f64; 3], b: [f64; 3]) -> f64 {
        let (d0, d1, d2) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
        d2.mul_add(d2, d1.mul_add(d1, d0 * d0)).sqrt()
    }

    #[test]
    fn a_wavelength_domain_row_seeds_from_its_own_spectrum() {
        let rows = [[620.0_f32, 45.0, 0.3]];
        let own = AbsorptionTensor::isotropic(vec![AbsorptionBand::new(620.0, 45.0, 0.3)]);
        let own_lab = lab_of(&own, 5.0);
        let [l, c, h] = lch_from_material(&with_rows(&rows), 5.0).expect("has a colour");
        let [own_l, _own_c, own_h] = lab_to_lch(own_lab);
        let hue_gap = ((h - own_h + 180.0).rem_euclid(360.0) - 180.0).abs();
        assert!(hue_gap < 5.0, "h {h} vs its own spectrum {own_h}");
        assert!((l - own_l).abs() < 2.0, "L* {l} vs {own_l}");
        assert!(c > 0.0);

        // The old seed: the 0.3 placed on the nearest basis band (620 nm, energy domain).
        let basis =
            AbsorptionTensor::isotropic(body_color_bands([0.0, 0.0, 0.0, 0.0, 0.0, 0.3, 0.0]));
        let basis_lab = lab_of(&basis, 5.0);
        assert!(
            lab_distance(own_lab, basis_lab) > 1.0,
            "the wavelength Gaussian {own_lab:?} and the basis band {basis_lab:?} are different spectra"
        );
    }

    #[test]
    fn an_off_basis_centre_still_seeds() {
        let [_l, c, _h] = lch_from_material(&with_rows(&[[700.0, 45.0, 1.0]]), 5.0)
            .expect("an off-basis row seeds");
        assert!(c > 0.0, "C* {c}");
    }

    #[test]
    fn duplicate_centres_add_up() {
        let row = [540.0_f32, 45.0, 0.1];
        let [one, _, _] = lch_from_material(&with_rows(&[row]), 5.0).expect("one row");
        let [two, _, _] = lch_from_material(&with_rows(&[row, row]), 5.0).expect("two rows");
        assert!(
            two < one,
            "two identical rows are darker: L* {two} vs {one}"
        );
    }

    #[test]
    fn a_triple_only_design_seeds_from_the_triple() {
        let with_triple = MaterialSelection::none().with_body_color(Some([0.2, 0.4, 2.8]));
        assert!(lch_from_material(&with_triple, 5.0).is_some());
    }

    #[test]
    fn a_picked_colour_is_clamped_into_the_slider_ranges() {
        let [l, c, h] = lch_from_srgb([0.0, 0.0, 1.0]);
        assert!((0.0..=100.0).contains(&l));
        assert!((0.0..=MAX_CHROMA).contains(&c));
        assert!((0.0..360.0).contains(&h));
        let round_trip = lch_to_lab(lch_from_srgb([0.5, 0.5, 0.5]));
        assert!(round_trip[0] > 40.0 && round_trip[0] < 60.0);
    }

    #[test]
    fn swatch_bytes_clamp_and_round() {
        assert_eq!(swatch_bytes([0.0, 1.0, 0.5]), [0, 255, 128]);
        assert_eq!(swatch_bytes([-0.2, 1.7, 0.0]), [0, 255, 0]);
    }

    #[test]
    fn the_badge_names_the_distance_when_not_reachable() {
        assert!(badge_text(0.8, true).starts_with("Reachable"));
        assert!(badge_text(7.2, false).contains("7.2"));
    }

    #[test]
    fn the_badge_does_not_blame_the_size() {
        assert!(!badge_text(7.2, false).contains("size"));
    }

    #[test]
    fn applying_a_solve_clears_the_stone_pattern() {
        use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
        let pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        let patterned = MaterialSelection::none().with_color_pattern(Some(pattern));
        assert!(patterned.color_pattern_override.is_some());
        let stored = recoloured_with_solve(&patterned, &solved(5.0)).expect("an edit");
        assert!(stored.color_pattern_override.is_none());
    }

    #[test]
    fn the_reference_path_follows_the_girdle() {
        assert!(reference_path_mm(Some(10.0)) > reference_path_mm(Some(5.0)));
        assert!(reference_path_mm(None) > 0.0);
    }

    /// Review fix (absorption units): a stored solve resolves to a `PerMm` material, the
    /// renderer supplies the scale (girdle 5 mm over a model width of 2 = 2.5 mm per model
    /// unit), and the colour of that material at the reference path (7.5 mm = 3 model units)
    /// is the swatch the editor showed, within Delta E 2000 of 2.
    #[test]
    fn a_stored_solve_renders_as_a_per_mm_material_matching_the_swatch() {
        use crate::material_lookup::{EditorMaterialLookup, resolved_gem_material};
        use pleochroma::{
            color::body_color::delta_e_2000,
            optics::materials::AbsorptionUnit,
            render_setup::{MaterialOverrides, apply_material_overrides},
        };
        let path_mm = 7.5;
        let solve = solved(path_mm);
        let base = MaterialSelection {
            name: Some("Diamond".to_string()),
            ..MaterialSelection::none()
        };
        let stored = recoloured_with_solve(&base, &solve).expect("a new colour is an edit");
        let gem = resolved_gem_material(&stored, &EditorMaterialLookup::new(&[]));
        assert_eq!(gem.absorption_unit, AbsorptionUnit::PerMm);

        let sized = apply_material_overrides(
            gem,
            &MaterialOverrides {
                inclusion_sigma_s: 0.0,
                c_axis_override: None,
                alpha_axis_override: None,
                edge_rounding_radius: 0.0,
                stone_width_mm: 5.0,
            },
            Some(2.0),
        );
        assert!((sized.absorption_path_scale - 2.5).abs() < 1e-6);
        let model_path = 3.0_f64;
        let mm = f64::from(sized.absorption_path_scale) * model_path;
        let rendered = body_colors(&sized.absorption, mm, Illuminant::D65).unpolarised;
        let rendered_lab = srgb_to_lab(rendered.srgb.map(|v| f64::from(v) / 255.0));
        let swatch = *solve.swatches.last().expect("the stone's own swatch");
        let swatch_lab = srgb_to_lab(swatch.map(f64::from));
        let delta_e = delta_e_2000(rendered_lab, swatch_lab);
        assert!(delta_e < 2.0, "rendered vs swatch Delta E {delta_e}");
    }

    /// Review fix (absorption units): a design with `LCh` bands AND an old stored
    /// `absorption_path_scale_override` (24.5, written by builds that stored the scale) now
    /// renders at `W / model_width` -- the stored number is ignored for a per-millimetre
    /// material, the renderer supplies the scale. THIS INTENTIONALLY CHANGES OLD FILES: they
    /// rendered with the stored 24.5 whatever the stone size.
    #[test]
    fn an_old_stored_path_scale_is_ignored_for_a_banded_colour() {
        use crate::material_lookup::{EditorMaterialLookup, resolved_gem_material};
        use pleochroma::render_setup::{MaterialOverrides, apply_material_overrides};
        let solve = solved(5.0);
        let old_file = MaterialSelection {
            name: Some("Diamond".to_string()),
            ..MaterialSelection::none()
        }
        .with_body_color_bands(Some(solve.triple), Some(solve.bands), Some(24.5));
        assert_eq!(old_file.absorption_path_scale_override, Some(24.5));
        let gem = resolved_gem_material(&old_file, &EditorMaterialLookup::new(&[]));
        // Before the render step the stored scale is still on the material...
        assert_eq!(gem.absorption_path_scale, 24.5);
        let overrides = |stone_width_mm| MaterialOverrides {
            inclusion_sigma_s: 0.0,
            c_axis_override: None,
            alpha_axis_override: None,
            edge_rounding_radius: 0.0,
            stone_width_mm,
        };
        // ...and the render replaces it: W / model_width, 7 mm while no size is set.
        let sized = apply_material_overrides(gem.clone(), &overrides(10.87), Some(2.0));
        assert!((f64::from(sized.absorption_path_scale) - 10.87 / 2.0).abs() < 1e-5);
        let unsized_ = apply_material_overrides(gem, &overrides(0.0), Some(2.0));
        assert!((unsized_.absorption_path_scale - 3.5).abs() < 1e-6);
    }

    #[test]
    fn a_preset_pick_after_a_custom_one_drops_the_bands() {
        let solve = solved(5.0);
        let stored = recoloured_with_solve(&MaterialSelection::none(), &solve).expect("edit");
        let preset = stored.with_body_color(Some([0.2, 0.4, 2.8]));
        assert_eq!(preset.body_color_bands_override, None);
    }

    const GREEN: [f64; 3] = [60.0, 35.0, 140.0];

    fn solved_at(lch: [f64; 3]) -> LchSolve {
        solve_lch(lch, 5.0, &AtomicBool::new(false)).expect("not cancelled")
    }

    #[test]
    fn the_rows_follow_the_hosts_rank() {
        let labels =
            |rank| -> Vec<&'static str> { axis_rows(rank).iter().map(|row| row.label).collect() };
        assert_eq!(labels(TensorRank::Isotropic), ["Color"]);
        assert_eq!(
            labels(TensorRank::Uniaxial),
            ["Along the Axis", "Across the Axis"]
        );
        assert_eq!(labels(TensorRank::Biaxial), ["Alpha", "Beta", "Gamma"]);
        for rank in [
            TensorRank::Isotropic,
            TensorRank::Uniaxial,
            TensorRank::Biaxial,
        ] {
            for row in axis_rows(rank) {
                assert!(!row.hint.is_empty() && !row.short.is_empty());
                assert!(!row.hint.contains("colour"), "user-facing text says color");
            }
        }
    }

    /// Custom on a uniaxial host (Sapphire) is a pleochroic tensor of the host's rank, on an
    /// isotropic host (Diamond) an isotropic one: a pleochroic stone never becomes isotropic.
    #[test]
    fn a_custom_color_has_the_rank_of_its_host() {
        use pleochroma::optics::materials::variants::{
            ColorVariant, VariantProvenance, tensor_rank,
        };
        let sapphire = GemMaterial::sapphire();
        let diamond = GemMaterial::diamond();
        let solves = [solved_at(BLUE), solved_at(GREEN)];

        let choice = custom_choice(host_rank(&sapphire), &solves, 5.0).expect("two rows");
        assert!(choice.absorption.is_pleochroic);
        assert!(choice.absorption.beta_ray.is_none());
        assert_eq!(tensor_rank(&choice.absorption), TensorRank::Uniaxial);
        assert_eq!(choice.key, CUSTOM_VARIANT_KEY);
        assert_eq!(choice.label, CUSTOM_VARIANT_LABEL);
        assert_eq!(choice.unit, AbsorptionUnit::PerMm);
        // The choice stores the stone width it was solved for (one meaning for every variant).
        assert!((choice.reference_path_mm - 5.0).abs() < 1e-6);
        // The two directions carry the two picked colors, not one color twice.
        assert_ne!(choice.absorption.o_ray, choice.absorption.e_ray);
        // The result is a variant the host accepts.
        let as_variant = ColorVariant {
            key: choice.key.clone(),
            label: choice.label.clone(),
            material: sapphire.name.clone(),
            absorption: choice.absorption.clone(),
            unit: choice.unit,
            reference_path_mm: choice.reference_path_mm,
            provenance: VariantProvenance::User,
        };
        assert_eq!(as_variant.validate_for(&sapphire), Ok(()));

        let one = custom_choice(host_rank(&diamond), &solves[..1], 5.0).expect("one row");
        assert!(!one.absorption.is_pleochroic);
        assert_eq!(tensor_rank(&one.absorption), TensorRank::Isotropic);
        // A solve count that does not match the rank is refused, never padded.
        assert!(custom_choice(TensorRank::Biaxial, &solves, 5.0).is_none());
        assert!(custom_choice(TensorRank::Isotropic, &solves, 5.0).is_none());
    }

    #[test]
    fn a_biaxial_host_gets_three_sets() {
        let solves = [
            solved_at(BLUE),
            solved_at(GREEN),
            solved_at([55.0, 40.0, 30.0]),
        ];
        let choice = custom_choice(TensorRank::Biaxial, &solves, 5.0).expect("three rows");
        assert!(choice.absorption.is_pleochroic);
        assert!(choice.absorption.beta_ray.is_some());
    }

    /// One colorless direction beside a colored one still gives a valid variant: no band set is
    /// empty while another is not.
    #[test]
    fn a_colorless_row_beside_a_colored_one_stays_valid() {
        // A solve that found nothing to absorb (`bands` is empty for a colorless target).
        let colorless = || {
            let mut solve = solved_at(BLUE);
            solve.bands.clear();
            solve
        };
        let tensor = tensor_from_solves(TensorRank::Uniaxial, &[solved_at(BLUE), colorless()])
            .expect("two rows");
        assert!(!tensor.o_ray.is_empty() && !tensor.e_ray.is_empty());
        // Two colorless rows stay a colorless (empty) pleochroic tensor.
        let none = tensor_from_solves(TensorRank::Uniaxial, &[colorless(), colorless()])
            .expect("two rows");
        assert!(none.o_ray.is_empty() && none.e_ray.is_empty() && none.is_pleochroic);
    }

    #[test]
    fn the_seed_of_a_pleochroic_host_keeps_its_two_colors() {
        let sapphire = GemMaterial::sapphire();
        let seed = seed_axis_lch(&MaterialSelection::none(), &sapphire, 7.0, 10.5);
        assert_eq!(seed.len(), 2);
        assert_ne!(seed[0], seed[1], "Sapphire's two rays are different colors");
        assert_eq!(
            seed,
            axis_lch_from_tensor(
                &sapphire.absorption,
                sapphire.absorption_unit,
                TensorRank::Uniaxial,
                7.0,
                10.5
            )
        );
        // An isotropic host seeds one row.
        assert_eq!(
            seed_axis_lch(
                &MaterialSelection::none(),
                &GemMaterial::diamond(),
                7.0,
                10.5
            )
            .len(),
            1
        );
    }

    /// Review finding 1 (P1): Sapphire's own tensor is per model unit, so the seed reads it at the
    /// face-up path of the stone (one unit at 7 mm), never at the solver's millimetre path, which
    /// is 5 to 10 times the optical density the render gives it and opened every built-in host
    /// black. The seed is the color of each ray at that path, and not dark.
    #[test]
    fn a_built_in_hosts_seed_is_its_face_up_color_not_black() {
        let sapphire = GemMaterial::sapphire();
        assert_eq!(sapphire.absorption_unit, AbsorptionUnit::ModelUnit);
        let seed = seed_axis_lch(&MaterialSelection::none(), &sapphire, 7.0, 10.5);
        let colors = body_colors(&sapphire.absorption, 1.0, Illuminant::D65);
        let expected = [
            lch_of_body_color(&colors.o_ray),
            lch_of_body_color(&colors.e_ray),
        ];
        assert_eq!(seed.len(), 2);
        for (row, (got, want)) in seed.iter().zip(expected).enumerate() {
            assert!(
                (got[0] - want[0]).abs() < 1.0,
                "row {row}: L* {} vs {}",
                got[0],
                want[0]
            );
            assert!(got[0] > 20.0, "row {row} opens black: L* {}", got[0]);
        }
        // A wider stone is deeper (the seed follows the width, like the render).
        let wide = seed_axis_lch(&MaterialSelection::none(), &sapphire, 14.0, 21.0);
        assert!(
            wide[0][0] < seed[0][0],
            "L* {} vs {}",
            wide[0][0],
            seed[0][0]
        );
    }

    /// A per-millimetre tensor (a stored Custom Color) is read at the solver's path, so it seeds
    /// what it solved; the width does not move it.
    #[test]
    fn a_per_millimetre_tensor_is_read_at_the_solver_path() {
        let tensor = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::energy(450.0, 3000.0, 0.08)],
            vec![AbsorptionBand::energy(620.0, 3000.0, 0.05)],
        );
        let read = |width, path| {
            axis_lch_from_tensor(
                &tensor,
                AbsorptionUnit::PerMm,
                TensorRank::Uniaxial,
                width,
                path,
            )
        };
        assert_eq!(read(7.0, 10.5), read(14.0, 10.5));
        assert_ne!(read(7.0, 10.5), read(7.0, 5.0));
    }

    #[test]
    fn the_seed_of_a_variant_is_the_color_of_its_directions() {
        let sapphire = GemMaterial::sapphire();
        let tensor = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::energy(450.0, 3000.0, 0.8)],
            vec![AbsorptionBand::energy(620.0, 3000.0, 0.5)],
        );
        let choice = VariantChoice {
            key: "sapphire.test".to_owned(),
            label: "Test".to_owned(),
            absorption: tensor.clone(),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
        };
        let selection = MaterialSelection::none().with_color_variant(Some(choice));
        assert_eq!(
            seed_axis_lch(&selection, &sapphire, 7.0, 10.5),
            axis_lch_from_tensor(
                &tensor,
                AbsorptionUnit::PerMm,
                TensorRank::Uniaxial,
                7.0,
                10.5
            )
        );
    }

    /// A per-model-unit variant snapshot (a copy of a Material Default made before copies were
    /// converted) is read in its own unit too, so it does not open black either.
    #[test]
    fn the_seed_of_a_model_unit_variant_is_read_in_its_own_unit() {
        let sapphire = GemMaterial::sapphire();
        let choice = VariantChoice {
            key: "user.old".to_owned(),
            label: "Old Copy".to_owned(),
            absorption: sapphire.absorption.clone(),
            unit: AbsorptionUnit::ModelUnit,
            reference_path_mm: 7.0,
        };
        let selection = MaterialSelection::none().with_color_variant(Some(choice));
        let seed = seed_axis_lch(&selection, &sapphire, 7.0, 10.5);
        assert!(seed.iter().all(|row| row[0] > 20.0), "{seed:?}");
    }

    #[test]
    fn the_stone_width_helpers_invert_the_reference_path() {
        for girdle in [4.0, 7.0, 10.87] {
            let path = reference_path_mm(Some(girdle));
            assert!((stone_width_for_path(path) - girdle).abs() < 1e-12);
            assert!((stone_width_mm(Some(girdle)) - girdle).abs() < 1e-12);
        }
        assert!((stone_width_mm(None) - 7.0).abs() < 1e-12);
        assert!((stone_width_mm(Some(0.0)) - 7.0).abs() < 1e-12);
    }

    /// "Convert": a legacy single color seeds every row of a pleochroic host with it.
    #[test]
    fn a_legacy_color_seeds_every_row() {
        let legacy = MaterialSelection::none().with_body_color(Some([0.2, 0.4, 2.8]));
        let seed = seed_axis_lch(&legacy, &GemMaterial::sapphire(), 3.5, 5.0);
        assert_eq!(seed.len(), 2);
        assert_eq!(seed[0], seed[1]);
        assert_eq!(Some(seed[0]), lch_from_material(&legacy, 5.0));
    }

    #[test]
    fn solving_the_rows_reuses_an_unchanged_one() {
        let cancel = AtomicBool::new(false);
        let first = solve_axes(&[BLUE, GREEN], &[], 5.0, &cancel).expect("not cancelled");
        assert_eq!(first.len(), 2);
        let previous: Vec<Option<LchSolve>> = first.iter().cloned().map(Some).collect();
        let moved = [BLUE, [60.0, 35.0, 150.0]];
        let second = solve_axes(&moved, &previous, 5.0, &cancel).expect("not cancelled");
        assert_eq!(second[0], first[0], "the unchanged row is kept");
        assert_ne!(second[1], first[1]);
        assert_eq!(second[1].lch, moved[1]);
        assert!(solve_axes(&moved, &[], 5.0, &AtomicBool::new(true)).is_none());
    }

    #[test]
    fn the_direction_swatches_lead_with_the_face_up_color() {
        let tensor = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::energy(450.0, 3000.0, 0.8)],
            vec![AbsorptionBand::energy(620.0, 3000.0, 0.5)],
        );
        assert_eq!(direction_swatches(&tensor, 5.0).len(), 3);
        let biaxial =
            AbsorptionTensor::biaxial(tensor.o_ray.clone(), tensor.o_ray.clone(), tensor.e_ray);
        assert_eq!(direction_swatches(&biaxial, 5.0).len(), 4);
        assert_eq!(
            direction_swatches(&AbsorptionTensor::isotropic(Vec::new()), 5.0).len(),
            3,
            "the colorless tensor also gives face-up, o and e"
        );
    }

    #[test]
    fn a_stored_custom_color_replaces_the_pattern_and_a_repeat_is_not_an_edit() {
        use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
        let solves = [solved_at(BLUE), solved_at(GREEN)];
        let choice = custom_choice(TensorRank::Uniaxial, &solves, 5.0).expect("two rows");
        let patterned = MaterialSelection::none().with_color_pattern(Some(template(
            TemplateId::Watermelon,
            PatternAxis::TABLE_NORMAL,
        )));
        let stored = recoloured_with_choice(&patterned, choice.clone()).expect("an edit");
        assert!(stored.color_pattern_override.is_none());
        assert_eq!(stored.color_variant_override.as_ref(), Some(&choice));
        assert!(recoloured_with_choice(&stored, choice).is_none());
    }

    #[test]
    fn color_families_name_the_basic_hues() {
        assert_eq!(color_family([50.0, 30.0, 265.0]), "Blue");
        assert_eq!(color_family([60.0, 35.0, 140.0]), "Green");
        assert_eq!(color_family([85.0, 60.0, 95.0]), "Yellow");
        assert_eq!(color_family([50.0, 60.0, 35.0]), "Red");
        assert_eq!(color_family([98.0, 2.0, 0.0]), "Colorless");
        assert_eq!(color_family([50.0, 2.0, 0.0]), "Grey");
        assert_eq!(color_family([50.0, 30.0, 625.0]), "Blue", "the hue wraps");
    }

    #[test]
    fn a_body_colors_lch_is_inside_the_slider_ranges() {
        let tensor = AbsorptionTensor::isotropic(vec![AbsorptionBand::energy(450.0, 3000.0, 2.0)]);
        let color = body_colors(&tensor, 5.0, Illuminant::D65).unpolarised;
        let [l, c, h] = lch_of_body_color(&color);
        assert!((0.0..=100.0).contains(&l) && (0.0..=MAX_CHROMA).contains(&c));
        assert!((0.0..360.0).contains(&h));
    }
}
