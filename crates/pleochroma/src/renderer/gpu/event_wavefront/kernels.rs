//! Event wavefront kernel enumeration, entry points, generated shader sources,
//! and per-scene kernel selection logic.

use crate::{
    optics::raytracer::{EnvironmentSource, FacetFinish},
    renderer::{buffers::GpuGemMaterial, frame_scene::FrameScene, gpu::frame::material_class},
};

/// The sixteen discrete event-wavefront compute kernels, in dispatch order.
///
/// Three heavy kernels are split in two along an event boundary so that none of them is slow to
/// compile in a browser; the variants are declared next to their other half. The halves split off
/// are only dispatched when the scene needs them (see [`kernels_for`] and
/// [`with_zoned_absorb`]), and an omitted kernel never receives a ray.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EwKernel {
    /// Ray generation: initializes ray core and channel state from camera.
    Generate,
    /// Scene intersection and environment miss evaluation.
    Extend,
    /// Homogeneous Henyey-Greenstein interior scattering evaluation (the plain haze).
    Scatter,
    /// Interior scattering of a stone with oriented inclusions or a domain structure (opal,
    /// labradorite): the oriented, Bragg and zoned estimators, split off [`Self::Scatter`].
    ScatterOriented,
    /// Henyey-Greenstein next-event estimation at a parked scatter event (HDR environments).
    NeeHg,
    /// Surface hit setup, shading normal, and interaction arm selection.
    Surface,
    /// Interior zoned absorption of a stone with color zones, split off [`Self::Surface`].
    ZonedAbsorb,
    /// Frosted facet finish interaction arm.
    ArmFrosted,
    /// Total internal reflection interaction arm.
    ArmTir,
    /// Uniaxial crystal entry interaction arm: the hero solve and the reflect branch.
    ArmUniEntry,
    /// Uniaxial crystal entry interaction arm, transmit branch, split off [`Self::ArmUniEntry`].
    ArmUniEntryTransmit,
    /// Uniaxial crystal internal reflection/exit interaction arm.
    ArmUniInternal,
    /// Generic isotropic and biaxial Fresnel reflection/refraction interaction arm.
    ArmGeneric,
    /// Deferred chromatic exit split resolution.
    SplitResolve,
    /// Surface glare and Russian roulette path termination.
    Tail,
    /// Final spectral integration and XYZ/debug buffer output.
    Finalize,
}

impl EwKernel {
    /// All event wavefront kernels in canonical sequence.
    pub const ALL: [Self; 16] = [
        Self::Generate,
        Self::Extend,
        Self::Scatter,
        Self::ScatterOriented,
        Self::NeeHg,
        Self::Surface,
        Self::ZonedAbsorb,
        Self::ArmFrosted,
        Self::ArmTir,
        Self::ArmUniEntry,
        Self::ArmUniEntryTransmit,
        Self::ArmUniInternal,
        Self::ArmGeneric,
        Self::SplitResolve,
        Self::Tail,
        Self::Finalize,
    ];

