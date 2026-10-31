//! The pure half of the GPU admission gate (plan section 6.5).
//!
//! It decides which scenes the GPU may trace and holds the stored per-adapter record and the
//! adapter key. No DOM, no Workers, so all of it is tested natively; the page's
//! `render::gpu_gate` and the pool in [`crate::host`] call it.
//!
//! - [`gpu_ineligibility_reason`]: why a scene cannot go to the GPU (a material class in
//!   [`ADMITTED_CLASSES`], not zoned, not fluorescent, no UV lamp, the HDR map fits the
//!   adapter); [`gpu_ineligibility_reason_with_tools`] adds the concave-tool limit.
//! - [`material_class`], [`kernels_for_class`], [`class_adapter_key`]: what each material
//!   class compiles (its base kernel set) and where its record is stored.
//! - [`KernelFeature`], [`scene_needs`], [`feature_unit`], [`unit_adapter_key`]: the kernels
//!   only some stones reach (scattering, frosted facets) are compiled on demand, each with a
//!   record of its own, and [`class_untested_reason`] keeps a stone on the CPU until the
//!   kernels it needs are compiled. [`CompiledKernels`] is the Worker's ledger of what it
//!   compiled.
//! - [`browser_pipeline_class`], [`browser_pass_class`]: the smaller kernel variant the browser
//!   compiles and traces for a class (no oriented inclusions, no zoning).
//! - [`AdmissionRecord`] / [`AdmissionState`]: what is stored per adapter, and
//!   [`find_probing_key`], the crash-marker scan.
//! - [`adapter_key`]: the FNV-1a hash that names the record.
//! - [`self_check_scene`] and the compile deadlines the gate uses.

use std::{collections::BTreeMap, fmt::Display};
mod browser_class;
pub use browser_class::{browser_pass_class, browser_pipeline_class, class_kind};
mod features;
pub use features::{
    CompiledKernels, FeatureSet, KernelFeature, REASON_FROSTED_NOT_COMPILED,
    REASON_SCATTER_NEE_NOT_COMPILED, REASON_SCATTER_NOT_COMPILED, SceneNeeds,
    feature_compile_label, feature_for_reason, feature_passed_text, feature_unit, kernels_for_unit,
    scene_needs, unit_class, unit_feature,
};
mod probe;
pub use probe::ProbeRecord;
mod whitted;
pub use whitted::{
    WHITTED_CLASS, WHITTED_KERNEL, engine_ineligibility_reason, whitted_adapter_key,
    whitted_ineligibility_reason,
};

use pleochroma::{
    geometry::{cuts::StandardGemCuts, tool::GPU_MAX_TOOL_PRIMITIVES},
    optics::{materials::CrystalSystem, raytracer::LightingPreset},
    render_setup::Backdrop,
};
use serde::{Deserialize, Serialize};

use crate::{
    scene::{
        CameraSpec, FinishSpec, LightingSpec, MaterialSpec, SceneSpec, planes_to_data,
        resolve_scene_material,
    },
    storage_keys,
};

/// The base kernel set of an isotropic or biaxial stone in smallest-first order (section 6.5).
///
/// It is what a stone without scattering and without frosted facets traces with. The kernels
/// only some stones reach are the [`KernelFeature`]s, compiled when a scene first needs them.
/// A test pins these names to the desktop's own selection (`event_wavefront::kernels::kernels_for`).
pub const ISOTROPIC_BASE_KERNELS: &[&str] = &[
    "Tail",
    "ArmTir",
    "SplitResolve",
    "Finalize",
    "Generate",
    "Surface",
    "Extend",
    "ArmGeneric",
];

/// The base kernels of a uniaxial stone: the isotropic set plus the uniaxial arms (the entry arm
/// is split into its reflect and transmit halves, WG8), which are the slowest to compile and so
/// come last.
pub const UNIAXIAL_BASE_KERNELS: &[&str] = &[
    "Tail",
    "ArmTir",
    "SplitResolve",
    "Finalize",
    "Generate",
    "Surface",
    "Extend",
    "ArmGeneric",
    "ArmUniEntry",
    "ArmUniEntryTransmit",
    "ArmUniInternal",
];

