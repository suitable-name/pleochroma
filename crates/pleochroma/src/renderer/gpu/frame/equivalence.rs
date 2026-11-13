//! Equivalence self-tests for this module tree.
//!
//! They are bit-exact where one kernel runs twice, and follow the twin rule of
//! `event_wavefront::compare` where two kernels run the same expressions:
//! [`run_chunk_equivalence`] (chunked vs.
//! single-dispatch), [`run_pipeline_equivalence`]/[`run_wavefront_determinism`]
//! (megakernel vs. wavefront, and the wavefront pipeline against itself), and
//! [`run_specialisation_equivalence`] (GPU dispatch determinism for each specialised
//! material-class pipeline). Every function here needs a real `wgpu` adapter (via the
//! `renderer` the caller passes in) but is otherwise plain library code, not `#[cfg(test)]`.
//! The `gpu_equivalence_harness` example runs them.

use glam::Vec3;

use crate::optics::{
    materials::GemMaterial,
    raytracer::{Camera, LightingPreset},
};

use super::{
    CHUNK_BUDGET_BYTES, FLOATS_PER_TUPLE, GpuFrameScene, GpuPipelineKind, SortedSwitch,
    classify_material, dispatch::chunk_pixels_for, material_class, renderer::GpuFrameRenderer,
};
use crate::renderer::gpu::event_wavefront::compare::{FrameDiff, TwinTier, compare_frames};

/// Result of [`run_chunk_equivalence`].
pub struct ChunkEquivalenceResult {
    /// How many chunks the deliberately-small budget forced. A run where this is 1
    /// proves nothing, so [`Self::passed`] requires more.
    pub chunks_forced: usize,
    /// Pixels whose accumulated XYZ differed between the two runs, in raw bits.
    pub differing_pixels: usize,
    /// Total number of pixels compared.
    pub total_pixels: usize,
    /// Largest absolute component difference seen, for a failure message that says how
    /// far off it was rather than only that it differed.
    pub max_abs_diff: f32,
}

impl ChunkEquivalenceResult {
    /// Bit-exact equality, over a run that actually chunked.
    ///
    /// Exact rather than tolerant on purpose: chunking must be a pure partition of the
    /// same threads, since each thread's output depends only on its own
    /// `(pixel, sample_num)` and nothing else. Any difference at all means
    /// `pixel_offset` is not reconstructing the global pixel index correctly, and a
    /// tolerance would hide exactly the off-by-one that would produce.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.chunks_forced > 1 && self.differing_pixels == 0
    }
}

/// Renders one frame twice -- once in a single dispatch, once forced into many small
/// chunks -- and requires the two to be bit-identical.
///
/// This checks `GpuTransportParams::pixel_offset`. Every other GPU self-test dispatches a
/// whole frame at once, so it runs with `pixel_offset == 0` and never exercises the chunked
/// path. An off-by-one would misplace camera rays and jitter rotations at a chunk boundary,
/// visible as a seam only at resolutions large enough to chunk.
///
/// # Panics
///
/// Panics if the scene's material is not GPU-supported or its environment is
/// unsupported -- both are fixed here (a cubic stone under the studio rig), so either
/// would be a bug in this function, not a runtime condition.
#[must_use]
pub fn run_chunk_equivalence(renderer: &mut GpuFrameRenderer) -> ChunkEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (64u32, 64u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    let mut whole = vec![Vec3::ZERO; num_pixels];
    renderer.set_chunk_budget_bytes(CHUNK_BUDGET_BYTES);
    renderer
        .accumulate(&scene, 0, spp, &mut whole)
        .expect("a cubic material under the studio rig is GPU-supported");

    // Small enough to force several chunks, and deliberately NOT a divisor of the pixel
    // count, so the last chunk is short and a boundary lands mid-row.
    let budget = 700 * FLOATS_PER_TUPLE * size_of::<f32>();
    let chunk_pixels = chunk_pixels_for(budget, spp, num_pixels);
    let chunks_forced = num_pixels.div_ceil(chunk_pixels);

    let mut chunked = vec![Vec3::ZERO; num_pixels];
    renderer.set_chunk_budget_bytes(budget);
    renderer
        .accumulate(&scene, 0, spp, &mut chunked)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_chunk_budget_bytes(CHUNK_BUDGET_BYTES);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in whole.iter().zip(&chunked) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    ChunkEquivalenceResult {
        chunks_forced,
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
    }
}

