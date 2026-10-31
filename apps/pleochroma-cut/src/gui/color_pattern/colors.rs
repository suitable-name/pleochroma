//! Colors of a pattern: the swatches the lists and previews draw, and the background solves that
//! turn a picked `[L*, C*, h]` into the band rows a pattern stores.
//!
//! A solve is `pleochroma_editor::lch_color::solve_lch`, the same cancellable solver the body-color
//! editor uses. Solved colors are remembered by their `[L*, C*, h]` ([`SolveCache`]), so an undo
//! back to a template whose colors were solved once needs no new solve.

use crate::gui::latest_worker::LatestWorker;
use pleochroma::{
    color::{Illuminant, body_color::lch_to_lab, body_colors},
    optics::{
        absorption::AbsorptionTensor,
        materials::{
            AbsorptionUnit, GemMaterial,
            variants::{face_up_swatch, face_up_swatch_path},
        },
    },
};
use pleochroma_cut_core::material::pattern::{
    ColorPattern, PatternColor, ZoneColor, bands_from_rows,
};
use pleochroma_editor::lch_color::{lch_from_srgb, solve_lch};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// An sRGB color, one byte per channel.
pub(super) type Rgb = [u8; 3];

/// One byte of a channel in `0..=1`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the channel is clamped to 0..=1 and scaled to 0..=255 before the cast"
)]
fn byte(channel: f64) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The sRGB transfer function of one linear channel.
fn encode(linear: f64) -> f64 {
    let v = linear.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055_f64.mul_add(v.powf(1.0 / 2.4), -0.055)
    }
}

/// The sRGB color of `[L*, C*, h]` (D65), clamped into the gamut: what a zone looks like before
/// its band rows are solved.
#[expect(
    clippy::many_single_char_names,
    reason = "a, b and x, y, z are the CIE Lab and XYZ coordinates"
)]
pub(super) fn lch_to_rgb(lch: [f64; 3]) -> Rgb {
    let [lightness, a, b] = lch_to_lab(lch);
    let fy = (lightness + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let cube = |t: f64| {
        const DELTA: f64 = 6.0 / 29.0;
        if t > DELTA {
            t * t * t
        } else {
            3.0 * DELTA * DELTA * (t - 4.0 / 29.0)
        }
    };
    let (x, y, z) = (0.950_47 * cube(fx), cube(fy), 1.088_83 * cube(fz));
    let red = 3.240_454_2_f64.mul_add(x, (-1.537_138_5_f64).mul_add(y, -0.498_531_4 * z));
    let green = (-0.969_266_0_f64).mul_add(x, 1.876_010_8_f64.mul_add(y, 0.041_556_0 * z));
    let blue = 0.055_643_4_f64.mul_add(x, (-0.204_025_9_f64).mul_add(y, 1.057_225_2 * z));
    [byte(encode(red)), byte(encode(green)), byte(encode(blue))]
}

/// The color of one picked color at the light path `path_mm`: its band rows when solved, else its
/// `[L*, C*, h]`.
pub(super) fn color_swatch(color: &ZoneColor, path_mm: f64) -> Rgb {
    if color.is_solved() {
        body_colors(
            &AbsorptionTensor::isotropic(bands_from_rows(&color.bands)),
            path_mm,
            Illuminant::D65,
        )
        .unpolarised
        .srgb
    } else {
        lch_to_rgb(color.lch)
    }
}

/// What a zone looks like: one color, or several with their blend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ZoneSwatch {
    /// The color, or the Along (Alpha) color of a pleochroic zone.
    pub primary: Rgb,
    /// The Across (Gamma) color of a pleochroic zone (the primary one for a one-color zone).
    pub across: Rgb,
    /// The Beta color of a three-color zone (the primary one otherwise).
    pub beta: Rgb,
    /// What the zone shows overall: the color, or the unpolarised blend of all of them.
    pub blend: Rgb,
    /// Whether the zone is pleochroic: Along and Across, Alpha, Beta and Gamma, or a pleochroic
    /// variant.
    pub dichroic: bool,
}

