//! The Show Surface Finish overlay (`SurfaceModel`): a dot on every facet that has a surface
//! finish, tinted by its grade, and for a polished facet a short tick along the direction of its
//! scratches, taken from the user's Faceting Machine preference.
//!
//! The marks are placed from the last solve (the facet words and planes of the design on the
//! bench) and the centroids of the frame on screen, so they follow the camera. They are drawn in
//! the Solid, Path-Traced, Ray-Traced and Both views, on the facets that face the camera; the
//! Diagram view and a stone partly cut back by the Cut slider show none (their facet numbering
//! differs from the finished stone's).

use super::{
    Shared,
    crystal_axis_overlay::{View, view_of},
};
use crate::{
    MainWindow, SurfaceModel, SurfaceTick,
    gui::editor::callbacks::{pick_margin, pick_pixels_to_logical},
};
use glam::Vec3;
use pleochroma::optics::raytracer::surface::{
    FacetSurface, FacetingMachine, SurfaceKind, sweep::scratch_direction,
};
use slint::{Color, ComponentHandle as _, ModelRc, VecModel};
use std::sync::PoisonError;

/// Half the length of a scratch tick, as a fraction of the stone's bounding radius.
const TICK_HALF_LENGTH: f32 = 0.07;

/// A scratch ratio above this has a direction worth drawing.
const MIN_DIRECTIONAL_ANISOTROPY: f32 = 1.001;

/// One mark in model space.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Mark {
    centre: Vec3,
    /// The two ends of the scratch tick; `None` for a facet whose finish is round.
    tick: Option<(Vec3, Vec3)>,
    tint: [u8; 3],
}

/// The tint of a finish: the polishes run from teal (a mirror) through green and yellow to amber
/// (a pre-polish), the ground finishes from orange to ruby as they get rougher.
fn tint_of(word: FacetSurface) -> [u8; 3] {
    match word.kind() {
        SurfaceKind::Ideal => [130, 130, 130],
        SurfaceKind::Polished => match word.rq_nm() {
            rq if rq <= 1.5 => [64, 201, 176],
            rq if rq <= 4.0 => [110, 200, 110],
            rq if rq <= 12.0 => [225, 200, 80],
            _ => [235, 140, 60],
        },
        SurfaceKind::Frosted | SurfaceKind::Ground => {
            if word.ground_alpha() < 0.4 {
                [225, 100, 80]
            } else {
                [200, 60, 90]
            }
        }
    }
}

/// The marks of the facets in `facets` (`(centroid, outward normal, finish word)` of each facet
/// that has a centroid) that face a camera at `eye`. A facet with the ideal finish has none.
fn marks(
    facets: &[(Vec3, Vec3, FacetSurface)],
    machine: &FacetingMachine,
    eye: Vec3,
    half_length: f32,
) -> Vec<Mark> {
    facets
        .iter()
        .filter(|(centre, normal, word)| !word.is_ideal() && normal.dot(eye - *centre) > 0.0)
        .map(|&(centre, normal, word)| {
            let directional = word.kind() == SurfaceKind::Polished
                && word.anisotropy() > MIN_DIRECTIONAL_ANISOTROPY;
            let tick = directional.then(|| {
                let angle = word
                    .custom_angle_deg()
                    .unwrap_or_else(|| machine.nominal_angle_deg());
                let along = scratch_direction(normal, angle) * half_length;
                (centre - along, centre + along)
            });
            Mark {
                centre,
                tick,
                tint: tint_of(word),
            }
        })
        .collect()
}

/// Hides the overlay.
fn clear(ui: &MainWindow) {
    let model = ui.global::<SurfaceModel>();
    model.set_overlay_visible(false);
    model.set_ticks(ModelRc::new(VecModel::from(Vec::<SurfaceTick>::new())));
}