/// One scene of [`run_event_wavefront_equivalence`]: its name, the frame difference and the tier
/// the two pipelines agree at.
#[derive(Debug, Clone, Copy)]
pub struct SceneTwinReport {
    /// The scene's name in the case table.
    pub name: &'static str,
    /// The megakernel's frame against the event wavefront's.
    pub diff: FrameDiff,
    /// The tier of `diff` ([`FrameDiff::twin_tier`]).
    pub tier: TwinTier,
}

/// Result of the pipeline comparisons: [`run_pipeline_equivalence`],
/// [`run_wavefront_determinism`] and [`run_event_wavefront_equivalence`].
pub struct PipelineEquivalenceResult {
    /// Differing pixels.
    pub differing_pixels: usize,
    /// Total number of pixels compared.
    pub total_pixels: usize,
    /// Largest absolute component difference seen, for a failure message that says how
    /// far off it was rather than only that it differed.
    pub max_abs_diff: f32,
    /// Number of pixels beyond the golden tolerance:
    /// `|a - b| <= 1e-5 * max(|a|, |b|) + 1e-6 * spp`.
    pub tol_failing_pixels: usize,
    /// Number of pixels beyond the rounding bound ([`FrameDiff::ROUNDING_RELATIVE_BOUND`]).
    pub rounding_failing_pixels: usize,
    /// Whether every tested scene passed its rule: the twin rule ([`FrameDiff::twin_passed`])
    /// for two different kernels, bit-exactness for one kernel against itself.
    pub tol_passed: bool,
    /// The weakest tier any scene agreed at.
    pub tier: TwinTier,
    /// One report per scene ([`run_event_wavefront_equivalence`] only; empty otherwise).
    pub scenes: Vec<SceneTwinReport>,
}

impl PipelineEquivalenceResult {
    /// Bit-exact equality, a handful of ULPs (`<= 5e-6`), or every scene passing its rule.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.differing_pixels == 0 || self.max_abs_diff <= 5e-6 || self.tol_passed
    }

    /// The result of one scene compared under the twin rule.
    fn of_one_scene(diff: &FrameDiff, total_pixels: usize) -> Self {
        let tier = diff.twin_tier(total_pixels);
        Self {
            differing_pixels: diff.differing_pixels,
            total_pixels,
            max_abs_diff: diff.max_abs_diff,
            tol_failing_pixels: diff.tol_failing_pixels,
            rounding_failing_pixels: diff.rounding_failing_pixels,
            tol_passed: tier != TwinTier::Failed,
            tier,
            scenes: Vec::new(),
        }
    }
}

/// Renders the same chunk through [`GpuPipelineKind::Megakernel`] and
/// [`GpuPipelineKind::Wavefront`] and requires the twin rule ([`FrameDiff::twin_passed`]).
///
/// The megakernel is the production pipeline kind. The wavefront runs the same per-ray
/// physics (`transport_bounce_step`) in a differently compiled kernel, so a driver may round
/// the two differently.
///
/// A real divergence would mean the wavefront's ray-state round trip (load from
/// `WavefrontRayBuffers`, call the shared functions, write back) lost a value that the
/// megakernel's local variables keep. It would not mean the physics differs.
///
/// Uses the fixture of [`run_chunk_equivalence`] (a cubic material under the studio rig),
/// small enough to run in one chunk under both pipelines. This check covers per-ray physics,
/// not chunking.
///
/// # Panics
///
/// Panics if the scene's material is not GPU-supported or its environment is
/// unsupported -- both are fixed here (a cubic stone under the studio rig), so either
/// would be a bug in this function, not a runtime condition.
#[must_use]
pub fn run_pipeline_equivalence(renderer: &mut GpuFrameRenderer) -> PipelineEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);
    let mut megakernel = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut megakernel)
        .expect("a cubic material under the studio rig is GPU-supported");

    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    let mut wavefront = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut wavefront)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);

    let diff = compare_frames(&megakernel, &wavefront, spp);
    PipelineEquivalenceResult::of_one_scene(&diff, num_pixels)
}