    /// WGSL entry point function name.
    #[must_use]
    pub const fn entry_point(self) -> &'static str {
        match self {
            Self::Generate => "ew_generate",
            Self::Extend => "ew_extend",
            Self::Scatter => "ew_scatter",
            Self::ScatterOriented => "ew_scatter_oriented",
            Self::NeeHg => "ew_nee_hg",
            Self::Surface => "ew_surface",
            Self::ZonedAbsorb => "ew_zoned_absorb",
            Self::ArmFrosted => "ew_arm_frosted",
            Self::ArmTir => "ew_arm_tir",
            Self::ArmUniEntry => "ew_arm_uni_entry",
            Self::ArmUniEntryTransmit => "ew_arm_uni_entry_transmit",
            Self::ArmUniInternal => "ew_arm_uni_internal",
            Self::ArmGeneric => "ew_arm_generic",
            Self::SplitResolve => "ew_split_resolve",
            Self::Tail => "ew_tail",
            Self::Finalize => "ew_finalize",
        }
    }

    /// Generated WGSL source code for this kernel.
    #[must_use]
    pub const fn source(self) -> &'static str {
        match self {
            Self::Generate => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_generate.generated.wgsl"))
            }
            Self::Extend => include_str!(concat!(env!("OUT_DIR"), "/ew_extend.generated.wgsl")),
            Self::Scatter => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_scatter.generated.wgsl"))
            }
            Self::ScatterOriented => include_str!(concat!(
                env!("OUT_DIR"),
                "/ew_scatter_oriented.generated.wgsl"
            )),
            Self::NeeHg => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_nee_hg.generated.wgsl"))
            }
            Self::Surface => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_surface.generated.wgsl"))
            }
            Self::ZonedAbsorb => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_zoned_absorb.generated.wgsl"))
            }
            Self::ArmFrosted => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_frosted.generated.wgsl"))
            }
            Self::ArmTir => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_tir.generated.wgsl"))
            }
            Self::ArmUniEntry => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_uni_entry.generated.wgsl"))
            }
            Self::ArmUniEntryTransmit => include_str!(concat!(
                env!("OUT_DIR"),
                "/ew_arm_uni_entry_transmit.generated.wgsl"
            )),
            Self::ArmUniInternal => include_str!(concat!(
                env!("OUT_DIR"),
                "/ew_arm_uni_internal.generated.wgsl"
            )),
            Self::ArmGeneric => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_arm_generic.generated.wgsl"))
            }
            Self::SplitResolve => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_split_resolve.generated.wgsl"))
            }
            Self::Tail => include_str!(concat!(env!("OUT_DIR"), "/ew_tail.generated.wgsl")),
            Self::Finalize => {
                include_str!(concat!(env!("OUT_DIR"), "/ew_finalize.generated.wgsl"))
            }
        }
    }
}

pub use crate::renderer::buffers::KERNEL_SET_HASH;

/// Returns whether `scene` has any frosted facet finish or frosted girdle facets.
#[must_use]
pub fn has_frosted_facets(scene: &FrameScene<'_>) -> bool {
    scene.facet_finishes.contains(&FacetFinish::Frosted)
}

/// The arm kernels for a material class: the pure core of [`needed_arms`].
///
/// Class 0 (GENERIC, runtime-dispatched) can tag a ray with every arm, so it gets all of them;
/// class 1 (isotropic) never reaches the uniaxial arms and class 3 (biaxial) forces
/// `is_biaxial`, which excludes them too.
#[must_use]
pub fn arms_for(class: u32, frosted: bool) -> Vec<EwKernel> {
    let mut arms = Vec::new();
    if frosted {
        arms.push(EwKernel::ArmFrosted);
    }
    arms.push(EwKernel::ArmTir);
    let kind = class & material_class::KIND_MASK;
    if kind == material_class::UNIAXIAL || kind == material_class::GENERIC {
        arms.push(EwKernel::ArmUniEntry);
        // The transmit half runs after the reflect half has parked its rays.
        arms.push(EwKernel::ArmUniEntryTransmit);
        arms.push(EwKernel::ArmUniInternal);
    }
    arms.push(EwKernel::ArmGeneric);
    arms
}

/// Selects the surface interaction arm kernels needed for `scene` under `material_class`.
#[must_use]
pub fn needed_arms(material_class: u32, scene: &FrameScene<'_>) -> Vec<EwKernel> {
    arms_for(material_class, has_frosted_facets(scene))
}

/// Whether the scatter phase needs the separate HG next-event kernel: scattering inside the
/// stone under an HDR environment map (`env_mode == 2` in the shader).
#[must_use]
pub fn needs_nee_hg(scene: &FrameScene<'_>) -> bool {
    scene.material.scatters() && matches!(scene.environment, EnvironmentSource::HdrMap(_))
}

/// Whether `scene`'s stone has color zones as the kernels see them.
///
/// That is the encoded material's zone count (`material.zones.header.zone_count` in the shader),
/// which is 0 for a stone whose zones the GPU cannot render. Such a stone needs
/// [`EwKernel::ZonedAbsorb`].
#[must_use]
pub fn is_zoned(scene: &FrameScene<'_>) -> bool {
    scene.material.zoning.is_some()
        && GpuGemMaterial::encode(scene.material)
            .zones
            .header
            .zone_count
            > 0
}