/// The material class of a uniaxial stone (`material_class::UNIAXIAL` in the desktop crate).
pub const UNIAXIAL_CLASS: u32 = 2;
/// The material class of a biaxial stone (`material_class::BIAXIAL` in the desktop crate).
pub const BIAXIAL_CLASS: u32 = 3;

/// The material classes the Path-Traced GPU renderer admits in the browser.
///
/// Only the classes listed here may reach the GPU; a stone of any other class is refused with
/// [`REASON_ANISOTROPIC`]. Each class ships on its own: add `UNIAXIAL_CLASS` or `BIAXIAL_CLASS`
/// here once the owner's browser probes (guide section 6.5, G0) have passed for it. The page
/// compiles and tests a class on first use (its own admission record, see
/// [`class_adapter_key`]) before a stone of it goes to the GPU.
pub const ADMITTED_CLASSES: &[u32] = &[ISOTROPIC_CLASS];

/// Whether the browser's GPU renderer admits stones of `class` (see [`ADMITTED_CLASSES`]).
#[must_use]
pub fn is_class_admitted(class: u32) -> bool {
    ADMITTED_CLASSES.contains(&class)
}

/// The base kernels to compile for `class`, in compile order (smallest first).
///
/// Isotropic and biaxial use the same kernels (a biaxial stone is the generic arm compiled at
/// class 3); uniaxial adds the two uniaxial arms. An unknown class gets the isotropic list.
/// The scattering and frosted-facet kernels are not in it: see [`KernelFeature`].
#[must_use]
pub const fn kernels_for_class(class: u32) -> &'static [&'static str] {
    if class_kind(class) == UNIAXIAL_CLASS {
        UNIAXIAL_BASE_KERNELS
    } else {
        ISOTROPIC_BASE_KERNELS
    }
}

/// The class of a resolved material: 1 isotropic, 2 uniaxial, 3 biaxial.
///
/// Mirrors the kind of `renderer::gpu::frame::specialisation::classify_material` (without
/// its feature bits). The desktop crate keeps that function crate-private, so a test checks
/// this one against the derivation the GPU upload makes (`GpuGemMaterial::encode`).
#[must_use]
pub fn material_class(material: &pleochroma::optics::materials::GemMaterial) -> u32 {
    if material.biaxial_delta_beta_alpha.is_some() {
        BIAXIAL_CLASS
    } else if material.crystal_system != CrystalSystem::Cubic
        && material.birefringence_delta.abs() > 1e-4
    {
        UNIAXIAL_CLASS
    } else {
        ISOTROPIC_CLASS
    }
}

/// The class of the stone `spec` describes, `None` when its material does not resolve.
#[must_use]
pub fn scene_material_class(spec: &SceneSpec) -> Option<u32> {
    let planes: Vec<pleochroma::geometry::GpuFacetPlane> =
        spec.planes.iter().copied().map(Into::into).collect();
    resolve_scene_material(&spec.material, &planes)
        .ok()
        .map(|material| material_class(&material))
}

/// A short name of `class` for status text: "isotropic", "uniaxial", "biaxial".
#[must_use]
pub const fn class_label(class: u32) -> &'static str {
    match class {
        UNIAXIAL_CLASS => "uniaxial",
        BIAXIAL_CLASS => "biaxial",
        _ => "isotropic",
    }
}

/// The bit of `class` in a compiled-classes mask (`1 << class`; 0 for a class that does not
/// fit in 32 bits).
#[must_use]
pub fn class_bit(class: u32) -> u32 {
    1_u32.checked_shl(class).unwrap_or(0)
}

