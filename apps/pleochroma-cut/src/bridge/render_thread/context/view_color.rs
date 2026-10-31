//! The Live Render toolbar's quick colour change, as the render pipeline sees it.
//!
//! A cutter can recolour the rendered stone from the toolbar without making a custom
//! material. When the view shows the editor's open design with the render material linked
//! to it, the pick is the design's own colour override and already travels in
//! `RenderContext::material_override` (see `gui::editor::view::sync_viewport_material_link`).
//! In every other case (a library design, or the render material unlinked) the pick is
//! only a view setting: [`RenderContext::view_body_color`]. It is never written to a design.
//!
//! The view color is a color variant of the viewed material (a picked variant or a per-axis Custom
//! Color, [`ViewVariant`]) or, for the colors saved before variants, the legacy triple and
//! seven-band rows. A variant is applied with `GemMaterial::with_color_variant_tensor`, so a
//! pleochroic stone keeps its pleochroism; the legacy colors are single colors and flatten it
//! exactly as they always did.
//!
//! [`RenderContext::tinted_material_override`] is the ONE hook that turns the view setting
//! into the material every consumer renders: the render loop's frame snapshot, the export
//! and tilt-video scene snapshot (which the remote workers are sent), and the tilt
//! analysis. The decisions are plain functions ([`color_target`], [`effective_view_color`])
//! so they are tested without a window.

use super::{PlanesOwner, RenderContext, resolve_material, resolve_material_with_override};
use pleochroma::optics::{
    absorption::BandShape,
    materials::{
        AbsorptionUnit, GemMaterial,
        variants::{ColorVariant, VariantProvenance},
    },
};
use pleochroma_cut_core::{
    MaterialSelection,
    material::{CUSTOM_VARIANT_KEY, VariantChoice},
};

/// Who a colour pick in the Live Render toolbar changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTarget {
    /// The open design's own colour override: an undoable edit, like the Edit tab's
    /// colour box.
    Design,
    /// Only the colour the view renders; the design and the catalogue are not touched.
    View,
}

/// Where a colour pick goes: into the design only while the view shows the editor's
/// open design (`owner`) with the render material linked to it (`linked`).
#[must_use]
pub const fn color_target(linked: bool, owner: PlanesOwner) -> ColorTarget {
    if linked && matches!(owner, PlanesOwner::Editor { .. }) {
        ColorTarget::Design
    } else {
        ColorTarget::View
    }
}

/// A color variant as the view holds it: the snapshot of the tensor (a picked variant, or a
/// Custom Color authored per axis) and the name of the material it was made for.
///
/// The snapshot is what renders, so the view colors the stone the same without the variant
/// library, after a restart and on a remote worker.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewVariant {
    /// The material the variant belongs to (the host's name when it was picked). A Custom
    /// Color ([`CUSTOM_VARIANT_KEY`]) belongs to no material: it fits any host of its rank.
    pub material: String,
    /// The snapshot: key, label, per-axis tensor, unit and reference path.
    pub choice: VariantChoice,
}

impl ViewVariant {
    /// The variant `choice` of `material`.
    #[must_use]
    pub fn new(material: impl Into<String>, choice: VariantChoice) -> Self {
        Self {
            material: material.into(),
            choice,
        }
    }

    /// Whether this variant can color `host`: it is a Custom Color or was made for the same
    /// material (ASCII case ignored), and `ColorVariant::validate_for` accepts its tensor for
    /// the host (a pleochroic host never receives an isotropic color, an isotropic one never a
    /// pleochroic color). A variant that does not fit is dormant: the host keeps its own color.
    #[must_use]
    pub fn fits(&self, host: &GemMaterial) -> bool {
        let belongs =
            self.choice.key == CUSTOM_VARIANT_KEY || self.material.eq_ignore_ascii_case(&host.name);
        belongs
            && ColorVariant {
                key: self.choice.key.clone(),
                label: self.choice.label.clone(),
                material: self.material.clone(),
                absorption: self.choice.absorption.clone(),
                unit: self.choice.unit,
                reference_path_mm: self.choice.reference_path_mm,
                provenance: VariantProvenance::User,
            }
            .validate_for(host)
            .is_ok()
    }