/// The kernel list for given scene properties: the pure core of [`needed_kernels`].
///
/// It is the list for a stone without color zones, which is what a browser compiles (the browser
/// refuses zoned stones); [`with_zoned_absorb`] adds the kernel a zoned stone needs. A class value
/// without the `NO_ORIENTED` bit may hold oriented inclusions, so it lists
/// [`EwKernel::ScatterOriented`] after the plain scatter kernel; the browser's class always sets
/// that bit and compiles neither.
///
/// Every kernel split off a heavy one follows the rule of the `NeeHg` guard: it is listed exactly
/// when the shader can tag a ray for it, so an omitted kernel never receives a ray.
/// `ew_extend` tags `TAG_SCATTER_ORIENTED` only when `material_oriented_active()`, which the
/// pipeline constant `ORIENTED_INCLUSIONS` (class bit `NO_ORIENTED`) folds to false.
#[must_use]
pub fn kernels_for(class: u32, frosted: bool, has_scatter: bool, nee_hg: bool) -> Vec<EwKernel> {
    let mut kernels = Vec::with_capacity(EwKernel::ALL.len());
    kernels.push(EwKernel::Generate);
    kernels.push(EwKernel::Extend);
    if has_scatter {
        kernels.push(EwKernel::Scatter);
        if class & material_class::NO_ORIENTED == 0 {
            kernels.push(EwKernel::ScatterOriented);
        }
    }
    // Match ew_scatter's tag guard: an omitted kernel must never receive a parked ray.
    if has_scatter && nee_hg && class & material_class::NO_SCATTER_NEE == 0 {
        kernels.push(EwKernel::NeeHg);
    }
    kernels.push(EwKernel::Surface);
    kernels.extend(arms_for(class, frosted));
    kernels.push(EwKernel::SplitResolve);
    kernels.push(EwKernel::Tail);
    kernels.push(EwKernel::Finalize);
    kernels
}

/// `kernels` for a stone with color zones: adds [`EwKernel::ZonedAbsorb`] right after the surface
/// kernel, before the arm kernels.
///
/// `ew_surface` tags `TAG_ZONE_ABSORB` only for a stone with zones ([`is_zoned`]), so a list for
/// an unzoned stone must not contain the kernel and a list for a zoned one must.
#[must_use]
pub fn with_zoned_absorb(mut kernels: Vec<EwKernel>) -> Vec<EwKernel> {
    if let Some(surface) = kernels.iter().position(|k| *k == EwKernel::Surface) {
        kernels.insert(surface + 1, EwKernel::ZonedAbsorb);
    }
    kernels
}

/// Selects all event wavefront kernels needed to render `scene` under `material_class` per §4.5.
#[must_use]
pub fn needed_kernels(material_class: u32, scene: &FrameScene<'_>) -> Vec<EwKernel> {
    needed_kernels_with_surface(material_class, scene, false)
}

