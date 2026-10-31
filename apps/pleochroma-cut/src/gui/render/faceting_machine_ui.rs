//! The Faceting Machine preference: the Preferences dialog's section (`FacetingMachineModel`).
//!
//! The machine decides the direction of the scratches a polished or ground facet shows
//! (`pleochroma::optics::raytracer::surface::sweep`). Cutters' machines differ, so it is a per-user
//! preference (`AppSettings::faceting_machine`) and never part of a design. Every change is saved
//! through the settings persister, becomes the machine in force
//! ([`set_current_faceting_machine`]: the library scans and the exports read it) and is handed to
//! the render context, which restarts the render when the stone on screen has a surface finish.
//!
//! The section also draws the machine: the lap seen from above, the band the stone travels over,
//! the dop arm and the scratch direction at three radii of the band. The drawing is plain path
//! data in a 200 by 200 box ([`figure`]), so it is tested without a window.

use std::{
    fmt::Write as _,
    sync::{Arc, Mutex},
};

use pleochroma::optics::raytracer::surface::{FacetingMachine, SweepPreset, SweepRange};
use slint::ComponentHandle;

use crate::{
    FacetingMachineModel, MainWindow,
    bridge::render_thread::RenderContext,
    settings::{
        SettingsPersister,
        model::{FacetingMachineSetting, set_current_faceting_machine},
    },
};

/// The centre of the drawing box, in its own units.
const CENTRE: f32 = 100.0;

/// The lap's radius in the drawing box.
const LAP_UNITS: f32 = 90.0;

/// Half the length of a scratch tick in the drawing box.
const TICK_HALF: f32 = 11.0;

/// How close two radii must be to count as the same, in millimetres.
const SAME_MM: f32 = 0.05;

/// The names of the sweep picker: 0 Narrow, 1 Typical, 2 Wide, 3 Custom.
const PRESETS: [SweepPreset; 3] = [SweepPreset::Narrow, SweepPreset::Typical, SweepPreset::Wide];

/// The picker entry that stands for a hand-set range.
const CUSTOM_PRESET: i32 = 3;

/// The path data of the drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Figure {
    lap: String,
    band: String,
    arm: String,
    scratches: String,
}

/// A screen point of the drawing for the lap point `(x, y)` in millimetres (y up), given the scale
/// in drawing units per millimetre.
fn screen(x_mm: f32, y_mm: f32, units_per_mm: f32) -> (f32, f32) {
    (
        x_mm.mul_add(units_per_mm, CENTRE),
        (-y_mm).mul_add(units_per_mm, CENTRE),
    )
}

/// A full circle of `radius` drawing units about the centre, as two arcs.
fn circle(radius: f32) -> String {
    format!(
        "M {:.2} {CENTRE} A {radius:.2} {radius:.2} 0 1 0 {:.2} {CENTRE} A {radius:.2} {radius:.2} 0 1 0 {:.2} {CENTRE} Z",
        CENTRE - radius,
        CENTRE + radius,
        CENTRE - radius,
    )
}

/// The distance along the arm's line from the lap's tangent point to the contact point at radius
/// `radius_mm`: the contact point sits at `(d, y)` with `d² + y² = r²`.
fn contact_y(machine: &FacetingMachine, radius_mm: f32) -> f32 {
    let d = machine.mast_distance_mm;
    radius_mm.mul_add(radius_mm, -(d * d)).max(0.0).sqrt()
}

/// The scratch ticks as pairs of end points in the drawing box: one at the inner, middle and outer
/// radius of the sweep. The tick at radius `r` lies along the tangent of the circle through the
/// contact point, turned by the arm offset (the same angle the renderer draws,
/// [`FacetingMachine::angle_at_radius_deg`]).
fn scratch_ticks(machine: &FacetingMachine) -> Vec<((f32, f32), (f32, f32))> {
    let units_per_mm = LAP_UNITS / machine.lap_radius_mm.max(1.0);
    let inner = machine.sweep.inner_mm;
    let outer = machine.sweep.outer_mm;
    [inner, f32::midpoint(inner, outer), outer]
        .into_iter()
        .map(|radius| {
            let centre = screen(
                machine.mast_distance_mm,
                contact_y(machine, radius),
                units_per_mm,
            );
            let theta = machine.angle_at_radius_deg(radius).to_radians();
            // On screen (y down) a scratch at `theta` clockwise from horizontal runs along
            // `(cos, sin)`.
            let (dx, dy) = (theta.cos() * TICK_HALF, theta.sin() * TICK_HALF);
            (
                (centre.0 - dx, centre.1 - dy),
                (centre.0 + dx, centre.1 + dy),
            )
        })
        .collect()
}