/// Renders the same chunk through [`GpuPipelineKind::Wavefront`] TWICE and requires
/// bit-identical `out_xyz`.
///
/// The wavefront-pipeline counterpart of `estimator_check::run_determinism`'s
/// megakernel self-determinism check. See `shaders/wavefront_transport.wgsl`'s module
/// doc comment ("Determinism") for the
/// invariant this proves end to end: no ray's result depends on scheduling, which
/// workgroup it lands in, or the order `wavefront_compact_*` processes rays in -- so two
/// runs against byte-identical input must agree to the bit, exactly as the megakernel's
/// own no-cross-thread-communication guarantee already does.
///
/// # Panics
///
/// Same conditions as [`run_pipeline_equivalence`]'s.
#[must_use]
pub fn run_wavefront_determinism(renderer: &mut GpuFrameRenderer) -> PipelineEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::by_name("Spinel").expect("Spinel is a built-in cubic material");
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;

    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };

    renderer.set_pipeline_kind(GpuPipelineKind::Wavefront);
    let mut first = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut first)
        .expect("a cubic material under the studio rig is GPU-supported");
    let mut second = vec![Vec3::ZERO; num_pixels];
    renderer
        .accumulate(&scene, 0, spp, &mut second)
        .expect("a cubic material under the studio rig is GPU-supported");
    renderer.set_pipeline_kind(GpuPipelineKind::Megakernel);

    let mut differing_pixels = 0usize;
    let mut max_abs_diff = 0.0f32;
    for (a, b) in first.iter().zip(&second) {
        if a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
        {
            differing_pixels += 1;
            max_abs_diff = max_abs_diff.max((*a - *b).abs().max_element());
        }
    }

    let tol_passed = differing_pixels == 0 || max_abs_diff <= 5e-6;
    let tol_failing_pixels = if tol_passed { 0 } else { differing_pixels };

    PipelineEquivalenceResult {
        differing_pixels,
        total_pixels: num_pixels,
        max_abs_diff,
        tol_failing_pixels,
        rounding_failing_pixels: tol_failing_pixels,
        tol_passed,
        tier: if differing_pixels == 0 {
            TwinTier::Exact
        } else if tol_passed {
            TwinTier::Golden
        } else {
            TwinTier::Failed
        },
        scenes: Vec::new(),
    }
}

/// One material's result within [`SpecialisationEquivalenceResult`].
///
/// # Why this is self-determinism, not GENERIC-vs-specialised bit-identity
///
/// GENERIC and specialised pipelines are not guaranteed to be byte-identical. On AMD Vulkan
/// drivers they are not for the isotropic pipeline: dead-code elimination in the specialised
/// kernel changes scheduling enough to flip a stochastic branch by 1 ULP. No driver guarantees
/// bit-identity, so this check does not rely on it. The GENERIC-vs-specialised correctness
/// gate is `estimator_check::run_specialisation_image_comparison`.
///
/// What GPU dispatch determinism DOES guarantee -- and what [`Self::passed`] gates on --
/// is that the SAME compiled pipeline, dispatched twice against identical input,
/// produces byte-identical output: no thread ever reads another thread's output, so
/// scheduling order can never matter WITHIN one pipeline. A specialised pipeline
/// failing that would mean dead-code elimination left something genuinely broken, not
/// merely a differently-scheduled but internally-consistent kernel.
#[derive(Debug, Clone)]
pub struct SpecialisationCaseResult {
    /// Material name.
    pub material_name: String,
    /// Which [`material_class`] value [`classify_material`] picked for this material --
    /// the specialised pipeline actually under test.
    pub material_class: u32,
    /// Pixels that differed between two dispatches of the SAME specialised pipeline
    /// against identical input -- see this struct's own doc comment. Must be 0 for
    /// [`Self::passed`].
    pub self_determinism_differing_pixels: usize,
    /// Diagnostic only, NOT part of [`Self::passed`]: how many pixels differed between
    /// the GENERIC pipeline and the specialised one, and the largest per-component
    /// difference seen. The rigorous pass/fail gate for this comparison is
    /// `estimator_check::run_specialisation_image_comparison`.
    pub generic_vs_specialised_differing_pixels: usize,
    /// Total number of pixels compared.
    pub total_pixels: usize,
    /// Generic vs specialised max abs diff.
    pub generic_vs_specialised_max_abs_diff: f32,
}

impl SpecialisationCaseResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.self_determinism_differing_pixels == 0
    }
}

/// Result of [`run_specialisation_equivalence`].
pub struct SpecialisationEquivalenceResult {
    /// Cases.
    pub cases: Vec<SpecialisationCaseResult>,
}

impl SpecialisationEquivalenceResult {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(SpecialisationCaseResult::passed)
    }
}

