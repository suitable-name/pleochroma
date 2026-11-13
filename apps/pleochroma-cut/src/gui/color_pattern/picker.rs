//! The window's color picker: an `LCh` plane (chroma to the right, lightness up) at the current
//! hue, a hue strip, number fields for L, C and H, a hex field, the material's variants as chips,
//! and the color as it comes out in the stone.
//!
//! The picker edits one color tile of the selected layer. Every change redraws the values at
//! once; the color in the stone needs a solve (`colors::SolveWorker`), which starts about
//! [`DEBOUNCE`] after the last change, so a drag on the plane stays smooth and the result row
//! shows the last solved color with "working it out" until the new one arrives. A solved color
//! goes into the session's cache, so "Use Color" stores it already solved.
//!
//! A tile of a variant zone opens the picker on the variant's color for that direction; using
//! the color turns the zone into a picked color (one undo step), as the "Custom color" entry of
//! the From variant list does.

use super::{
    actions,
    colors::{cache_key, color_swatch, lch_of_rows, lch_to_rgb, zone_swatch},
    form::{SwatchPart, picker_title, slot_for, tile_specs},
    lch::{PLANE_MAX_CHROMA, delta_e, hue_strip, in_srgb_gamut, picker_plane, rgb_to_lch},
    model::f32_of,
    preview::raster_image,
    session::{Host, Session, on_host, with_session},
};
use crate::{ColorPatternModel, ColorPatternWindow, PatternVariantChip};
use pleochroma_cut_core::material::pattern::{ColorPattern, ColorRef, PatternColor, ZoneColor};
use slint::{Color, ComponentHandle, ModelRc, TimerMode, VecModel};
use std::time::Duration;

/// How long the solve waits after the last change of the picked color.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// The plane's picture size in pixels.
const PLANE_SIZE: (usize, usize) = (280, 150);
/// The hue strip's picture size in pixels.
const STRIP_SIZE: (usize, usize) = (280, 16);
/// The largest hue the strip reaches, so the end of the strip is not the start again.
const MAX_HUE: f64 = 359.99;

/// The open picker: which tile it edits and the color as picked.
#[derive(Debug, Clone)]
pub(super) struct PickerState {
    /// The zone it edits (a stored index).
    pub zone: usize,
    /// The tile it was opened for (`PatternColorTile.slot`).
    pub which: i32,
    /// The picked `[L*, C*, h]`.
    pub lch: [f64; 3],
    /// The zone holds a variant: using the color first turns it into a picked color.
    pub from_variant: bool,
    /// The solve for the picked color failed.
    pub failed: bool,
    /// The color in the stone last solved, and its distance from the picked color (CIE76).
    pub last: Option<([u8; 3], f64)>,
}

/// A swatch color as Slint paints it.
const fn paint(rgb: [u8; 3]) -> Color {
    Color::from_rgb_u8(rgb[0], rgb[1], rgb[2])
}

/// `lch` held inside the picker's ranges: lightness 0 to 100, chroma 0 to the plane's edge, hue
/// 0 up to 360.
fn clamped(lch: [f64; 3]) -> [f64; 3] {
    let finite = |value: f64, fallback: f64| if value.is_finite() { value } else { fallback };
    [
        finite(lch[0], 50.0).clamp(0.0, 100.0),
        finite(lch[1], 0.0).clamp(0.0, PLANE_MAX_CHROMA),
        finite(lch[2], 0.0).rem_euclid(360.0),
    ]
}

/// The whole hue degree the plane is drawn for (the planes are cached per degree).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the hue is reduced to 0..360 and rounded before the cast"
)]
fn hue_degree(hue: f64) -> i32 {
    (hue.rem_euclid(360.0).round() as i32) % 360
}

/// The hex text of `lch` as the screen shows it ("#d6406f"): the color clamped into sRGB.
fn hex_of(lch: [f64; 3]) -> String {
    let [red, green, blue] = lch_to_rgb(lch);
    format!("#{red:02x}{green:02x}{blue:02x}")
}

/// The color of hex text: six digits, or three, with or without the "#".
fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let digits = text.trim().trim_start_matches('#');
    let value = |slice: &str| u8::from_str_radix(slice, 16).ok();
    match digits.len() {
        6 if digits.is_ascii() => Some([
            value(&digits[0..2])?,
            value(&digits[2..4])?,
            value(&digits[4..6])?,
        ]),
        3 if digits.is_ascii() => {
            let one = |at: usize| value(&digits[at..=at]).map(|nibble| nibble * 17);
            Some([one(0)?, one(1)?, one(2)?])
        }
        _ => None,
    }
}

