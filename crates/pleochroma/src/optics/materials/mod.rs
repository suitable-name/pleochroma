//! Gemstone optical material data: [`GemMaterial`] (dispersion, birefringence,
//! pleochroic absorption, inclusion scattering) and the built-in material table.
//!
//! This file defines the types and [`GemMaterial::all_materials`], which joins the
//! builder functions. Each builder module covers one species range. [`custom`] holds
//! the builder-style constructors, [`body_color`] the body-color presets, [`lookup`]
//! [`GemMaterial::by_name`] and the accessors, and [`optics`] the biaxial-indicatrix,
//! extraordinary-index and GPU-routing methods. `zoned` holds the zoned-absorption
//! helpers; Ametrine (`ametrine`, bicolour quartz) is the only zoned built-in. `provenance`
//! holds the published source of each built-in's dispersion curve and absorption spectrum.

use super::{absorption::AbsorptionTensor, dispersion::DispersionModel};
use glam::Vec3;

/// A 64-bit fingerprint of everything a [`GemMaterial`] holds, so a cache key can stand for
/// the material without owning a copy of it.
///
/// The fingerprint hashes the material's `Debug` rendering, which prints every field
/// (floats in their shortest round-trip form, so distinct `f32` values never print alike),
/// so a field added later is covered too. `DefaultHasher::new()` has fixed keys, so the
/// value is stable within a process. A collision (about 2^-64 per pair) would serve one
/// material's metrics for another.
pub(crate) fn material_fingerprint(material: &GemMaterial) -> u64 {
    /// Streams formatted text into a hasher without building the `String` first.
    struct HashWriter(std::collections::hash_map::DefaultHasher);

    impl std::fmt::Write for HashWriter {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }

    use std::{fmt::Write as _, hash::Hasher as _};
    let mut writer = HashWriter(std::collections::hash_map::DefaultHasher::new());
    // Writing into a hasher cannot fail; the `Result` is only the `fmt::Write` contract.
    let _ = write!(writer, "{material:?}");
    writer.0.finish()
}

mod amethyst_through_citrine;
mod ametrine;
mod andalusite_through_glass;
mod aquamarine_through_citrine;
pub mod body_color;
mod custom;
mod diamond_through_emerald;
mod feldspars_and_precious_opal;
mod garnets_grossular_and_andradite;
mod garnets_pyrope_through_spessartine;
mod look;
mod lookup;
mod nanosital;
mod optics;
mod peridot_through_benitoite;
mod provenance;
mod rutile;
mod spinel_through_tourmaline;
mod tanzanite_through_cubic_zirconia;
#[cfg(test)]
mod tests;
mod titanite;
pub mod variants;
mod zircon_through_topaz;
mod zoned;
pub use ametrine::{AMETRINE_AMETHYST_FE_PPMA, AMETRINE_CITRINE_FE_PPMA, ametrine_zoning};
pub use look::{MOONSTONE_PRESET, MaterialLook};
pub use provenance::{OpticsProvenance, optics_provenance};
pub use zoned::{
    ZONE_SWATCH_FALLBACK_MODEL_WIDTH, face_up_swatch, zone_swatches, zone_swatches_at_scale,
    zone_swatches_face_up, zone_swatches_face_up_at_scale,
};

/// A gemstone material's crystallographic system.
///
/// [`OpticalCharacter`] selects the birefringence regime, but the crystal system also
/// gates the optics path: a material is treated as anisotropic only when its system is
/// not `Cubic` AND `|birefringence_delta| > 1e-4` (the `is_anisotropic` field built by
/// `build_ray_material_context` in `optics::raytracer::transport::inner`, mirrored by
/// `DispersionParams` in `renderer::buffers`' material encode). A `Cubic` system
/// therefore forces the isotropic path whatever the stored `birefringence_delta` says;
/// the same predicate is true for biaxial (orthorhombic) materials, which then branch
/// on `biaxial_delta_beta_alpha` instead of the uniaxial machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CrystalSystem {
    Cubic,
    Tetragonal,
    Hexagonal,
    Trigonal,
    Orthorhombic,
    Monoclinic,
    Triclinic,
}