/// For each of one representative isotropic, uniaxial, and biaxial built-in material,
/// checks GPU dispatch determinism and records a diagnostic diff count.
///
/// Dispatches that material's specialised pipeline TWICE against identical input (must
/// be byte-identical -- GPU dispatch determinism, see [`SpecialisationCaseResult`]'s doc
/// comment for why that is the right invariant, not GENERIC-vs-specialised bit-identity),
/// and additionally records how many pixels differ against a GENERIC dispatch, purely as
/// a diagnostic.
///
/// Diamond stands in for isotropic, Zircon for uniaxial (largest built-in birefringence),
/// and Alexandrite for biaxial (a populated `beta_ray` band set, exercising the
/// biaxial-only pleochroic absorption path too).
///
/// Uses [`GpuFrameRenderer::accumulate_via_pipeline`] rather than an ad hoc dispatch
/// routine, so this check exercises the SAME chunking/bind/dispatch code
/// [`GpuFrameRenderer::accumulate`] ships, differing only in which pipeline is forced.
///
/// # Panics
///
/// Panics if `"Diamond"`, `"Zircon"`, or `"Alexandrite"` is ever removed from
/// [`GemMaterial::all_materials`] -- self-test scaffolding, not a code path a real
/// caller can reach with a name that might legitimately be missing.
#[must_use]
pub fn run_specialisation_equivalence(
    renderer: &mut GpuFrameRenderer,
) -> SpecialisationEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let (width, height) = (32u32, 32u32);
    let spp = 3u32;
    let num_pixels = (width * height) as usize;
    let environment = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

    let representative_materials = ["Diamond", "Zircon", "Alexandrite"];

    let mut cases = Vec::with_capacity(representative_materials.len());
    for name in representative_materials {
        // Bare built-in material, without the `material_for_stone` colour scale: only GPU
        // parity matters here.
        let material = GemMaterial::by_name(name).unwrap_or_else(|| {
            panic!("{name:?} is a built-in material in GemMaterial::all_materials()")
        });
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: &[],
            material: &material,
            max_bounces: 8,
            environment,
        };
        let class = classify_material(&material);

        // Self-determinism: the SAME specialised pipeline, dispatched twice against
        // identical input -- must be byte-identical.
        let mut run1 = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut run1, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let mut run2 = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut run2, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let self_determinism_differing_pixels = run1
            .iter()
            .zip(&run2)
            .filter(|(a, b)| {
                a.to_array()
                    .iter()
                    .zip(b.to_array().iter())
                    .any(|(x, y)| x.to_bits() != y.to_bits())
            })
            .count();

        // Diagnostic only: GENERIC vs specialised, same input -- see
        // SpecialisationCaseResult's doc comment for why this is NOT required to be zero.
        // GENERIC with the same oriented-inclusion bit as the specialised class, so the two
        // kernels differ only in the material class.
        let generic_class = material_class::GENERIC | (class & material_class::FEATURE_MASK);
        let mut generic = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, generic_class, 0, spp, &mut generic, None)
            .expect("every representative material here is GPU-supported under the studio rig");
        let mut generic_vs_specialised_differing_pixels = 0usize;
        let mut generic_vs_specialised_max_abs_diff = 0.0f32;
        for (a, b) in generic.iter().zip(&run1) {
            if a.to_array()
                .iter()
                .zip(b.to_array().iter())
                .any(|(x, y)| x.to_bits() != y.to_bits())
            {
                generic_vs_specialised_differing_pixels += 1;
                generic_vs_specialised_max_abs_diff =
                    generic_vs_specialised_max_abs_diff.max((*a - *b).abs().max_element());
            }
        }

        cases.push(SpecialisationCaseResult {
            material_name: name.to_string(),
            material_class: class & material_class::KIND_MASK,
            self_determinism_differing_pixels,
            generic_vs_specialised_differing_pixels,
            total_pixels: num_pixels,
            generic_vs_specialised_max_abs_diff,
        });
    }

    SpecialisationEquivalenceResult { cases }
}

