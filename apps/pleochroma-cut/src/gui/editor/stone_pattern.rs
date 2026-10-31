//! The Stone tab's per-design and preview-only color pattern actions.
//!
//! The Color popup and the Stone tab also offer the typical zoning of the stone's species
//! ([`typical`]), applied through the same guarded [`Port::apply`] as the Color Pattern editor.

mod typical;

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
};

use pleochroma::optics::materials::{
    AbsorptionUnit, CrystalSystem, GemMaterial,
    variants::{ColorVariant, TensorRank, host_rank},
};
use pleochroma_cut_core::{
    CrystalAxis, MaterialSelection,
    material::pattern::{ColorPattern, OwnColor, VARIANT_SEED_LCH, layers_across_axis},
};
use pleochroma_editor::material_lookup::{EditorMaterialLookup, traced_gem_material_at_width};
use slint::ComponentHandle;

use super::{render_color::MaterialEdit, state::EditorState};
use crate::{
    MainWindow, ViewportModel, Zoning,
    bridge::render_thread::{ColorTarget, PlanesOwner, RenderContext},
    gui::{
        color_pattern::{
            StonePatternOpening, open_stone_pattern, own_color, seed_stone_pattern, zone_variants,
        },
        rough_colour::material_zones::registered_vault,
        show_toast,
    },
};

#[derive(Clone)]
struct DesignTarget {
    epoch: u64,
    generation: u64,
    material: MaterialSelection,
}

#[derive(Clone)]
struct Target {
    owner: PlanesOwner,
    material: GemMaterial,
    rendered: GemMaterial,
    /// The custom materials the design resolves its material in.
    custom: Arc<Vec<GemMaterial>>,
    width: f32,
    design: Option<DesignTarget>,
    pattern: Option<ColorPattern>,
}

impl Target {
    /// The stone's material as it looks without a pattern: what a typical pattern takes its
    /// color from. (`material` and `rendered` carry the pattern the stone has, if any.) A design
    /// resolves its selection with the pattern dropped; a preview that has a pattern shows the
    /// untinted source; otherwise the stone shows its own color (a view tint included).
    fn own_material(&self) -> GemMaterial {
        match &self.design {
            Some(design) => traced_gem_material_at_width(
                &self.material.name,
                &design.material.clone().with_color_pattern(None),
                &EditorMaterialLookup::new(&self.custom),
                effective_width(self.width),
            )
            .unwrap_or_else(|| self.material.clone()),
            None if self.pattern.is_some() => self.material.clone(),
            None => self.rendered.clone(),
        }
    }
}

/// The stone width a pattern is resolved at, millimeters (the default for an unset width).
fn effective_width(width: f32) -> f64 {
    if width > 0.0 {
        f64::from(width)
    } else {
        f64::from(pleochroma::render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM)
    }
}

/// Camera movement is harmless; changing the stone or its material invalidates an edit.
fn same_target(before: &Target, now: &Target) -> bool {
    if before.width.to_bits() != now.width.to_bits() {
        return false;
    }
    match (&before.design, &now.design) {
        (Some(a), Some(b)) => {
            a.epoch == b.epoch
                && a.generation == b.generation
                && a.material == b.material
                && before.material.name == now.material.name
        }
        (None, None) => {
            before.owner == now.owner
                && before.material.name == now.material.name
                && before.pattern == now.pattern
        }
        _ => false,
    }
}

#[derive(Clone)]
struct Port {
    state: Rc<RefCell<EditorState>>,
    ctx: Arc<Mutex<RenderContext>>,
    edit: MaterialEdit,
}

impl Port {
    fn capture(&self) -> Result<Target, String> {
        let (target, owner, material, rendered, width, pattern, custom) = {
            let ctx = RenderContext::lock(&self.ctx);
            if ctx.material_unresolved.is_some() {
                return Err(
                    "Resolve the stone's missing material before editing its pattern.".into(),
                );
            }
            if ctx.physics_color() {
                return Err("This material's physics recipe determines its color.".into());
            }
            if ctx.active_planes.is_empty() {
                return Err("Select or open a stone first.".into());
            }
            let material = ctx
                .base_pattern_material()
                .ok_or("The stone's material could not be resolved.")?;
            (
                ctx.color_target(),
                ctx.planes_owner,
                material.clone(),
                ctx.tinted_material_override().unwrap_or(material),
                ctx.stone_width_mm,
                ctx.current_view_color_pattern().cloned(),
                Arc::clone(&ctx.custom_materials),
            )
        };
        let design = if target == ColorTarget::Design {
            let st = self
                .state
                .try_borrow()
                .map_err(|_| "The design is busy. Try again in a moment.")?;
            if !st.has_design {
                return Err("Open a design first.".into());
            }
            Some(DesignTarget {
                epoch: st.design_epoch.load(Ordering::SeqCst),
                generation: st.generation.load(Ordering::SeqCst),
                material: st.design.material.clone(),
            })
        } else {
            None
        };
        let pattern = design
            .as_ref()
            .map_or(pattern, |d| d.material.color_pattern_override.clone());
        Ok(Target {
            owner,
            material,
            rendered,
            custom,
            width,
            design,
            pattern,
        })
    }