/// Why the Path-Traced `engine` must not send `spec` to the GPU yet, though it would
/// otherwise: its kernels are not compiled in the Worker (`None`: they are).
///
/// `compiled_units` is the pool's mask (see [`class_bit`] and [`feature_unit`]): one bit per
/// compile unit, the base set of a class under the class value and each feature under
/// its own unit id.
///
/// - [`REASON_CLASS_NOT_TESTED`]: the stone's class (any but isotropic, whose base set is the
///   start-up compile the page's admitted flag already stands for) has no compiled base set.
/// - The reason of the first missing [`KernelFeature`] ([`KernelFeature::reason`]): the stone
///   scatters light or has frosted facets and that feature's kernels are not compiled yet.
///
/// Ray-Traced has one class-independent kernel and never waits.
#[must_use]
pub fn class_untested_reason(
    engine: pleochroma::renderer::engine::EngineSpec,
    spec: &SceneSpec,
    compiled_units: u32,
) -> Option<&'static str> {
    if engine.is_ray() {
        return None;
    }
    let needs = scene_needs(spec)?;
    if needs.class != ISOTROPIC_CLASS && compiled_units & class_bit(needs.class) == 0 {
        return Some(REASON_CLASS_NOT_TESTED);
    }
    needs
        .features
        .iter()
        .find(|feature| {
            feature_unit(needs.class, *feature)
                .is_some_and(|unit| compiled_units & class_bit(unit) == 0)
        })
        .map(KernelFeature::reason)
}

/// Dedicated scene id of the 16x16 self-check render.
pub const SELF_CHECK_SCENE_ID: u64 = 0xffff_ffff_ffff_ffff;
/// Samples per pixel of the self-check pass.
pub const SELF_CHECK_SPP: u32 = 4;
/// The material class the v1 kernels are compiled for (1 = isotropic).
pub const ISOTROPIC_CLASS: u32 = 1;
/// The worker's soft compile limit: a kernel slower than this fails the test.
///
/// Measured on Chrome 156 with an AMD RDNA2 adapter (D3D12, probe results of 2026-10-26): the
/// slowest single event-wavefront kernels took 9.5 to 11 s (`Scatter`), up to 8.1 s (`Surface`)
/// and 6.5 s (`ArmUniEntry`), so 5 s failed every class. 15 s lets a slow but finishing compile
/// pass and stays below [`COMPILE_HARD_MS`].
pub const COMPILE_SOFT_MS: u32 = 15_000;
/// The worker's hard compile limit per request.
pub const COMPILE_HARD_MS: u32 = 20_000;
/// How long the page waits for the next answer of the worker while a test or an admission
/// compile runs (reset on every answer). Past it the page gives the worker up.
pub const STAGE_DEADLINE_MS: u32 = COMPILE_HARD_MS + 10_000;

/// Reason text: the material did not resolve.
pub const REASON_UNRESOLVED: &str = "material unresolved";
/// Reason text: the stone is birefringent and its class is not in [`ADMITTED_CLASSES`].
pub const REASON_ANISOTROPIC: &str = "anisotropic stones not yet supported";
/// Reason text: the stone's material class is admitted but its kernels are not compiled and
/// tested on this adapter yet (the page compiles a class on first use).
pub const REASON_CLASS_NOT_TESTED: &str =
    "this stone's material class is not tested on the GPU yet";
/// Reason text: the stone has color zones.
pub const REASON_ZONED: &str = "color-zoned stones not supported on GPU";
/// Reason text: the material carries oriented inclusions (silk, play of color), which the
/// browser's kernel variant compiles out.
pub const REASON_ORIENTED: &str =
    "stones with oriented inclusions render on the CPU in the browser";
/// Reason text: the material has an extraordinary-ray dispersion curve outside the uniaxial
/// class, which the browser's kernel variant compiles out.
pub const REASON_EXTRAORDINARY: &str =
    "stones with an extraordinary-ray dispersion curve render on the CPU in the browser";