/// [`needed_kernels`] for a scene with a surface finish.
///
/// `ground_arm` (a Ground facet surface or the abraded-edge band, `GpuSurfaceScene::needs_ground_arm`) schedules the ground arm kernel
/// even when no facet finish is frosted, because the surface phase then routes rays to it.
#[must_use]
pub fn needed_kernels_with_surface(
    material_class: u32,
    scene: &FrameScene<'_>,
    ground_arm: bool,
) -> Vec<EwKernel> {
    let kernels = kernels_for(
        material_class,
        has_frosted_facets(scene) || ground_arm,
        scene.material.scatters(),
        needs_nee_hg(scene),
    );
    if is_zoned(scene) {
        with_zoned_absorb(kernels)
    } else {
        kernels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_class_has_every_arm_a_ray_can_be_tagged_with() {
        let arms = arms_for(material_class::GENERIC, true);
        for arm in [
            EwKernel::ArmFrosted,
            EwKernel::ArmTir,
            EwKernel::ArmUniEntry,
            EwKernel::ArmUniEntryTransmit,
            EwKernel::ArmUniInternal,
            EwKernel::ArmGeneric,
        ] {
            assert!(arms.contains(&arm), "class 0 is missing {arm:?}");
        }
    }

    #[test]
    fn isotropic_and_biaxial_skip_the_uniaxial_arms_uniaxial_keeps_them() {
        for class in [material_class::ISOTROPIC, material_class::BIAXIAL] {
            let arms = arms_for(class, false);
            assert!(!arms.contains(&EwKernel::ArmUniEntry));
            assert!(!arms.contains(&EwKernel::ArmUniEntryTransmit));
            assert!(!arms.contains(&EwKernel::ArmUniInternal));
            assert!(!arms.contains(&EwKernel::ArmFrosted));
        }
        let arms = arms_for(material_class::UNIAXIAL, false);
        assert!(arms.contains(&EwKernel::ArmUniEntry) && arms.contains(&EwKernel::ArmUniInternal));
        assert!(arms.contains(&EwKernel::ArmUniEntryTransmit));
    }

    #[test]
    fn the_uniaxial_entry_transmit_half_runs_after_the_reflect_half() {
        let arms = arms_for(material_class::UNIAXIAL, false);
        let reflect = arms.iter().position(|k| *k == EwKernel::ArmUniEntry);
        let transmit = arms
            .iter()
            .position(|k| *k == EwKernel::ArmUniEntryTransmit);
        assert!(
            reflect < transmit,
            "the reflect half parks the transmit rays"
        );
    }

    #[test]
    fn nee_hg_is_scheduled_only_with_scattering_and_hdr() {
        let with = kernels_for(material_class::ISOTROPIC, false, true, true);
        let without = kernels_for(material_class::ISOTROPIC, false, true, false);
        assert!(with.contains(&EwKernel::NeeHg) && with.contains(&EwKernel::Scatter));
        assert!(!without.contains(&EwKernel::NeeHg));
        let scatter = with.iter().position(|k| *k == EwKernel::Scatter);
        let nee = with.iter().position(|k| *k == EwKernel::NeeHg);
        assert!(scatter < nee, "ew_nee_hg runs after ew_scatter");
        let no_scatter = kernels_for(material_class::ISOTROPIC, false, false, true);
        assert!(!no_scatter.contains(&EwKernel::NeeHg));
        assert!(!no_scatter.contains(&EwKernel::Scatter));
    }

    #[test]
    fn disabled_scattering_nee_is_never_scheduled_but_keeps_the_scatter_stage() {
        let class = material_class::UNIAXIAL | material_class::NO_SCATTER_NEE;
        let kernels = kernels_for(class, false, true, true);
        assert!(kernels.contains(&EwKernel::Scatter));
        assert!(!kernels.contains(&EwKernel::NeeHg));
        assert!(kernels.contains(&EwKernel::ArmUniEntry));
        assert!(kernels.contains(&EwKernel::ArmUniInternal));
    }

    #[test]
    fn the_oriented_scatter_kernel_follows_the_no_oriented_class_bit() {
        for kind in [
            material_class::GENERIC,
            material_class::ISOTROPIC,
            material_class::UNIAXIAL,
            material_class::BIAXIAL,
        ] {
            // A stone that may hold oriented inclusions: both scatter kernels, plain first.
            let with = kernels_for(kind, false, true, false);
            let plain = with.iter().position(|k| *k == EwKernel::Scatter);
            let oriented = with.iter().position(|k| *k == EwKernel::ScatterOriented);
            assert!(plain.is_some() && plain < oriented, "class {kind}");
            // A stone with none (the browser's class): the oriented kernel is never listed.
            let without = kernels_for(kind | material_class::NO_ORIENTED, false, true, false);
            assert!(without.contains(&EwKernel::Scatter), "class {kind}");
            assert!(
                !without.contains(&EwKernel::ScatterOriented),
                "class {kind}"
            );
            // Without scattering neither exists.
            let none = kernels_for(kind, false, false, false);
            assert!(!none.contains(&EwKernel::Scatter));
            assert!(!none.contains(&EwKernel::ScatterOriented));
        }
    }

    #[test]
    fn the_oriented_scatter_kernel_runs_before_the_hg_nee_kernel_that_finishes_its_events() {
        let kernels = kernels_for(material_class::ISOTROPIC, false, true, true);
        let oriented = kernels.iter().position(|k| *k == EwKernel::ScatterOriented);
        let nee = kernels.iter().position(|k| *k == EwKernel::NeeHg);
        assert!(oriented.is_some() && oriented < nee);
    }

    #[test]
    fn zoned_absorption_is_listed_only_for_a_zoned_stone_between_surface_and_the_arms() {
        let unzoned = kernels_for(material_class::BIAXIAL, false, false, false);
        assert!(!unzoned.contains(&EwKernel::ZonedAbsorb));
        let zoned = with_zoned_absorb(kernels_for(material_class::BIAXIAL, true, true, true));
        let surface = zoned.iter().position(|k| *k == EwKernel::Surface);
        let absorb = zoned.iter().position(|k| *k == EwKernel::ZonedAbsorb);
        let first_arm = zoned
            .iter()
            .position(|k| matches!(k, EwKernel::ArmFrosted | EwKernel::ArmTir));
        assert!(surface < absorb && absorb < first_arm);
        // Everything else is the unzoned list.
        let mut without = zoned;
        without.retain(|k| *k != EwKernel::ZonedAbsorb);
        assert_eq!(
            without,
            kernels_for(material_class::BIAXIAL, true, true, true)
        );
    }

    /// What the browser compiles: the class its stones reach (no oriented inclusions, no zones),
    /// without scattering or frosting, is the small base set; scattering and frosting add their
    /// kernels only on top.
    #[test]
    fn the_browser_class_base_set_has_no_scatter_nee_or_frosted_kernel() {
        let browser =
            |kind: u32| kind | material_class::NO_ORIENTED | material_class::NO_ZONED_ORIENTED;
        let base_iso = kernels_for(browser(material_class::ISOTROPIC), false, false, false);
        let base_uni = kernels_for(browser(material_class::UNIAXIAL), false, false, false);
        assert_eq!(base_iso.len(), 8, "{base_iso:?}");
        assert_eq!(base_uni.len(), 11, "{base_uni:?}");
        for kernel in [
            EwKernel::Scatter,
            EwKernel::ScatterOriented,
            EwKernel::NeeHg,
            EwKernel::ZonedAbsorb,
            EwKernel::ArmFrosted,
        ] {
            assert!(!base_iso.contains(&kernel) && !base_uni.contains(&kernel));
        }
        let full_iso = kernels_for(browser(material_class::ISOTROPIC), true, true, true);
        let full_uni = kernels_for(browser(material_class::UNIAXIAL), true, true, true);
        assert_eq!(full_iso.len(), 11);
        assert_eq!(full_uni.len(), 14);
        assert!(!full_iso.contains(&EwKernel::ScatterOriented));
    }

    #[test]
    fn extraordinary_dispersion_specialisation_does_not_change_event_routing() {
        for class in [
            material_class::GENERIC,
            material_class::ISOTROPIC,
            material_class::UNIAXIAL,
            material_class::BIAXIAL,
        ] {
            assert_eq!(
                kernels_for(class, true, true, true),
                kernels_for(
                    class | material_class::NO_EXTRAORDINARY_DISPERSION,
                    true,
                    true,
                    true
                )
            );
        }
    }

    #[test]
    fn all_lists_every_variant_once_in_dispatch_order() {
        let mut sorted = EwKernel::ALL;
        sorted.sort();
        assert_eq!(sorted, EwKernel::ALL, "ALL is in declaration order");
        assert_eq!(EwKernel::ALL.len(), 16);
        let mut unique = EwKernel::ALL.to_vec();
        unique.dedup();
        assert_eq!(unique.len(), EwKernel::ALL.len());
        let mut entry_points: Vec<&str> = EwKernel::ALL.iter().map(|k| k.entry_point()).collect();
        entry_points.sort_unstable();
        entry_points.dedup();
        assert_eq!(
            entry_points.len(),
            EwKernel::ALL.len(),
            "entry points are unique"
        );
    }
}
