//! `metrics`: the optical and geometric figures of a design, as text, JSON or one CSV row.
//!
//! The optical figures are the ones the Optimize tab and the sweep score with: one table-up
//! measurement under the lighting preset at the canonical light pose. With `--tilt` the tilt
//! averages are the mean over every tilt to 90 degrees on four axes (181 angles each, the
//! scoring tools' figure); `--tilt-range DEG` (15, 20, 30, 45, 60 or 90) narrows the mean to the
//! tilts up to that range (`average_axis_profiles_within`), as the app's Tilt Average Range
//! does for Compare and Baseline, and the report then names the range. The command line never
//! reads the app's settings file. With `--ri-range` the stone is traced across refractive
//! indices and the report adds the range it works in (the `ri_report` module). The proportions
//! and the yield come from the solid.
//!
//! A design with concave tiers is measured as the stone with its tools, the stone the Compare
//! window draws and Optimize scores (`Design::tools_for_scoring`, `StoneGeometry`): the optics
//! see the grooves and dimples, and the volume (so Volume over width cubed and the yield) is
//! the carved volume. The widths, heights and the table stay the flat stone's, as in the cut
//! sheet. A design without concave tiers has no tools and its figures are exactly the flat
//! stone's.
//!
//! Frosted facets and a frosted girdle are measured too (`Design::facet_finishes_from_solved`):
//! they scatter the traced light as they do in the render. A frosted stone's figures are a
//! sampled estimate, repeatable for the same design but not smooth in the angles. A design
//! without frosting is exactly the polished stone's.
//!
//! The table-up group also carries Contrast and the zone shares of the face-up view
//! (`GemOpticalMetrics::aset`): the percentages of the stone's face whose light leaves in the red
//! (45-75 deg), green (0-45 deg) or blue (75-90 deg) zone of the Contrast view (ASET-style)
//! lighting, and the dark share, split into leak (out through the pavilion, the windowing rays)
//! and lost (no exit within the bounce budget, or out sideways). Contrast is Pleochroma's own
//! measure, 0 to 100: the mean difference in Blue share between neighbouring spots of the 18 x 18
//! grid. `--aset-grid` prints those Blue shares under the figures, so Contrast can be recomputed
//! by hand. ASET is a registered trademark of the Gemological Institute of America, Inc. (GIA).
//! Pleochroma is not affiliated with or endorsed by GIA or the American Gem Society.
//!
//! The girdle is given twice. `girdle_pct_of_width` is the band, the full height of the girdle
//! faces (`StoneProportions`), the figure the total depth adds up with. `girdle_line_pct_of_width`
//! is the girdle read at its line, as the editor's proportion verdict and the cutting sheet read
//! it (`pleochroma_editor::view_model::girdle_reading::GirdleReading`, its thinnest end), and is
//! empty without a girdle line. It is the first of the appended CSV columns
//! ([`APPENDED_CSV_KEYS`]), which follow every older column, so the older columns keep their place.

use super::{deliver, key_values, list_block, open};
use crate::{
    args::{MetricsArgs, ReportFormat, lighting_name},
    format::{
        INDEX_DECIMALS, PERCENT_DECIMALS, csv_line, fixed, fixed_or_na, json_number, json_optional,
        json_text,
    },
    load::Loaded,
    materials::{Catalogue, Scoring, resolve_scoring},
    outcome::{CliError, CommandResult},
    stone::{Analysis, analyze},
};
use pleochroma::{
    color::metrics::{
        ASET_GRID, AsetAreaShares, AsetSpotGrid, FinishedStone, GemOpticalMetrics,
        evaluate_all_axes_profiles_finished_threaded, evaluate_aset_spot_grid_finished,
        evaluate_gem_optical_metrics_finished, pattern_contrast,
    },
    geometry::{
        StoneGeometry,
        meet_solver::SolvedTier,
        stone_metrics::{StoneProportions, measure_solid},
    },
    optics::{LightingPreset, materials::GemMaterial, raytracer::EnvironmentSource},
};
use pleochroma_cut_core::{
    Design,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
    volumetric_yield,
};
use pleochroma_editor::{
    metric_deltas::{DEFAULT_DISPLAY_TILT_WINDOW_DEG, average_axis_profiles_within},
    optimize_view::facet_count_from_solved,
    ri_range::RiRange,
    solve_policy::design_to_gpu_planes_from_solved,
    sweep::TiltAverages,
    view_model::girdle_reading::GirdleReading,
};
use serde_json::{Map, Value, json};
use std::fmt::Write as _;

/// The heading of the table-up figures.
const GROUP_TABLE_UP: &str = "Optics, table up";

/// The heading of the tilt averages over every tilt, the default range.
const GROUP_TILT: &str = "Optics, tilt average over 4 axes and 181 angles";

/// The heading of the tilt averages for a range of `range_deg` each way: the heading of every
/// tilt at 90, else one that names the range and its angle count (2 x range + 1). A table of
/// `&'static str`, because a figure's group is static.
fn tilt_group(range_deg: u8) -> &'static str {
    match range_deg {
        15 => "Optics, tilt average to ±15° over 4 axes and 31 angles",
        20 => "Optics, tilt average to ±20° over 4 axes and 41 angles",
        30 => "Optics, tilt average to ±30° over 4 axes and 61 angles",
        45 => "Optics, tilt average to ±45° over 4 axes and 91 angles",
        60 => "Optics, tilt average to ±60° over 4 axes and 121 angles",
        _ => GROUP_TILT,
    }
}

/// The heading of the proportions.
const GROUP_PROPORTIONS: &str = "Proportions";

/// The heading of the counts and the yield.
const GROUP_STONE: &str = "Stone";

/// The camera pitch of the table-up figures (90 degrees): looking straight down on the table,
/// as the sweep and Optimize's fast score do.
const TABLE_UP_PITCH: f32 = std::f32::consts::FRAC_PI_2;

/// The key of the girdle read at its line (see [`girdle_line_percent`]).
const GIRDLE_LINE_KEY: &str = "girdle_line_pct_of_width";

/// The key of Contrast, the mean neighbour difference of the Blue shares.
const CONTRAST_KEY: &str = "table_up_contrast";

/// The caption printed above the grid of `--aset-grid`.
const GRID_CAPTION: &str = "Blue-zone (ASET-style) share per spot, percent (18 x 18, as seen face-up; '.' = off the stone):";

