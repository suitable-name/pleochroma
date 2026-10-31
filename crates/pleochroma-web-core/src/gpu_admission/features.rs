//! The kernels only some stones reach, and the units the browser compiles them in.
//!
//! The browser's compiler builds the event-wavefront kernels one after another and some are
//! slow (`Scatter` takes 9.5 to 11 s on Chrome 156 / AMD RDNA2). A stone that neither scatters
//! light nor has frosted facets never runs them, so the admission compile of a class builds
//! only the base set ([`super::kernels_for_class`]) and each [`KernelFeature`] is compiled
//! the first time a scene needs it, under its own crash marker and record.
//!
//! A compile is named by its *unit*: the class value itself for a base set (1, 2, 3, and the
//! Ray-Traced class), and [`feature_unit`] for a feature of a class. The unit is what travels
//! in `GpuCompile`/`GpuCompiled` and what keys the page's records and the pool's compiled
//! mask ([`super::class_bit`]), so no message or pool call had to change for the split.
//!
//! The name tables repeat the desktop's selection (`event_wavefront::kernels::kernels_for`,
//! which this crate cannot call: the `gpu` feature is a dev-dependency only). A test pins them
//! to it for every kind and every combination of scattering, HDR lighting and frosted facets.

use std::collections::{BTreeMap, BTreeSet};

use pleochroma::optics::raytracer::FacetFinish;

use super::{ISOTROPIC_CLASS, class_kind, class_label, kernels_for_class, material_class};
use crate::scene::{SceneSpec, resolve_finishes, resolve_scene_material};

/// Reason text: the stone scatters light and the scattering kernels are not compiled yet.
pub const REASON_SCATTER_NOT_COMPILED: &str =
    "this stone scatters light and the GPU scattering kernels are not compiled yet";
/// Reason text: the stone scatters light under an HDR map and those kernels are not compiled
/// yet.
pub const REASON_SCATTER_NEE_NOT_COMPILED: &str =
    "this stone scatters light under an HDR map and those GPU kernels are not compiled yet";
/// Reason text: the stone has frosted facets and the frosted-facet kernel is not compiled yet.
pub const REASON_FROSTED_NOT_COMPILED: &str =
    "this stone has frosted facets and the GPU frosted-facet kernel is not compiled yet";

/// The first unit id that names a feature (the ids below it are class values).
const FIRST_FEATURE_UNIT: u32 = 8;
/// Unit ids reserved per feature: the three kinds of material class fit in the low two bits.
const UNITS_PER_FEATURE: u32 = 4;

/// Kernels that only some stones reach, compiled on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KernelFeature {
    /// Interior scattering: the `Scatter` kernel (a stone whose material scatters).
    Scattering,
    /// The next-event estimate of scattering under an HDR map: the `NeeHg` kernel.
    ScatterNee,
    /// Frosted facets: the `ArmFrosted` kernel.
    Frosted,
}

impl KernelFeature {
    /// Every feature, in the order the page compiles missing ones.
    pub const ALL: [Self; 3] = [Self::Scattering, Self::ScatterNee, Self::Frosted];

    const fn index(self) -> u32 {
        match self {
            Self::Scattering => 0,
            Self::ScatterNee => 1,
            Self::Frosted => 2,
        }
    }

    /// The kernels of this feature, in compile order, as the names `GpuCompile` takes. They
    /// are the same for every material class.
    #[must_use]
    pub const fn kernels(self) -> &'static [&'static str] {
        match self {
            Self::Scattering => &["Scatter"],
            Self::ScatterNee => &["NeeHg"],
            Self::Frosted => &["ArmFrosted"],
        }
    }

    /// The tag in the feature's admission key and in nothing the user sees.
    #[must_use]
    pub const fn key_tag(self) -> &'static str {
        match self {
            Self::Scattering => "scatter",
            Self::ScatterNee => "scatter-nee",
            Self::Frosted => "frosted",
        }
    }

    /// What the feature is, for status text: "Compiling the scattering kernels".
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Scattering => "scattering",
            Self::ScatterNee => "HDR scattering",
            Self::Frosted => "frosted-facet",
        }
    }

    /// Why a stone that needs this feature stays on the CPU until it is compiled.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Scattering => REASON_SCATTER_NOT_COMPILED,
            Self::ScatterNee => REASON_SCATTER_NEE_NOT_COMPILED,
            Self::Frosted => REASON_FROSTED_NOT_COMPILED,
        }
    }
}

/// The feature a pool reason stands for (one of the `REASON_*_NOT_COMPILED` texts), `None` for
/// any other reason. The page reads it to know what to compile next.
#[must_use]
pub fn feature_for_reason(reason: &str) -> Option<KernelFeature> {
    KernelFeature::ALL
        .into_iter()
        .find(|feature| feature.reason() == reason)
}

/// The unit id of `feature` for material `class` (a kind or a class value that carries
/// feature bits), `None` for a value that names no material class (the Ray-Traced class has no
/// features).
///
/// The id is `8 + 4 * feature + kind`, so it never collides with a class value and
/// [`super::class_bit`] gives every unit its own bit in the pool's mask.
#[must_use]
pub const fn feature_unit(class: u32, feature: KernelFeature) -> Option<u32> {
    let kind = class_kind(class);
    if matches!(kind, 1..=3) {
        Some(FIRST_FEATURE_UNIT + UNITS_PER_FEATURE * feature.index() + kind)
    } else {
        None
    }
}

/// The material class a unit belongs to: the unit itself for a base set, the kind of the
/// class for a feature.
#[must_use]
pub const fn unit_class(unit: u32) -> u32 {
    if unit < FIRST_FEATURE_UNIT {
        unit
    } else {
        (unit - FIRST_FEATURE_UNIT) % UNITS_PER_FEATURE
    }
}