    fn apply(
        &self,
        expected: &RefCell<Target>,
        pattern: Option<ColorPattern>,
    ) -> Result<(), String> {
        let now = self.capture()?;
        if !same_target(&expected.borrow(), &now) {
            return Err(
                "The stone or its material changed. Reopen Color Pattern for the current stone."
                    .into(),
            );
        }
        if !RenderContext::lock(&self.ctx).matches_pattern_material(&expected.borrow().material) {
            return Err("The material changed. Reopen Color Pattern for the current stone.".into());
        }
        if let Some(pattern) = &pattern {
            pleochroma_cut_core::material::pattern::apply_color_pattern(
                now.material.clone(),
                pattern,
                f64::from(now.width),
            )
            .map_err(|error| error.to_string())?;
        }
        if let Some(design) = &now.design {
            let next = design.material.clone().with_color_pattern(pattern);
            (self.edit)(&|current| (current != &next).then(|| next.clone()))?;
        } else {
            RenderContext::lock(&self.ctx).set_view_color_pattern(pattern)?;
        }
        // Repeated Apply in the same window is allowed, but an unrelated edit is not.
        *expected.borrow_mut() = self.capture()?;
        Ok(())
    }

    fn present(&self, ui: &MainWindow) {
        let zoning = ui.global::<Zoning>();
        match self.capture() {
            Ok(target) => {
                zoning.set_stone_pattern_enabled(true);
                zoning.set_stone_pattern_overridden(target.pattern.is_some());
                zoning.set_stone_pattern_hint(if target.design.is_some() {
                    "Color zones for this stone. Saved with the design; Undo restores the previous pattern."
                } else {
                    "Preview only. Open the design in Edit before editing zones to save them with the design."
                }.into());
                typical::push(&zoning, &typical::state_of(&target));
                // The Stone tab compares the axis tilt itself (the design's, else this one), so the
                // hint follows the axis controls without another refresh.
                zoning.set_pattern_layers_across_axis(
                    target.pattern.as_ref().is_some_and(layers_across_axis),
                );
                zoning.set_material_axis_tilt_deg(crate::gui::render::inclusion_effects::narrow(
                    CrystalAxis::from_direction(target.material.c_axis.as_dvec3()).tilt_deg(),
                ));
            }
            Err(message) => {
                zoning.set_stone_pattern_enabled(false);
                zoning.set_stone_pattern_overridden(false);
                zoning.set_stone_pattern_hint(message.into());
                typical::push(&zoning, &typical::TypicalState::default());
                zoning.set_pattern_layers_across_axis(false);
            }
        }
    }

    /// The pattern `material` would have as a Uniform seed, for the stone `width` millimeters
    /// wide. A model-unit color needs the stone's measured width.
    fn seed(&self, material: &GemMaterial, width: f64) -> Result<ColorPattern, String> {
        let model_width =
            if material.absorption_unit == AbsorptionUnit::ModelUnit && material.zoning.is_none() {
                let planes = Arc::clone(&RenderContext::lock(&self.ctx).active_planes);
                pleochroma::render_setup::measure_model_width(&planes)
            } else {
                None
            };
        seed_stone_pattern(material, width, model_width)
    }

    /// The color the stone has without a pattern, per direction: what the typical patterns are
    /// built on (`color_pattern::own_color`). `variants` are the color variants of the stone's
    /// material. The stone's color is read from the stone as it looks without a pattern; a stone
    /// without pleochroism needs its seed (an error when it cannot be read), a pleochroic one
    /// reads its tensor.
    fn own_color(
        &self,
        target: &Target,
        width: f64,
        variants: &[ColorVariant],
    ) -> Result<OwnColor, String> {
        let own_material = target.own_material();
        let seed_lch = match self.seed(&own_material, width) {
            Ok(seed) => seed.base_lch(),
            Err(error) if host_rank(&own_material) == TensorRank::Isotropic => return Err(error),
            Err(_) => VARIANT_SEED_LCH,
        };
        let choice = target
            .design
            .as_ref()
            .and_then(|design| design.material.color_variant_override.as_ref());
        Ok(own_color(&own_material, choice, variants, seed_lch, width))
    }