/// Reason text: the material fluoresces.
pub const REASON_FLUORESCENT: &str = "fluorescent stones not supported on GPU";
/// Reason text: the lighting is a UV lamp.
pub const REASON_UV_LAMP: &str = "UV lamp lighting not supported on GPU";
/// Reason text: the HDR map does not fit the adapter's storage binding.
pub const REASON_HDR_TOO_BIG: &str = "HDR map exceeds GPU memory limits";
/// Reason text: the stone has more concave tools than the GPU kernels trace
/// (`pleochroma::geometry::tool::GPU_MAX_TOOL_PRIMITIVES`, 128; a test keeps the number in the
/// text equal to the constant).
pub const REASON_TOO_MANY_TOOLS: &str = "more than 128 concave tools (the GPU traces at most 128)";

/// Why `spec` is not eligible for WebGPU event-wavefront rendering in v1 (`None` = eligible).
///
/// `fluorescent` is whether the scene's material carries fluorescence. A [`SceneSpec`]
/// carries no physics recipe and neither web worker builds fluorescence today, so callers
/// pass `false`; the rule is here so that a spec that learns to carry one cannot reach the
/// GPU by accident.
#[must_use]
pub fn gpu_ineligibility_reason(
    spec: &SceneSpec,
    max_storage_binding: u64,
    hdr_texels: Option<u64>,
    fluorescent: bool,
) -> Option<&'static str> {
    let planes: Vec<pleochroma::geometry::GpuFacetPlane> =
        spec.planes.iter().copied().map(Into::into).collect();
    let Ok(material) = resolve_scene_material(&spec.material, &planes) else {
        return Some(REASON_UNRESOLVED);
    };
    if !is_class_admitted(material_class(&material)) {
        return Some(REASON_ANISOTROPIC);
    }
    if material.zoning.is_some() {
        return Some(REASON_ZONED);
    }
    // The browser variant compiles oriented inclusions out (`browser_pass_class`), so the Worker
    // refuses a pass for such a stone. Refusing it here keeps the page from compiling kernel
    // features for it and from reading the Worker's refusal as a lost scene (GPU-01).
    if material.oriented_inclusions().is_some() {
        return Some(REASON_ORIENTED);
    }
    if material_class(&material) != UNIAXIAL_CLASS
        && material.uniaxial_extraordinary_dispersion.is_some()
    {
        return Some(REASON_EXTRAORDINARY);
    }
    if fluorescent {
        return Some(REASON_FLUORESCENT);
    }
    if spec.lighting.preset().is_uv_lamp() {
        return Some(REASON_UV_LAMP);
    }
    if spec.hdr_id.is_some()
        && let Some(texels) = hdr_texels
        && max_storage_binding > 0
        && texels.saturating_mul(16) > max_storage_binding
    {
        return Some(REASON_HDR_TOO_BIG);
    }
    None
}

/// [`gpu_ineligibility_reason`] for a stone with `tool_count` concave tools.
///
/// The tools come from a [`GeometrySceneSpec`](crate::scene::GeometrySceneSpec) or are sent
/// with [`ToWorker::SetGeometryScene`](crate::protocol::ToWorker::SetGeometryScene).
///
/// The event-wavefront kernels trace concave stones (the tools ride in the `planes` storage
/// binding, `renderer::gpu::event_wavefront::EventWavefrontRenderer::accumulate_with_tools`)
/// up to `GPU_MAX_TOOL_PRIMITIVES`; beyond that the stone is the CPU workers' alone. Every
/// other reason keeps its precedence, so a stone that is both anisotropic and tool-heavy
/// still names the material. With `tool_count == 0` this is exactly
/// [`gpu_ineligibility_reason`].
#[must_use]
pub fn gpu_ineligibility_reason_with_tools(
    spec: &SceneSpec,
    tool_count: usize,
    max_storage_binding: u64,
    hdr_texels: Option<u64>,
    fluorescent: bool,
) -> Option<&'static str> {
    gpu_ineligibility_reason(spec, max_storage_binding, hdr_texels, fluorescent)
        .or_else(|| (tool_count > GPU_MAX_TOOL_PRIMITIVES).then_some(REASON_TOO_MANY_TOOLS))
}