/// The feature a unit compiles, `None` for a base set (and for an id that names nothing).
#[must_use]
pub fn unit_feature(unit: u32) -> Option<KernelFeature> {
    if unit < FIRST_FEATURE_UNIT || unit_class(unit) == 0 {
        return None;
    }
    let index = (unit - FIRST_FEATURE_UNIT) / UNITS_PER_FEATURE;
    KernelFeature::ALL
        .into_iter()
        .find(|feature| feature.index() == index)
}

/// The kernels a unit compiles: the base set of a class, or the kernels of a feature.
#[must_use]
pub fn kernels_for_unit(unit: u32) -> &'static [&'static str] {
    unit_feature(unit).map_or_else(|| kernels_for_class(unit), KernelFeature::kernels)
}

/// The progress text of a feature compile: "Compiling the scattering kernels", with the count
/// when the feature has more than one kernel, and the class when it is not the isotropic one.
#[must_use]
pub fn feature_compile_label(
    feature: KernelFeature,
    class: u32,
    done: usize,
    total: usize,
) -> String {
    let count = if total > 1 {
        format!(" ({done}/{total})")
    } else {
        String::new()
    };
    format!(
        "Compiling the {} kernels{count}{}",
        feature.noun(),
        class_suffix(class)
    )
}

/// The sentence of the Render panel after a feature compile passed.
#[must_use]
pub fn feature_passed_text(feature: KernelFeature, class: u32, total_ms: f64) -> String {
    format!(
        "GPU {} kernels compiled{} (total {:.1} s)",
        feature.noun(),
        class_suffix(class),
        total_ms / 1000.0
    )
}

/// " for uniaxial stones" for a class other than the isotropic one, else nothing.
fn class_suffix(class: u32) -> String {
    if class == ISOTROPIC_CLASS {
        String::new()
    } else {
        format!(" for {} stones", class_label(class))
    }
}

/// A set of [`KernelFeature`]s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeatureSet(u8);

impl FeatureSet {
    /// No feature.
    pub const EMPTY: Self = Self(0);

    /// The set with `feature` added.
    #[must_use]
    pub const fn with(self, feature: KernelFeature) -> Self {
        Self(self.0 | (1 << feature.index()))
    }

    /// Whether `feature` is in the set.
    #[must_use]
    pub const fn contains(self, feature: KernelFeature) -> bool {
        self.0 & (1 << feature.index()) != 0
    }

    /// Whether the set is empty.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The features in the set, in [`KernelFeature::ALL`] order.
    pub fn iter(self) -> impl Iterator<Item = KernelFeature> {
        KernelFeature::ALL
            .into_iter()
            .filter(move |feature| self.contains(*feature))
    }
}

/// What a scene needs compiled: its material class (a kind) and the features it reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneNeeds {
    /// 1 isotropic, 2 uniaxial, 3 biaxial.
    pub class: u32,
    /// The features beyond the class's base set.
    pub features: FeatureSet,
}

/// The class and the kernel features of the stone `spec` describes, `None` when its material
/// does not resolve.
///
/// The three conditions are the arguments of the desktop's kernel selection
/// (`kernels_for(class, frosted, has_scatter, nee_hg)`): any frosted facet finish, a material
/// that scatters, and scattering under an HDR map (`spec.hdr_id`). The browser never has a
/// ground surface (effects render on the CPU), so the ground arm is not a condition.
#[must_use]
pub fn scene_needs(spec: &SceneSpec) -> Option<SceneNeeds> {
    let planes: Vec<pleochroma::geometry::GpuFacetPlane> =
        spec.planes.iter().copied().map(Into::into).collect();
    let material = resolve_scene_material(&spec.material, &planes).ok()?;
    let scatters = material.scatters();
    let frosted = resolve_finishes(&spec.finishes, &planes)
        .is_ok_and(|finishes| finishes.contains(&FacetFinish::Frosted));
    let mut features = FeatureSet::EMPTY;
    if scatters {
        features = features.with(KernelFeature::Scattering);
        if spec.hdr_id.is_some() {
            features = features.with(KernelFeature::ScatterNee);
        }
    }
    if frosted {
        features = features.with(KernelFeature::Frosted);
    }
    Some(SceneNeeds {
        class: material_class(&material),
        features,
    })
}

/// The kernels a Worker has compiled, per material kind, by name.
///
/// The Worker records each kernel that answered a `GpuCompile` and checks, before a pass,
/// that every kernel the pass will build is in it, so a pass never compiles a pipeline inside
/// itself (past the page's deadlines and crash markers).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CompiledKernels {
    by_kind: BTreeMap<u32, BTreeSet<String>>,
}

impl CompiledKernels {
    /// Remembers that `kernel` is compiled for the kind of `class`.
    pub fn record(&mut self, class: u32, kernel: &str) {
        self.by_kind
            .entry(class_kind(class))
            .or_default()
            .insert(kernel.to_owned());
    }

    /// Whether `kernel` is compiled for the kind of `class`.
    #[must_use]
    pub fn contains(&self, class: u32, kernel: &str) -> bool {
        self.by_kind
            .get(&class_kind(class))
            .is_some_and(|names| names.contains(kernel))
    }

    /// The kernels of `needed` that are not compiled for the kind of `class`, in the order
    /// given.
    #[must_use]
    pub fn missing<'a>(
        &self,
        class: u32,
        needed: impl IntoIterator<Item = &'a str>,
    ) -> Vec<&'a str> {
        needed
            .into_iter()
            .filter(|kernel| !self.contains(class, kernel))
            .collect()
    }

    /// Forgets everything (a new device compiled nothing).
    pub fn clear(&mut self) {
        self.by_kind.clear();
    }
}