    /// Everything that decides what the variant renders, as bit patterns, for the scene identity
    /// (a floating point `NaN` can then never make a key unequal to itself): the unit, the
    /// reference path, every band of every ray, the material and whether it is a Custom Color
    /// (both decide [`Self::fits`]). The label and key text are not part of it.
    #[must_use]
    pub fn identity_words(&self) -> Vec<u32> {
        let tensor = &self.choice.absorption;
        let mut words = vec![
            match self.choice.unit {
                AbsorptionUnit::ModelUnit => 0,
                AbsorptionUnit::PerMm => 1,
            },
            self.choice.reference_path_mm.to_bits(),
            u32::from(tensor.is_pleochroic),
            u32::from(self.choice.key == CUSTOM_VARIANT_KEY),
        ];
        words.extend(self.material.bytes().map(u32::from));
        let beta = tensor.beta_ray.as_deref();
        for (marker, set) in [
            (1_u32, Some(tensor.o_ray.as_slice())),
            (2, Some(tensor.e_ray.as_slice())),
            (3, beta),
        ] {
            let Some(set) = set else {
                continue;
            };
            words.push(marker);
            words.push(u32::try_from(set.len()).unwrap_or(u32::MAX));
            for band in set {
                words.extend([
                    band.center_nm.to_bits(),
                    band.width_nm.to_bits(),
                    band.peak.to_bits(),
                    match band.shape {
                        BandShape::GaussianWavelength => 0,
                        BandShape::GaussianEnergy => 1,
                    },
                ]);
            }
        }
        words
    }
}

/// The Live Render toolbar's view-only color. A color variant ([`Self::variant`]: a picked
/// variant or a per-axis Custom Color) keeps every direction of a pleochroic stone and wins
/// over the legacy fields; the legacy triple every consumer can show, and the seven-band rows
/// when the color came from the old L*C*h editor (they win over the triple), stay readable for
/// the colors saved before variants and keep rendering as saved.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewColor {
    /// The absorption triple `GemMaterial::with_body_color` takes. `[0.0; 3]` and unused while
    /// [`Self::variant`] is set.
    pub triple: [f32; 3],
    /// The L*C*h editor's band rows `[centre_nm, width_nm, amplitude_per_mm]`; `None` for a
    /// color that is only the triple (a preset or the quick picker).
    pub bands: Option<Vec<[f32; 3]>>,
    /// The color variant, `None` for a legacy color.
    pub variant: Option<ViewVariant>,
}

impl ViewColor {
    /// A color that is only the legacy triple (no band rows, no variant).
    #[must_use]
    pub const fn triple_only(triple: [f32; 3]) -> Self {
        Self {
            triple,
            bands: None,
            variant: None,
        }
    }

    /// A color variant of `material` (see [`ViewVariant`]); the legacy fields stay empty.
    #[must_use]
    pub fn from_variant(material: impl Into<String>, choice: VariantChoice) -> Self {
        Self {
            triple: [0.0; 3],
            bands: None,
            variant: Some(ViewVariant::new(material, choice)),
        }
    }

    /// This color as the color fields of a design's [`MaterialSelection`] (a variant snapshot,
    /// else the triple with its band rows), so the shared color list of the editor
    /// (`pleochroma_editor::material`) can say which of its entries shows it.
    #[must_use]
    pub fn as_selection(&self) -> MaterialSelection {
        if let Some(variant) = &self.variant {
            return MaterialSelection::none().with_color_variant(Some(variant.choice.clone()));
        }
        self.band_rows().map_or_else(
            || MaterialSelection::none().with_body_color(Some(self.triple)),
            |rows| {
                MaterialSelection::none().with_body_color_bands(
                    Some(self.triple),
                    Some(rows.to_vec()),
                    None,
                )
            },
        )
    }