impl ZoneSwatch {
    /// A zone of one color.
    pub(super) const fn uniform(rgb: Rgb) -> Self {
        Self {
            primary: rgb,
            across: rgb,
            beta: rgb,
            blend: rgb,
            dichroic: false,
        }
    }
}

/// The stone a swatch is drawn for.
///
/// A picked color is shown at the solver's light path. A color variant is shown the way the
/// variant lists show it (lead decisions D-A and D-G): per-millimeter numbers calibrated for a
/// stone `width_mm` wide, read at `variants::face_up_swatch_path`, and its overall color is the
/// oriented face-up color of the stone.
#[derive(Debug, Clone, Copy)]
pub(super) struct SwatchStone<'a> {
    /// The light path the picked colors are shown at, millimeters.
    pub path_mm: f64,
    /// The stone width a variant color is shown for, millimeters.
    pub width_mm: f64,
    /// The stone's material, whose default Crystal Axis orients a variant's face-up color;
    /// `None` shows the unpolarised color (a material the window does not know).
    pub host: Option<&'a GemMaterial>,
}

/// The swatches of a variant's per-millimeter `tensor` for `stone`.
///
/// The colors along each axis are read at the face-up path of a stone `stone.width_mm` wide, and
/// the overall color is the oriented face-up color (`variants::face_up_swatch`).
#[expect(
    clippy::cast_possible_truncation,
    reason = "a stone width in millimeters is a few dozen at most; f32 is the precision of every stored width"
)]
fn variant_swatch(tensor: &AbsorptionTensor, stone: &SwatchStone<'_>) -> ZoneSwatch {
    let path = face_up_swatch_path(AbsorptionUnit::PerMm, stone.width_mm as f32);
    let colors = body_colors(tensor, path, Illuminant::D65);
    let blend = stone.host.map_or(colors.unpolarised.srgb, |host| {
        face_up_swatch(tensor, AbsorptionUnit::PerMm, stone.width_mm, host).srgb
    });
    ZoneSwatch {
        primary: colors.o_ray.srgb,
        across: colors.e_ray.srgb,
        beta: colors.beta_ray.map_or(colors.o_ray.srgb, |beta| beta.srgb),
        blend,
        dichroic: tensor.is_pleochroic,
    }
}

/// The swatches of `color` for `stone`.
pub(super) fn zone_swatch(color: &PatternColor, stone: &SwatchStone<'_>) -> ZoneSwatch {
    let path_mm = stone.path_mm;
    match color {
        PatternColor::Single(single) => ZoneSwatch::uniform(color_swatch(single, path_mm)),
        PatternColor::Dichroic { along, across } => {
            let (primary, second) = (color_swatch(along, path_mm), color_swatch(across, path_mm));
            let blend = if along.is_solved() && across.is_solved() {
                body_colors(
                    &AbsorptionTensor::uniaxial(
                        bands_from_rows(&along.bands),
                        bands_from_rows(&across.bands),
                    ),
                    path_mm,
                    Illuminant::D65,
                )
                .unpolarised
                .srgb
            } else {
                primary
            };
            ZoneSwatch {
                primary,
                across: second,
                beta: primary,
                blend,
                dichroic: true,
            }
        }
        PatternColor::Trichroic { alpha, beta, gamma } => {
            let (first, middle, last) = (
                color_swatch(alpha, path_mm),
                color_swatch(beta, path_mm),
                color_swatch(gamma, path_mm),
            );
            let blend = if alpha.is_solved() && beta.is_solved() && gamma.is_solved() {
                body_colors(
                    &AbsorptionTensor::biaxial(
                        bands_from_rows(&alpha.bands),
                        bands_from_rows(&beta.bands),
                        bands_from_rows(&gamma.bands),
                    ),
                    path_mm,
                    Illuminant::D65,
                )
                .unpolarised
                .srgb
            } else {
                first
            };
            ZoneSwatch {
                primary: first,
                across: last,
                beta: middle,
                blend,
                dichroic: true,
            }
        }
        PatternColor::Variant { absorption, .. } => variant_swatch(&absorption.to_tensor(), stone),
    }
}

