//! Material-class kernel specialisation: which of the three specialised
//! `transport_main` pipelines (isotropic/uniaxial/biaxial) a material's real render
//! should dispatch through, and the lazy per-class pipeline cache on
//! [`super::GpuFrameRenderer`]. See the parent module's doc comment's "Material-class
//! kernel specialisation" section for the full rationale.

use crate::{
    optics::materials::{CrystalSystem, GemMaterial},
    renderer::buffers::GpuGemMaterial,
};

use super::{SHADER_SRC, renderer::GpuFrameRenderer};
use crate::renderer::gpu::compute;

/// The values `spectral_transport.wgsl`'s `MATERIAL_CLASS` pipeline-overridable
/// constant accepts.
///
/// See that override's own doc comment. `pub(crate)` rather than
/// private: `estimator_check::dispatch_transport_for_class` (Tier 3 statistical image
/// comparisons) also needs these to dispatch through the same specialised pipelines
/// [`GpuFrameRenderer::accumulate`] does -- see the parent module's doc comment's
/// "Material-class kernel specialisation" section.
pub mod material_class {
    /// Every class, runtime-dispatched inside the kernel -- the override's own declared
    /// default, so a dispatch that never sets it (every self-test) is unaffected.
    pub const GENERIC: u32 = 0;
    /// Identifier for isotropic.
    pub const ISOTROPIC: u32 = 1;
    /// Identifier for uniaxial.
    pub const UNIAXIAL: u32 = 2;
    /// Identifier for biaxial.
    pub const BIAXIAL: u32 = 3;
    /// The bits of a class value that name one of the four classes above.
    pub const KIND_MASK: u32 = 3;
    /// Flag bit on top of the class: the stone has no oriented inclusions or domain structure.
    ///
    /// Its pipeline then compiles the oriented and Bragg estimators out
    /// (`ORIENTED_INCLUSIONS = false`, see `01_header_and_constants.wgsl`).
    pub const NO_ORIENTED: u32 = 4;
    /// Flag bit on top of the class: the stone is unzoned, so its pipeline compiles the
    /// zoned oriented estimator out (`ZONED_ORIENTED = false`).
    pub const NO_ZONED_ORIENTED: u32 = 8;
    /// Flag bit on top of the class: the stone is a zoned host with oriented inclusions.
    ///
    /// Such a stone never takes the scattering-point NEE (`scatter_nee_zone_free`), so its
    /// pipeline compiles that NEE out (`SCATTER_NEE = false`).
    pub const NO_SCATTER_NEE: u32 = 16;
    /// The stone has no independent extraordinary-ray dispersion curve. Its pipeline uses
    /// the ordinary index plus the constant birefringence without retaining curve dataflow.
    pub const NO_EXTRAORDINARY_DISPERSION: u32 = 32;
    /// The flag bits above.
    pub const FEATURE_MASK: u32 =
        NO_ORIENTED | NO_ZONED_ORIENTED | NO_SCATTER_NEE | NO_EXTRAORDINARY_DISPERSION;