/// Which birefringence regime a material's optics take.
///
/// None (isotropic), uniaxial with a positive or negative sign (`n_e` above or below
/// `n_o`), or biaxial with a positive or negative sign (the same convention applied to
/// `n_beta`'s position between `n_alpha` and `n_gamma`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum OpticalCharacter {
    Isotropic,
    UniaxialPositive,
    UniaxialNegative,
    BiaxialPositive,
    BiaxialNegative,
}

/// What a material's absorption numbers are measured per.
///
/// Metadata for the CPU-side render setup only (`render_setup::apply_material_overrides`
/// reads it to choose [`GemMaterial::absorption_path_scale`]); the tracer and the GPU material
/// layout never see it, so the WGSL structs are unchanged.
///
/// The two variants are the two calibrations the crate has:
///
/// * [`Self::ModelUnit`]: the number is an optical density per MODEL unit of path, tuned by eye
///   for a stone about 7 mm wide, as the swatch looks over ONE model unit (the built-in table
///   apart from the zoned Ametrine, the legacy `[R, G, B]` triples and the
///   fantasy customs built from them). Light leaving a
///   face-up stone has travelled about `render_setup::MODEL_UNIT_FACE_UP_PATH` model units, so
///   the render scale is `W / 7 / MODEL_UNIT_FACE_UP_PATH` (`W` = 7 mm with no size set): the
///   face-up look then equals the swatch at 7 mm, and a stone of width `W` mm is `W / 7` times
///   as dense.
/// * [`Self::PerMm`]: the number is a physical absorption per millimetre (the physics
///   chromophore recipes and the seven-band L*C*h / library body colours). The scale is then
///   `W / model_width` millimetres per model unit, with `W` the stone width, or 7 mm when none
///   is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AbsorptionUnit {
    /// Absorption per model unit, calibrated for a 7 mm stone (see the type's doc comment).
    #[default]
    ModelUnit,
    /// Absorption per millimetre.
    PerMm,
}

