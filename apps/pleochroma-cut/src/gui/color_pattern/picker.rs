//! Zone-color picking inside the pattern window for a stone session.

use super::{
    actions,
    colors::lch_to_rgb,
    model::f32_of,
    session::{Host, on_host, with_session},
};
use crate::{ColorPatternModel, ColorPatternWindow, PhysicscolorModel, gui::optics::physics_ui};
use pleochroma_cut_core::material::pattern::ZoneColor;
use pleochroma_editor::lch_color::lch_from_srgb;
use slint::{Color, ComponentHandle};

fn paint(lch: [f64; 3]) -> Color {
    let [red, green, blue] = lch_to_rgb(lch);
    Color::from_rgb_u8(red, green, blue)
}

fn current(model: &ColorPatternModel<'_>) -> [f64; 3] {
    [
        model.get_picker_tone(),
        model.get_picker_chroma(),
        model.get_picker_hue(),
    ]
    .map(f64::from)
}

/// Opens in this window, so a stone never has to open the material dialog to pick a color.
pub(super) fn opened(host: &Host) {
    let Some(lch) = actions::pick_seed(host) else {
        return;
    };
    let model = host.window.global::<ColorPatternModel>();
    model.set_picker_tone(f32_of(lch[0]));
    model.set_picker_chroma(f32_of(lch[1]));
    model.set_picker_hue(f32_of(lch[2]));
    model.set_picker_swatch(paint(lch));
    model.set_picker_open(true);
}

fn commit(host: &Host, lch: [f64; 3]) {
    let at = with_session(host, |session| session.pending_pick.take()).flatten();
    let Some(at) = at else {
        return;
    };
    actions::edit(host, None, |session| {
        let mut next = session.pattern.clone();
        let old = next.color(at)?;
        // Keep the original spectrum when Apply is pressed without a color change.
        if old
            .lch
            .iter()
            .zip(lch)
            .all(|(old, new)| (old - new).abs() < 1e-4)
        {
            return None;
        }
        next.set_color(at, ZoneColor::unsolved(lch)).then_some(next)
    });
    actions::present(host);
}

/// Wires the local sliders and the shared HSV/hex picker on this window's own globals.
pub(super) fn wire(window: &ColorPatternWindow) {
    let physics = window.global::<PhysicscolorModel>();
    physics.on_format_hex(|color| physics_ui::format_hex(color).into());
    physics.on_parse_hex(|text, fallback| {
        physics_ui::parse_hex(&text).map_or(fallback, |[red, green, blue]| {
            Color::from_rgb_u8(red, green, blue)
        })
    });
    let model = window.global::<ColorPatternModel>();
    model.on_picker_changed(|index, value| {
        on_host(|host| {
            if !value.is_finite() {
                return;
            }
            let model = host.window.global::<ColorPatternModel>();
            match index {
                0 => model.set_picker_tone(value.clamp(0.0, 100.0)),
                1 => model.set_picker_chroma(value.clamp(0.0, 150.0)),
                2 => model.set_picker_hue(value.clamp(0.0, 360.0)),
                _ => return,
            }
            model.set_picker_swatch(paint(current(&model)));
        });
    });
    model.on_picker_applied(|| {
        on_host(|host| {
            commit(host, current(&host.window.global::<ColorPatternModel>()));
        });
    });
    model.on_picker_srgb(|color| {
        on_host(|host| {
            let rgb = [color.red(), color.green(), color.blue()]
                .map(|channel| f64::from(channel) / 255.0);
            commit(host, lch_from_srgb(rgb));
        });
    });
    model.on_picker_cancelled(|| {
        on_host(|host| {
            with_session(host, |session| session.pending_pick = None);
            actions::present(host);
        });
    });
}