/// The drawing of `machine`.
fn figure(machine: &FacetingMachine) -> Figure {
    let units_per_mm = LAP_UNITS / machine.lap_radius_mm.max(1.0);
    let inner = machine.sweep.inner_mm * units_per_mm;
    let outer = machine.sweep.outer_mm * units_per_mm;
    // The band between the two sweep circles; a zero inner radius leaves a filled disc.
    let mut band = circle(outer.max(0.5));
    if inner >= 0.5 {
        band.push(' ');
        band.push_str(&circle(inner));
    }
    // The dop arm: a line at distance `d` from the lap centre, running from below the lap to the
    // contact point at the outer radius of the sweep.
    let d = machine
        .mast_distance_mm
        .clamp(-machine.lap_radius_mm, machine.lap_radius_mm);
    let from = screen(d, -machine.lap_radius_mm, units_per_mm);
    let to = screen(d, contact_y(machine, machine.sweep.outer_mm), units_per_mm);
    let arm = format!("M {:.2} {:.2} L {:.2} {:.2}", from.0, from.1, to.0, to.1);
    let mut scratches = String::new();
    for (a, b) in scratch_ticks(machine) {
        // Writing to a `String` cannot fail.
        let _ = write!(
            scratches,
            "M {:.2} {:.2} L {:.2} {:.2} ",
            a.0, a.1, b.0, b.1
        );
    }
    Figure {
        lap: circle(LAP_UNITS),
        band,
        arm,
        scratches: scratches.trim_end().to_owned(),
    }
}

/// `value` rounded to whole degrees, never `-0`.
fn degrees(value: f32) -> String {
    format!("{:.0}", value.round() + 0.0)
}

/// One plain sentence about the scratch angles the machine gives.
fn angles_text(machine: &FacetingMachine) -> String {
    let (low, high) = machine.angle_range_deg();
    if high - low < 0.5 {
        format!(
            "The scratches run {} degrees from the line parallel to the girdle.",
            degrees(low)
        )
    } else {
        format!(
            "The scratches fan out from {} to {} degrees from the line parallel to the girdle.",
            degrees(low),
            degrees(high)
        )
    }
}

/// The picker entry that matches `machine`'s sweep: Narrow, Typical or Wide when the range is that
/// preset's for the lap, else Custom.
fn preset_of(machine: &FacetingMachine) -> i32 {
    PRESETS
        .iter()
        .position(|preset| {
            let range = preset.range(machine.lap_radius_mm);
            (range.inner_mm - machine.sweep.inner_mm).abs() < SAME_MM
                && (range.outer_mm - machine.sweep.outer_mm).abs() < SAME_MM
        })
        .and_then(|position| i32::try_from(position).ok())
        .unwrap_or(CUSTOM_PRESET)
}

/// The range picker entry `preset` stands for on a lap of `lap_radius_mm`; `None` for Custom (and
/// anything out of range), which keeps the radii as they are.
fn range_of_preset(preset: i32, lap_radius_mm: f32) -> Option<SweepRange> {
    usize::try_from(preset)
        .ok()
        .and_then(|position| PRESETS.get(position))
        .map(|preset| preset.range(lap_radius_mm))
}

/// The setting the model holds.
fn setting_of(ui: &MainWindow) -> FacetingMachineSetting {
    let model = ui.global::<FacetingMachineModel>();
    FacetingMachineSetting {
        arm_offset_deg: model.get_arm_offset_deg(),
        lap_radius_mm: model.get_lap_radius_mm(),
        mast_distance_mm: model.get_mast_distance_mm(),
        sweep_inner_mm: model.get_sweep_inner_mm(),
        sweep_outer_mm: model.get_sweep_outer_mm(),
    }
}