    /// The pipeline-overridable constants a pipeline for `class` is built with.
    #[must_use]
    pub fn pipeline_constants(class: u32) -> [(&'static str, f64); 5] {
        let on = |bit: u32| if class & bit == 0 { 1.0 } else { 0.0 };
        [
            ("MATERIAL_CLASS", f64::from(class & KIND_MASK)),
            ("ORIENTED_INCLUSIONS", on(NO_ORIENTED)),
            ("ZONED_ORIENTED", on(NO_ZONED_ORIENTED)),
            ("SCATTER_NEE", on(NO_SCATTER_NEE)),
            ("EXTRAORDINARY_DISPERSION", on(NO_EXTRAORDINARY_DISPERSION)),
        ]
    }

    /// The flag bits for the encoded `material`.
    ///
    /// [`NO_ORIENTED`] when its inclusion block
    /// turns the oriented estimator off in the kernel (`material_oriented_active`: no family
    /// and no structure), [`NO_ZONED_ORIENTED`] when it is unzoned or has no oriented
    /// inclusions (the zoned oriented estimator then never runs), [`NO_SCATTER_NEE`] when it
    /// is both (the scattering-point NEE then never runs). [`NO_EXTRAORDINARY_DISPERSION`]
    /// is independent of these flags and follows the uploaded extraordinary-curve flag.
    #[must_use]
    pub const fn feature_bits(material: &crate::renderer::buffers::GpuGemMaterial) -> u32 {
        let header = material.inclusions.header;
        let inclusion_bits = if header[0] == 0 && header[1] == 0 {
            NO_ORIENTED | NO_ZONED_ORIENTED
        } else if material.zones.header.zone_count == 0 {
            NO_ZONED_ORIENTED
        } else {
            NO_SCATTER_NEE
        };
        let dispersion_bits = if material.has_extraordinary_dispersion == 0 {
            NO_EXTRAORDINARY_DISPERSION
        } else {
            0
        };
        inclusion_bits | dispersion_bits
    }
}

/// Which of [`material_class`]'s values a real render of `material` should dispatch
/// through.
///
/// MIRRORS `renderer::buffers::GpuGemMaterial::encode`'s own `is_anisotropic`/
/// `has_biaxial_delta` derivation exactly -- never a second, independently-maintained
/// definition: biaxial takes priority (`biaxial_delta_beta_alpha.is_some()`), then
/// uniaxial (`crystal_system != Cubic && |birefringence_delta| > 1e-4`, matching the
/// kernel's own `is_anisotropic`), else isotropic. If `encode`'s formula ever changes,
/// this must change with it, or [`GpuFrameRenderer::accumulate`] would pick a specialised
/// pipeline that forces off state the material's own encoded flags say it needs --
/// silently wrong output, not a crash. `estimator_check::run_specialisation_image_comparison`
/// (statistical, not bit-exact) catches a drift of the KIND only: its GENERIC side takes the
/// material's own feature bits. A drift of a FEATURE bit (see [`material_class::feature_bits`])
/// is caught by `estimator_check::run_specialisation_feature_image_comparison`, whose GENERIC
/// side compiles every feature in.
///
/// The value carries [`material_class::feature_bits`] on top, read from the same
/// `GpuGemMaterial::encode` the upload uses.
#[must_use]
pub fn classify_material(material: &GemMaterial) -> u32 {
    let kind = if material.biaxial_delta_beta_alpha.is_some() {
        material_class::BIAXIAL
    } else if material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4
    {
        material_class::UNIAXIAL
    } else {
        material_class::ISOTROPIC
    };
    kind | material_class::feature_bits(&GpuGemMaterial::encode(material))
}

impl GpuFrameRenderer {
    /// Builds and caches the specialised pipeline for `class`, if not already built --
    /// see the parent module's doc comment's "Material-class kernel specialisation"
    /// section for why this is lazy. A no-op for [`material_class::GENERIC`], which
    /// [`Self::pipeline_for_class`] compiles on its first request instead.
    pub(super) fn ensure_specialized_pipeline(&mut self, class: u32) {
        // The `_` arm below treats every out-of-range class exactly like
        // GENERIC (a silent no-op) -- correct for GENERIC itself, but it would just as
        // quietly swallow a caller bug that passes a bogus class, rather than that bug
        // showing up as a panic here instead of much later in `pipeline_for_class`'s own
        // `expect`.
        debug_assert!(
            class <= (material_class::BIAXIAL | material_class::FEATURE_MASK),
            "ensure_specialized_pipeline: class out of range (kind <= BIAXIAL, plus FEATURE_MASK)"
        );
        // Plain GENERIC has nothing to build here: `Self::pipeline_for_class` owns its lazy
        // compile.
        if class == material_class::GENERIC {
            return;
        }
        if !self.pipelines_specialised.contains_key(&class) {
            let label = format!(
                "transport_main (MATERIAL_CLASS={}, features {:#x})",
                class & material_class::KIND_MASK,
                class & material_class::FEATURE_MASK
            );
            let pipeline = compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &self.ctx.adapter.get_info(),
                &label,
                SHADER_SRC,
                "transport_main",
                &material_class::pipeline_constants(class),
            );
            self.pipelines_specialised.insert(class, pipeline);
        }
    }

