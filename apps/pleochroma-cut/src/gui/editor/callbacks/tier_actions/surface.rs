//! Surface finish of a tier and of a single facet: [`Edit::SetTierSurface`], one undo step each.
//!
//! The tier table's right-click menu (Surface Finish), the inspector's Surface Finish section and
//! the per-facet override all end here. Like frosting, a surface finish changes no geometry, so an
//! edit needs no solid-preview replan; what it needs is the path tracer, which is handed the
//! stone's new facet words at once ([`super::frosting::push_frosted_planes`]).
//!
//! The scratch direction is not authored here: it comes from the user's Faceting Machine
//! preference, so a saved design holds only the choices below (the grade, the Custom values, a
//! Custom Angle and No Sweep, per tier and per facet).
//!
//! [`publish_selected_tier`] pushes the loaded tier's state into `SurfaceModel` for the inspector;
//! the pure parts ([`finish_with_grade`], [`finish_from_controls`], [`inspector_state`]) are
//! tested without a window.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use pleochroma_cut_core::{Design, Edit, SurfaceFinish, SurfaceGrade, TierSurface};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::frosting::{facet_position, push_frosted_planes};
use crate::{
    EditorModel, MainWindow, SurfaceModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale},
        },
        show_toast,
    },
};

/// The first entry of a facet's override list: the facet takes its tier's finish.
const FOLLOW_TIER: &str = "Follow Tier";

/// Two index-wheel positions this close are the same facet (the design's own tolerance).
const SAME_FACET_EPSILON: f64 = 1e-6;

/// The grade labels in menu order (`SurfaceGrade::ALL`).
fn grade_labels() -> Vec<&'static str> {
    SurfaceGrade::ALL
        .into_iter()
        .map(SurfaceGrade::label)
        .collect()
}

/// The grade at menu position `index`.
fn grade_at(index: i32) -> Option<SurfaceGrade> {
    usize::try_from(index)
        .ok()
        .and_then(|position| SurfaceGrade::ALL.get(position).copied())
}

/// The menu position of `grade`.
fn grade_index(grade: SurfaceGrade) -> i32 {
    SurfaceGrade::ALL
        .iter()
        .position(|&candidate| candidate == grade)
        .and_then(|position| i32::try_from(position).ok())
        .unwrap_or(0)
}

/// `old` moved to `grade`: the new grade's own values, with the old finish's scratch choices (Custom
/// Angle, No Sweep) and frosted roughness kept where the new grade can use them. A Custom grade
/// picked again keeps its Rq and anisotropy; a Custom grade picked from another grade starts from
/// the Custom defaults and never inherits a frosted roughness (that would make it a ground finish).
fn finish_with_grade(old: &SurfaceFinish, grade: SurfaceGrade) -> SurfaceFinish {
    let mut finish = SurfaceFinish::of_grade(grade);
    if grade == SurfaceGrade::Custom && old.grade == SurfaceGrade::Custom {
        finish.rq_nm = old.rq_nm;
        finish.aniso = old.aniso;
    }
    finish.lap_deg = old.lap_deg;
    finish.no_sweep = old.no_sweep;
    if grade != SurfaceGrade::Custom || old.grade == SurfaceGrade::Custom {
        finish.ground_alpha = old.ground_alpha;
    }
    finish.sanitised()
}

/// The values of the inspector's controls for the loaded tier.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Controls {
    rq_nm: f32,
    aniso: f32,
    use_angle: bool,
    angle_deg: f32,
    no_sweep: bool,
}

/// `base` with the controls' values: Rq and anisotropy for a Custom grade, the Custom Angle (when
/// `use_angle`) and No Sweep for any grade that has a scratch direction. Sanitised, so a value the
/// grade does not use is dropped.
fn finish_from_controls(base: &SurfaceFinish, controls: &Controls) -> SurfaceFinish {
    let mut finish = *base;
    if base.grade == SurfaceGrade::Custom {
        finish.rq_nm = f64::from(controls.rq_nm);
        finish.aniso = f64::from(controls.aniso);
    }
    finish.lap_deg = controls.use_angle.then(|| f64::from(controls.angle_deg));
    finish.no_sweep = controls.no_sweep;
    finish.sanitised()
}