/// Figures placed last in the CSV row, in this order and after every older column, so a script
/// that reads the columns by position keeps working. Text and JSON keep them beside their kin.
const APPENDED_CSV_KEYS: [&str; 8] = [
    GIRDLE_LINE_KEY,
    CONTRAST_KEY,
    "table_up_aset_red_pct",
    "table_up_aset_green_pct",
    "table_up_aset_blue_pct",
    "table_up_aset_dark_pct",
    "table_up_aset_leak_pct",
    "table_up_aset_lost_pct",
];

/// What one figure reads as.
enum Reading {
    /// A whole number.
    Count(usize),
    /// A measurement with a fixed number of decimals; `None` when it cannot be read off this
    /// stone (a design with no girdle has no girdle thickness).
    Number { value: Option<f64>, decimals: usize },
}

/// One figure of the report.
struct Figure {
    /// The group it is printed under.
    group: &'static str,
    /// The JSON key and the CSV column.
    key: &'static str,
    /// The text label.
    label: &'static str,
    /// The unit printed after the value in text (`%`, or nothing).
    unit: &'static str,
    /// The value.
    reading: Reading,
}

impl Figure {
    const fn percent(
        group: &'static str,
        key: &'static str,
        label: &'static str,
        value: Option<f64>,
    ) -> Self {
        Self {
            group,
            key,
            label,
            unit: "%",
            reading: Reading::Number {
                value,
                decimals: PERCENT_DECIMALS,
            },
        }
    }

    const fn plain(
        group: &'static str,
        key: &'static str,
        label: &'static str,
        value: Option<f64>,
    ) -> Self {
        Self {
            unit: "",
            ..Self::percent(group, key, label, value)
        }
    }

    const fn count(
        group: &'static str,
        key: &'static str,
        label: &'static str,
        value: usize,
    ) -> Self {
        Self {
            group,
            key,
            label,
            unit: "",
            reading: Reading::Count(value),
        }
    }

    /// The value as text, without its unit.
    fn value_text(&self) -> String {
        match &self.reading {
            Reading::Count(count) => count.to_string(),
            Reading::Number { value, decimals } => fixed_or_na(*value, *decimals),
        }
    }

    /// The value and its unit, as the text report prints them.
    fn text(&self) -> String {
        if self.unit.is_empty() {
            self.value_text()
        } else {
            format!("{} {}", self.value_text(), self.unit)
        }
    }

    fn json(&self) -> Value {
        match &self.reading {
            Reading::Count(count) => Value::from(*count),
            Reading::Number { value, decimals } => {
                json_optional(*value, i32::try_from(*decimals).unwrap_or(2))
            }
        }
    }

    fn csv(&self) -> String {
        match &self.reading {
            Reading::Count(count) => count.to_string(),
            Reading::Number { value, decimals } => {
                value.map_or_else(String::new, |v| fixed(v, *decimals))
            }
        }
    }
}

/// Everything measured off one stone.
struct Measured {
    table_up: GemOpticalMetrics,
    /// The Blue share of every spot of the face-up grid, when it was asked for (`--aset-grid`).
    aset_grid: Option<AsetSpotGrid>,
    tilt: Option<TiltAverages>,
    /// The range the tilt averages cover, in degrees each way (90: every tilt).
    tilt_range_deg: u8,
    proportions: StoneProportions,
    /// The thinnest girdle thickness at its line, percent of the width ([`girdle_line_percent`]).
    girdle_line_pct: Option<f64>,
    vol_w3: Option<f64>,
    depth_pct: Option<f64>,
    yield_pct: Option<f64>,
    facets: usize,
    tiers: usize,
    warnings: usize,
}

/// [`measure_within`] with the tilt averages over every tilt when `tilt` is set.
#[cfg(test)]
fn measure(
    design: &Design,
    analysis: &Analysis,
    solved: &[SolvedTier],
    scoring: &Scoring,
    lighting: LightingPreset,
    tilt: bool,
    aset_grid: bool,
) -> Result<Measured, CliError> {
    let range = tilt.then_some(DEFAULT_DISPLAY_TILT_WINDOW_DEG);
    measure_within(
        design, analysis, solved, scoring, lighting, range, aset_grid,
    )
}

/// Measures the stone of `design`, whose solve is `solved` and whose analysis is `analysis`.
/// `tilt_range` also averages the tilts up to that many degrees each way; `aset_grid` also reads
/// the grid of Blue shares Contrast is computed from.
fn measure_within(
    design: &Design,
    analysis: &Analysis,
    solved: &[SolvedTier],
    scoring: &Scoring,
    lighting: LightingPreset,
    tilt_range: Option<u8>,
    aset_grid: bool,
) -> Result<Measured, CliError> {
    let planes_gpu = design_to_gpu_planes_from_solved(design, solved);
    let planes = design.planes_from_solved(solved);
    // The concave tools of the design (none for a flat design), placed on the flat stone: the
    // stone the optics and the volume are read off.
    let tools = design.tools_for_scoring(&planes);
    let finishes = design.facet_finishes_from_solved(solved, &planes_gpu);
    let stone = FinishedStone::new(
        StoneGeometry {
            planes: &planes_gpu,
            tools: &tools,
        },
        &finishes,
    );
    // Sized by the design's girdle diameter, like the render and `optimize`.
    let gem = &scoring.sized_gem(design, &planes_gpu);
    let environment = lighting.studio(1.0, CANONICAL_LIGHT_YAW, CANONICAL_LIGHT_PITCH);
    let table_up =
        evaluate_gem_optical_metrics_finished(stone, gem, 0.0, TABLE_UP_PITCH, environment);
    let aset_grid = if aset_grid {
        evaluate_aset_spot_grid_finished(stone, gem, 0.0, TABLE_UP_PITCH, environment)
    } else {
        None
    };
    let tilt = tilt_range.map(|range| tilt_average(stone, gem, environment, range));
    let solid = measure_solid(&planes)
        .ok_or_else(|| CliError::design("the stone cannot be measured: it has no usable volume"))?;
    let mesh = analysis
        .mesh
        .as_ref()
        .ok_or_else(|| CliError::design("the stone has no solid to measure"))?;
    let width = solid.width_axis;
    // The carved volume (the flat one without tools, or when the tools leave no closed stone),
    // the measure the cut sheet and the yield report read.
    let volume =
        Design::carved_metrics(Some(solid), &planes, &tools).map_or(solid.volume, |m| m.volume);
    let (vol_w3, depth_pct) = if width > 1e-9 {
        (
            Some(volume / (width * width * width)),
            Some(100.0 * solid.total_height / width),
        )
    } else {
        (None, None)
    };
    let yield_pct = measure_solid(&design.preform.planes())
        .and_then(|rough| volumetric_yield(volume, rough.volume))
        .map(|fraction| fraction * 100.0);
    let proportions = StoneProportions::from_solid(&solid, mesh, &planes);
    let girdle_line_pct = girdle_line_percent(GirdleReading::of(design, solved, &proportions));
    Ok(Measured {
        table_up,
        aset_grid,
        tilt,
        tilt_range_deg: tilt_range.unwrap_or(DEFAULT_DISPLAY_TILT_WINDOW_DEG),
        proportions,
        girdle_line_pct,
        vol_w3,
        depth_pct,
        yield_pct,
        facets: facet_count_from_solved(design, solved),
        tiers: design.tiers.len(),
        warnings: analysis.warnings.len(),
    })
}

