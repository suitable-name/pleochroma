//! Material-class kernel specialisation: which of the three specialised
//! `transport_main` pipelines (isotropic/uniaxial/biaxial) a material's real render
//! should dispatch through, and the lazy per-class pipeline cache on
//! [`super::GpuFrameRenderer`]. See the parent module's doc comment's "Material-class
//! kernel specialisation" section for the full rationale.

use crate::{
    optics::{
        materials::{CrystalSystem, GemMaterial},
        tables::{OpticsTables, default_optics_tables},
    },
    renderer::buffers::GpuGemMaterial,
};

use super::{renderer::GpuFrameRenderer, tray_shadow_source::megakernel_source_for};
use crate::renderer::gpu::compute;

/// The values `spectral_transport.wgsl`'s `MATERIAL_CLASS` pipeline-overridable
/// constant accepts.
///
/// See that override's own doc comment in the shader. Shared with `estimator_check`, whose
/// statistical comparisons dispatch through the same specialised pipelines as
/// [`GpuFrameRenderer::accumulate`].
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
    /// Flag bit on top of the class: the kernels use the optics lookup tables (`--optics-tables`).
    ///
    /// It is no pipeline-overridable constant: it selects the table variant of the shader source
    /// (`super::tables_source`) and adds the table buffer to the bind group, and it keeps that
    /// pipeline apart from the plain one in every cache keyed by class. Without it the class
    /// is the plain kernel.
    pub const OPTICS_TABLES: u32 = 64;
    /// Scene bit selecting the tray variant of the megakernel for a Stone Shadow, and binding 190.
    ///
    /// It is no pipeline-overridable constant and is never dropped silently: a renderer that
    /// cannot bind the shadow buffer declines the scene (`GpuFrameError::TrayVariant`) rather
    /// than rendering it without the shadow.
    pub const TRAY_SHADOW: u32 = 128;
    /// Scene bit selecting the tray variant for a glossy tray (the Tray Reflection) and binding 190.
    ///
    /// Like [`TRAY_SHADOW`] it is no pipeline-overridable constant and is never dropped silently:
    /// a renderer that cannot bind the buffer declines the scene (`GpuFrameError::TrayVariant`)
    /// so the CPU traces the reflection. Either bit alone selects the same variant source; the
    /// buffer header says which feature is active.
    pub const TRAY_MIRROR: u32 = 256;

    /// Whether the scene requires the tray variant of the megakernel: the Stone Shadow, the
    /// Tray Reflection or both.
    #[must_use]
    pub const fn uses_tray_variant(class: u32) -> bool {
        class & (TRAY_SHADOW | TRAY_MIRROR) != 0
    }

    /// The key of the pipeline caches for `class`, with the two tray bits folded into one.
    ///
    /// The Stone Shadow, the Tray Reflection and both select the same source, and the pipeline
    /// takes the same constants and bind group layout and label whichever bit is set, so one
    /// compiled pipeline serves all three. A class without a tray bit is its own key.
    #[must_use]
    pub const fn pipeline_key(class: u32) -> u32 {
        if uses_tray_variant(class) {
            (class & !TRAY_MIRROR) | TRAY_SHADOW
        } else {
            class
        }
    }
    /// Every flag bit above the kind bits.
    pub const FEATURE_MASK: u32 = NO_ORIENTED
        | NO_ZONED_ORIENTED
        | NO_SCATTER_NEE
        | NO_EXTRAORDINARY_DISPERSION
        | OPTICS_TABLES
        | TRAY_SHADOW
        | TRAY_MIRROR;

    /// Whether `class` asks for the table variant of the kernels.
    #[must_use]
    pub const fn uses_optics_tables(class: u32) -> bool {
        class & OPTICS_TABLES != 0
    }

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

    /// [`pipeline_constants`] plus `GATED_LOOP = true`: the megakernel's heavy-arm gated loop.
    ///
    /// Only the plain production megakernel and the live (adaptive) megakernel can take this, and
    /// only when [`gated_loop_enabled`] is on. The beam reference pipelines, the path cache's
    /// phase B and completion pipelines, the event wavefront and the estimator check always keep
    /// the ungated loop (the override's default), because their per-arm hooks live in it.
    #[must_use]
    pub fn pipeline_constants_gated(class: u32) -> [(&'static str, f64); 6] {
        let [kind, oriented, zoned, scatter_nee, dispersion] = pipeline_constants(class);
        [
            kind,
            oriented,
            zoned,
            scatter_nee,
            dispersion,
            ("GATED_LOOP", 1.0),
        ]
    }

    /// Whether `value`, the content of `PLEOCHROMA_GATED_LOOP`, asks for the gated loop:
    /// exactly `1`.
    #[must_use]
    pub fn gated_loop_requested(value: Option<&str>) -> bool {
        matches!(value, Some("1"))
    }

    /// Whether production and live pipelines use the gated loop (`PLEOCHROMA_GATED_LOOP`).
    ///
    /// Off unless the variable is exactly `1`. At 2048 samples per pixel the gated loop stayed
    /// within 6 % of the plain one: faster only at 64 bounces and more, slower on sapphire and
    /// at 8 bounces, and untested on DX12 and AMD. The switch is kept for A/B timing. It reaches
    /// the production chunks of a plain class only with `PLEOCHROMA_STAGED=0`, since the staged
    /// default has its own kernel. Read once per process.
    #[must_use]
    pub fn gated_loop_enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| {
            gated_loop_requested(std::env::var("PLEOCHROMA_GATED_LOOP").ok().as_deref())
        })
    }

    /// The constants for the plain production and live megakernel pipelines of `class`.
    ///
    /// [`pipeline_constants`] unless [`gated_loop_enabled`] is on, else
    /// [`pipeline_constants_gated`].
    #[must_use]
    pub fn production_pipeline_constants(class: u32) -> Vec<(&'static str, f64)> {
        if gated_loop_enabled() {
            pipeline_constants_gated(class).to_vec()
        } else {
            pipeline_constants(class).to_vec()
        }
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
/// Mirrors the `is_anisotropic` and `has_biaxial_delta` derivation of
/// `renderer::buffers::GpuGemMaterial::encode`; the two must change together. A drift would
/// make [`GpuFrameRenderer::accumulate`] pick a pipeline that forces off state the material's
/// encoded flags need: a silently wrong image, not a crash. Biaxial takes priority
/// (`biaxial_delta_beta_alpha.is_some()`), then uniaxial (`crystal_system != Cubic` and
/// `|birefringence_delta| > 1e-4`, as the kernel's `is_anisotropic`), else isotropic.
/// `estimator_check::run_specialisation_image_comparison` catches a drift of the kind and
/// `run_specialisation_feature_image_comparison` a drift of a feature bit; both are
/// statistical, not bit-exact.
///
/// The value carries [`material_class::feature_bits`] from the same `GpuGemMaterial::encode`
/// the upload uses, and [`material_class::OPTICS_TABLES`] when the process default of the
/// optics tables is on ([`default_optics_tables`], which the CPU tracer's
/// `PreparedMaterial::new` also reads).
#[must_use]
pub fn classify_material(material: &GemMaterial) -> u32 {
    classify_material_with_tables(material, default_optics_tables())
}

/// [`classify_material`] with an explicit optics-tables choice instead of the process default.
///
/// [`OpticsTables::Off`] is exactly the class the kernels always had; [`OpticsTables::On`] adds
/// [`material_class::OPTICS_TABLES`].
#[must_use]
pub fn classify_material_with_tables(material: &GemMaterial, tables: OpticsTables) -> u32 {
    let tables_bit = if tables == OpticsTables::On {
        material_class::OPTICS_TABLES
    } else {
        0
    };
    let kind = if material.biaxial_delta_beta_alpha.is_some() {
        material_class::BIAXIAL
    } else if material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4
    {
        material_class::UNIAXIAL
    } else {
        material_class::ISOTROPIC
    };
    kind | material_class::feature_bits(&GpuGemMaterial::encode(material)) | tables_bit
}

impl GpuFrameRenderer {
    /// Builds and caches the specialised pipeline for `class`, if not already built --
    /// see the parent module's doc comment's "Material-class kernel specialisation"
    /// section for why this is lazy. A no-op for [`material_class::GENERIC`], which
    /// [`Self::pipeline_for_class`] compiles on its first request instead.
    pub(super) fn ensure_specialized_pipeline(&mut self, class: u32) {
        // Catches a bogus class here, since the constants below would mask its extra bits.
        debug_assert!(
            class <= (material_class::BIAXIAL | material_class::FEATURE_MASK),
            "ensure_specialized_pipeline: class out of range (kind <= BIAXIAL, plus FEATURE_MASK)"
        );
        // Plain GENERIC has nothing to build here: `Self::pipeline_for_class` owns its lazy
        // compile.
        if class == material_class::GENERIC {
            return;
        }
        // The shadow, the mirror and both share one pipeline: key and build by the folded class.
        let class = material_class::pipeline_key(class);
        if !self.pipelines_specialised.contains_key(&class) {
            let pipeline =
                self.build_specialised_pipeline(class, material_class::gated_loop_enabled());
            self.pipelines_specialised.insert(class, pipeline);
        }
    }

    /// Compiles the plain production megakernel pipeline of `class`, gated loop on or off.
    ///
    /// [`Self::ensure_specialized_pipeline`] passes [`material_class::gated_loop_enabled`];
    /// the gated-against-ungated equivalence check builds both.
    pub(super) fn build_specialised_pipeline(
        &self,
        class: u32,
        gated: bool,
    ) -> wgpu::ComputePipeline {
        let label = format!(
            "transport_main (MATERIAL_CLASS={}, features {:#x}, gated {gated})",
            class & material_class::KIND_MASK,
            class & material_class::FEATURE_MASK
        );
        let constants = if gated {
            material_class::pipeline_constants_gated(class).to_vec()
        } else {
            material_class::pipeline_constants(class).to_vec()
        };
        compute::create_persisted_compute_pipeline(
            &self.ctx.device,
            &self.ctx.adapter.get_info(),
            &label,
            &megakernel_source_for(class).expect("tray variant checked before pipeline creation"),
            "transport_main",
            &constants,
        )
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
                super::SHADER_SRC,
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
        // Out-of-range classes are caught here rather than as a missing pipeline below.
        debug_assert!(
            class <= (material_class::BIAXIAL | material_class::FEATURE_MASK),
            "pipeline_for_class: class out of range (kind <= BIAXIAL, plus FEATURE_MASK)"
        );
        if class == material_class::GENERIC {
            return self.generic_pipeline();
        }
        self.pipelines_specialised
            .get(&material_class::pipeline_key(class))
            .expect("ensure_specialized_pipeline must be called for this class before dispatching")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GemMaterial, GpuGemMaterial, OpticsTables, classify_material,
        classify_material_with_tables, material_class,
    };

    #[test]
    fn the_tables_off_class_is_the_class_the_kernels_always_had() {
        for name in ["Diamond", "Zircon", "Quartz", "Tanzanite", "Rutile"] {
            let material = GemMaterial::by_name(name).expect("built-in material");
            let off = classify_material_with_tables(&material, OpticsTables::Off);
            assert!(!material_class::uses_optics_tables(off), "{name}");
            assert_eq!(off & material_class::OPTICS_TABLES, 0, "{name}");
            // Nothing has set the process default in this test binary.
            assert_eq!(classify_material(&material), off, "{name}");
            let on = classify_material_with_tables(&material, OpticsTables::On);
            assert!(material_class::uses_optics_tables(on), "{name}");
            assert_eq!(on, off | material_class::OPTICS_TABLES, "{name}");
            assert_eq!(
                on & !material_class::FEATURE_MASK,
                off & !material_class::FEATURE_MASK
            );
        }
    }

    #[test]
    fn the_tables_bit_is_a_feature_flag_that_sets_no_pipeline_override() {
        assert_eq!(material_class::OPTICS_TABLES & material_class::KIND_MASK, 0);
        assert_ne!(
            material_class::FEATURE_MASK & material_class::OPTICS_TABLES,
            0
        );
        for class in [
            material_class::GENERIC,
            material_class::ISOTROPIC | material_class::NO_ORIENTED,
            material_class::UNIAXIAL | material_class::NO_EXTRAORDINARY_DISPERSION,
            material_class::BIAXIAL,
        ] {
            assert_eq!(
                material_class::pipeline_constants(class | material_class::OPTICS_TABLES),
                material_class::pipeline_constants(class),
                "class {class}"
            );
        }
    }

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
    fn gated_constants_are_the_plain_constants_plus_the_gated_loop() {
        for class in [
            material_class::GENERIC,
            material_class::ISOTROPIC | material_class::NO_ORIENTED,
            material_class::UNIAXIAL | material_class::FEATURE_MASK,
            material_class::BIAXIAL,
        ] {
            let plain = material_class::pipeline_constants(class);
            let gated = material_class::pipeline_constants_gated(class);
            assert_eq!(gated[..5], plain, "class {class}");
            assert_eq!(gated[5], ("GATED_LOOP", 1.0), "class {class}");
            assert!(plain.iter().all(|&(name, _)| name != "GATED_LOOP"));
        }
    }

    #[test]
    fn production_constants_follow_the_gated_loop_switch() {
        let class = material_class::UNIAXIAL;
        let constants = material_class::production_pipeline_constants(class);
        if material_class::gated_loop_enabled() {
            assert_eq!(constants, material_class::pipeline_constants_gated(class));
        } else {
            assert_eq!(constants, material_class::pipeline_constants(class));
        }
    }

    #[test]
    fn the_gated_loop_is_off_unless_the_switch_is_exactly_one() {
        assert!(!material_class::gated_loop_requested(None));
        assert!(!material_class::gated_loop_requested(Some("")));
        assert!(!material_class::gated_loop_requested(Some("0")));
        assert!(!material_class::gated_loop_requested(Some("true")));
        assert!(material_class::gated_loop_requested(Some("1")));
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