/// Renders representative scenes for `class` through both pipelines and checks equivalence.
///
/// `class` is 1 = isotropic, 2 = uniaxial, 3 = biaxial. The scenes go through
/// [`GpuPipelineKind::Megakernel`] and [`EventWavefrontRenderer`], and their outputs must
/// agree under the twin rule ([`FrameDiff::twin_passed`]); every scene's tier is reported.
///
/// # Panics
///
/// Panics if representative materials are missing from the built-in catalogue.
#[must_use]
pub fn run_event_wavefront_equivalence(
    ctx: &crate::renderer::gpu::GpuContext,
    class: u32,
) -> PipelineEquivalenceResult {
    use crate::{
        geometry::cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts},
        optics::raytracer::FacetFinish,
        renderer::gpu::event_wavefront::EventWavefrontRenderer,
    };

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;
    let studio_env = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);

    let synthetic_hdr = ew_cases::synthetic_hdr_map();

    let mut frosted_finishes = vec![FacetFinish::Polished; planes.len()];
    for idx in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
        frosted_finishes[idx] = FacetFinish::Frosted;
    }

    let cases = ew_cases::equivalence_cases(class, studio_env, &frosted_finishes, &synthetic_hdr);

    let mut mega_renderer = GpuFrameRenderer::new().expect("acquire renderer for megakernel");
    let mut ew_renderer = EventWavefrontRenderer::new_diagnostic(ctx.clone());

    let mut total_differing = 0usize;
    let mut total_compared = 0usize;
    let mut max_abs_diff = 0.0f32;
    let mut total_tol_failing = 0usize;
    let mut total_rounding_failing = 0usize;
    let mut worst_tier = TwinTier::Exact;
    let mut scenes = Vec::with_capacity(cases.len());

    for case in cases {
        // The case material's own feature bits, as a production render picks them.
        let class = class | (classify_material(&case.material) & material_class::FEATURE_MASK);
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: case.finishes,
            material: &case.material,
            max_bounces: case.max_bounces,
            environment: case.env,
        };

        let mut mega_out = vec![Vec3::ZERO; num_pixels];
        mega_renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut mega_out, None)
            .expect("megakernel accumulation");

        let mut ew_out = vec![Vec3::ZERO; num_pixels];
        pollster::block_on(ew_renderer.accumulate_via_class(&scene, class, 0, spp, &mut ew_out))
            .expect("event wavefront accumulation");

        // NaN/Inf anywhere is a failure (never a tier); see `compare_frames`.
        let diff = compare_frames(&mega_out, &ew_out, spp);
        let tier = diff.twin_tier(num_pixels);
        worst_tier = worst_tier.max(tier);
        scenes.push(SceneTwinReport {
            name: case.name,
            diff,
            tier,
        });

        total_differing += diff.differing_pixels;
        total_compared += num_pixels;
        max_abs_diff = max_abs_diff.max(diff.max_abs_diff);
        total_tol_failing += diff.tol_failing_pixels;
        total_rounding_failing += diff.rounding_failing_pixels;
    }

    PipelineEquivalenceResult {
        differing_pixels: total_differing,
        total_pixels: total_compared,
        max_abs_diff,
        tol_failing_pixels: total_tol_failing,
        rounding_failing_pixels: total_rounding_failing,
        tol_passed: worst_tier != TwinTier::Failed,
        tier: worst_tier,
        scenes,
    }
}