/// Builds the 16x16 4-spp Spinel self-check scene (section 6.5).
#[must_use]
pub fn self_check_scene() -> SceneSpec {
    let planes = StandardGemCuts::standard_round_brilliant();
    SceneSpec {
        planes: planes_to_data(&planes),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Spinel"),
        camera: CameraSpec {
            yaw: 0.0,
            pitch: 0.5,
            distance: 4.5,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.0,
            0.0,
            Backdrop::AsLit,
            16.0,
        ),
        max_bounces: 8,
        width: 16,
        height: 16,
        hdr_id: None,
    }
}

/// The lifecycle state of a GPU adapter's admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdmissionState {
    /// Never tested.
    #[default]
    Unknown,
    /// A test or compile is running (a record left in this state is a crash marker).
    Probing,
    /// Passed.
    Ok,
    /// Failed, or the GPU was lost.
    Failed,
}

/// The admission record of one adapter, tracking its compile verdict and compile times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdmissionRecord {
    /// The verdict.
    pub state: AdmissionState,
    /// Compile time per kernel in milliseconds.
    pub per_kernel_ms: BTreeMap<String, f64>,
    /// The slowest kernel.
    pub max_ms: f64,
    /// All kernels together.
    pub total_ms: f64,
    /// Why it failed.
    pub reason: Option<String>,
    /// ISO-8601 UTC time of the last change.
    pub when: String,
}

impl Default for AdmissionRecord {
    fn default() -> Self {
        Self {
            state: AdmissionState::Unknown,
            per_kernel_ms: BTreeMap::new(),
            max_ms: 0.0,
            total_ms: 0.0,
            reason: None,
            when: String::new(),
        }
    }
}

/// The first key whose JSON value parses into an [`AdmissionRecord`] in the
/// [`AdmissionState::Probing`] state (only admission keys are looked at).
#[must_use]
pub fn find_probing_key<'a, I>(entries: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    probing_keys(entries).into_iter().next()
}

/// Every admission key in the [`AdmissionState::Probing`] state, in input order.
#[must_use]
pub fn probing_keys<'a, I>(entries: I) -> Vec<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let prefix = format!("{}{}.", storage_keys::PREFIX, storage_keys::ADMISSION_NAME);
    entries
        .into_iter()
        .filter(|(key, _)| key.starts_with(&prefix))
        .filter(|(_, value)| {
            serde_json::from_str::<AdmissionRecord>(value)
                .is_ok_and(|record| record.state == AdmissionState::Probing)
        })
        .map(|(key, _)| key.to_string())
        .collect()
}

/// FNV-1a, 64 bit.
#[must_use]
pub fn fnv1a_64(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The Chrome (or Chromium, or Edge) major version in a user-agent string, `"0"` when there
/// is none.
#[must_use]
pub fn chrome_major_from_user_agent(user_agent: &str) -> String {
    for prefix in ["Chrome/", "Chromium/", "Edg/"] {
        if let Some(pos) = user_agent.find(prefix) {
            let rest = &user_agent[pos + prefix.len()..];
            let end = rest.find('.').unwrap_or(rest.len());
            let version = &rest[..end];
            if !version.is_empty() && version.chars().all(|c| c.is_ascii_digit()) {
                return version.to_string();
            }
        }
    }
    "0".to_string()
}

/// What the GPU worker reports about the adapter; the admission key is hashed from it.
///
/// On the WebGPU backend wgpu 30 fills only `description` (vendor and device are 0), and
/// Chrome often leaves that empty, so the adapter's limits are part of the key too: two
/// different GPUs rarely share every limit and a description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterFacts<'a> {
    /// Vendor (hex or name).
    pub vendor: &'a str,
    /// Architecture.
    pub architecture: &'a str,
    /// Device.
    pub device: &'a str,
    /// Description.
    pub description: &'a str,
    /// Backend name.
    pub backend: &'a str,
    /// `maxStorageBuffersPerShaderStage`.
    pub max_storage_buffers_per_stage: u32,
    /// `maxStorageBufferBindingSize`.
    pub max_storage_binding: u64,
    /// `maxBufferSize`.
    pub max_buffer: u64,
}