/// `current` with its tier-wide finish replaced by `finish`; the facets that differ stay as they
/// were. Normalised: `None` when nothing is left to state.
fn tier_with_finish(current: Option<&TierSurface>, finish: SurfaceFinish) -> Option<TierSurface> {
    TierSurface {
        finish,
        facets: current.map_or_else(Vec::new, |surface| surface.facets.clone()),
    }
    .normalised()
}

/// Whether `surface` has an entry of its own for the facet at `position`.
fn has_facet_entry(surface: &TierSurface, position: f64) -> bool {
    surface
        .facets
        .iter()
        .any(|(at, _)| (at - position).abs() < SAME_FACET_EPSILON)
}

/// The override list entry of the facet at `position`: `0` follows the tier, else `1` plus the
/// menu position of the facet's grade.
fn facet_option_of(surface: Option<&TierSurface>, position: f64) -> i32 {
    surface
        .filter(|surface| has_facet_entry(surface, position))
        .map_or(0, |surface| {
            grade_index(surface.finish_at(position).grade) + 1
        })
}

/// `current` with the facet at `position` set to override-list entry `option`: `0` follows the tier
/// again, otherwise the facet takes that grade (with the tier's scratch choices). `None` when
/// nothing is left to state, or for an entry out of range.
fn tier_with_facet_option(
    current: Option<&TierSurface>,
    position: f64,
    option: i32,
) -> Option<TierSurface> {
    let base = current
        .cloned()
        .unwrap_or_else(|| TierSurface::of_tier(SurfaceFinish::IDEAL));
    let finish = match option.checked_sub(1) {
        None | Some(i32::MIN..=-1) => None,
        Some(grade) => Some(finish_with_grade(&base.finish, grade_at(grade)?)),
    };
    base.with_facet(position, finish)
}

/// The label of a facet position in the override list: `24`, `12.5`.
fn position_label(position: f64) -> String {
    if (position - position.round()).abs() < SAME_FACET_EPSILON {
        format!("{position:.0}")
    } else {
        format!("{position}")
    }
}

/// What the inspector shows of the loaded tier's surface finish.
#[derive(Debug, Clone, PartialEq)]
struct InspectorState {
    grade: i32,
    controls: Controls,
    is_custom: bool,
    directional: bool,
    summary: String,
    facet_labels: Vec<String>,
    facet_positions: Vec<f32>,
    facet_pick: i32,
    facet_option: i32,
}

/// The inspector state of the tier at `tier` (`None`: no tier loaded), with the facet row on
/// `facet_pick` (clamped to the tier's facets).
#[expect(
    clippy::cast_possible_truncation,
    reason = "the controls carry f32; the stored f64 values are small"
)]
fn inspector_state(design: &Design, tier: Option<usize>, facet_pick: i32) -> InspectorState {
    let surface = tier.and_then(|index| design.tier_surface(index));
    let finish = surface.map_or(SurfaceFinish::IDEAL, |surface| surface.finish);
    let indices: &[f64] = tier
        .and_then(|index| design.tiers.get(index))
        .map_or(&[], |tier| tier.indices.as_slice());
    let last = i32::try_from(indices.len().saturating_sub(1)).unwrap_or(0);
    let pick = facet_pick.clamp(0, last);
    let picked = usize::try_from(pick)
        .ok()
        .and_then(|position| indices.get(position))
        .copied();
    let custom = finish.grade == SurfaceGrade::Custom;
    InspectorState {
        grade: grade_index(finish.grade),
        controls: Controls {
            rq_nm: if custom { finish.rq_nm as f32 } else { 6.0 },
            aniso: if custom { finish.aniso as f32 } else { 1.0 },
            use_angle: finish.lap_deg.is_some(),
            angle_deg: finish.lap_deg.map_or(0.0, |angle| angle as f32),
            no_sweep: finish.no_sweep,
        },
        is_custom: custom,
        directional: custom || finish.grade.polish_preset().is_some(),
        summary: tier.map_or_else(String::new, |index| design.tier_surface_text(index)),
        facet_labels: indices.iter().map(|&at| position_label(at)).collect(),
        facet_positions: indices.iter().map(|&at| at as f32).collect(),
        facet_pick: pick,
        facet_option: picked.map_or(0, |at| facet_option_of(surface, at)),
    }
}