/// Writes `setting` into the numbers of the model.
fn put_setting(ui: &MainWindow, setting: &FacetingMachineSetting) {
    let model = ui.global::<FacetingMachineModel>();
    model.set_arm_offset_deg(setting.arm_offset_deg);
    model.set_lap_radius_mm(setting.lap_radius_mm);
    model.set_mast_distance_mm(setting.mast_distance_mm);
    model.set_sweep_inner_mm(setting.sweep_inner_mm);
    model.set_sweep_outer_mm(setting.sweep_outer_mm);
}

/// Pushes the drawing and the angle sentence for `machine`.
fn publish_figure(ui: &MainWindow, machine: &FacetingMachine) {
    let model = ui.global::<FacetingMachineModel>();
    let drawing = figure(machine);
    model.set_lap_commands(drawing.lap.into());
    model.set_band_commands(drawing.band.into());
    model.set_arm_commands(drawing.arm.into());
    model.set_scratch_commands(drawing.scratches.into());
    model.set_angles_text(angles_text(machine).into());
}

/// Saves `setting`, makes it the machine in force, restarts the render when it matters and redraws
/// the figure. The sweep radii the model holds are the user's; only the picker follows them.
fn apply(
    ui: &MainWindow,
    setting: &FacetingMachineSetting,
    render_ctx: &Arc<Mutex<RenderContext>>,
    store: &SettingsPersister,
) {
    let saved = *setting;
    store.update(|file| file.settings.faceting_machine = saved);
    let machine = set_current_faceting_machine(setting);
    {
        let mut ctx = RenderContext::lock(render_ctx);
        ctx.set_faceting_machine(machine);
        // The machine only shows on a stone with a surface finish; the other renders stay put.
        if ctx.scene_effects.has_surface() {
            ctx.dirty = true;
        }
    }
    ui.global::<FacetingMachineModel>()
        .set_sweep_preset(preset_of(&machine));
    publish_figure(ui, &machine);
}