/// The swatches of every zone of `pattern` for `stone`, the base first.
pub(super) fn pattern_swatches(pattern: &ColorPattern, stone: &SwatchStone<'_>) -> Vec<ZoneSwatch> {
    std::iter::once(&pattern.base)
        .chain(&pattern.zones)
        .map(|zone| zone_swatch(&zone.color, stone))
        .collect()
}

/// The `[L*, C*, h]` of band rows at the light path `path_mm`: what seeds the sliders of a color
/// that was not picked in the editor (the zones of the Rough Color wizard).
pub(super) fn lch_of_rows(rows: &[[f32; 3]], path_mm: f64) -> [f64; 3] {
    let rgb = body_colors(
        &AbsorptionTensor::isotropic(bands_from_rows(rows)),
        path_mm,
        Illuminant::D65,
    )
    .unpolarised
    .srgb;
    lch_from_srgb(rgb.map(|channel| f64::from(channel) / 255.0))
}

// ---- solving ---------------------------------------------------------------------------------

/// Band rows already solved, by the `[L*, C*, h]` they were solved for.
pub(super) type SolveCache = HashMap<[u64; 3], Vec<[f32; 3]>>;

/// The key of `lch` in a [`SolveCache`].
pub(super) fn cache_key(lch: [f64; 3]) -> [u64; 3] {
    lch.map(f64::to_bits)
}

/// Fills every unsolved color of `pattern` that `cache` has a solution for; returns whether
/// anything was filled.
pub(super) fn fill_from_cache(pattern: &mut ColorPattern, cache: &SolveCache) -> bool {
    let mut changed = false;
    for at in pattern.unsolved_colors() {
        let Some(lch) = pattern.color(at).map(|color| color.lch) else {
            continue;
        };
        if let Some(bands) = cache.get(&cache_key(lch)) {
            changed |= pattern.set_color_bands(at, bands.clone());
        }
    }
    changed
}

/// The `[L*, C*, h]` of the unsolved colors of `pattern` that `cache` cannot fill, each once.
pub(super) fn missing_colors(pattern: &ColorPattern, cache: &SolveCache) -> Vec<[f64; 3]> {
    let mut missing: Vec<[f64; 3]> = Vec::new();
    for at in pattern.unsolved_colors() {
        let Some(lch) = pattern.color(at).map(|color| color.lch) else {
            continue;
        };
        if !cache.contains_key(&cache_key(lch)) && !missing.contains(&lch) {
            missing.push(lch);
        }
    }
    missing
}

/// Colors a solve finished: `(the [L*, C*, h] asked for, its band rows)`.
pub(super) type Solved = Vec<([f64; 3], Vec<[f32; 3]>)>;

/// What the worker is asked: solve `colors` for `path_mm` unless `cancel` is raised.
struct SolveJob {
    colors: Vec<[f64; 3]>,
    path_mm: f64,
    cancel: Arc<AtomicBool>,
}

/// The one background thread that solves colors. A new request supersedes the one before it: the
/// older solve is cancelled and its result never delivered.
pub(super) struct SolveWorker {
    worker: LatestWorker<SolveJob>,
    running: RefCell<Arc<AtomicBool>>,
}

impl SolveWorker {
    /// Starts the thread; `deliver` is called ON THE WORKER THREAD with each finished request
    /// (the colors solved, and whether a solve failed for a reason other than a cancel), so it
    /// must hop to the UI thread itself and recheck the supplied cancellation token there.
    /// A close or a newer request may cancel a result already queued for UI delivery.
    pub(super) fn new(
        deliver: impl Fn(Solved, bool, Arc<AtomicBool>) + Send + Sync + 'static,
    ) -> Self {
        let worker = LatestWorker::spawn_with_panic_hook(
            "color-pattern-solve",
            move |job: SolveJob| {
                let mut solved = Solved::new();
                let mut failed = false;
                for lch in job.colors {
                    if job.cancel.load(Ordering::SeqCst) {
                        return;
                    }
                    let Some(solve) = solve_lch(lch, job.path_mm, &job.cancel) else {
                        // `None` is a cancel, or a solve that could not finish: tell the UI
                        // either way unless it was a cancel, so it never waits forever.
                        failed = true;
                        break;
                    };
                    solved.push((lch, solve.bands));
                }
                if !job.cancel.load(Ordering::SeqCst) {
                    deliver(solved, failed, Arc::clone(&job.cancel));
                }
            },
            || {},
        );
        Self {
            worker,
            running: RefCell::new(Arc::default()),
        }
    }