/// The tilt average of `stone`: the mean over the tilts up to `range_deg` each way on the four
/// axes, from the same 724 poses the scoring tools trace. At 90 it is the mean of every tilt, bit
/// for bit what `average_axis_profiles` gives.
fn tilt_average(
    stone: FinishedStone<'_>,
    gem: &GemMaterial,
    environment: EnvironmentSource<'_>,
    range_deg: u8,
) -> TiltAverages {
    let axes = evaluate_all_axes_profiles_finished_threaded(stone, gem, environment, None);
    average_axis_profiles_within(&axes, f32::from(range_deg))
}

/// The girdle's thinnest thickness at its LINE as a percentage of the width: the corners where
/// the girdle faces meet and the ends of level edges, where the editor's proportion verdict and
/// the cutting sheet read it (`GirdleReading::Line`, its thinnest end). `None` when `reading`
/// has no girdle line (no girdle tier, or no crown or pavilion facet reaching the girdle faces):
/// the reading's band fallback is what `girdle_pct_of_width` already gives.
fn girdle_line_percent(reading: Option<GirdleReading>) -> Option<f64> {
    match reading? {
        line @ GirdleReading::Line { .. } => Some(line.judged_percent()),
        GirdleReading::Band { .. } => None,
    }
}

/// The figures of `measured`, in report order.
fn figures(measured: &Measured) -> Vec<Figure> {
    let mut all = optics_figures(measured);
    all.extend(shape_figures(measured));
    all
}

/// The table-up figures, and the tilt averages when they were measured.
fn optics_figures(measured: &Measured) -> Vec<Figure> {
    let table = &measured.table_up;
    let mut figures = vec![
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_brilliance_pct",
            "Brilliance",
            Some(f64::from(table.brilliance_pct)),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_windowing_pct",
            "Windowing",
            Some(f64::from(table.windowing_pct)),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_extinction_pct",
            "Extinction",
            Some(f64::from(table.extinction_pct)),
        ),
        Figure::plain(
            GROUP_TABLE_UP,
            "table_up_fire_index",
            "Fire index",
            Some(f64::from(table.fire_index)),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_scintillation_pct",
            "Scintillation",
            Some(f64::from(table.scintillation_pct)),
        ),
        Figure::plain(
            GROUP_TABLE_UP,
            CONTRAST_KEY,
            "Contrast",
            table.aset.map(|shares| f64::from(shares.contrast)),
        ),
    ];
    figures.extend(zone_figures(table.aset));
    if let Some(tilt) = &measured.tilt {
        let group = tilt_group(measured.tilt_range_deg);
        figures.extend([
            Figure::percent(
                group,
                "tilt_brilliance_pct",
                "Brilliance",
                Some(f64::from(tilt.brilliance_pct)),
            ),
            Figure::percent(
                group,
                "tilt_windowing_pct",
                "Windowing",
                Some(f64::from(tilt.windowing_pct)),
            ),
            Figure::percent(
                group,
                "tilt_extinction_pct",
                "Extinction",
                Some(f64::from(tilt.extinction_pct)),
            ),
        ]);
    }
    figures
}

/// The zone shares of the face-up view, each `None` when the stone has no readings. Red, green
/// and blue are the zones of the Contrast view (ASET-style) lighting; dark is leak plus lost.
fn zone_figures(aset: Option<AsetAreaShares>) -> [Figure; 6] {
    let share =
        |pick: fn(&AsetAreaShares) -> f32| aset.as_ref().map(|shares| f64::from(pick(shares)));
    [
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_aset_red_pct",
            "Zone red, 45-75 deg",
            share(|shares| shares.red_pct),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_aset_green_pct",
            "Zone green, 0-45 deg",
            share(|shares| shares.green_pct),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_aset_blue_pct",
            "Zone blue, 75-90 deg",
            share(|shares| shares.blue_pct),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_aset_dark_pct",
            "Zone dark",
            share(AsetAreaShares::dark_pct),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_aset_leak_pct",
            "Zone dark: leak",
            share(|shares| shares.leak_pct),
        ),
        Figure::percent(
            GROUP_TABLE_UP,
            "table_up_aset_lost_pct",
            "Zone dark: lost",
            share(|shares| shares.lost_pct),
        ),
    ]
}

/// The proportions, the yield and the counts.
fn shape_figures(measured: &Measured) -> Vec<Figure> {
    let proportions = &measured.proportions;
    vec![
        Figure::percent(
            GROUP_PROPORTIONS,
            "table_pct_of_width",
            "Table, of width",
            proportions.table_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "crown_pct_of_width",
            "Crown height, of width",
            proportions.crown_to_width_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "pavilion_pct_of_width",
            "Pavilion depth, of width",
            proportions.pavilion_to_width_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "girdle_pct_of_width",
            "Girdle thickness, of width",
            proportions.girdle_to_width_percent,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            GIRDLE_LINE_KEY,
            "Girdle line, of width",
            measured.girdle_line_pct,
        ),
        Figure::percent(
            GROUP_PROPORTIONS,
            "total_depth_pct_of_width",
            "Total depth, of width",
            measured.depth_pct,
        ),
        Figure::plain(
            GROUP_PROPORTIONS,
            "length_to_width",
            "Length to width",
            proportions.length_to_width,
        ),
        Figure::plain(
            GROUP_PROPORTIONS,
            "volume_over_width_cubed",
            "Volume over width cubed",
            measured.vol_w3,
        ),
        Figure::percent(
            GROUP_STONE,
            "yield_pct",
            "Yield, of the rough",
            measured.yield_pct,
        ),
        Figure::count(GROUP_STONE, "facets", "Facets", measured.facets),
        Figure::count(GROUP_STONE, "tiers", "Tiers", measured.tiers),
        Figure::count(GROUP_STONE, "warnings", "Warnings", measured.warnings),
    ]
}