    /// The GENERIC pipeline, compiled on first request and kept for the renderer's life.
    /// Production never reaches it (see [`classify_material`]); the self-tests and the
    /// specialisation equivalence checks do.
    fn generic_pipeline(&self) -> &wgpu::ComputePipeline {
        self.pipeline_generic.get_or_init(|| {
            compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &self.ctx.adapter.get_info(),
                "transport_main",
                SHADER_SRC,
                "transport_main",
                &[],
            )
        })
    }

    /// Returns the already-built pipeline for `class` -- [`Self::ensure_specialized_pipeline`]
    /// must have been called for this exact `class` first (every caller in this module
    /// tree does so immediately before dispatching).
    ///
    /// # Panics
    ///
    /// Panics if `class` is a specialised value whose pipeline was never built via
    /// [`Self::ensure_specialized_pipeline`] -- a bug in this module's own call
    /// ordering, never a condition a caller outside it can trigger.
    pub(super) fn pipeline_for_class(&self, class: u32) -> &wgpu::ComputePipeline {
        // Same reasoning as `ensure_specialized_pipeline`'s identical
        // assertion -- the `_` arm below silently routes any out-of-range class onto the
        // generic pipeline instead of the `expect` below ever firing.
        debug_assert!(
            class <= (material_class::BIAXIAL | material_class::FEATURE_MASK),
            "pipeline_for_class: class out of range (kind <= BIAXIAL, plus FEATURE_MASK)"
        );
        if class == material_class::GENERIC {
            return self.generic_pipeline();
        }
        self.pipelines_specialised
            .get(&class)
            .expect("ensure_specialized_pipeline must be called for this class before dispatching")
    }
}

#[cfg(test)]
mod tests {
    use super::{GemMaterial, GpuGemMaterial, classify_material, material_class};

    #[test]
    fn extraordinary_specialisation_preserves_real_curves_and_prunes_absent_ones() {
        for (name, has_curve) in [("Zircon", false), ("Quartz", true), ("Rutile", true)] {
            let material = GemMaterial::by_name(name).expect("built-in material");
            let encoded = GpuGemMaterial::encode(&material);
            assert_eq!(
                encoded.has_extraordinary_dispersion != 0,
                has_curve,
                "{name}"
            );
            let class = classify_material(&material);
            assert_eq!(class & material_class::KIND_MASK, material_class::UNIAXIAL);
            assert_eq!(
                class & material_class::NO_EXTRAORDINARY_DISPERSION == 0,
                has_curve,
                "{name} must retain exactly its uploaded dispersion capability"
            );
        }
    }

    #[test]
    fn extraordinary_flag_is_independent_of_inclusion_and_zoning_features() {
        let material = GemMaterial::by_name("Zircon").expect("built-in material");
        let mut encoded = GpuGemMaterial::encode(&material);
        for (family_count, structure, zone_count, expected_inclusions) in [
            (
                0,
                0,
                0,
                material_class::NO_ORIENTED | material_class::NO_ZONED_ORIENTED,
            ),
            (
                0,
                0,
                1,
                material_class::NO_ORIENTED | material_class::NO_ZONED_ORIENTED,
            ),
            (1, 0, 0, material_class::NO_ZONED_ORIENTED),
            (1, 0, 1, material_class::NO_SCATTER_NEE),
            (0, 1, 1, material_class::NO_SCATTER_NEE),
        ] {
            encoded.inclusions.header[0] = family_count;
            encoded.inclusions.header[1] = structure;
            encoded.zones.header.zone_count = zone_count;
            for has_curve in [0, 1] {
                encoded.has_extraordinary_dispersion = has_curve;
                let expected_dispersion = if has_curve == 0 {
                    material_class::NO_EXTRAORDINARY_DISPERSION
                } else {
                    0
                };
                assert_eq!(
                    material_class::feature_bits(&encoded),
                    expected_inclusions | expected_dispersion
                );
            }
        }
    }

    #[test]
    fn pipeline_constants_retain_generic_runtime_features_and_honor_all_feature_bits() {
        let generic = material_class::pipeline_constants(material_class::GENERIC);
        assert_eq!(generic[0], ("MATERIAL_CLASS", 0.0));
        assert!(generic[1..].iter().all(|(_, value)| *value == 1.0));
        let disabled = material_class::pipeline_constants(
            material_class::UNIAXIAL | material_class::FEATURE_MASK,
        );
        assert_eq!(disabled[0], ("MATERIAL_CLASS", 2.0));
        assert!(disabled[1..].iter().all(|(_, value)| *value == 0.0));
        assert!(
            generic
                .iter()
                .any(|&(name, value)| name == "EXTRAORDINARY_DISPERSION" && value == 1.0)
        );
    }
}
