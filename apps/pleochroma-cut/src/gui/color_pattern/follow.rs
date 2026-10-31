//! "Follows Crystal Axis": a pattern whose zones turn with the stone's Crystal Axis.
//!
//! The pattern is stored with the table-normal frame (`PatternAxis::FollowsCrystalAxis::frame`),
//! so a consumer that knows nothing of the stone's axis draws it the way it always drew a
//! pattern. This module turns the zones to the axis wherever the axis is known
//! (`pleochroma_cut_core::material::pattern::follow_c_axis`: the zones' `+Y` goes onto the axis
//! line). The mode is stored on the zones themselves (`ZonedAbsorption::follows_axis`, set by
//! `ColorPattern::to_zoned`), so no library read and no process-wide note is needed and every
//! surface (the CLI, the web app, the phone) turns the zones the same way.
//!
//! # Where the zones are turned
//!
//! The axis is the resolved material's own `c_axis`, which is the design's setting when it has one,
//! else the custom material's stored default, else the built-in's:
//!
//! * [`reframe_material`]: a material whose axis is already resolved (every stored-zone install,
//!   `rough_colour::store::apply_stored_zoning`, so the material list at startup and after a
//!   save, the width rescale, the Color Pattern window and the catalogue thumbnails, whose
//!   `sized` step calls it on the stone's own material).
//! * [`axis_applied`]: the live render context, whenever the design's axis changes (the Stone tab,
//!   its presets, the editor refresh that loads or undoes an axis, a Color Pattern save). It is the
//!   hook the stone-width change has in `material_zones::stone_width_applied`.
//! * The Current Design preview of the Color Pattern window re-frames the material it renders
//!   (`preview::render_design`) with the same [`pleochroma_cut_core::material::pattern`] function.
//!
//! A pattern that does not follow is never touched, so every old pattern renders as it did.

use crate::bridge::render_thread::RenderContext;
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::material::pattern::{crystal_axis_frame, follow_noted_pattern};
use std::sync::{Arc, Mutex};

/// Turns the zones of every pattern that follows the Crystal Axis, in the render context's
/// material list and in its material override, to the axis the context traces with (the
/// design's override axis, else the material's own). Reads the mode off the zones
/// (`ZonedAbsorption::follows_axis`), so it needs no library and can run wherever the context is
/// already locked: every path that changes the axis, installs a material or saves one. Returns how
/// many materials moved; the context is marked dirty when any did.
pub fn follow_context(ctx: &mut RenderContext) -> usize {
    let override_axis = ctx.c_axis_override;
    let mut moved = 0;
    let mut turn = |material: &mut GemMaterial| {
        let c_axis = override_axis.unwrap_or(material.c_axis);
        let Some(zoning) = material.zoning.as_mut() else {
            return;
        };
        if !zoning.follows_axis {
            return;
        }
        let frame = crystal_axis_frame(c_axis);
        if zoning.frame != frame {
            zoning.frame = frame;
            moved += 1;
        }
    };
    let zoned_follower = ctx.custom_materials.iter().any(|material| {
        material
            .zoning
            .as_ref()
            .is_some_and(|zoning| zoning.follows_axis)
    });
    if zoned_follower {
        Arc::make_mut(&mut ctx.custom_materials)
            .iter_mut()
            .for_each(&mut turn);
    }
    if let Some(material) = ctx.material_override.as_mut() {
        turn(material);
    }
    if moved > 0 {
        ctx.dirty = true;
    }
    moved
}

/// Turns the zones of `material` to its own Crystal Axis when its pattern follows the axis.
/// Returns whether the zones were turned (`false`: a material without zones or without a
/// following pattern, or zones already on the axis).
pub fn reframe_material(material: &mut GemMaterial) -> bool {
    follow_noted_pattern(material)
}

/// The design's Crystal Axis changed (or a material list was rebuilt): turns the zones of every
/// pattern that follows the axis, in the render context's material list and in the override that
/// names one, and restarts the render when anything moved. Returns how many materials moved.
pub fn axis_applied(render_ctx: &Arc<Mutex<RenderContext>>) -> usize {
    let mut ctx = RenderContext::lock(render_ctx);
    follow_context(&mut ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{DVec3, Vec3};
    use pleochroma::optics::zoning::ZoneFrame;
    use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};

    fn zoned_material(name: &str, axis: PatternAxis) -> GemMaterial {
        let mut pattern = template(TemplateId::Bicolor, axis);
        for at in pattern.unsolved_colors() {
            pattern.set_color_bands(at, vec![[520.0, 40.0, 0.3]]);
        }
        let mut material = GemMaterial::diamond();
        material.name = name.to_owned();
        material.zoning = pattern.to_zoned().expect("converts");
        material
    }

    #[test]
    fn a_following_pattern_turns_with_the_materials_axis() {
        let mut material = zoned_material("Fan", PatternAxis::FollowsCrystalAxis);
        // Through the table: the stored frame, nothing to turn.
        assert!(!reframe_material(&mut material));

        material.c_axis = Vec3::X;
        assert!(reframe_material(&mut material));
        let frame = material.zoning.as_ref().expect("zoned").frame;
        assert!((frame.direction(DVec3::Y) - DVec3::X).length() < 1e-9);
        assert!(!reframe_material(&mut material), "already on the axis");
    }

    #[test]
    fn a_fixed_pattern_never_moves() {
        let mut material = zoned_material("Fixed", PatternAxis::ACROSS);
        let before = material.zoning.clone();
        material.c_axis = Vec3::X;
        assert!(!reframe_material(&mut material));
        assert_eq!(material.zoning, before);
    }

    #[test]
    fn the_context_turns_a_following_pattern_to_the_design_axis_without_a_library() {
        let material = zoned_material("Ctx Fan", PatternAxis::FollowsCrystalAxis);
        // The same name and a fixed pattern: the zones' own flag decides, not the name.
        let fixed = zoned_material("Ctx Fan", PatternAxis::ACROSS);

        let mut ctx = RenderContext {
            custom_materials: Arc::new(vec![material, fixed.clone()]),
            dirty: false,
            ..Default::default()
        };
        // No override: the material's own axis (through the table) is the stored frame.
        assert_eq!(follow_context(&mut ctx), 0);
        assert!(!ctx.dirty);

        ctx.c_axis_override = Some(Vec3::X);
        assert_eq!(
            follow_context(&mut ctx),
            1,
            "only the following pattern moves"
        );
        assert!(ctx.dirty);
        let turned = ctx.custom_materials[0]
            .zoning
            .as_ref()
            .expect("zoned")
            .frame;
        assert!((turned.direction(DVec3::Y) - DVec3::X).length() < 1e-9);
        assert_eq!(
            ctx.custom_materials[1], fixed,
            "a fixed pattern is untouched"
        );
        assert_eq!(follow_context(&mut ctx), 0, "already on the axis");

        // The override goes away: the zones go back to the material's own axis.
        ctx.c_axis_override = None;
        assert_eq!(follow_context(&mut ctx), 1);
        let back = ctx.custom_materials[0]
            .zoning
            .as_ref()
            .expect("zoned")
            .frame;
        assert_eq!(back, ZoneFrame::IDENTITY);
    }

    #[test]
    fn the_stored_zones_of_a_following_pattern_have_the_table_normal_frame() {
        let material = zoned_material("Fan", PatternAxis::FollowsCrystalAxis);
        let zoned = material.zoning.expect("has zones");
        assert_eq!(zoned.frame, ZoneFrame::IDENTITY);
        assert!(zoned.follows_axis, "the mode travels on the zones");
    }
}