/// A measured design, ready to print.
struct Built {
    name: String,
    file: String,
    material: String,
    n_d: f64,
    lighting: &'static str,
    figures: Vec<Figure>,
    /// The tilt range as `--tilt-range` gave it; `None` without the flag, when neither the JSON
    /// nor the CSV names a range.
    tilt_range_deg: Option<u8>,
    /// The Blue shares of the face-up grid, printed under the figures of the text report.
    aset_grid: Option<AsetSpotGrid>,
    /// The traced range of refractive indices, when it was asked for (`--ri-range`).
    ri_range: Option<RiRange>,
    notes: Vec<String>,
}

/// Measures `loaded` for `args`.
///
/// # Errors
///
/// [`CliError`] when the material is unknown or the design is not a usable stone.
fn build(loaded: &Loaded, catalogue: &Catalogue, args: &MetricsArgs) -> Result<Built, CliError> {
    let scoring = resolve_scoring(&loaded.design, catalogue, args.material.as_ref())?;
    let analysis = analyze(&loaded.design);
    let solved = analysis.require_stone(&loaded.design)?;
    let tilt_range = args.tilt.then(|| {
        args.tilt_range_deg
            .unwrap_or(DEFAULT_DISPLAY_TILT_WINDOW_DEG)
    });
    let measured = measure_within(
        &loaded.design,
        &analysis,
        solved,
        &scoring,
        args.lighting,
        tilt_range,
        args.aset_grid && matches!(args.format, ReportFormat::Text),
    )?;
    let ri_range = if args.ri_range {
        Some(ri_report::trace(&loaded.design, solved, &scoring)?)
    } else {
        None
    };
    let mut notes = loaded.notes.clone();
    notes.extend(scoring.note.clone());
    Ok(Built {
        name: loaded.name.clone(),
        file: loaded.file_name.clone(),
        material: scoring.label.clone(),
        n_d: scoring.n_d(),
        lighting: lighting_name(args.lighting),
        figures: figures(&measured),
        tilt_range_deg: args.tilt_range_deg.filter(|_| args.tilt),
        aset_grid: measured.aset_grid,
        ri_range,
        notes,
    })
}

/// The number of left-right and up-down neighbour pairs of spots that are both on the stone:
/// the pairs Contrast averages over.
fn neighbour_pairs(grid: &AsetSpotGrid) -> usize {
    let on_stone = |ix: usize, iz: usize| {
        ix < ASET_GRID && iz < ASET_GRID && grid.blue[ix * ASET_GRID + iz].is_some()
    };
    let mut pairs = 0;
    for ix in 0..ASET_GRID {
        for iz in 0..ASET_GRID {
            if on_stone(ix, iz) {
                pairs += usize::from(on_stone(ix + 1, iz)) + usize::from(on_stone(ix, iz + 1));
            }
        }
    }
    pairs
}

/// The grid of `--aset-grid`: the Blue share of every spot in percent, rows from the top of the
/// view down and columns left to right, `.` off the stone, then the Contrast line that the
/// shares add up to.
fn aset_grid_text(grid: &AsetSpotGrid) -> String {
    let mut text = String::from(GRID_CAPTION);
    text.push('\n');
    for iz in (0..ASET_GRID).rev() {
        for ix in 0..ASET_GRID {
            match grid.blue[ix * ASET_GRID + iz] {
                Some(blue) => {
                    let _ = write!(text, "{:>6.1}", f64::from(blue) * 100.0);
                }
                None => {
                    let _ = write!(text, "{:>6}", ".");
                }
            }
        }
        text.push('\n');
    }
    let _ = writeln!(
        text,
        "Contrast = 100 x mean |difference| over the {} left-right and up-down pairs = {}",
        neighbour_pairs(grid),
        fixed(f64::from(pattern_contrast(&grid.blue)), PERCENT_DECIMALS)
    );
    text
}

/// The figures as groups of right-aligned values.
fn figure_lines(figures: &[Figure]) -> String {
    let label_width = figures
        .iter()
        .map(|figure| figure.label.chars().count())
        .max()
        .unwrap_or(0);
    let value_width = figures
        .iter()
        .map(|figure| figure.text().chars().count())
        .max()
        .unwrap_or(0);
    let mut text = String::new();
    let mut group = "";
    for figure in figures {
        if figure.group != group {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(figure.group);
            text.push_str(":\n");
            group = figure.group;
        }
        let _ = writeln!(
            text,
            "  {:<label_width$}  {:>value_width$}",
            figure.label,
            figure.text()
        );
    }
    text
}

/// The text report.
fn text_report(built: &Built) -> String {
    let mut text = key_values(&[
        ("Design", format!("{} ({})", built.name, built.file)),
        ("Material", built.material.clone()),
        ("Index", fixed(built.n_d, INDEX_DECIMALS)),
        ("Lighting", built.lighting.to_string()),
    ]);
    text.push('\n');
    text.push_str(&figure_lines(&built.figures));
    if let Some(range) = &built.ri_range {
        text.push('\n');
        text.push_str(&ri_report::text(range));
    }
    if let Some(grid) = &built.aset_grid {
        text.push('\n');
        text.push_str(&aset_grid_text(grid));
    }
    if !built.notes.is_empty() {
        text.push('\n');
        text.push_str(&list_block("Notes", &built.notes));
    }
    text
}

/// The JSON report.
fn json_report(built: &Built) -> String {
    let mut figures = Map::new();
    for figure in &built.figures {
        figures.insert(figure.key.to_string(), figure.json());
    }
    let mut report = json!({
        "design": built.name,
        "file": built.file,
        "material": built.material,
        "refractive_index": json_number(built.n_d, i32::try_from(INDEX_DECIMALS).unwrap_or(4)),
        "lighting": built.lighting,
        "figures": figures,
        "notes": built.notes,
    });
    if let Some(degrees) = built.tilt_range_deg {
        report["tilt_range_deg"] = json!(degrees);
    }
    if let Some(range) = &built.ri_range {
        report["ri_range"] = ri_report::json(range);
    }
    json_text(&report)
}