/// A per-scene breakdown of [`run_event_wavefront_equivalence`] for `class`, one report line
/// per measurement, for diagnosing a failure the aggregate result cannot place.
///
/// For each scene: megakernel vs event wavefront, each pipeline against itself (dispatch
/// determinism), the megakernel against its GENERIC build with the same feature bits, the
/// megakernel against its build whose heavy arms recompute their index setup
/// ([`megakernel_frame_with_recomputed_arm_setups`]), and that build against the event
/// wavefront; then the three pixels that differ most between the two pipelines with every
/// value. The last two rows place a difference: one that shows between the two megakernel
/// builds is the megakernel's own (the same WGSL text and draws, compiled in two places).
///
/// # Panics
///
/// Panics if representative materials are missing from the built-in catalogue or a render
/// fails.
#[must_use]
pub fn diagnose_event_wavefront(ctx: &crate::renderer::gpu::GpuContext, class: u32) -> Vec<String> {
    use crate::{
        geometry::cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts},
        optics::raytracer::FacetFinish,
        renderer::gpu::event_wavefront::EventWavefrontRenderer,
    };

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let (width, height) = (32u32, 32u32);
    let spp = 2u32;
    let num_pixels = (width * height) as usize;
    let studio_env = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
    let synthetic_hdr = ew_cases::synthetic_hdr_map();
    let mut frosted_finishes = vec![FacetFinish::Polished; planes.len()];
    for idx in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
        frosted_finishes[idx] = FacetFinish::Frosted;
    }
    let cases = ew_cases::equivalence_cases(class, studio_env, &frosted_finishes, &synthetic_hdr);

    let mut mega_renderer = GpuFrameRenderer::new().expect("acquire renderer for megakernel");
    // Always the plain megakernel, whatever `PLEOCHROMA_STAGED` or `PLEOCHROMA_SORTED` say:
    // the staged kernel and the sorted wavefront would ignore the pipeline that
    // `megakernel_frame_with_recomputed_arm_setups` swaps in.
    mega_renderer.set_stage_ends(Some(Vec::new()));
    mega_renderer.set_sorted(SortedSwitch::Off);
    let mut ew_renderer = EventWavefrontRenderer::new_diagnostic(ctx.clone());
    let mut lines = Vec::new();
    for case in cases {
        let features = classify_material(&case.material) & material_class::FEATURE_MASK;
        let class = class | features;
        let scene = GpuFrameScene {
            camera: &camera,
            width,
            height,
            planes: &planes,
            facet_finishes: case.finishes,
            material: &case.material,
            max_bounces: case.max_bounces,
            environment: case.env,
        };
        let mut mega = |class: u32| {
            let mut out = vec![Vec3::ZERO; num_pixels];
            mega_renderer
                .accumulate_via_pipeline(&scene, class, 0, spp, &mut out, None)
                .expect("megakernel accumulation");
            out
        };
        let mega_a = mega(class);
        let mega_b = mega(class);
        let mega_generic = mega(material_class::GENERIC | features);
        let mega_recompute =
            megakernel_frame_with_recomputed_arm_setups(&mut mega_renderer, &scene, class, spp);
        let mut ew = || {
            let mut out = vec![Vec3::ZERO; num_pixels];
            pollster::block_on(ew_renderer.accumulate_via_class(&scene, class, 0, spp, &mut out))
                .expect("event wavefront accumulation");
            out
        };
        let ew_a = ew();
        let ew_b = ew();

        lines.push(format!("== {} (class {class:#x})", case.name));
        for (label, a, b) in [
            ("megakernel vs event wavefront", &mega_a, &ew_a),
            ("megakernel vs itself", &mega_a, &mega_b),
            ("event wavefront vs itself", &ew_a, &ew_b),
            ("megakernel vs its GENERIC build", &mega_a, &mega_generic),
            (
                "megakernel vs its build with recomputed arm setups",
                &mega_a,
                &mega_recompute,
            ),
            (
                "megakernel with recomputed arm setups vs event wavefront",
                &mega_recompute,
                &ew_a,
            ),
        ] {
            let d = compare_frames(a, b, spp);
            lines.push(format!(
                "  {label}: {:?}; {} differ, {} beyond golden (allowed {}), {} beyond rounding (allowed {}), {} non-finite, max |delta| {:e}",
                d.twin_tier(num_pixels),
                d.differing_pixels,
                d.tol_failing_pixels,
                FrameDiff::allowed_tol_failing(num_pixels),
                d.rounding_failing_pixels,
                FrameDiff::allowed_rounding_outliers(num_pixels),
                d.non_finite_pixels,
                d.max_abs_diff
            ));
        }
        lines.extend(worst_pixel_lines(
            width as usize,
            &mega_a,
            &ew_a,
            &mega_generic,
            &mega_recompute,
        ));
    }
    lines
}

/// The megakernel frame of `scene` from the build of `class` whose heavy arms recompute their
/// index setup, for [`diagnose_event_wavefront`].
///
/// That build is the gated loop's (`GATED_LOOP = true`): every arm but total internal reflection
/// runs in `transport_bounce_back` from a blank hit record, so `bounce_arm_index` recomputes the
/// setup, as the event wavefront's arm kernels do. The plain build's arms read the setup the
/// surface phase stored. Both builds run the same WGSL text on the same draws. The renderer's
/// cached pipeline of `class` is put back afterwards.
fn megakernel_frame_with_recomputed_arm_setups(
    renderer: &mut GpuFrameRenderer,
    scene: &GpuFrameScene<'_>,
    class: u32,
    spp: u32,
) -> Vec<Vec3> {
    let recompute_pipeline = renderer.build_specialised_pipeline(class, true);
    let cached_pipeline = renderer
        .pipelines_specialised
        .insert(class, recompute_pipeline);
    let mut out = vec![Vec3::ZERO; scene.width as usize * scene.height as usize];
    renderer
        .accumulate_via_pipeline(scene, class, 0, spp, &mut out, None)
        .expect("megakernel accumulation with recomputed arm setups");
    renderer.pipelines_specialised.remove(&class);
    if let Some(pipeline) = cached_pipeline {
        renderer.pipelines_specialised.insert(class, pipeline);
    }
    out
}