/// Pushes the loaded tier's surface finish into `SurfaceModel` for the inspector. Called with the
/// other selected-tier state (`view::inspector::push_selected_tier_chips`) after every refresh, so
/// an undo, a redo, a load and a selection change all reach the section.
pub(in crate::gui::editor) fn publish_selected_tier(ui: &MainWindow, design: &Design) {
    let model = ui.global::<SurfaceModel>();
    let selected = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok();
    let state = inspector_state(design, selected, model.get_facet_pick());
    model.set_tier_grade(state.grade);
    model.set_custom_rq(state.controls.rq_nm);
    model.set_custom_aniso(state.controls.aniso);
    model.set_use_angle(state.controls.use_angle);
    model.set_angle_deg(state.controls.angle_deg);
    model.set_no_sweep(state.controls.no_sweep);
    model.set_is_custom(state.is_custom);
    model.set_directional(state.directional);
    model.set_summary(state.summary.into());
    model.set_facet_labels(ModelRc::new(VecModel::from(
        state
            .facet_labels
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    model.set_facet_positions(ModelRc::new(VecModel::from(state.facet_positions)));
    model.set_facet_pick(state.facet_pick);
    model.set_facet_option(state.facet_option);
}

/// Everything an edit callback shares.
#[derive(Clone)]
struct Wiring {
    ui: slint::Weak<MainWindow>,
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    solid_last_solved: SolidLastSolved,
}

impl Wiring {
    /// Applies `surface` to the tier at `index` as one undoable edit and refreshes the panel, the
    /// render and the overlay. A surface equal to the stored one is no edit (no empty undo step).
    fn apply(&self, index: usize, surface: Option<TierSurface>) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let surface = surface.and_then(TierSurface::normalised);
        {
            let mut st = self.state.borrow_mut();
            if st.design.tier_surface(index) == surface.as_ref() {
                return;
            }
            if let Err(e) = st.apply(Edit::SetTierSurface { index, surface }) {
                show_toast(&ui, &e.to_string(), "error");
                return;
            }
            refresh_editor_panel_stale(&ui, &self.render_ctx, &st, &BTreeSet::new());
            push_frosted_planes(&ui, &self.render_ctx, &self.solid_last_solved, &st);
            publish_selected_tier(&ui, &st.design);
        }
        // After the editor state is released: the overlay's watcher reads the design again.
        let model = ui.global::<SurfaceModel>();
        model.set_design_version(model.get_design_version().wrapping_add(1));
    }

    /// The tier-wide finish of the tier at `index` as stored.
    fn finish_of(&self, index: usize) -> SurfaceFinish {
        self.state
            .borrow()
            .design
            .tier_surface(index)
            .map_or(SurfaceFinish::IDEAL, |surface| surface.finish)
    }

    /// The tier's stored surface, cloned.
    fn surface_of(&self, index: usize) -> Option<TierSurface> {
        self.state.borrow().design.tier_surface(index).cloned()
    }
}

/// The inspector's controls as the model holds them.
fn controls_of(ui: &MainWindow) -> Controls {
    let model = ui.global::<SurfaceModel>();
    Controls {
        rq_nm: model.get_custom_rq(),
        aniso: model.get_custom_aniso(),
        use_angle: model.get_use_angle(),
        angle_deg: model.get_angle_deg(),
        no_sweep: model.get_no_sweep(),
    }
}

/// Wires every `SurfaceModel` edit callback and fills the grade lists. Called once with the other
/// tier callbacks (`tier_crud::setup_tier_crud_callbacks`).
pub(super) fn setup_surface_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid_last_solved: &SolidLastSolved,
) {
    let model = ui.global::<SurfaceModel>();
    let labels = grade_labels();
    model.set_grade_names(ModelRc::new(VecModel::from(
        labels
            .iter()
            .map(|&label| SharedString::from(label))
            .collect::<Vec<_>>(),
    )));
    model.set_facet_option_names(ModelRc::new(VecModel::from(
        std::iter::once(FOLLOW_TIER)
            .chain(labels.iter().copied())
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));

    let wiring = Wiring {
        ui: ui.as_weak(),
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        solid_last_solved: Arc::clone(solid_last_solved),
    };

    // A grade, from the context menu or the inspector's combo.
    let w = wiring.clone();
    model.on_grade_chosen(move |tier: i32, grade: i32| {
        let (Ok(index), Some(grade)) = (usize::try_from(tier), grade_at(grade)) else {
            return;
        };
        let finish = finish_with_grade(&w.finish_of(index), grade);
        w.apply(
            index,
            tier_with_finish(w.surface_of(index).as_ref(), finish),
        );
    });

    // Rq, anisotropy, Custom Angle or No Sweep of the loaded tier.
    let w = wiring.clone();
    model.on_controls_changed(move |tier: i32| {
        let (Some(ui), Ok(index)) = (w.ui.upgrade(), usize::try_from(tier)) else {
            return;
        };
        let finish = finish_from_controls(&w.finish_of(index), &controls_of(&ui));
        w.apply(
            index,
            tier_with_finish(w.surface_of(index).as_ref(), finish),
        );
    });

    // The context menu's No Sweep entries.
    let w = wiring.clone();
    model.on_no_sweep_set(move |tier: i32, on: bool| {
        let Ok(index) = usize::try_from(tier) else {
            return;
        };
        let mut finish = w.finish_of(index);
        finish.no_sweep = on;
        w.apply(
            index,
            tier_with_finish(w.surface_of(index).as_ref(), finish.sanitised()),
        );
    });

    // The override row picked another facet: show that facet's override.
    let w = wiring.clone();
    model.on_facet_picked(move |tier: i32, facet: i32| {
        let (Some(ui), Ok(index)) = (w.ui.upgrade(), usize::try_from(tier)) else {
            return;
        };
        let st = w.state.borrow();
        let state = inspector_state(&st.design, Some(index), facet);
        let model = ui.global::<SurfaceModel>();
        model.set_facet_pick(state.facet_pick);
        model.set_facet_option(state.facet_option);
    });

    // The override row chose a finish for a facet.
    let w = wiring;
    model.on_facet_option_chosen(move |tier: i32, position: f32, option: i32| {
        let Ok(index) = usize::try_from(tier) else {
            return;
        };
        let Some(stored) = facet_position(&w.state.borrow(), index, position) else {
            return;
        };
        let updated = tier_with_facet_option(w.surface_of(index).as_ref(), stored, option);
        w.apply(index, updated);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use pleochroma_cut_core::ConstraintTier;

    fn state_with_one_tier(indices: &[f64]) -> EditorState {
        let mut state = EditorState::fresh();
        state.design.tiers.push(ConstraintTier {
            angle_deg: -40.0,
            name: "P1".to_string(),
            indices: indices.to_vec(),
            constraint: MeetConstraint::MeetExisting,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        });
        state
    }

    #[test]
    fn the_menu_lists_every_grade_in_order() {
        let labels = grade_labels();
        assert_eq!(labels.len(), 8);
        assert_eq!(labels[0], "Ideal");
        assert_eq!(labels[5], "Ground (Fine)");
        assert_eq!(labels[6], "Ground (Coarse)");
        assert_eq!(labels[7], "Custom");
        for (position, grade) in SurfaceGrade::ALL.into_iter().enumerate() {
            let index = i32::try_from(position).expect("a small index");
            assert_eq!(grade_at(index), Some(grade));
            assert_eq!(grade_index(grade), index);
        }
        assert_eq!(grade_at(-1), None);
        assert_eq!(grade_at(8), None);
    }

    #[test]
    fn a_new_grade_keeps_the_scratch_choices_it_can_use() {
        let old = SurfaceFinish {
            lap_deg: Some(30.0),
            no_sweep: true,
            ..SurfaceFinish::of_grade(SurfaceGrade::Commercial)
        };
        let fine = finish_with_grade(&old, SurfaceGrade::Fine);
        assert_eq!(fine.grade, SurfaceGrade::Fine);
        assert_eq!(fine.lap_deg, Some(30.0));
        assert!(fine.no_sweep);
        // A ground finish is round: the scratch choices are dropped.
        let ground = finish_with_grade(&old, SurfaceGrade::GroundFine);
        assert_eq!(ground.lap_deg, None);
        assert!(!ground.no_sweep);
        // And the ideal finish states nothing.
        assert!(finish_with_grade(&old, SurfaceGrade::Ideal).is_ideal());
    }

    #[test]
    fn custom_picked_again_keeps_its_values_and_picked_fresh_starts_at_the_defaults() {
        let custom = SurfaceFinish {
            rq_nm: 12.0,
            aniso: 2.5,
            ..SurfaceFinish::of_grade(SurfaceGrade::Custom)
        };
        let again = finish_with_grade(&custom, SurfaceGrade::Custom);
        assert!((again.rq_nm - 12.0).abs() < 1e-9 && (again.aniso - 2.5).abs() < 1e-9);
        let fresh = finish_with_grade(
            &SurfaceFinish::of_grade(SurfaceGrade::Fine),
            SurfaceGrade::Custom,
        );
        assert!((fresh.rq_nm - 6.0).abs() < 1e-9 && (fresh.aniso - 1.0).abs() < 1e-9);
        // A frosted roughness never turns a fresh Custom finish into a ground one.
        let frosted = SurfaceFinish {
            ground_alpha: Some(0.4),
            ..SurfaceFinish::of_grade(SurfaceGrade::Fine)
        };
        assert_eq!(
            finish_with_grade(&frosted, SurfaceGrade::Custom).ground_alpha,
            None
        );
    }

    #[test]
    fn the_controls_set_the_custom_values_the_angle_and_no_sweep() {
        let controls = Controls {
            rq_nm: 20.0,
            aniso: 3.0,
            use_angle: true,
            angle_deg: 45.0,
            no_sweep: true,
        };
        let custom =
            finish_from_controls(&SurfaceFinish::of_grade(SurfaceGrade::Custom), &controls);
        assert!((custom.rq_nm - 20.0).abs() < 1e-6);
        assert!((custom.aniso - 3.0).abs() < 1e-6);
        assert_eq!(custom.lap_deg, Some(45.0));
        assert!(custom.no_sweep);
        // A preset ignores Rq and anisotropy, and a cleared Custom Angle clears the angle.
        let preset = finish_from_controls(
            &SurfaceFinish::of_grade(SurfaceGrade::Fine),
            &Controls {
                use_angle: false,
                ..controls
            },
        );
        assert_eq!(preset.rq_nm.to_bits(), 0.0_f64.to_bits());
        assert_eq!(preset.lap_deg, None);
        assert!(preset.no_sweep);
        // An ideal tier has no scratches to point.
        assert!(finish_from_controls(&SurfaceFinish::IDEAL, &controls).is_ideal());
    }

    #[test]
    fn a_tier_finish_keeps_the_facets_that_differ_and_an_ideal_tier_with_none_is_a_clear() {
        let tier = TierSurface::of_tier(SurfaceFinish::of_grade(SurfaceGrade::Fine))
            .with_facet(24.0, Some(SurfaceFinish::of_grade(SurfaceGrade::Prepolish)))
            .expect("a surface");
        let moved = tier_with_finish(
            Some(&tier),
            SurfaceFinish::of_grade(SurfaceGrade::Commercial),
        )
        .expect("a surface");
        assert_eq!(moved.finish.grade, SurfaceGrade::Commercial);
        assert_eq!(moved.facets, tier.facets);
        assert_eq!(tier_with_finish(None, SurfaceFinish::IDEAL), None);
    }

    #[test]
    fn a_facet_override_is_set_read_and_cleared() {
        let tier = TierSurface::of_tier(SurfaceFinish::of_grade(SurfaceGrade::Fine));
        assert_eq!(facet_option_of(Some(&tier), 24.0), 0);
        // Option 1 + the menu position of Prepolish (4).
        let set = tier_with_facet_option(Some(&tier), 24.0, 5).expect("a surface");
        assert_eq!(facet_option_of(Some(&set), 24.0), 5);
        assert_eq!(set.finish_at(24.0).grade, SurfaceGrade::Prepolish);
        assert_eq!(facet_option_of(Some(&set), 48.0), 0);
        // Follow Tier clears it, and an ideal tier with nothing left is a clear.
        let cleared = tier_with_facet_option(Some(&set), 24.0, 0).expect("the tier stays");
        assert_eq!(cleared.facets.len(), 0);
        let none = tier_with_facet_option(None, 24.0, 0);
        assert_eq!(none, None);
        // An entry out of range is no edit.
        assert_eq!(tier_with_facet_option(Some(&tier), 24.0, 99), None);
    }

    #[test]
    fn the_inspector_state_describes_the_loaded_tier() {
        let mut state = state_with_one_tier(&[0.0, 24.0, 12.5]);
        let none = inspector_state(&state.design, Some(0), 0);
        assert_eq!(none.grade, 0);
        assert!(!none.is_custom && !none.directional);
        assert_eq!(none.summary, "");
        assert_eq!(none.facet_labels, ["0", "24", "12.5"]);
        assert_eq!(none.facet_option, 0);

        let surface = TierSurface::of_tier(SurfaceFinish {
            lap_deg: Some(30.0),
            ..SurfaceFinish::of_grade(SurfaceGrade::Commercial)
        })
        .with_facet(
            24.0,
            Some(SurfaceFinish::of_grade(SurfaceGrade::GroundFine)),
        );
        state
            .apply(Edit::SetTierSurface { index: 0, surface })
            .expect("the surface applies");
        let shown = inspector_state(&state.design, Some(0), 1);
        assert_eq!(shown.grade, 3);
        assert!(shown.directional && !shown.is_custom);
        assert!(shown.controls.use_angle);
        assert!((shown.controls.angle_deg - 30.0).abs() < 1e-6);
        assert_eq!(shown.summary, "Commercial, 1 facet differs");
        assert_eq!(shown.facet_pick, 1);
        // Option 1 + the menu position of Ground (Fine) (5).
        assert_eq!(shown.facet_option, 6);
        // The facet pick is kept inside the tier's facets, and no tier is a quiet default.
        assert_eq!(inspector_state(&state.design, Some(0), 9).facet_pick, 2);
        let nothing = inspector_state(&state.design, None, 3);
        assert_eq!(nothing.facet_pick, 0);
        assert_eq!(nothing.facet_labels.len(), 0);
    }

    #[test]
    fn a_surface_edit_is_one_undo_step() {
        let mut state = state_with_one_tier(&[0.0]);
        let surface = tier_with_finish(None, SurfaceFinish::of_grade(SurfaceGrade::Mirror));
        state
            .apply(Edit::SetTierSurface { index: 0, surface })
            .expect("the surface applies");
        assert!(state.design.tier_surface(0).is_some());
        assert!(state.undo().expect("undo"), "one step undoes it");
        assert!(state.design.tier_surface(0).is_none());
        assert!(state.redo().expect("redo"));
        assert_eq!(
            state.design.tier_surface(0).map(|s| s.finish.grade),
            Some(SurfaceGrade::Mirror)
        );
    }

    #[test]
    fn the_same_surface_again_is_not_an_edit() {
        let surface = tier_with_finish(None, SurfaceFinish::of_grade(SurfaceGrade::Fine));
        let state = state_with_one_tier(&[0.0]);
        assert_eq!(state.design.tier_surface(0), None);
        // `Wiring::apply` compares the normalised surface with the stored one before editing.
        assert_ne!(state.design.tier_surface(0), surface.as_ref());
        assert_eq!(
            tier_with_finish(surface.as_ref(), surface.as_ref().expect("set").finish),
            surface
        );
    }
}