/// The swatch of a variant's direction that tile `which` of a zone holding `color` shows.
fn direction_rgb(
    session: &Session,
    color: &PatternColor,
    which: i32,
    variant: usize,
) -> Option<[u8; 3]> {
    let variant = session.variants.get(variant)?;
    let swatch = zone_swatch(
        &PatternColor::from_variant(variant),
        &session.swatch_stone(),
    );
    let part = if matches!(color, PatternColor::Single(_)) {
        SwatchPart::Blend
    } else {
        tile_specs(color)
            .into_iter()
            .find(|tile| tile.slot == which)
            .map_or(SwatchPart::Primary, |tile| tile.part)
    };
    Some(swatch.part(part))
}

// ---- opening and closing ---------------------------------------------------------------------

/// Opens the picker for color tile `which` of the selected layer.
pub(super) fn open(host: &Host, which: i32) {
    let opened = with_session(host, |session| {
        let zone = session.selected();
        let color = &session.pattern.zone(zone)?.color;
        let (lch, from_variant) = if matches!(color, PatternColor::Variant { .. }) {
            let swatch = zone_swatch(color, &session.swatch_stone());
            let part = tile_specs(color)
                .into_iter()
                .find(|tile| tile.slot == which)?
                .part;
            (rgb_to_lch(swatch.part(part)), true)
        } else {
            let slot = slot_for(color, which)?;
            (session.pattern.color(ColorRef { zone, slot })?.lch, false)
        };
        session.pending_pick = None;
        session.picker = Some(PickerState {
            zone,
            which,
            lch: clamped(lch),
            from_variant,
            failed: false,
            last: None,
        });
        Some(())
    })
    .flatten();
    if opened.is_none() {
        return;
    }
    push_static(host);
    push_values(host);
    solve_now(host);
}

/// "Use Color": the picked color goes to the tile as one undo step.
fn applied(host: &Host) {
    let Some(state) = with_session(host, |session| session.picker.clone()).flatten() else {
        return;
    };
    let changed = actions::edit(host, None, |session| picked_pattern(session, &state));
    if !changed {
        with_session(host, |session| session.picker = None);
        actions::present(host);
    }
}

/// Closes the picker without changing the pattern.
pub(super) fn cancelled(host: &Host) {
    with_session(host, |session| session.picker = None);
    actions::present(host);
}

/// The pattern with the picked color in its tile; `None` when that changes nothing. A variant
/// zone becomes a picked color first.
fn picked_pattern(session: &Session, state: &PickerState) -> Option<ColorPattern> {
    let mut next = if state.from_variant {
        actions::custom_of(session, state.zone)?
    } else {
        session.pattern.clone()
    };
    let slot = slot_for(&next.zone(state.zone)?.color, state.which);
    let Some(slot) = slot else {
        // The converted zone has no such tile: it keeps the conversion only.
        return state.from_variant.then_some(next);
    };
    let at = ColorRef {
        zone: state.zone,
        slot,
    };
    if !state.from_variant
        && next
            .color(at)?
            .lch
            .iter()
            .zip(state.lch)
            .all(|(old, new)| (old - new).abs() < 1e-4)
    {
        return None;
    }
    next.set_color(at, ZoneColor::unsolved(state.lch))
        .then_some(next)
}

// ---- the picture and the numbers --------------------------------------------------------------

/// Sets what stays the same while the picker is open: its title, the hue strip, the variant
/// chips and the slot.
fn push_static(host: &Host) {
    let model = host.window.global::<ColorPatternModel>();
    let Ok(mut guard) = host.session.try_borrow_mut() else {
        return;
    };
    let Some(session) = guard.as_mut() else {
        return;
    };
    let Some(state) = session.picker.clone() else {
        model.set_picker_open(false);
        return;
    };
    let color = session
        .pattern
        .zone(state.zone)
        .map(|zone| zone.color.clone());
    if let Some(color) = &color {
        model.set_picker_title(picker_title(color, state.which).into());
    }
    model.set_picker_slot(state.which);
    if session.hue_strip.is_none() {
        session.hue_strip = raster_image(&hue_strip(STRIP_SIZE.0, STRIP_SIZE.1));
    }
    if let Some(strip) = &session.hue_strip {
        model.set_picker_hue_strip(strip.clone());
    }
    let stone = session.swatch_stone();
    let chips: Vec<PatternVariantChip> = session
        .variants
        .iter()
        .map(|variant| {
            let swatch = zone_swatch(&PatternColor::from_variant(variant), &stone);
            PatternVariantChip {
                label: variant.label.as_str().into(),
                along: paint(swatch.primary),
                across: paint(swatch.across),
            }
        })
        .collect();
    model.set_picker_variants(ModelRc::new(VecModel::from(chips)));
    model.set_picker_open(true);
}