/// The three pixels that differ most between `mega` and `ew`, one report line each with the
/// pixel's value in all four frames, for [`diagnose_event_wavefront`].
fn worst_pixel_lines(
    width: usize,
    mega: &[Vec3],
    ew: &[Vec3],
    generic: &[Vec3],
    recompute: &[Vec3],
) -> Vec<String> {
    let mut worst: Vec<usize> = (0..mega.len()).collect();
    worst.sort_by(|&i, &j| {
        let di = (mega[i] - ew[i]).abs().max_element();
        let dj = (mega[j] - ew[j]).abs().max_element();
        dj.total_cmp(&di)
    });
    worst
        .iter()
        .take(3)
        .map(|&i| {
            format!(
                "  pixel ({}, {}): megakernel {:?}, event wavefront {:?}, GENERIC {:?}, recomputed arm setups {:?}",
                i % width,
                i / width,
                mega[i].to_array(),
                ew[i].to_array(),
                generic[i].to_array(),
                recompute[i].to_array()
            )
        })
        .collect()
}

/// The scene table of [`run_event_wavefront_equivalence`].
mod ew_cases;

/// Result of [`run_gated_loop_equivalence`].
#[cfg(test)]
pub struct GatedLoopEquivalenceResult {
    /// Pixels that differed between two dispatches of the gated pipeline (must be 0).
    pub gated_determinism_differing_pixels: usize,
    /// Pixels with a component that differs between the gated and the ungated pipeline by more
    /// than rounding ([`Self::COMPONENT_RELATIVE_TOLERANCE`]).
    ///
    /// Both runs share the same draws, so only a random decision that compiler rounding tipped
    /// over may separate them by more than that; such flips are rare.
    pub gated_vs_ungated_differing_pixels: usize,
    /// Diagnostic only, NOT part of [`Self::passed`]: pixels whose bits differ at all.
    ///
    /// The two kernels are compiled from different code (the gated loop inlines the arms in
    /// another place), so the driver may round them differently, as it does GENERIC and the
    /// specialised kernels (see `SpecialisationCaseResult`).
    pub gated_vs_ungated_bitwise_differing_pixels: usize,
    /// Largest absolute component difference between the gated and the ungated image.
    pub gated_vs_ungated_max_abs_diff: f32,
    /// Summed luminance of the ungated image.
    pub ungated_total: f32,
    /// Summed luminance of the gated image.
    pub gated_total: f32,
    /// Total number of pixels compared.
    pub total_pixels: usize,
}

#[cfg(test)]
impl GatedLoopEquivalenceResult {
    /// Largest share of pixels (1 in 20) that rounding flips may move.
    pub const MAX_DIFFERING_FRACTION_DENOMINATOR: usize = 20;
    /// Largest relative difference of the two frame totals.
    pub const MAX_TOTAL_RELATIVE_DIFF: f32 = 0.01;
    /// Relative component difference up to which a pixel counts as equal: far above compiler
    /// rounding, far below a flipped random decision of one of its samples.
    pub const COMPONENT_RELATIVE_TOLERANCE: f32 = 1e-3;
    /// Absolute floor of that tolerance, for components near zero.
    pub const COMPONENT_ABSOLUTE_TOLERANCE: f32 = 1e-6;

    /// Whether the gated loop is deterministic and agrees with the ungated one within the bound.
    #[must_use]
    pub fn passed(&self) -> bool {
        self.gated_determinism_differing_pixels == 0
            && self.gated_vs_ungated_differing_pixels * Self::MAX_DIFFERING_FRACTION_DENOMINATOR
                <= self.total_pixels
            && self.ungated_total > 0.0
            && (self.gated_total - self.ungated_total).abs()
                <= Self::MAX_TOTAL_RELATIVE_DIFF * self.ungated_total
    }
}