    /// The band rows when there are any to render (`None` and an empty list both mean the
    /// triple alone).
    #[must_use]
    pub fn band_rows(&self) -> Option<&[[f32; 3]]> {
        self.bands.as_deref().filter(|rows| !rows.is_empty())
    }
}

/// The view colour that is in force: `view` only when picks go to the view
/// ([`ColorTarget::View`]) and the traced material does not define its own colour
/// (`physics`). A design's own override is never a view colour, and a material with a
/// physics recipe is never recoloured.
#[must_use]
pub fn effective_view_color<T>(view: Option<T>, target: ColorTarget, physics: bool) -> Option<T> {
    match (target, physics) {
        (ColorTarget::View, false) => view,
        _ => None,
    }
}

/// The built-in material table, built once: the hook below runs every frame while a
/// view color is in force.
fn builtin_materials() -> &'static [GemMaterial] {
    GemMaterial::built_in()
}

impl RenderContext {
    /// Who a colour pick changes right now: see [`color_target`].
    #[must_use]
    pub const fn color_target(&self) -> ColorTarget {
        color_target(self.material_linked, self.planes_owner)
    }

    /// The view-only body colour that applies to the stone being rendered, if any: see
    /// [`effective_view_color`]. Part of the scene identity, so a change restarts the
    /// accumulation (local and remote).
    #[must_use]
    pub fn effective_view_body_color(&self) -> Option<ViewColor> {
        // Asked every frame: nothing else is computed while the setting is off.
        let view = self.view_body_color.as_ref()?;
        if self.current_view_color_pattern().is_some() {
            return None;
        }
        effective_view_color(
            Some(view.clone()),
            self.color_target(),
            self.physics_color() || self.zoned_material(),
        )
    }

    /// Whether the material being traced (the override when set, else the by-name lookup) has
    /// colour zones. Its colour is its zones, so no view colour applies to it (like a physics
    /// recipe, see [`effective_view_color`]).
    #[must_use]
    pub fn zoned_material(&self) -> bool {
        if let Some(pattern) = self.current_view_color_pattern() {
            return !pattern.zones.is_empty();
        }
        resolve_material_with_override(
            builtin_materials(),
            &self.custom_materials,
            self.material_override.as_ref(),
            &self.material_name,
        )
        .is_some_and(|material| material.zoning.is_some())
    }

    /// The face-up colors of the zones of the material being traced (the base zone first, each a
    /// two-color zone's unpolarised blend), as sRGB in `[0, 1]`; empty for a material without
    /// zones. The Material chip draws them as a strip.
    #[must_use]
    pub fn zone_swatch_colors(&self) -> Vec<[f32; 3]> {
        self.patterned_view_material()
            .or_else(|| {
                resolve_material_with_override(
                    builtin_materials(),
                    &self.custom_materials,
                    self.material_override.as_ref(),
                    &self.material_name,
                )
            })
            .map_or_else(Vec::new, |material| {
                // The colors a viewer sees through the table: the stone's own Crystal Axis and
                // Second Axis decide which of a two-color zone's colors that is.
                let material = pleochroma_editor::stone_orientation::with_orientation(
                    material,
                    self.c_axis_override
                        .map(|gamma| (gamma, self.alpha_axis_override)),
                );
                pleochroma::optics::materials::zone_swatches_face_up(&material, self.stone_width_mm)
            })
    }