/// Seeds the model from the saved preference and wires its callbacks. Called once with the other
/// render callbacks.
pub fn setup_faceting_machine_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let saved = settings_store.snapshot().settings.faceting_machine;
    put_setting(ui, &saved);
    let machine = saved.to_machine();
    ui.global::<FacetingMachineModel>()
        .set_sweep_preset(preset_of(&machine));
    publish_figure(ui, &machine);

    let model = ui.global::<FacetingMachineModel>();

    // A number changed by hand. While the sweep is a named preset it follows the lap radius.
    let (weak, ctx, store) = (
        ui.as_weak(),
        Arc::clone(render_ctx),
        Arc::clone(settings_store),
    );
    model.on_value_changed(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut setting = setting_of(&ui);
        let preset = ui.global::<FacetingMachineModel>().get_sweep_preset();
        if let Some(range) = range_of_preset(preset, setting.lap_radius_mm) {
            setting.sweep_inner_mm = range.inner_mm;
            setting.sweep_outer_mm = range.outer_mm;
            put_setting(&ui, &setting);
        }
        apply(&ui, &setting, &ctx, &store);
    });

    // A sweep name was picked: Narrow, Typical and Wide set the radii, Custom keeps them.
    let (weak, ctx, store) = (
        ui.as_weak(),
        Arc::clone(render_ctx),
        Arc::clone(settings_store),
    );
    model.on_sweep_preset_chosen(move |preset: i32| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let mut setting = setting_of(&ui);
        if let Some(range) = range_of_preset(preset, setting.lap_radius_mm) {
            setting.sweep_inner_mm = range.inner_mm;
            setting.sweep_outer_mm = range.outer_mm;
            put_setting(&ui, &setting);
        }
        apply(&ui, &setting, &ctx, &store);
        // `apply` shows the preset the radii match; a picked Custom stays Custom.
        ui.global::<FacetingMachineModel>().set_sweep_preset(preset);
    });

    // Back to the plain machine.
    let (weak, ctx, store) = (
        ui.as_weak(),
        Arc::clone(render_ctx),
        Arc::clone(settings_store),
    );
    model.on_reset_requested(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let setting = FacetingMachineSetting::default();
        put_setting(&ui, &setting);
        apply(&ui, &setting, &ctx, &store);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side_machine() -> FacetingMachine {
        FacetingMachine {
            mast_distance_mm: 30.0,
            ..FacetingMachine::DEFAULT
        }
    }

    #[test]
    #[expect(
        clippy::suboptimal_flops,
        reason = "test arithmetic written as the formula"
    )]
    fn a_centred_arm_draws_level_scratch_ticks_in_a_vertical_line() {
        let ticks = scratch_ticks(&FacetingMachine::DEFAULT);
        assert_eq!(ticks.len(), 3);
        for ((x0, y0), (x1, y1)) in &ticks {
            assert!((y0 - y1).abs() < 1e-3, "level: {y0} {y1}");
            assert!(((x0 + x1) * 0.5 - CENTRE).abs() < 1e-3, "on the arm");
            assert!((x1 - x0 - 2.0 * TICK_HALF).abs() < 1e-3);
        }
    }

    #[test]
    fn a_mast_to_the_side_fans_the_ticks_out_and_an_offset_turns_them_all() {
        let ticks = scratch_ticks(&side_machine());
        let slope = |((x0, y0), (x1, y1)): &((f32, f32), (f32, f32))| (y1 - y0).atan2(x1 - x0);
        // The tangent turns with the radius: the inner tick leans more than the outer one.
        assert!(slope(&ticks[0]).abs() > slope(&ticks[2]).abs());
        // The offset turns every tick by the same angle (a clockwise turn is positive on screen).
        let turned = scratch_ticks(&FacetingMachine {
            arm_offset_deg: 30.0,
            ..side_machine()
        });
        for (plain, with_offset) in ticks.iter().zip(&turned) {
            let difference = (slope(with_offset) - slope(plain)).to_degrees();
            assert!((difference - 30.0).abs() < 0.01, "{difference}");
        }
    }

    #[test]
    fn the_drawing_has_a_lap_a_band_with_a_hole_an_arm_and_three_ticks() {
        let drawing = figure(&FacetingMachine::DEFAULT);
        assert!(drawing.lap.starts_with("M 10.00 100"));
        // Two circles: the outer and the inner edge of the band.
        assert_eq!(drawing.band.matches('M').count(), 2);
        assert!(drawing.arm.starts_with("M 100.00 190.00 L 100.00"));
        assert_eq!(drawing.scratches.matches('M').count(), 3);
        // A sweep from the lap centre fills the disc.
        let filled = figure(&FacetingMachine {
            sweep: SweepRange {
                inner_mm: 0.0,
                outer_mm: 80.0,
            },
            ..FacetingMachine::DEFAULT
        });
        assert_eq!(filled.band.matches('M').count(), 1);
    }

    #[test]
    fn the_angle_sentence_says_one_angle_or_the_fan() {
        let one = angles_text(&FacetingMachine::DEFAULT);
        assert!(one.contains("run 0 degrees"), "{one}");
        let fan = angles_text(&side_machine());
        assert!(fan.contains("fan out from"), "{fan}");
        assert!(!fan.contains("-0"), "{fan}");
    }

    #[test]
    fn the_picker_follows_the_radii_and_a_named_sweep_sets_them() {
        assert_eq!(preset_of(&FacetingMachine::DEFAULT), 1, "Typical");
        for (index, preset) in PRESETS.iter().enumerate() {
            let machine = FacetingMachine {
                lap_radius_mm: 150.0,
                sweep: preset.range(150.0),
                ..FacetingMachine::DEFAULT
            };
            let expected = i32::try_from(index).expect("a small index");
            assert_eq!(preset_of(&machine), expected);
            assert_eq!(range_of_preset(expected, 150.0), Some(preset.range(150.0)));
        }
        let custom = FacetingMachine {
            sweep: SweepRange {
                inner_mm: 12.0,
                outer_mm: 90.0,
            },
            ..FacetingMachine::DEFAULT
        };
        assert_eq!(preset_of(&custom), CUSTOM_PRESET);
        assert_eq!(range_of_preset(CUSTOM_PRESET, 100.0), None);
        assert_eq!(range_of_preset(-1, 100.0), None);
    }

    #[test]
    fn the_setting_round_trips_through_the_model_numbers_and_back_to_a_machine() {
        let setting = FacetingMachineSetting {
            arm_offset_deg: 12.5,
            lap_radius_mm: 120.0,
            mast_distance_mm: 35.0,
            sweep_inner_mm: 50.0,
            sweep_outer_mm: 100.0,
        };
        let machine = setting.to_machine();
        assert_eq!(FacetingMachineSetting::from_machine(&machine), setting);
    }
}