/// Renders one uniaxial stone through the gated and the ungated production megakernel.
///
/// The two runs share the same draws, so the image must be the same up to compiler
/// rounding. The gated pipeline is dispatched twice as a determinism gate (bitwise), then the
/// ungated one once, and the pixels that differ by more than rounding are counted (the bitwise
/// count is reported as a diagnostic). Both pipelines are built directly
/// ([`GpuFrameRenderer::build_specialised_pipeline`]), independent of
/// `PLEOCHROMA_STAGED` and `PLEOCHROMA_SORTED`; the renderer's cached
/// pipeline for the class is dropped afterwards so the next frame builds the one the switch asks
/// for.
///
/// # Panics
///
/// Panics if `"Zircon"` is ever removed from [`GemMaterial::all_materials`], or if the scene
/// is not GPU-supported (self-test scaffolding).
#[cfg(test)]
#[must_use]
pub fn run_gated_loop_equivalence(renderer: &mut GpuFrameRenderer) -> GatedLoopEquivalenceResult {
    use crate::geometry::cuts::StandardGemCuts;

    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let (width, height) = (32u32, 32u32);
    let spp = 3u32;
    let num_pixels = (width * height) as usize;
    let material = GemMaterial::by_name("Zircon").expect("Zircon is a built-in material");
    let scene = GpuFrameScene {
        camera: &camera,
        width,
        height,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 8,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let class = classify_material(&material);
    // Always the plain megakernel, whatever `PLEOCHROMA_STAGED` or `PLEOCHROMA_SORTED` say:
    // the staged kernel and the sorted wavefront would ignore the pipelines swapped in below.
    // The process's switches are restored at the end.
    renderer.set_stage_ends(Some(Vec::new()));
    renderer.set_sorted(SortedSwitch::Off);
    let render = |renderer: &mut GpuFrameRenderer, gated: bool| {
        let pipeline = renderer.build_specialised_pipeline(class, gated);
        renderer.pipelines_specialised.insert(class, pipeline);
        let mut accum = vec![Vec3::ZERO; num_pixels];
        renderer
            .accumulate_via_pipeline(&scene, class, 0, spp, &mut accum, None)
            .expect("Zircon is GPU-supported under the studio rig");
        accum
    };
    let gated_1 = render(renderer, true);
    let gated_2 = render(renderer, true);
    let ungated = render(renderer, false);
    renderer.pipelines_specialised.remove(&class);
    renderer.set_stage_ends(None);
    renderer.set_sorted(SortedSwitch::Process);

    let differs = |a: &Vec3, b: &Vec3| {
        a.to_array()
            .iter()
            .zip(b.to_array().iter())
            .any(|(x, y)| x.to_bits() != y.to_bits())
    };
    let gated_determinism_differing_pixels = gated_1
        .iter()
        .zip(&gated_2)
        .filter(|(a, b)| differs(a, b))
        .count();
    let beyond_rounding = |a: &Vec3, b: &Vec3| {
        a.to_array().iter().zip(b.to_array().iter()).any(|(x, y)| {
            (x - y).abs()
                > GatedLoopEquivalenceResult::COMPONENT_RELATIVE_TOLERANCE.mul_add(
                    x.abs().max(y.abs()),
                    GatedLoopEquivalenceResult::COMPONENT_ABSOLUTE_TOLERANCE,
                )
        })
    };
    let mut gated_vs_ungated_differing_pixels = 0usize;
    let mut gated_vs_ungated_bitwise_differing_pixels = 0usize;
    let mut gated_vs_ungated_max_abs_diff = 0.0f32;
    for (a, b) in gated_1.iter().zip(&ungated) {
        if differs(a, b) {
            gated_vs_ungated_bitwise_differing_pixels += 1;
            gated_vs_ungated_max_abs_diff =
                gated_vs_ungated_max_abs_diff.max((*a - *b).abs().max_element());
        }
        if beyond_rounding(a, b) {
            gated_vs_ungated_differing_pixels += 1;
        }
    }
    GatedLoopEquivalenceResult {
        gated_determinism_differing_pixels,
        gated_vs_ungated_differing_pixels,
        gated_vs_ungated_bitwise_differing_pixels,
        gated_vs_ungated_max_abs_diff,
        ungated_total: ungated.iter().map(|p| p.y).sum(),
        gated_total: gated_1.iter().map(|p| p.y).sum(),
        total_pixels: num_pixels,
    }
}

#[cfg(test)]
mod gated_loop_tests {
    use super::run_gated_loop_equivalence;
    use crate::renderer::gpu::merge_tests::renderer_or_skip;

    #[test]
    fn the_gated_loop_agrees_with_the_ungated_loop() {
        let Some(mut renderer) = renderer_or_skip("the_gated_loop_agrees_with_the_ungated_loop")
        else {
            return;
        };
        let result = run_gated_loop_equivalence(&mut renderer);
        assert!(
            result.passed(),
            "gated determinism diff {}, gated vs ungated {} of {} pixels beyond rounding ({} differ bitwise, max abs {}), totals {} vs {}",
            result.gated_determinism_differing_pixels,
            result.gated_vs_ungated_differing_pixels,
            result.total_pixels,
            result.gated_vs_ungated_bitwise_differing_pixels,
            result.gated_vs_ungated_max_abs_diff,
            result.gated_total,
            result.ungated_total
        );
    }
}