    /// Solves `colors` for the light path `path_mm`, cancelling the request before it.
    pub(super) fn submit(&self, colors: Vec<[f64; 3]>, path_mm: f64) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.running
            .replace(Arc::clone(&cancel))
            .store(true, Ordering::SeqCst);
        self.worker.submit(SolveJob {
            colors,
            path_mm,
            cancel,
        });
    }

    /// Cancels the request that is running, if any.
    pub(super) fn cancel(&self) {
        self.running.borrow().store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};

    #[test]
    fn lch_gray_and_white_map_to_the_expected_rgb() {
        let white = lch_to_rgb([100.0, 0.0, 0.0]);
        assert!(white.iter().all(|c| *c >= 254), "{white:?}");
        let black = lch_to_rgb([0.0, 0.0, 0.0]);
        assert!(black.iter().all(|c| *c <= 1), "{black:?}");
        let grey = lch_to_rgb([50.0, 0.0, 0.0]);
        assert!(grey[0].abs_diff(grey[1]) <= 1 && grey[1].abs_diff(grey[2]) <= 1);
        assert!((115..=125).contains(&grey[0]), "{grey:?}");
    }

    #[test]
    fn a_red_lch_is_red() {
        let [r, g, b] = lch_to_rgb([55.0, 80.0, 40.0]);
        assert!(r > g && r > b, "{r} {g} {b}");
    }

    #[test]
    fn the_cache_fills_what_it_knows_and_names_what_it_does_not() {
        let mut pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        let PatternColor::Single(base) = &pattern.base.color else {
            panic!("a template's base color is a single color");
        };
        let base_lch = base.lch;
        let mut cache = SolveCache::new();
        assert_eq!(missing_colors(&pattern, &cache).len(), 2);
        cache.insert(cache_key(base_lch), vec![[500.0, 40.0, 0.2]]);
        assert_eq!(missing_colors(&pattern, &cache).len(), 1);
        assert!(fill_from_cache(&mut pattern, &cache));
        assert_eq!(pattern.unsolved_colors().len(), 1);
        assert!(!fill_from_cache(&mut pattern, &cache));
    }

    #[test]
    fn a_variant_zone_shows_the_face_up_color_of_the_stone_at_the_variants_own_width() {
        use pleochroma::optics::materials::variants::built_in_variants;
        let sapphire = GemMaterial::sapphire();
        // The Material Default is per model unit: the zone color holds it converted to per mm.
        let default = built_in_variants("Sapphire").remove(0);
        let color = PatternColor::from_variant(&default);
        let PatternColor::Variant { absorption, .. } = &color else {
            panic!("a variant zone color");
        };
        let tensor = absorption.to_tensor();
        let stone = SwatchStone {
            path_mm: 10.5,
            width_mm: 7.0,
            host: Some(&sapphire),
        };
        let swatch = zone_swatch(&color, &stone);
        // The overall color is the oriented face-up color, not the unpolarised blend.
        let face_up = face_up_swatch(&tensor, AbsorptionUnit::PerMm, 7.0, &sapphire).srgb;
        assert_eq!(swatch.blend, face_up);
        assert!(swatch.dichroic);
        // The per-direction colors stay (the tooltip's): the o ray first, at the face-up path.
        let path = face_up_swatch_path(AbsorptionUnit::PerMm, 7.0);
        let colors = body_colors(&tensor, path, Illuminant::D65);
        assert_eq!(
            (swatch.primary, swatch.across),
            (colors.o_ray.srgb, colors.e_ray.srgb)
        );
        // Without a material the overall color is the unpolarised one at the same path.
        let bare = SwatchStone {
            host: None,
            ..stone
        };
        assert_eq!(zone_swatch(&color, &bare).blend, colors.unpolarised.srgb);
    }
}