/// Recomputes the marks (the switch, the view or the design changed, or a frame landed).
pub(super) fn refresh(ui: &MainWindow, ctx: &Shared) {
    let model = ui.global::<SurfaceModel>();
    if !model.get_show_overlay() {
        clear(ui);
        return;
    }
    let Some(ticks) = compute(ui, ctx) else {
        clear(ui);
        return;
    };
    model.set_ticks(ModelRc::new(VecModel::from(ticks)));
    model.set_overlay_visible(true);
}

/// The marks in logical viewport coordinates, or `None` while there is nothing to draw (no frame, a
/// view without marks, a stone without a surface finish).
fn compute(ui: &MainWindow, ctx: &Shared) -> Option<Vec<SurfaceTick>> {
    let geometry = ctx
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    if geometry.diagram.is_some() || geometry.visible_tiers.is_some() {
        return None;
    }
    let masts = ctx
        .solid_last_solved
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(_, masts)| masts.clone())?;
    let facets: Vec<(Vec3, Vec3, FacetSurface)> = {
        let state = ctx.state.borrow();
        let design = &state.design;
        if !design.has_surface() || masts.len() != design.tiers.len() {
            return None;
        }
        let words = design.facet_surfaces_from_solved(&masts);
        let planes = design.planes_from_solved(&masts);
        words
            .iter()
            .zip(&planes)
            .enumerate()
            .filter_map(|(id, (&word, &(normal, _)))| {
                let centre = geometry.facet_centroids.get(id).copied().flatten()?;
                Some((centre, normal.as_vec3(), word))
            })
            .collect()
    };
    if facets.is_empty() {
        return None;
    }
    let (view, _) = view_of(&geometry)?;
    let View::Camera(camera, _) = &view else {
        return None;
    };
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the bounding radius is a small model-unit length; the camera math is f32"
    )]
    let half_length = TICK_HALF_LENGTH * geometry.bounding_radius as f32;
    let machine = crate::settings::model::current_faceting_machine();
    let placed = marks(&facets, &machine, camera.origin, half_length);
    let scale = ui.window().scale_factor();
    let margin = pick_margin(ui);
    let logical = |p: Vec3| {
        view.project(p)
            .map(|px| pick_pixels_to_logical(px, scale, margin))
    };
    let ticks: Vec<SurfaceTick> = placed
        .iter()
        .filter_map(|mark| {
            let centre = logical(mark.centre)?;
            let (from, to) = match mark.tick {
                Some((a, b)) => (logical(a)?, logical(b)?),
                None => (centre, centre),
            };
            let [red, green, blue] = mark.tint;
            Some(SurfaceTick {
                cx: centre.0,
                cy: centre.1,
                x0: from.0,
                y0: from.1,
                x1: to.0,
                y1: to.1,
                tint: Color::from_rgb_u8(red, green, blue),
            })
        })
        .collect();
    (!ticks.is_empty()).then_some(ticks)
}