/// The CSV report: a header line and one row. A figure the stone has no value for is left
/// empty. The figures of [`APPENDED_CSV_KEYS`] come last, after every older column. Notes are
/// not part of it (they go to standard error). `tilt_range_deg` is the last column, and only when
/// `--tilt-range` was given. With `--ri-range` the CSV is the table of the
/// traced range instead ([`ri_report::csv`]).
fn csv_report(built: &Built) -> String {
    if let Some(range) = &built.ri_range {
        return ri_report::csv(&built.name, &built.material, built.lighting, range);
    }
    let mut header: Vec<String> = ["design", "material", "refractive_index", "lighting"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let mut row = vec![
        built.name.clone(),
        built.material.clone(),
        fixed(built.n_d, INDEX_DECIMALS),
        built.lighting.to_string(),
    ];
    let older = built
        .figures
        .iter()
        .filter(|figure| !APPENDED_CSV_KEYS.contains(&figure.key));
    let appended = APPENDED_CSV_KEYS
        .iter()
        .filter_map(|key| built.figures.iter().find(|figure| figure.key == *key));
    for figure in older.chain(appended) {
        header.push(figure.key.to_string());
        row.push(figure.csv());
    }
    // The range goes last, and only when `--tilt-range` was given.
    if let Some(degrees) = built.tilt_range_deg {
        header.push("tilt_range_deg".to_owned());
        row.push(degrees.to_string());
    }
    let mut text = csv_line(&header);
    text.push_str(&csv_line(&row));
    text
}

mod ri_report;

#[cfg(test)]
mod tests_concave;

#[cfg(test)]
mod tests_ri_range;

#[cfg(test)]
mod tests_frosted;

/// Runs `metrics`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened, the material is unknown, or the design is not
/// a usable stone (exit 2).
pub fn run(args: &MetricsArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    let built = build(&loaded, &catalogue, args)?;
    let report = match args.format {
        ReportFormat::Text => text_report(&built),
        ReportFormat::Json => json_report(&built),
        ReportFormat::Csv => csv_report(&built),
    };
    let mut outcome = deliver(report, args.out.as_deref());
    if matches!(args.format, ReportFormat::Csv) {
        for note in &built.notes {
            outcome = outcome.with_note(&format!("note: {note}"));
        }
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        args::MaterialArg,
        outcome::{EXIT_DESIGN, EXIT_OK, EXIT_USAGE},
        testing,
    };
    use pleochroma_cut_core::CANONICAL_LIGHTING_PRESET;
    use std::path::PathBuf;

    fn args(tilt: bool, format: ReportFormat) -> MetricsArgs {
        MetricsArgs {
            design: PathBuf::new(),
            material: Some(MaterialArg::Ri(1.76)),
            tilt,
            tilt_range_deg: None,
            aset_grid: false,
            ri_range: false,
            format,
            lighting: CANONICAL_LIGHTING_PRESET,
            out: None,
            db: None,
        }
    }

    fn built(tilt: bool) -> Built {
        let mut design = testing::template();
        design.meta.headers = vec!["Round".to_string()];
        let loaded = Loaded::in_memory(design, "round.psdd");
        build(
            &loaded,
            &Catalogue::default(),
            &args(tilt, ReportFormat::Text),
        )
        .expect("measures")
    }

    fn figure<'a>(built: &'a Built, key: &str) -> &'a Figure {
        built
            .figures
            .iter()
            .find(|figure| figure.key == key)
            .unwrap_or_else(|| panic!("no figure {key}"))
    }

    #[test]
    fn the_table_up_figures_are_percentages_of_the_light() {
        let built = built(false);
        for key in [
            "table_up_brilliance_pct",
            "table_up_windowing_pct",
            "table_up_extinction_pct",
        ] {
            let value: f64 = figure(&built, key).value_text().parse().expect("a number");
            assert!((0.0..=100.0).contains(&value), "{key} = {value}");
        }
        assert!(
            built
                .figures
                .iter()
                .all(|figure| figure.group != GROUP_TILT)
        );
        assert_eq!(built.material, "refractive index 1.7600");
        assert!((built.n_d - 1.76).abs() < 1e-9);
    }

    #[test]
    fn the_proportions_and_counts_describe_the_template() {
        let built = built(false);
        let tiers = figure(&built, "tiers").value_text();
        assert_eq!(tiers, testing::template().tiers.len().to_string());
        let facets: usize = figure(&built, "facets")
            .value_text()
            .parse()
            .expect("a count");
        assert!(facets > 8, "{facets}");
        let yield_pct: f64 = figure(&built, "yield_pct")
            .value_text()
            .parse()
            .expect("a number");
        assert!(yield_pct > 0.0 && yield_pct <= 100.0, "{yield_pct}");
        let table: f64 = figure(&built, "table_pct_of_width")
            .value_text()
            .parse()
            .expect("a round brilliant has a table");
        assert!(table > 30.0 && table < 90.0, "{table}");
    }

    #[test]
    fn the_text_report_groups_the_figures() {
        let text = text_report(&built(false));
        assert!(text.starts_with("Design:    "), "{text}");
        assert!(text.contains("\nOptics, table up:\n  Brilliance"), "{text}");
        assert!(text.contains("\nProportions:\n"), "{text}");
        assert!(text.contains("\nStone:\n"), "{text}");
        assert!(
            text.contains("Material:  refractive index 1.7600\n"),
            "{text}"
        );
        assert!(!text.contains("tilt average"), "{text}");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn the_json_report_has_one_number_per_figure() {
        let built = built(false);
        let value: Value = serde_json::from_str(&json_report(&built)).expect("valid JSON");
        let figures = value["figures"].as_object().expect("a figures object");
        assert_eq!(figures.len(), built.figures.len());
        assert!(figures["table_up_brilliance_pct"].is_number());
        assert!(figures["facets"].is_u64());
        assert_eq!(value["lighting"], lighting_name(CANONICAL_LIGHTING_PRESET));
        assert_eq!(value["refractive_index"], 1.76);
        assert!(figures.get("tilt_brilliance_pct").is_none());
    }

    #[test]
    fn the_csv_report_is_a_header_and_one_row_of_the_same_width() {
        let text = csv_report(&built(false));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let header: Vec<&str> = lines[0].split(',').collect();
        let row: Vec<&str> = lines[1].split(',').collect();
        assert_eq!(header.len(), row.len());
        assert_eq!(
            &header[..4],
            ["design", "material", "refractive_index", "lighting"]
        );
        assert_eq!(row[2], "1.7600");
        assert_eq!(row[3], lighting_name(CANONICAL_LIGHTING_PRESET));
        assert!(header.contains(&"table_up_brilliance_pct"));
    }

    fn built_from(loaded: &Loaded) -> Built {
        build(
            loaded,
            &Catalogue::default(),
            &args(false, ReportFormat::Text),
        )
        .expect("measures")
    }

    fn number(built: &Built, key: &str) -> Option<f64> {
        match figure(built, key).reading {
            Reading::Number { value, .. } => value,
            Reading::Count(_) => None,
        }
    }

    /// The band runs the full height of the girdle faces; the line is where the editor and the
    /// cutting sheet read the girdle. Pentalume's tiny P3 and C3 thin its line at the corners of
    /// the clipped pentagon; Cam's Portuguese has a level line of about 2.9 %.
    #[test]
    fn the_girdle_is_read_at_its_line_beside_the_band() {
        let pentalume = built_from(&testing::asc("pentalume.asc", testing::PENTALUME_ASC));
        let line = number(&pentalume, GIRDLE_LINE_KEY).expect("Pentalume has a girdle line");
        let band = number(&pentalume, "girdle_pct_of_width").expect("and a band");
        assert!(
            (2.0..3.0).contains(&line),
            "the thinnest end of the line: {line}"
        );
        assert!(line < band, "line {line}, band {band}");
        let cams = built_from(&testing::asc("cams.asc", testing::CAMS_ASC));
        let level = number(&cams, GIRDLE_LINE_KEY).expect("Cam's Portuguese has a girdle line");
        assert!((2.85..2.95).contains(&level), "{level}");
    }

    #[test]
    fn the_girdle_line_figure_is_empty_without_a_girdle_line() {
        assert_eq!(girdle_line_percent(None), None);
        assert_eq!(
            girdle_line_percent(Some(GirdleReading::Band {
                thickness: 0.1,
                percent: 5.0
            })),
            None,
            "the band fallback is not a line"
        );
        let line = girdle_line_percent(Some(GirdleReading::Line {
            thinnest: 0.05,
            thickest: 0.08,
            width: 2.0,
            level: false,
        }))
        .expect("a line");
        assert!((line - 2.5).abs() < 1e-12, "the thinnest end: {line}");
        let missing = Figure::percent(GROUP_PROPORTIONS, GIRDLE_LINE_KEY, "Girdle line", None);
        assert_eq!(missing.csv(), "");
        assert_eq!(missing.json(), Value::Null);
    }

    /// Scripts that read the CSV by position keep working: the appended columns are the last
    /// ones, and every older column keeps its place. Text keeps the girdle line beside the band.
    #[test]
    fn the_appended_columns_come_last_in_the_csv_and_the_girdle_line_sits_beside_the_band_in_text()
    {
        let built = built(false);
        let text = csv_report(&built);
        let lines: Vec<&str> = text.lines().collect();
        let header: Vec<&str> = lines[0].split(',').collect();
        assert_eq!(header.len(), lines[1].split(',').count(), "{text}");
        assert_eq!(
            header.last().copied(),
            APPENDED_CSV_KEYS.last().copied(),
            "{text}"
        );
        let at = |key: &str| {
            header
                .iter()
                .position(|column| *column == key)
                .unwrap_or_else(|| panic!("no column {key}: {text}"))
        };
        assert_eq!(
            at("total_depth_pct_of_width"),
            at("girdle_pct_of_width") + 1
        );
        assert_eq!(at(GIRDLE_LINE_KEY), at("warnings") + 1);
        assert_eq!(
            header
                .iter()
                .filter(|column| **column == GIRDLE_LINE_KEY)
                .count(),
            1
        );

        let report = text_report(&built);
        let position = |label: &str| {
            report
                .find(label)
                .unwrap_or_else(|| panic!("no {label}: {report}"))
        };
        assert!(position("Girdle thickness, of width") < position("Girdle line, of width"));
        assert!(position("Girdle line, of width") < position("Total depth, of width"));

        let value: Value = serde_json::from_str(&json_report(&built)).expect("valid JSON");
        let figure = &value["figures"][GIRDLE_LINE_KEY];
        assert!(figure.is_number() || figure.is_null(), "{figure}");
    }

    #[test]
    fn the_report_is_the_same_every_time() {
        assert_eq!(text_report(&built(false)), text_report(&built(false)));
        assert_eq!(json_report(&built(false)), json_report(&built(false)));
    }

    fn built_with_grid() -> Built {
        let mut design = testing::template();
        design.meta.headers = vec!["Round".to_string()];
        let loaded = Loaded::in_memory(design, "round.psdd");
        let grid_args = MetricsArgs {
            aset_grid: true,
            ..args(false, ReportFormat::Text)
        };
        build(&loaded, &Catalogue::default(), &grid_args).expect("measures")
    }

    fn reading(built: &Built, key: &str) -> f64 {
        figure(built, key).value_text().parse().expect("a number")
    }

    #[test]
    fn contrast_and_the_zone_shares_follow_scintillation_in_the_text_report() {
        let text = text_report(&built(false));
        let labels = [
            "Scintillation",
            "Contrast",
            "Zone red, 45-75 deg",
            "Zone green, 0-45 deg",
            "Zone blue, 75-90 deg",
            "Zone dark",
            "Zone dark: leak",
            "Zone dark: lost",
        ];
        let positions: Vec<usize> = labels
            .iter()
            .map(|label| {
                text.find(label)
                    .unwrap_or_else(|| panic!("no {label}: {text}"))
            })
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
        assert!(
            text.find("Contrast").expect("contrast") < text.find("\nProportions:").expect("group"),
            "{text}"
        );
        assert!(!text.contains("ASET Zones"), "{text}");
        assert!(!text.contains(GRID_CAPTION), "{text}");
    }

    #[test]
    fn the_json_report_carries_contrast_and_the_zone_shares() {
        let value: Value = serde_json::from_str(&json_report(&built(false))).expect("valid JSON");
        for key in APPENDED_CSV_KEYS.iter().skip(1) {
            assert!(value["figures"][*key].is_number(), "{key}");
        }
        let contrast = value["figures"][CONTRAST_KEY].as_f64().expect("a number");
        assert!((0.0..=100.0).contains(&contrast), "{contrast}");
    }

    #[test]
    fn the_zone_shares_add_up_and_dark_is_leak_plus_lost() {
        let built = built(false);
        let share = |name: &str| reading(&built, &format!("table_up_aset_{name}_pct"));
        let total = share("red") + share("green") + share("blue") + share("leak") + share("lost");
        assert!((99.0..=100.05).contains(&total), "{total}");
        assert!(
            (share("dark") - (share("leak") + share("lost"))).abs() < 0.02,
            "dark {} leak {} lost {}",
            share("dark"),
            share("leak"),
            share("lost")
        );
        assert_eq!(
            figure(&built, "table_up_aset_leak_pct").value_text(),
            figure(&built, "table_up_windowing_pct").value_text(),
            "leak is the windowing rays"
        );
    }

    #[test]
    fn the_contrast_and_zone_columns_are_appended_after_the_girdle_line_in_the_csv() {
        let text = csv_report(&built(false));
        let header: Vec<&str> = text.lines().next().expect("a header").split(',').collect();
        let tail = &header[header.len() - APPENDED_CSV_KEYS.len()..];
        assert_eq!(tail, APPENDED_CSV_KEYS, "{text}");
        assert_eq!(tail[0], GIRDLE_LINE_KEY);
        assert_eq!(tail[1], CONTRAST_KEY);
        let position = |key: &str| header.iter().position(|column| *column == key);
        assert_eq!(
            position(GIRDLE_LINE_KEY),
            position("warnings").map(|at| at + 1),
            "the older columns keep their place"
        );
    }

    #[test]
    fn the_neighbour_pairs_count_left_right_and_up_down_pairs_on_the_stone() {
        let mut grid = AsetSpotGrid {
            blue: [None; ASET_GRID * ASET_GRID],
        };
        assert_eq!(neighbour_pairs(&grid), 0);
        grid.blue[0] = Some(0.0);
        assert_eq!(neighbour_pairs(&grid), 0, "one spot has no neighbour");
        grid.blue[ASET_GRID] = Some(0.5);
        grid.blue[ASET_GRID + 1] = Some(1.0);
        assert_eq!(neighbour_pairs(&grid), 2);
        // A diagonal is not a neighbour.
        grid.blue[1] = None;
        grid.blue[ASET_GRID + 1] = None;
        grid.blue[2 * ASET_GRID + 1] = Some(1.0);
        assert_eq!(neighbour_pairs(&grid), 1);
    }

    #[test]
    fn the_grid_text_has_a_row_per_iz_with_dots_off_the_stone_and_percent_on_it() {
        let mut grid = AsetSpotGrid {
            blue: [None; ASET_GRID * ASET_GRID],
        };
        // Column 3, row 17 (the top row), and column 4 beside it.
        grid.blue[3 * ASET_GRID + 17] = Some(0.2);
        grid.blue[4 * ASET_GRID + 17] = Some(0.0);
        let text = aset_grid_text(&grid);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], GRID_CAPTION);
        assert_eq!(lines.len(), ASET_GRID + 2, "{text}");
        let top: Vec<&str> = lines[1].split_whitespace().collect();
        assert_eq!(top.len(), ASET_GRID);
        assert_eq!(&top[2..6], [".", "20.0", "0.0", "."], "{text}");
        assert!(lines[2..=ASET_GRID].iter().all(|row| !row.contains('2')));
        assert_eq!(
            lines[ASET_GRID + 1],
            "Contrast = 100 x mean |difference| over the 1 left-right and up-down pairs = 20.00"
        );
    }

    #[test]
    fn the_aset_grid_prints_eighteen_rows_and_a_contrast_line_equal_to_the_figure() {
        let built = built_with_grid();
        let grid = built.aset_grid.as_ref().expect("the grid was asked for");
        let text = text_report(&built);
        let start = text.find(GRID_CAPTION).expect("the caption");
        let block: Vec<&str> = text[start..].lines().collect();
        assert_eq!(block[0], GRID_CAPTION);
        assert!(
            block[1..=ASET_GRID]
                .iter()
                .all(|row| row.split_whitespace().count() == ASET_GRID),
            "{text}"
        );
        let pairs = neighbour_pairs(grid);
        assert!((300..=328).contains(&pairs), "{pairs}");
        let contrast = figure(&built, CONTRAST_KEY).value_text();
        assert_eq!(
            block[ASET_GRID + 1],
            format!(
                "Contrast = 100 x mean |difference| over the {pairs} left-right and up-down pairs = {contrast}"
            ),
            "{text}"
        );
        // The grid sits between the figures and the notes, never in JSON or CSV.
        assert!(start > text.find("\nStone:\n").expect("the stone group"));
        assert!(!json_report(&built).contains("Blue-zone"));
        assert!(!csv_report(&built).contains("Blue-zone"));
        assert!(built_without_grid().aset_grid.is_none());
    }

    fn built_without_grid() -> Built {
        built(false)
    }

    #[test]
    fn end_to_end_the_aset_grid_flag_prints_the_grid_and_refuses_json_and_csv() {
        let input = testing::write_design("metrics-e2e-grid.psdd", &testing::template());
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76", "--aset-grid"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(outcome.stdout.contains(GRID_CAPTION), "{}", outcome.stdout);
        let json = testing::run(&["metrics", &input, "--ri", "1.76", "--aset-grid", "--json"]);
        assert_eq!(json.exit, EXIT_USAGE, "{}", json.stderr);
        let plain = testing::run(&["metrics", &input, "--ri", "1.76"]);
        assert_eq!(plain.exit, EXIT_OK, "{}", plain.stderr);
        assert!(!plain.stdout.contains(GRID_CAPTION));
    }

    #[test]
    fn a_figure_with_no_value_reads_n_a_in_text_null_in_json_and_empty_in_csv() {
        let missing = Figure::percent(GROUP_PROPORTIONS, "girdle_pct_of_width", "Girdle", None);
        assert_eq!(missing.text(), "n/a %");
        assert_eq!(missing.json(), Value::Null);
        assert_eq!(missing.csv(), "");
        let count = Figure::count(GROUP_STONE, "facets", "Facets", 12);
        assert_eq!(count.text(), "12");
        assert_eq!(count.json(), Value::from(12_usize));
        assert_eq!(count.csv(), "12");
    }

    #[test]
    fn end_to_end_metrics_prints_json() {
        let input = testing::write_design("metrics-e2e.psdd", &testing::template());
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76", "--json"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(outcome.files.len(), 0);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        assert!(value["figures"]["table_up_windowing_pct"].is_number());
    }

    #[test]
    fn end_to_end_metrics_writes_csv_to_a_file() {
        let input = testing::write_design("metrics-e2e-csv.psdd", &testing::template());
        let target = testing::temp_path("metrics-e2e.csv")
            .to_string_lossy()
            .into_owned();
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76", "--csv", "--out", &target]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
        let csv = testing::file_text(&outcome, &target).expect("the CSV is written");
        assert_eq!(csv.lines().count(), 2);
    }

    #[test]
    fn end_to_end_metrics_with_tilt_adds_the_tilt_averages() {
        let input = testing::write_design("metrics-e2e-tilt.psdd", &testing::template());
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76", "--tilt", "--json"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let value: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
        let figures = &value["figures"];
        for key in [
            "tilt_brilliance_pct",
            "tilt_windowing_pct",
            "tilt_extinction_pct",
        ] {
            let reading = figures[key].as_f64().unwrap_or(-1.0);
            assert!((0.0..=100.0).contains(&reading), "{key} = {reading}");
        }
        let text = testing::run(&["metrics", &input, "--ri", "1.76", "--tilt"]);
        assert_eq!(text.exit, EXIT_OK, "{}", text.stderr);
        assert!(
            text.stdout
                .contains("\nOptics, tilt average over 4 axes and 181 angles:\n  Brilliance"),
            "{}",
            text.stdout
        );
        assert!(
            !value
                .as_object()
                .is_some_and(|map| map.contains_key("tilt_range_deg"))
        );
    }

    #[test]
    fn the_tilt_heading_is_the_old_one_at_ninety_and_names_a_narrower_range() {
        assert_eq!(DEFAULT_DISPLAY_TILT_WINDOW_DEG, 90);
        assert_eq!(
            tilt_group(90),
            "Optics, tilt average over 4 axes and 181 angles"
        );
        assert_eq!(tilt_group(90), GROUP_TILT);
        for (range, angles) in [(15_u8, 31), (20, 41), (30, 61), (45, 91), (60, 121)] {
            assert_eq!(
                tilt_group(range),
                format!("Optics, tilt average to ±{range}° over 4 axes and {angles} angles"),
                "{range}"
            );
            assert_eq!(
                u16::try_from(angles).expect("small"),
                pleochroma_editor::metric_deltas::tilt_angles_within(range)
            );
        }
        assert_eq!(
            tilt_group(0),
            GROUP_TILT,
            "an unlisted value reads as every tilt"
        );
    }

    #[test]
    fn a_narrower_range_names_itself_in_the_report_and_gives_other_figures() {
        let input = testing::write_design("metrics-e2e-tilt-range.psdd", &testing::template());
        let narrow = testing::run(&[
            "metrics",
            &input,
            "--ri",
            "1.76",
            "--tilt",
            "--tilt-range",
            "30",
            "--json",
        ]);
        assert_eq!(narrow.exit, EXIT_OK, "{}", narrow.stderr);
        let narrow: Value = serde_json::from_str(&narrow.stdout).expect("valid JSON");
        assert_eq!(narrow["tilt_range_deg"], 30);
        let full = testing::run(&["metrics", &input, "--ri", "1.76", "--tilt", "--json"]);
        assert_eq!(full.exit, EXIT_OK, "{}", full.stderr);
        let full: Value = serde_json::from_str(&full.stdout).expect("valid JSON");
        assert_ne!(
            narrow["figures"]["tilt_brilliance_pct"], full["figures"]["tilt_brilliance_pct"],
            "a window of 30 degrees reads other means than every tilt"
        );
        assert_eq!(
            narrow["figures"]["table_up_brilliance_pct"],
            full["figures"]["table_up_brilliance_pct"],
            "the table-up figures never follow the range"
        );
        let text = testing::run(&[
            "metrics",
            &input,
            "--ri",
            "1.76",
            "--tilt",
            "--tilt-range",
            "30",
        ]);
        assert_eq!(text.exit, EXIT_OK, "{}", text.stderr);
        assert!(
            text.stdout.contains(
                "\nOptics, tilt average to ±30° over 4 axes and 61 angles:\n  Brilliance"
            ),
            "{}",
            text.stdout
        );
        let csv = testing::run(&[
            "metrics",
            &input,
            "--ri",
            "1.76",
            "--tilt",
            "--tilt-range",
            "45",
            "--csv",
        ]);
        assert_eq!(csv.exit, EXIT_OK, "{}", csv.stderr);
        let lines: Vec<&str> = csv.stdout.lines().collect();
        assert!(lines[0].ends_with(",tilt_range_deg"), "{}", csv.stdout);
        assert!(lines[1].ends_with(",45"), "{}", csv.stdout);
    }

    #[test]
    fn the_tilt_range_with_ninety_still_names_the_range_in_json_only_when_given() {
        let input = testing::write_design("metrics-e2e-tilt-90.psdd", &testing::template());
        let given = testing::run(&[
            "metrics",
            &input,
            "--ri",
            "1.76",
            "--tilt",
            "--tilt-range",
            "90",
            "--json",
        ]);
        assert_eq!(given.exit, EXIT_OK, "{}", given.stderr);
        let given: Value = serde_json::from_str(&given.stdout).expect("valid JSON");
        assert_eq!(given["tilt_range_deg"], 90);
        let plain = testing::run(&["metrics", &input, "--ri", "1.76", "--tilt", "--json"]);
        let plain: Value = serde_json::from_str(&plain.stdout).expect("valid JSON");
        assert_eq!(
            given["figures"], plain["figures"],
            "the flag at 90 is the default figures, bit for bit"
        );
    }

    #[test]
    fn the_range_flag_needs_the_tilt_flag_and_a_listed_value() {
        let input = testing::write_design("metrics-e2e-tilt-usage.psdd", &testing::template());
        let alone = testing::run(&["metrics", &input, "--ri", "1.76", "--tilt-range", "30"]);
        assert_eq!(alone.exit, EXIT_USAGE, "{}", alone.stdout);
        assert!(
            alone.stderr.contains("--tilt-range needs --tilt"),
            "{}",
            alone.stderr
        );
        let odd = testing::run(&[
            "metrics",
            &input,
            "--ri",
            "1.76",
            "--tilt",
            "--tilt-range",
            "25",
        ]);
        assert_eq!(odd.exit, EXIT_USAGE, "{}", odd.stdout);
        assert!(
            odd.stderr.contains("15, 20, 30, 45, 60 or 90"),
            "{}",
            odd.stderr
        );
    }

    #[test]
    fn an_unknown_material_is_a_usage_error() {
        let input = testing::write_design("metrics-e2e-unknown.psdd", &testing::template());
        let outcome = testing::run(&["metrics", &input, "--material", "Unobtainium"]);
        assert_eq!(outcome.exit, EXIT_USAGE);
        assert!(
            outcome.stderr.contains("unknown material"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_missing_file_is_an_io_error_and_an_empty_design_a_design_error() {
        let outcome = testing::run(&["metrics", "no-such-file.psdd", "--ri", "1.76"]);
        assert_eq!(outcome.exit, crate::outcome::EXIT_IO, "{}", outcome.stderr);
        let design = Design::new(
            pleochroma_cut_core::PreformSpec::block(2.0, 1.0, 4.0),
            pleochroma_cut_core::ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let input = testing::write_design("metrics-e2e-empty.psdd", &design);
        let outcome = testing::run(&["metrics", &input, "--ri", "1.76"]);
        assert_eq!(outcome.exit, EXIT_DESIGN, "{}", outcome.stderr);
        assert_eq!(outcome.stdout, "");
    }
}