/// The text hashed into the adapter key: every field of `facts`, the browser major version
/// and the kernel set hash, `|`-separated.
#[must_use]
pub fn adapter_key_text(
    facts: &AdapterFacts<'_>,
    browser_major: &str,
    kernel_set_hash: impl Display,
) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        facts.vendor,
        facts.architecture,
        facts.device,
        facts.description,
        facts.backend,
        browser_major,
        kernel_set_hash,
        facts.max_storage_buffers_per_stage,
        facts.max_storage_binding,
        facts.max_buffer,
    )
}

/// The storage key of the admission record of this adapter, browser version and kernel set:
/// `pleochroma.admission.<fnv1a-64 hex>.v1`.
#[must_use]
pub fn adapter_key(
    facts: &AdapterFacts<'_>,
    browser_major: &str,
    kernel_set_hash: impl Display,
) -> String {
    let hash = fnv1a_64(&adapter_key_text(facts, browser_major, kernel_set_hash));
    storage_keys::admission_key(&format!("{hash:016x}"))
}

/// Which compiled kernel variant and kernel split the stored records stand for.
///
/// Raise it whenever the variant the browser compiles for a class changes
/// ([`browser_pipeline_class`]) or what a record covers changes, so every record and crash
/// marker of the old shape is left behind and tested once more. Version 1 was the variant with
/// the oriented, zoned and (isotropic, biaxial) extraordinary-dispersion features compiled
/// out, one record per class for the whole kernel list (scattering and frosted-facet kernels
/// included). Version 2 splits that list: the class record covers the base set, and each
/// [`KernelFeature`] has a record of its own ([`unit_adapter_key`]). A version 1 record that
/// failed on the `Scatter` kernel therefore no longer blocks stones that never scatter.
pub const BROWSER_PIPELINE_VARIANT: u32 = 2;

/// The admission key of one material `class` on this adapter.
///
/// The kernel set hash gets a `:c<class>:p<variant>` suffix, the way [`whitted_adapter_key`]
/// does, so each class has its own record and crash marker, and a record made for another
/// compiled variant ([`BROWSER_PIPELINE_VARIANT`]) is never read. That includes the isotropic
/// records made before the variant existed: they are not read, and the class tests once more.
#[must_use]
pub fn class_adapter_key(
    facts: &AdapterFacts<'_>,
    browser_major: &str,
    kernel_set_hash: impl Display,
    class: u32,
) -> String {
    adapter_key(
        facts,
        browser_major,
        format!("{kernel_set_hash}:c{class}:p{BROWSER_PIPELINE_VARIANT}"),
    )
}

/// The admission key of one compile `unit` on this adapter.
///
/// It is [`class_adapter_key`] for a class's base set, and the same key with an `:f<feature>`
/// suffix for a [`KernelFeature`]'s kernels (see [`feature_unit`]), so each feature has its own
/// record and crash marker.
///
/// A failed scattering compile is then stored against the scattering feature only; the class's
/// base set, and every stone that does not scatter, keep their verdict.
#[must_use]
pub fn unit_adapter_key(
    facts: &AdapterFacts<'_>,
    browser_major: &str,
    kernel_set_hash: impl Display,
    unit: u32,
) -> String {
    match unit_feature(unit) {
        None => class_adapter_key(facts, browser_major, kernel_set_hash, unit),
        Some(feature) => adapter_key(
            facts,
            browser_major,
            format!(
                "{kernel_set_hash}:c{}:p{BROWSER_PIPELINE_VARIANT}:f{}",
                unit_class(unit),
                feature.key_tag()
            ),
        ),
    }
}

#[cfg(test)]
mod tests;