    /// [`Self::material_override`] with the view colour applied: the material every
    /// render of this context uses in place of the raw override.
    ///
    /// With no view colour in force this is exactly `material_override.clone()`, so a
    /// scene without the setting renders bit-identically to one made before it existed.
    /// With one, the material that would have been rendered (the override, or the
    /// by-name lookup) is recoloured the way a design's own colour override recolours
    /// it (`GemMaterial::with_body_color`): same species, optics and name, isotropic
    /// absorption of the chosen colour. A color that carries the L*C*h editor's band rows
    /// takes them instead (`GemMaterial::with_body_color_bands`, per millimetre like a
    /// design's own bands; the render setup then sizes the material from the stone width).
    /// A name that resolves to nothing stays unresolved (`None` with no override), exactly
    /// as before.
    /// A scoped preview pattern takes precedence over the view-only body color.
    #[must_use]
    pub fn tinted_material_override(&self) -> Option<GemMaterial> {
        if let Some(material) = self.patterned_view_material() {
            return Some(material);
        }
        let Some(color) = self.effective_view_body_color() else {
            return self.material_override.clone();
        };
        resolve_material_with_override(
            builtin_materials(),
            &self.custom_materials,
            self.material_override.as_ref(),
            &self.material_name,
        )
        .map(|material| recolored(material, &color))
        .or_else(|| self.material_override.clone())
    }

    /// The material whose colors the toolbar offers: the traced material (the override's name,
    /// else `material_name`) WITHOUT any color applied, so its rank, its own tensor and its
    /// variants are the host's. `None` when the name resolves to nothing.
    #[must_use]
    pub fn color_host(&self) -> Option<GemMaterial> {
        let name = self
            .material_override
            .as_ref()
            .map_or(self.material_name.as_str(), |material| {
                material.name.as_str()
            });
        resolve_material(builtin_materials(), &self.custom_materials, name)
    }
}