/// Sets what follows the picked color: the numbers, the plane at its hue and the result row.
fn push_values(host: &Host) {
    let model = host.window.global::<ColorPatternModel>();
    let Ok(mut guard) = host.session.try_borrow_mut() else {
        return;
    };
    let Some(session) = guard.as_mut() else {
        return;
    };
    let Some(state) = session.picker.clone() else {
        model.set_picker_open(false);
        return;
    };
    let lch = state.lch;
    model.set_picker_l(f32_of(lch[0]));
    model.set_picker_c(f32_of(lch[1]));
    model.set_picker_h(f32_of(lch[2]));
    model.set_picker_hex(hex_of(lch).into());
    let degree = hue_degree(lch[2]);
    let plane = session
        .planes
        .entry(degree)
        .or_insert_with(|| {
            raster_image(&picker_plane(f64::from(degree), PLANE_SIZE.0, PLANE_SIZE.1))
                .unwrap_or_default()
        })
        .clone();
    model.set_picker_plane(plane);

    // The color in the stone: the solved one when the cache has it, else the last one.
    let path_mm = session.path_mm;
    let solved = session.cache.get(&cache_key(lch)).cloned();
    let mut last = state.last;
    if let Some(bands) = &solved {
        let color = ZoneColor::solved(lch, bands.clone());
        last = Some((
            color_swatch(&color, path_mm),
            delta_e(lch, lch_of_rows(bands, path_mm)),
        ));
        if let Some(open) = session.picker.as_mut() {
            open.last = last;
        }
    }
    let solving = solved.is_none() && !state.failed;
    let gamut = if in_srgb_gamut(lch) {
        ""
    } else {
        " Outside sRGB, so it is shown at its nearest color."
    };
    let text = match (last, state.failed) {
        (_, true) => "This color could not be worked out. Pick another one.".to_owned(),
        (Some((_, distance)), false) if !solving => format!(
            "In the stone at {path_mm:.1} mm. Closest absorption, \u{394}E {distance:.1}.{gamut}"
        ),
        (Some(_) | None, false) => format!("Working out the color in the stone.{gamut}"),
    };
    model.set_picker_result_from(paint(lch_to_rgb(lch)));
    model.set_picker_result_to(paint(last.map_or_else(|| lch_to_rgb(lch), |(rgb, _)| rgb)));
    model.set_picker_result_text(text.into());
    model.set_picker_solving(solving);
}

// ---- changing the color ----------------------------------------------------------------------

/// The picked color is now `lch`: redraws the values and solves it a moment after the last change.
fn set_lch(host: &Host, lch: [f64; 3]) {
    let lch = clamped(lch);
    let open = with_session(host, |session| {
        let state = session.picker.as_mut()?;
        state.lch = lch;
        state.failed = false;
        Some(())
    })
    .flatten();
    if open.is_none() {
        return;
    }
    push_values(host);
    host.picker_timer
        .start(TimerMode::SingleShot, DEBOUNCE, || {
            on_host(|host| solve_now(host));
        });
}

/// The picked color as it is now, if the picker is open.
fn current(host: &Host) -> Option<[f64; 3]> {
    with_session(host, |session| {
        session.picker.as_ref().map(|state| state.lch)
    })
    .flatten()
}

/// A drag on the plane: chroma from `x`, lightness from `y` (fractions of the picture); the hue
/// stays. A release solves at once.
fn plane_pointer(host: &Host, x: f32, y: f32, phase: i32) {
    let Some(lch) = current(host) else {
        return;
    };
    let (x, y) = (f64::from(x).clamp(0.0, 1.0), f64::from(y).clamp(0.0, 1.0));
    set_lch(host, [100.0 * (1.0 - y), PLANE_MAX_CHROMA * x, lch[2]]);
    if phase == 2 {
        solve_now(host);
    }
}

/// A drag on the hue strip: the hue from `x` (a fraction of the strip); lightness and chroma stay.
fn hue_pointer(host: &Host, x: f32, phase: i32) {
    let Some(lch) = current(host) else {
        return;
    };
    let hue = (f64::from(x).clamp(0.0, 1.0) * 360.0).min(MAX_HUE);
    set_lch(host, [lch[0], lch[1], hue]);
    if phase == 2 {
        solve_now(host);
    }
}