    fn open(&self, ui: &MainWindow) -> Result<(), String> {
        let target = self.capture()?;
        let width = effective_width(target.width);
        let pattern = if let Some(pattern) = &target.pattern {
            pattern.clone()
        } else {
            self.seed(&target.rendered, width)?
        };
        // What the editor's typical patterns are built on; a stone whose color cannot be read
        // back opens the editor without them being offered on the right color. The saved colors
        // of the material come from the main window's vault.
        let vault = registered_vault();
        let variants = zone_variants(&target.material, vault.as_deref());
        let own = self.own_color(&target, width, &variants).ok();
        let port = self.clone();
        let expected = RefCell::new(target.clone());
        let weak = ui.as_weak();
        open_stone_pattern(
            ui,
            StonePatternOpening {
                label: format!(
                    "{} — {}",
                    target.material.name,
                    if target.design.is_some() {
                        "This Stone"
                    } else {
                        "Preview"
                    }
                ),
                pattern,
                stone_width_mm: width,
                two_colors_allowed: target.material.crystal_system != CrystalSystem::Cubic
                    && target.material.birefringence_delta.abs() > f32::EPSILON,
                own,
                base_material: target.material,
                targets_design: target.design.is_some(),
                apply: Rc::new(move |pattern| {
                    port.apply(&expected, Some(pattern.clone()))?;
                    if let Some(ui) = weak.upgrade() {
                        ui.global::<ViewportModel>().invoke_render_color_refresh();
                        port.present(&ui);
                    }
                    Ok(())
                }),
            },
        );
        Ok(())
    }
}

/// Registers independent stone actions; the custom-material editor keeps its own callbacks.
pub(super) fn setup(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    ctx: &Arc<Mutex<RenderContext>>,
    edit: MaterialEdit,
) {
    let port = Port {
        state: Rc::clone(state),
        ctx: Arc::clone(ctx),
        edit,
    };
    let (weak, opening) = (ui.as_weak(), port.clone());
    ui.global::<Zoning>().on_edit_stone_pattern(move || {
        if let Some(ui) = weak.upgrade()
            && let Err(message) = opening.open(&ui)
        {
            show_toast(&ui, &message, "warning");
        }
    });
    let (weak, reset) = (ui.as_weak(), port.clone());
    ui.global::<Zoning>().on_reset_stone_pattern(move || {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        let result = reset
            .capture()
            .and_then(|target| reset.apply(&RefCell::new(target), None));
        if let Err(message) = result {
            show_toast(&ui, &message, "warning");
        }
        ui.global::<ViewportModel>().invoke_render_color_refresh();
        reset.present(&ui);
    });
    let (weak, typical) = (ui.as_weak(), port.clone());
    ui.global::<Zoning>()
        .on_apply_typical_pattern(move |index| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(index) else {
                return;
            };
            if let Err(message) = typical.apply_typical(&ui, index) {
                show_toast(&ui, &message, "warning");
            }
        });
    let weak = ui.as_weak();
    ui.global::<Zoning>().on_refresh_stone_pattern(move || {
        if let Some(ui) = weak.upgrade() {
            port.present(&ui);
        }
    });
    ui.global::<Zoning>().invoke_refresh_stone_pattern();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preview(entry_id: i64) -> Target {
        let material = GemMaterial::ametrine();
        Target {
            owner: PlanesOwner::Catalogue { entry_id },
            rendered: material.clone(),
            material,
            custom: Arc::default(),
            width: 7.0,
            design: None,
            pattern: None,
        }
    }

    #[test]
    fn a_preview_edit_cannot_land_on_another_stone_or_size() {
        let before = preview(12);
        assert!(same_target(&before, &before));
        assert!(!same_target(&before, &preview(13)));
        let mut resized = before.clone();
        resized.width = 12.0;
        assert!(!same_target(&before, &resized));
    }

    #[test]
    fn replacing_or_editing_a_design_invalidates_its_pending_pattern() {
        let mut before = preview(12);
        before.owner = PlanesOwner::Editor { generation: 10 };
        before.design = Some(DesignTarget {
            epoch: 2,
            generation: 10,
            material: MaterialSelection::none(),
        });
        let mut now = before.clone();
        // A delayed preview frame can change its owner stamp without changing the design.
        now.owner = PlanesOwner::Editor { generation: 9 };
        assert!(same_target(&before, &now));
        now.design.as_mut().unwrap().epoch = 3;
        assert!(!same_target(&before, &now));
        now.design.as_mut().unwrap().epoch = 2;
        now.design.as_mut().unwrap().generation = 11;
        assert!(!same_target(&before, &now));
        assert!(!same_target(&before, &preview(12)));
    }
}