/// A gemstone material's full optical description.
///
/// Dispersion, birefringence, pleochroic absorption, and optional inclusion
/// scattering / edge rounding / path scale. Built by [`Self::all_materials`] for the
/// built-in table, or [`Self::new_custom`] for a caller-supplied one.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GemMaterial {
    /// Display name of the material.
    pub name: String,
    /// Crystal system of the material.
    pub crystal_system: CrystalSystem,
    /// Optical character (isotropic, uniaxial or biaxial).
    pub optical_character: OpticalCharacter,
    /// Wavelength-dependent refractive-index model.
    pub dispersion: DispersionModel,
    /// Birefringence `n_e - n_o` (or max - min for biaxial).
    pub birefringence_delta: f32,
    /// Absorption bands per eigenmode.
    pub absorption: AbsorptionTensor,
    /// Optical (crystallographic) c-axis direction, in crystal/model space. Uniaxial
    /// birefringence (`effective_extraordinary_index`, `extraordinary_poynting_dir`)
    /// is evaluated against this axis. Defaults to `Vec3::Y`. For a biaxial material
    /// (`biaxial_delta_beta_alpha.is_some()`) this doubles as the `n_gamma` principal
    /// axis -- see `biaxial_indicatrix`.
    pub c_axis: Vec3,
    /// `n_beta - n_alpha` at the sodium D line, for the biaxial built-ins. `None` for
    /// every isotropic/uniaxial material, which keep using the
    /// `c_axis` + `birefringence_delta` uniaxial machinery.
    ///
    /// Convention: `birefringence_delta` is `n_gamma - n_alpha` for a biaxial entry, and
    /// the base `dispersion` curve is `n_beta(lambda)`, the middle principal index (see
    /// `biaxial_indicatrix`). This one extra scalar then places all three principal
    /// indices `n_alpha <= n_beta <= n_gamma`, treating the SPREAD between them as
    /// wavelength-independent (only the base curve disperses) -- the same
    /// achromatic-delta approximation the uniaxial `n_e = n_o + birefringence_delta`
    /// already makes.
    pub biaxial_delta_beta_alpha: Option<f32>,
    /// The `n_alpha` principal direction of a biaxial material (unit, perpendicular to
    /// [`Self::c_axis`], which is `n_gamma`): the second direction of a full crystal
    /// frame. `None` (every built-in, and [`Self::new_custom`]'s default) keeps the
    /// arbitrary completion `stable_orthonormal_basis(c_axis)`. Ignored for uniaxial and
    /// isotropic materials, whose second direction has no optical meaning. See
    /// `birefringence::alpha_axis_from_turn` for building one from a turn angle.
    ///
    /// `#[serde(default)]` lets an older self-describing file (a worker's `scene.json`)
    /// load as `None`. It deliberately does NOT skip serialising `None`: the material
    /// also travels inside the net protocol's `SceneState` as postcard, a fixed-layout
    /// format that cannot omit a field, so a skipped field would shift every field after it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub alpha_axis: Option<Vec3>,
    /// Inclusion/subsurface scattering: the homogeneous Henyey-Greenstein scattering
    /// coefficient (`sigma_s`, a physical, linear coefficient in inverse model units of
    /// path length -- no perceptual/logarithmic remapping) modeling silk, rutile
    /// needles, and clouds as a single averaged-out volumetric density.
    ///
    /// `0.0` (every built-in's own stored value, and `new_custom`'s) means no
    /// scattering medium: `optics::raytracer::absorption::apply_absorption`'s deterministic
    /// Beer-Lambert path is taken unconditionally whenever this is `<= 0.0`. See
    /// `optics::raytracer::scattering::hg::maybe_scatter_or_extinguish` for the estimator this
    /// feeds once
    /// nonzero (extinction `sigma_t = sigma_a + sigma_s`, free-path distance sampling,
    /// single-scattering albedo `sigma_s / sigma_t`). Use [`Self::with_scattering`] (or
    /// [`Self::with_recommended_scattering`] for a per-species starting point) to opt a
    /// material into a nonzero value.
    ///
    /// Per MODEL unit of path, independent of the stone's physical size: inclusions are a
    /// property of the design's geometry, so a 5 mm and a 12 mm stone of the same cut carry
    /// the same haze (unlike the absorption, which gets denser with size -- see
    /// [`Self::absorption_unit`]).
    ///
    /// # Useful range
    ///
    /// The built-in cuts (`geometry::StandardGemCuts`) have a girdle radius of order 1
    /// model unit, so a typical internal chord length is roughly 0.5-2 units and the
    /// mean free path between scattering events is `1/sigma_s`. `sigma_s` in roughly
    /// `0.05` (barely perceptible haze) to `3.0` (milky/heavily included) covers the
    /// visually meaningful range for this geometry scale.
    pub scattering_sigma_s: f32,
    /// The Henyey-Greenstein phase function's asymmetry parameter `g` in `(-1, 1)`: `0`
    /// is isotropic scattering, positive values forward-scatter (silk/rutile needle
    /// inclusions are usually forward-scattering), negative values back-scatter.
    /// Meaningless while `scattering_sigma_s <= 0.0`; defaults to `0.0` alongside it.
    /// See [`Self::DEFAULT_SCATTERING_G`] for a sensible default when a caller only
    /// wants to control the amount ([`Self::with_scattering_amount`]).
    pub scattering_g: f32,
    /// Facet edge rounding radius, in the same model units as `scattering_sigma_s` (girdle
    /// radius of order 1 model unit). `0.01` rounds an edge over about 1% of the stone's
    /// scale: a soft glint without a visible bevel. `0.0` (every built-in) disables the
    /// effect:
    /// `optics::raytracer::intersect::shading_normal_near_edge` returns the flat facet normal
    /// unperturbed
    /// whenever this is `<= 0.0`. See [`Self::with_edge_rounding`] to opt in.
    pub edge_rounding_radius: f32,
    /// Model units to absorption-length units: every interior path length is
    /// multiplied by this before Beer-Lambert absorption. Inclusion scattering is NOT scaled
    /// by it: `scattering_sigma_s` is per model unit whatever the stone's size (the tracer
    /// converts it to absorption-length units with `sigma_s / absorption_path_scale`).
    /// `1.0` (every built-in, and `new_custom`'s default) is a no-op. See
    /// [`Self::with_absorption_path_scale`] to opt a material into a different
    /// physical size (e.g. a larger or smaller real-world stone rendered at the same
    /// ~1-model-unit girdle radius as every built-in cut). The render setup chooses this value
    /// from the stone's size and [`Self::absorption_unit`]; a material's own stored value is
    /// only what an unsized `ModelUnit` material keeps.
    pub absorption_path_scale: f32,
    /// What the numbers of [`Self::absorption`] are measured per -- see [`AbsorptionUnit`].
    /// CPU-side metadata for `render_setup::apply_material_overrides`; the GPU material
    /// layout does not carry it. Every built-in is [`AbsorptionUnit::ModelUnit`] except
    /// the zoned Ametrine, which is [`AbsorptionUnit::PerMm`]. Band colours
    /// ([`Self::with_body_color_bands`]) and physics chromophore absorption
    /// ([`Self::with_chromophore_absorption`]) are [`AbsorptionUnit::PerMm`] too.
    /// `#[serde(default)]` keeps an older serialized material loading as `ModelUnit`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub absorption_unit: AbsorptionUnit,
    /// An optional, genuinely wavelength-dependent extraordinary-ray dispersion curve
    /// for a uniaxial material, evaluated instead of the constant-offset approximation
    /// `n_e(lambda) = n_o(lambda) + birefringence_delta` (see
    /// [`Self::extraordinary_index_at`], the single place both are read). Real
    /// birefringence is not wavelength-flat, but modelling that needs the
    /// extraordinary ray's own independent dispersion curve. Nine built-ins set it:
    /// Quartz, Amethyst and Citrine (all one `SiO2` host, from G. Ghosh 1999 -- see the
    /// Quartz entry's comment; Ametrine, which is Citrine's optics, sets it too),
    /// Rutile (`DeVore` 1951), Sapphire and Ruby (Malitson & Dodge 1972), Synthetic
    /// Moissanite (Wang 2013) and Benitoite (a per-ray Cauchy fit, Laurs et al. 1997).
    ///
    /// `None` (every other built-in, and [`Self::new_custom`]'s default) falls back to
    /// `n_o + birefringence_delta` in `extraordinary_index_at`. Meaningless for an
    /// isotropic or biaxial material -- only `optics::raytracer::refraction`'s uniaxial
    /// per-channel index lookups read it.
    ///
    /// The GPU backend receives the curve through `renderer::buffers::GpuGemMaterial`
    /// (`has_extraordinary_dispersion`, `extraordinary_model_type` and the
    /// `extraordinary_param_*` fields, encoded by `renderer::buffers::encode_dispersion_model`).
    /// The WGSL twin of [`Self::extraordinary_index_at`] is `extraordinary_dispersion_evaluate`
    /// (`shaders/transport_physics/03_dispersion_absorption_frosted.wgsl`, same formula and
    /// `n >= 1.0` floor), called from `shaders/transport_bounce/08a_index_setup.wgsl` and
    /// `shaders/event_wavefront/ew_surface.wgsl` when `has_extraordinary_dispersion != 0`.
    /// The constant-offset `per_channel_uniaxial_index` in that physics file does not read
    /// this curve.
    pub uniaxial_extraordinary_dispersion: Option<DispersionModel>,
    /// Optional wavelength-dependent principal-index curves `[n_alpha, n_gamma]` of a
    /// biaxial material. [`Self::dispersion`] stays the `n_beta` curve, so the three
    /// curves together give the full indicatrix at every wavelength (and with it the
    /// optic-axis angle 2V that varies with color).
    ///
    /// `None` (every material but Alexandrite, Chrysoberyl (Yellow) and Titanite, and
    /// [`Self::new_custom`]'s default) keeps the achromatic spreads of
    /// [`Self::biaxial_delta_beta_alpha`] and [`Self::birefringence_delta`]. `Some` is read
    /// only by `biaxial_indicatrix`, and only for a material with
    /// `biaxial_delta_beta_alpha.is_some()`. `biaxial_delta_beta_alpha` and
    /// `birefringence_delta` stay the sodium D line values of the same curves.
    ///
    /// `#[serde(default)]`, NOT skipped when `None`, for the same postcard reason as
    /// [`Self::alpha_axis`].
    #[cfg_attr(feature = "serde", serde(default))]
    pub biaxial_principal_dispersion: Option<[DispersionModel; 2]>,
    /// Spatially zoned absorption (colour zoning, bicolour, watermelon).
    ///
    /// `None` (every built-in except Ametrine, and every custom material) keeps the homogeneous
    /// Beer-Lambert path. `Some` makes the CPU tracer integrate the optical depth through the
    /// zones (`optics::raytracer::absorption::apply_segment_absorption`, and
    /// `optics::raytracer::zoned` for the tables and the frame chain); [`Self::absorption`] is then
    /// ignored in favour of the base zone (see [`Self::with_zoning`]). Zoned materials are
    /// [`AbsorptionUnit::PerMm`] only and carry no fluorescence. Never serialised inside the
    /// material: zones travel in the zoning payload, so wire and file bytes are unchanged. The
    /// colour setters ([`Self::with_body_color`], [`Self::with_body_color_bands`]) drop the zones:
    /// a zoned material's colour is its zones.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub zoning: Option<crate::optics::zoning::ZonedAbsorption>,
    /// Oriented inclusions (silk, needles, platelets), resolved into the stone frame.
    ///
    /// `None` (every built-in and custom material, until a design turns a preset on through
    /// `render_setup::apply_material_overrides_with_inclusions`) keeps the plain
    /// Henyey-Greenstein haze and the deterministic Beer-Lambert path, bit for bit. `Some`
    /// makes the tracer scatter off the families as well
    /// (`optics::raytracer::scattering::oriented`). Never serialised inside the material: the set
    /// travels beside it, so `scene.json`, jobs and the postcard wire bytes of the material do not
    /// move. Ignored for a zoned material.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub inclusions: Option<crate::optics::inclusions::InclusionSet>,
}