/// `material` with the view color `color` applied.
///
/// A color variant replaces ONLY the absorption tensor and its unit
/// (`GemMaterial::with_color_variant_tensor`), so a pleochroic stone keeps every direction; a
/// variant that does not fit the material ([`ViewVariant::fits`]) leaves the material's own color.
/// A legacy color is applied as it always was: the band rows (per millimetre, the material's path
/// scale kept) when it has them, else the triple.
fn recolored(material: GemMaterial, color: &ViewColor) -> GemMaterial {
    if let Some(variant) = &color.variant {
        return if variant.fits(&material) {
            material
                .with_color_variant_tensor(variant.choice.absorption.clone(), variant.choice.unit)
        } else {
            material
        };
    }
    match color.band_rows() {
        Some(rows) => {
            let scale = material.absorption_path_scale;
            material.with_body_color_bands(rows, scale)
        }
        None => material.with_body_color(color.triple),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::materials::body_color::BODY_COLOR_PRESETS;
    use std::sync::Arc;

    const YELLOW: [f32; 3] = BODY_COLOR_PRESETS[5].absorption_rgb;
    const EDITOR: PlanesOwner = PlanesOwner::Editor { generation: 3 };
    const LIBRARY: PlanesOwner = PlanesOwner::Catalogue { entry_id: 7 };

    fn view_ctx(name: &str, colour: Option<[f32; 3]>) -> RenderContext {
        RenderContext {
            material_name: name.to_string(),
            view_body_color: colour.map(ViewColor::triple_only),
            material_linked: false,
            ..Default::default()
        }
    }

    /// The seven-band rows an L*C*h solve produces: `[centre_nm, width_nm, amplitude_per_mm]`.
    fn band_rows() -> Vec<[f32; 3]> {
        vec![[460.0, 45.0, 0.25], [580.0, 60.0, 0.1]]
    }

    #[test]
    fn a_banded_view_color_recolours_with_bands_and_per_mm() {
        use pleochroma::optics::materials::AbsorptionUnit;

        let rows = band_rows();
        let mut ctx = view_ctx("Sapphire", None);
        ctx.view_body_color = Some(ViewColor {
            triple: YELLOW,
            bands: Some(rows.clone()),
            variant: None,
        });
        let tinted = ctx.tinted_material_override().expect("Sapphire resolves");
        let base = GemMaterial::sapphire();
        let expected =
            GemMaterial::sapphire().with_body_color_bands(&rows, base.absorption_path_scale);
        assert_eq!(tinted.name, "Sapphire", "the species name must not change");
        assert_eq!(tinted.dispersion, base.dispersion);
        assert_eq!(tinted.absorption, expected.absorption);
        assert_eq!(tinted.absorption_unit, AbsorptionUnit::PerMm);
        assert_ne!(
            tinted.absorption,
            GemMaterial::sapphire().with_body_color(YELLOW).absorption,
            "the bands win over the triple"
        );

        // Bands that are empty fall back to the triple (a colourless solve).
        ctx.view_body_color = Some(ViewColor {
            triple: YELLOW,
            bands: Some(Vec::new()),
            variant: None,
        });
        let triple_only = ctx.tinted_material_override().expect("Sapphire resolves");
        assert_eq!(triple_only.absorption_unit, AbsorptionUnit::ModelUnit);
        assert_eq!(
            triple_only.absorption,
            GemMaterial::sapphire().with_body_color(YELLOW).absorption
        );
    }

    #[test]
    fn a_pick_goes_to_the_design_only_while_the_view_shows_the_linked_open_design() {
        assert_eq!(color_target(true, EDITOR), ColorTarget::Design);
        assert_eq!(color_target(false, EDITOR), ColorTarget::View);
        assert_eq!(color_target(true, LIBRARY), ColorTarget::View);
        assert_eq!(color_target(false, LIBRARY), ColorTarget::View);
        assert_eq!(color_target(true, PlanesOwner::Builtin), ColorTarget::View);
        assert_eq!(color_target(false, PlanesOwner::Builtin), ColorTarget::View);
    }

    #[test]
    fn the_view_colour_applies_only_to_the_view_and_never_to_a_physics_material() {
        assert_eq!(
            effective_view_color(Some(YELLOW), ColorTarget::View, false),
            Some(YELLOW)
        );
        assert_eq!(
            effective_view_color(Some(YELLOW), ColorTarget::Design, false),
            None,
            "a design colour is the design's own override, never a view colour on top"
        );
        assert_eq!(
            effective_view_color(Some(YELLOW), ColorTarget::View, true),
            None,
            "a material that defines its own colour is not recoloured"
        );
        assert_eq!(
            effective_view_color(None::<[f32; 3]>, ColorTarget::View, false),
            None
        );
    }

    #[test]
    fn without_a_view_colour_the_override_is_passed_through_untouched() {
        let mut ctx = view_ctx("Sapphire", None);
        assert_eq!(ctx.tinted_material_override(), None);
        ctx.material_override = Some(GemMaterial::diamond());
        assert_eq!(ctx.tinted_material_override(), Some(GemMaterial::diamond()));
    }

    #[test]
    fn a_view_colour_recolours_the_named_material_and_keeps_its_optics() {
        let tinted = view_ctx("Sapphire", Some(YELLOW))
            .tinted_material_override()
            .expect("Sapphire resolves");
        let base = GemMaterial::sapphire();
        let expected = GemMaterial::sapphire().with_body_color(YELLOW);
        assert_eq!(tinted.name, "Sapphire", "the species name must not change");
        assert_eq!(tinted.dispersion, base.dispersion);
        assert_eq!(tinted.absorption, expected.absorption);
        assert_ne!(
            tinted.absorption, base.absorption,
            "test premise: the yellow variant differs from sapphire's own colour"
        );
    }

    #[test]
    fn a_view_colour_recolours_a_custom_material_found_by_name() {
        let mut custom = GemMaterial::diamond();
        custom.name = "My Garnet".to_string();
        let mut ctx = view_ctx("my garnet", Some(YELLOW));
        ctx.custom_materials = Arc::new(vec![custom.clone()]);
        let tinted = ctx.tinted_material_override().expect("custom resolves");
        assert_eq!(tinted.name, "My Garnet");
        assert_eq!(tinted.absorption, custom.with_body_color(YELLOW).absorption);
    }

    #[test]
    fn a_view_colour_goes_on_top_of_an_override_material() {
        let mut ctx = view_ctx("Diamond", Some(YELLOW));
        ctx.material_override = Some(GemMaterial::sapphire());
        let tinted = ctx.tinted_material_override().expect("override resolves");
        assert_eq!(tinted.name, "Sapphire", "the override decides the species");
        assert_eq!(
            tinted.absorption,
            GemMaterial::sapphire().with_body_color(YELLOW).absorption
        );
    }

    #[test]
    fn the_linked_open_design_keeps_its_own_colour_and_ignores_the_view_setting() {
        let design_material =
            GemMaterial::sapphire().with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb);
        let mut ctx = view_ctx("Sapphire", Some(YELLOW));
        ctx.material_override = Some(design_material.clone());
        ctx.planes_owner = EDITOR;
        ctx.material_linked = true;
        assert_eq!(ctx.effective_view_body_color(), None);
        assert_eq!(ctx.tinted_material_override(), Some(design_material));

        // Unlinking turns the view colour back on, on top of the dropdown material.
        ctx.material_linked = false;
        ctx.material_override = None;
        assert_eq!(
            ctx.effective_view_body_color(),
            Some(ViewColor::triple_only(YELLOW))
        );
        // A library design takes the view colour even while the link toggle is on.
        ctx.material_linked = true;
        ctx.planes_owner = LIBRARY;
        assert_eq!(
            ctx.effective_view_body_color(),
            Some(ViewColor::triple_only(YELLOW))
        );
    }

    #[test]
    fn a_physics_material_is_not_recoloured() {
        let mut custom = GemMaterial::diamond();
        custom.name = "Physics Ruby".to_string();
        let mut ctx = view_ctx("Physics Ruby", Some(YELLOW));
        ctx.custom_materials = Arc::new(vec![custom]);
        ctx.set_custom_material_physics("Physics Ruby", true);
        assert_eq!(ctx.effective_view_body_color(), None);
        assert_eq!(ctx.tinted_material_override(), None);
    }

    /// A zoned material's colour is its zones: a view colour must not recolour it (that would
    /// swap its `PerMm` zones for a `ModelUnit` triple and change the zone path scale).
    #[test]
    fn a_zoned_material_is_not_recoloured() {
        use pleochroma::optics::{
            absorption::AbsorptionTensor,
            zoning::{ZoneAbsorption, ZonedAbsorption},
        };
        let zoning = ZonedAbsorption::new(ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(
            Vec::new(),
        )));
        let mut custom = GemMaterial::diamond().with_zoning(zoning);
        custom.name = "Zoned Rough".to_string();
        let mut ctx = view_ctx("zoned rough", Some(YELLOW));
        ctx.custom_materials = Arc::new(vec![custom]);
        assert!(ctx.zoned_material());
        assert_eq!(ctx.effective_view_body_color(), None);
        assert_eq!(ctx.tinted_material_override(), None);
        assert!(!view_ctx("Sapphire", Some(YELLOW)).zoned_material());
    }

    #[test]
    fn an_unresolved_name_stays_unresolved_instead_of_turning_into_something_else() {
        let ctx = view_ctx("No Such Stone", Some(YELLOW));
        assert_eq!(ctx.tinted_material_override(), None);
    }

    #[test]
    fn changing_the_effective_view_colour_restarts_the_scene_but_a_dormant_one_does_not() {
        let mut ctx = view_ctx("Sapphire", None);
        let first = ctx.scene_generation();
        ctx.view_body_color = Some(ViewColor::triple_only(YELLOW));
        let second = ctx.scene_generation();
        assert!(
            second > first,
            "a new view colour must restart accumulation"
        );
        assert_eq!(ctx.scene_generation(), second, "nothing changed since");

        // The same setting held by a design-driven view is dormant: not a scene change.
        ctx.planes_owner = EDITOR;
        ctx.material_linked = true;
        let third = ctx.scene_generation();
        assert!(
            third > second,
            "the colour stopped applying, the scene changed"
        );
        ctx.view_body_color = Some(ViewColor::triple_only(BODY_COLOR_PRESETS[2].absorption_rgb));
        assert_eq!(
            ctx.scene_generation(),
            third,
            "a view colour that does not apply must not restart the render"
        );
    }

    // --- Color variants as the view color ---

    use pleochroma::optics::absorption::{AbsorptionBand, AbsorptionTensor};

    /// A pleochroic (distinct o and e sets) uniaxial choice under `key`.
    fn pleochroic_choice(key: &str) -> VariantChoice {
        VariantChoice {
            key: key.to_owned(),
            label: "Test Pink".to_owned(),
            absorption: AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::energy(550.0, 1500.5, 0.35)],
                vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
            ),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 6.0,
        }
    }

    /// A one-set isotropic Custom Color.
    fn isotropic_choice() -> VariantChoice {
        VariantChoice {
            key: CUSTOM_VARIANT_KEY.to_owned(),
            label: "Custom Color".to_owned(),
            absorption: AbsorptionTensor::isotropic(vec![AbsorptionBand::new(520.0, 50.0, 0.2)]),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 5.0,
        }
    }

    #[test]
    fn a_view_variant_on_a_pleochroic_host_keeps_its_pleochroism() {
        let chosen = pleochroic_choice("sapphire.test");
        let mut ctx = view_ctx("Sapphire", None);
        ctx.view_body_color = Some(ViewColor::from_variant("Sapphire", chosen.clone()));
        let tinted = ctx.tinted_material_override().expect("Sapphire resolves");
        let base = GemMaterial::sapphire();
        assert!(tinted.absorption.is_pleochroic, "o and e stay different");
        assert!(tinted.zoning.is_none());
        assert_eq!(tinted.absorption, chosen.absorption);
        assert_eq!(tinted.absorption_unit, AbsorptionUnit::PerMm);
        assert_eq!(tinted.name, base.name);
        assert_eq!(tinted.dispersion, base.dispersion);
        assert_eq!(tinted.birefringence_delta, base.birefringence_delta);
    }

    #[test]
    fn a_legacy_view_color_on_a_pleochroic_host_still_renders_as_saved() {
        let tinted = view_ctx("Sapphire", Some(YELLOW))
            .tinted_material_override()
            .expect("Sapphire resolves");
        assert!(
            !tinted.absorption.is_pleochroic,
            "the legacy triple is one color"
        );
        assert_eq!(
            tinted.absorption,
            GemMaterial::sapphire().with_body_color(YELLOW).absorption
        );
    }

    #[test]
    fn a_variant_that_does_not_fit_the_host_leaves_the_hosts_own_color() {
        // A pleochroic variant on an isotropic host.
        let mut ctx = view_ctx("Diamond", None);
        ctx.view_body_color = Some(ViewColor::from_variant(
            "Sapphire",
            pleochroic_choice("sapphire.test"),
        ));
        let tinted = ctx.tinted_material_override().expect("Diamond resolves");
        assert_eq!(tinted.absorption, GemMaterial::diamond().absorption);

        // An isotropic Custom Color on a pleochroic host.
        let mut ctx = view_ctx("Sapphire", None);
        ctx.view_body_color = Some(ViewColor::from_variant("Sapphire", isotropic_choice()));
        let tinted = ctx.tinted_material_override().expect("Sapphire resolves");
        assert_eq!(tinted.absorption, GemMaterial::sapphire().absorption);
    }

    #[test]
    fn a_named_variant_belongs_to_its_material_and_a_custom_color_to_any_host_of_its_rank() {
        let sapphire = GemMaterial::sapphire();
        let ruby = GemMaterial::by_name("Ruby").expect("a built-in");
        let named = ViewVariant::new("Sapphire", pleochroic_choice("sapphire.test"));
        assert!(named.fits(&sapphire));
        let mut upper = GemMaterial::sapphire();
        upper.name = "SAPPHIRE".to_owned();
        assert!(named.fits(&upper), "ASCII case is ignored");
        assert!(!named.fits(&ruby), "another material, though the same rank");

        let custom = ViewVariant::new("Sapphire", pleochroic_choice(CUSTOM_VARIANT_KEY));
        assert!(custom.fits(&sapphire));
        assert!(custom.fits(&ruby), "a Custom Color belongs to no material");
        assert!(
            !custom.fits(&GemMaterial::diamond()),
            "but the rank must match"
        );
    }

    #[test]
    fn the_variants_identity_covers_every_band_the_unit_and_the_material() {
        let base = ViewVariant::new("Sapphire", pleochroic_choice("sapphire.test"));
        // The same content built again has the same words.
        let again = ViewVariant::new("Sapphire", pleochroic_choice("sapphire.test"));
        assert_eq!(base.identity_words(), again.identity_words());

        let mut other_peak = base.clone();
        other_peak.choice.absorption.e_ray[0].peak = 0.13;
        assert_ne!(other_peak.identity_words(), base.identity_words());

        let mut other_unit = base.clone();
        other_unit.choice.unit = AbsorptionUnit::ModelUnit;
        assert_ne!(other_unit.identity_words(), base.identity_words());

        let mut other_material = base.clone();
        other_material.material = "Ruby".to_owned();
        assert_ne!(other_material.identity_words(), base.identity_words());

        let mut other_label = base.clone();
        other_label.choice.label = "Renamed".to_owned();
        assert_eq!(
            other_label.identity_words(),
            base.identity_words(),
            "a label does not change what renders"
        );
    }

    #[test]
    fn the_color_host_is_the_traced_material_without_any_color() {
        let mut ctx = view_ctx("Sapphire", None);
        ctx.material_override =
            Some(GemMaterial::sapphire().with_color_variant_tensor(
                pleochroic_choice("x").absorption,
                AbsorptionUnit::PerMm,
            ));
        let host = ctx.color_host().expect("Sapphire resolves");
        assert_eq!(host.absorption, GemMaterial::sapphire().absorption);

        let mut custom = GemMaterial::diamond();
        custom.name = "My Garnet".to_string();
        let mut ctx = view_ctx("my garnet", None);
        ctx.custom_materials = Arc::new(vec![custom]);
        assert_eq!(ctx.color_host().expect("custom resolves").name, "My Garnet");
        assert_eq!(view_ctx("No Such Stone", None).color_host(), None);
    }

    #[test]
    fn a_view_color_reads_back_as_the_color_fields_of_a_selection() {
        let chosen = pleochroic_choice("sapphire.test");
        let variant = ViewColor::from_variant("Sapphire", chosen.clone()).as_selection();
        assert_eq!(variant.color_variant_override, Some(chosen));
        assert_eq!(variant.body_color_override, None);

        let banded = ViewColor {
            triple: YELLOW,
            bands: Some(band_rows()),
            variant: None,
        }
        .as_selection();
        assert_eq!(banded.body_color_override, Some(YELLOW));
        assert_eq!(banded.body_color_bands_override, Some(band_rows()));
        assert_eq!(banded.color_variant_override, None);

        let plain = ViewColor::triple_only(YELLOW).as_selection();
        assert_eq!(plain.body_color_override, Some(YELLOW));
        assert_eq!(plain.body_color_bands_override, None);
    }

    #[test]
    fn a_view_variant_is_dormant_while_the_linked_open_design_drives_the_color() {
        let mut ctx = view_ctx("Sapphire", None);
        ctx.view_body_color = Some(ViewColor::from_variant(
            "Sapphire",
            pleochroic_choice("sapphire.test"),
        ));
        assert!(ctx.effective_view_body_color().is_some());
        ctx.planes_owner = EDITOR;
        ctx.material_linked = true;
        assert_eq!(ctx.effective_view_body_color(), None);
        assert_eq!(ctx.tinted_material_override(), None);
    }
}