/// The hex field was committed: its color becomes the picked color; text that is no color puts
/// the real value back.
fn hex_committed(host: &Host, text: &str) {
    // The field also reports a focus loss with the text it already had: that changes nothing.
    match parse_hex(text) {
        Some(rgb) => {
            if current(host).is_some_and(|lch| lch_to_rgb(lch) == rgb) {
                return;
            }
            set_lch(host, rgb_to_lch(rgb));
        }
        None => push_values(host),
    }
}

/// A variant chip was clicked: the variant's color for this tile's direction is picked.
fn variant_picked(host: &Host, index: i32) {
    let rgb = with_session(host, |session| {
        let state = session.picker.as_ref()?;
        let color = &session.pattern.zone(state.zone)?.color;
        direction_rgb(session, color, state.which, usize::try_from(index).ok()?)
    })
    .flatten();
    if let Some(rgb) = rgb {
        set_lch(host, rgb_to_lch(rgb));
    }
}

// ---- solving ---------------------------------------------------------------------------------

/// Solves the picked color on the worker unless the cache has it or another solve is running (its
/// delivery calls [`after_solve`], which asks again).
pub(super) fn solve_now(host: &Host) {
    let job = with_session(host, |session| {
        let lch = session.picker.as_ref()?.lch;
        (!session.cache.contains_key(&cache_key(lch)) && !session.solving)
            .then_some((lch, session.path_mm))
    })
    .flatten();
    if let Some((lch, path_mm)) = job {
        host.solver.submit(vec![lch], path_mm);
    }
}

/// A solve was delivered: shows the picked color's result if the cache has it, says so when the
/// solve failed, else asks for it again (a newer solve may have taken its place).
pub(super) fn after_solve(host: &Host, failed: bool) {
    let state = with_session(host, |session| {
        let lch = session.picker.as_ref()?.lch;
        Some(session.cache.contains_key(&cache_key(lch)))
    })
    .flatten();
    match state {
        None => {}
        Some(true) => push_values(host),
        Some(false) if failed => {
            with_session(host, |session| {
                if let Some(state) = session.picker.as_mut() {
                    state.failed = true;
                }
            });
            push_values(host);
        }
        Some(false) => solve_now(host),
    }
}

/// Registers the picker's callbacks.
pub(super) fn wire(window: &ColorPatternWindow) {
    let model = window.global::<ColorPatternModel>();
    model.on_open_picker(|slot| on_host(|host| open(host, slot)));
    model.on_picker_lch_changed(|lightness, chroma, hue| {
        on_host(|host| {
            set_lch(host, [lightness, chroma, hue].map(f64::from));
        });
    });
    model.on_picker_plane_pointer(|x, y, phase| on_host(|host| plane_pointer(host, x, y, phase)));
    model.on_picker_hue_pointer(|x, phase| on_host(|host| hue_pointer(host, x, phase)));
    model.on_picker_hex_committed(|text| on_host(|host| hex_committed(host, text.as_str())));
    model.on_picker_variant_picked(|index| on_host(|host| variant_picked(host, index)));
    model.on_picker_applied(|| on_host(|host| applied(host)));
    model.on_picker_cancelled(|| on_host(|host| cancelled(host)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_text_reads_with_or_without_the_hash_and_in_short_form() {
        assert_eq!(parse_hex("#d6406f"), Some([0xd6, 0x40, 0x6f]));
        assert_eq!(parse_hex(" D6406F "), Some([0xd6, 0x40, 0x6f]));
        assert_eq!(parse_hex("#fa0"), Some([0xff, 0xaa, 0x00]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gg0000"), None);
        assert_eq!(parse_hex(""), None);
        assert_eq!(parse_hex("#\u{e9}\u{e9}\u{e9}"), None);
    }

    #[test]
    fn the_hex_of_a_color_reads_back_as_the_same_color() {
        let lch = [55.0, 58.0, 354.0];
        let rgb = lch_to_rgb(lch);
        assert_eq!(parse_hex(&hex_of(lch)), Some(rgb));
    }

    #[test]
    fn the_picked_color_is_held_inside_the_ranges() {
        assert_eq!(
            clamped([120.0, 400.0, 725.0]),
            [100.0, PLANE_MAX_CHROMA, 5.0]
        );
        let broken = clamped([f64::NAN, -3.0, f64::INFINITY]);
        assert_eq!(broken, [50.0, 0.0, 0.0]);
    }

    #[test]
    fn the_plane_is_drawn_per_whole_degree() {
        assert_eq!(hue_degree(354.4), 354);
        assert_eq!(hue_degree(359.6), 0);
        assert_eq!(hue_degree(-0.4), 0);
    }
}