impl GemMaterial {
    /// The full built-in material table, aggregated from each species-range builder
    /// module (see this module's own doc comment).
    #[must_use]
    pub fn all_materials() -> Vec<Self> {
        let mut materials = Self::built_in_materials_diamond_through_emerald();
        materials.extend(Self::built_in_materials_zircon_through_topaz());
        materials.extend(Self::built_in_materials_spinel_through_tourmaline());
        materials.extend(Self::built_in_materials_tanzanite_through_cubic_zirconia());
        materials.extend(Self::built_in_materials_aquamarine_through_citrine());
        materials.extend(Self::built_in_materials_amethyst_through_citrine());
        // The one zoned built-in, right after the Amethyst and Citrine entries it combines.
        materials.push(Self::built_in_material_ametrine());
        materials.extend(Self::built_in_materials_garnets_pyrope_through_spessartine());
        materials.extend(Self::built_in_materials_garnet_color_change());
        materials.extend(Self::built_in_materials_garnets_grossular_and_andradite());
        materials.extend(Self::built_in_materials_peridot_through_benitoite());
        materials.extend(Self::built_in_materials_andalusite_through_glass());
        materials.push(Self::built_in_material_rutile());
        // Structural-color stones (Precious Opal (White), Precious Opal (Black), Labradorite,
        // Moonstone) come after the earlier entries, so those keep their list positions.
        materials.extend(Self::built_in_materials_feldspars_and_precious_opal());
        // Titanite and Nanosital come last, so no existing material moves in the list.
        materials.push(Self::built_in_material_titanite());
        materials.push(Self::built_in_material_nanosital());
        materials
    }

    /// The built-in table of [`Self::all_materials`], built once per process. Callers
    /// that only look a material up by name (the live overlay does so every capture)
    /// borrow this instead of rebuilding the table each time.
    #[must_use]
    pub fn built_in() -> &'static [Self] {
        static TABLE: std::sync::LazyLock<Vec<GemMaterial>> =
            std::sync::LazyLock::new(GemMaterial::all_materials);
        &TABLE
    }
}

#[cfg(test)]
mod built_in_table_tests {
    use super::GemMaterial;

    #[test]
    fn the_shared_built_in_table_equals_a_freshly_built_one() {
        let shared = GemMaterial::built_in();
        let fresh = GemMaterial::all_materials();
        assert_eq!(shared.len(), fresh.len());
        assert!(
            shared
                .iter()
                .zip(&fresh)
                .all(|(a, b)| a.name == b.name && a == b),
            "the shared table is the same table, in the same order"
        );
        assert!(std::ptr::eq(shared, GemMaterial::built_in()));
    }
}