/// Wires `SurfaceModel.overlay_changed`. Called from `wire_drag_callbacks`.
pub(super) fn setup(ui: &MainWindow, ctx: &Shared) {
    let (weak, shared) = (ui.as_weak(), ctx.clone());
    ui.global::<SurfaceModel>().on_overlay_changed(move || {
        if let Some(ui) = weak.upgrade() {
            refresh(&ui, &shared);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::raytracer::surface::PolishPreset;

    fn polished(preset: PolishPreset) -> FacetSurface {
        FacetSurface::from_preset(preset, 0.3)
    }

    /// A facet pointing at +Z, a unit above the origin.
    fn facing_z(word: FacetSurface) -> (Vec3, Vec3, FacetSurface) {
        (Vec3::new(0.0, 0.0, 1.0), Vec3::Z, word)
    }

    #[test]
    fn only_facets_facing_the_camera_with_a_finish_get_a_mark() {
        let machine = FacetingMachine::DEFAULT;
        let facets = [
            facing_z(polished(PolishPreset::Commercial)),
            facing_z(FacetSurface::IDEAL),
            (
                Vec3::new(0.0, 0.0, -1.0),
                -Vec3::Z,
                polished(PolishPreset::Fine),
            ),
        ];
        let found = marks(&facets, &machine, Vec3::new(0.0, 0.0, 10.0), 0.1);
        assert_eq!(found.len(), 1, "the ideal and the back facet have none");
        assert_eq!(found[0].centre, Vec3::Z);
    }

    #[test]
    fn a_polish_with_scratches_gets_a_tick_along_the_machines_direction_in_the_facet_plane() {
        // The plain machine: scratches parallel to the girdle, `normal x Y` for a slanted facet.
        let normal = Vec3::new(0.0, 0.6, 0.8).normalize();
        let facets = [(
            Vec3::new(0.0, 0.0, 1.0),
            normal,
            polished(PolishPreset::Commercial),
        )];
        let found = marks(
            &facets,
            &FacetingMachine::DEFAULT,
            Vec3::new(0.0, 5.0, 10.0),
            0.1,
        );
        let (a, b) = found[0].tick.expect("Commercial has scratches");
        let along = (b - a).normalize();
        assert!(along.dot(normal).abs() < 1e-5, "the tick lies in the facet");
        let girdle = normal.cross(Vec3::Y).normalize();
        assert!(along.dot(girdle).abs() > 0.999, "{along:?} vs {girdle:?}");
        assert!(((b - a).length() - 0.2).abs() < 1e-5);
        assert!(((a + b) * 0.5 - found[0].centre).length() < 1e-6);
    }

    #[test]
    fn the_machine_and_the_custom_angle_turn_the_tick() {
        let normal = Vec3::Z;
        let eye = Vec3::new(0.0, 0.0, 10.0);
        let machine = FacetingMachine {
            arm_offset_deg: 90.0,
            ..FacetingMachine::DEFAULT
        };
        let word = polished(PolishPreset::Prepolish);
        let tick_of = |word, machine: &FacetingMachine| {
            let found = marks(&[(Vec3::Z, normal, word)], machine, eye, 0.1);
            let (a, b) = found[0].tick.expect("a tick");
            (b - a).normalize()
        };
        let plain = tick_of(word, &FacetingMachine::DEFAULT);
        let turned = tick_of(word, &machine);
        assert!(
            plain.dot(turned).abs() < 1e-3,
            "a quarter turn: {plain:?} {turned:?}"
        );
        // A Custom Angle ignores the machine.
        let fixed = tick_of(word.with_custom_angle(90.0), &FacetingMachine::DEFAULT);
        assert!(fixed.dot(turned).abs() > 0.99, "{fixed:?} {turned:?}");
    }

    #[test]
    fn a_mirror_and_a_ground_facet_get_a_dot_but_no_tick() {
        let facets = [
            facing_z(polished(PolishPreset::Mirror)),
            facing_z(FacetSurface::ground(0.3, 1.0)),
        ];
        let found = marks(
            &facets,
            &FacetingMachine::DEFAULT,
            Vec3::new(0.0, 0.0, 10.0),
            0.1,
        );
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|mark| mark.tick.is_none()));
    }

    #[test]
    fn the_tint_runs_from_teal_to_amber_and_the_ground_finishes_are_warm() {
        let teal = tint_of(polished(PolishPreset::Mirror));
        let amber = tint_of(polished(PolishPreset::Prepolish));
        assert_ne!(teal, amber);
        assert_ne!(
            tint_of(polished(PolishPreset::Fine)),
            tint_of(polished(PolishPreset::Commercial))
        );
        assert_ne!(
            tint_of(FacetSurface::ground(0.3, 1.0)),
            tint_of(FacetSurface::ground(0.5, 1.0))
        );
        assert_ne!(tint_of(FacetSurface::ground(0.3, 1.0)), amber);
    }
}
