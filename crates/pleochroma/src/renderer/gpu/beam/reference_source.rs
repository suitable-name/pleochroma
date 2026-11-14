//! The WGSL reference variant: run-time patches with counted anchors on the generated megakernel
//! and event-wavefront sources (beam guide section 5.5 "GPU twin", section 14.3).
//!
//! The reference estimator is the production tracer with two changes: a channel stays live at a
//! refraction only if its refractive indices are **bitwise equal** to the hero's (never a cosine
//! threshold, never a recomputed direction), and exit splitting is off, done the CPU's way
//! (`optics/raytracer/refraction/reflect_refract.rs`): a mismatched channel gets `stokes = 0` and
//! `path_pdf = 0` and stages no split, and the plain spectral integration replaces the families
//! one. On top of that the variant accumulates the two-lane branch signature at every facet event
//! (`shaders/beam/branch_sig.wgsl`) and calls the kept hook `beam_kept` per live channel before the
//! spectral integration.
//!
//! # How the variant is made
//!
//! Exactly as `live_variant` (`renderer/gpu/frame/live_source.rs`) does for the adaptive passes: the
//! generated texts that `build.rs` assembles into `OUT_DIR` (prelude, zoned-patched bounce text,
//! body: the megakernel `SHADER_SRC` and every `EwKernel::source()`) are patched at run time, one
//! exact one-line anchor per edit, and each anchor must occur **exactly its expected number of
//! times** in the unit it patches. Any other count (an anchor that moved, vanished or appeared
//! twice) makes [`reference_variant`] return `None`, so a drift declines instead of mis-patching.
//! The shipped sources, their hashes and the ordinary pipelines are never touched (guide section
//! 2.3: off means identical).
//!
//! Anchors are single-line substrings without leading whitespace or line ends, because the piece
//! files are CRLF on a Windows checkout and LF elsewhere.
//!
//! # Where the signature lives
//!
//! * Megakernel: a module-scope `var<private> ref_sig` (one ray per invocation, initialised from
//!   the seed by the declaration itself), updated after the arm of every bounce.
//! * Event wavefront: a new storage buffer `ray_sig: array<vec2<u32>>` at binding
//!   [`RAY_SIG_BINDING`] (group 0), seeded by `ew_generate`, updated by `ew_tail` (every facet
//!   event passes it) and read by the finalize hook. Not the spare lanes of `EwRayCore`: a
//!   `bitcast<f32>` of a `u32` risks NaN or denormal canonicalisation.

use std::sync::OnceLock;

use super::kept::{KEPT_STORAGE_BUFFERS, kept_wgsl};
use crate::renderer::gpu::{
    MEGAKERNEL_STORAGE_BUFFERS, event_wavefront::EwKernel, frame::SHADER_SRC,
};

/// `shaders/beam/branch_sig.wgsl`: `branch_step`, the seed and the kind codes (the WGSL twin of the
/// Rust `branch_step`).
pub const BRANCH_SIG_SRC: &str = include_str!("../../shaders/beam/branch_sig.wgsl");

/// `shaders/beam/reference_patch.wgsl`: the helpers the patches call and the no-op `beam_kept`.
pub const REFERENCE_PATCH_SRC: &str = include_str!("../../shaders/beam/reference_patch.wgsl");

/// `@binding` (group 0) of the per-ray signature buffer `ray_sig` in the event-wavefront variants.
///
/// 200 is the first of the beam range 200-219, which no shipped shader uses.
pub const RAY_SIG_BINDING: u32 = 200;

/// The megakernel's signature: a private variable, so the megakernel gains no binding.
const MEGAKERNEL_SIG_DECL: &str = "\n// Reference variant: the branch signature of the ray in flight.\nvar<private> ref_sig: vec2<u32> = BRANCH_SIG_SEED;\n\nfn ref_sig_for(idx: u32) -> vec2<u32> {\n    return ref_sig;\n}\n";

/// The unit a variant is made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceUnit {
    /// The megakernel (`transport_main`, `frame::SHADER_SRC`).
    Megakernel,
    /// One event-wavefront kernel (`EwKernel::source`).
    EventWavefront(EwKernel),
}

/// Which units a patch applies to.
#[derive(Clone, Copy)]
enum Scope {
    /// The megakernel only.
    Megakernel,
    /// The shared prelude or bounce text: the megakernel and every event-wavefront kernel.
    Both,
    /// One event-wavefront kernel's own body.
    Kernel(EwKernel),
}

impl Scope {
    fn applies(self, unit: ReferenceUnit) -> bool {
        match (self, unit) {
            (Self::Both, _) | (Self::Megakernel, ReferenceUnit::Megakernel) => true,
            (Self::Kernel(kernel), ReferenceUnit::EventWavefront(other)) => kernel == other,
            _ => false,
        }
    }
}

/// One anchored edit.
struct Patch {
    /// What the edit does, for diagnostics and the tests.
    what: &'static str,
    /// The exact one-line text replaced (every occurrence).
    anchor: &'static str,
    /// The replacement.
    replacement: &'static str,
    /// How often `anchor` occurs in a unit the patch applies to.
    count: usize,
    /// Occurrences added by one event-wavefront kernel's own body (a shared line repeated there).
    extra: Option<(EwKernel, usize)>,
    /// The units the patch applies to.
    scope: Scope,
}

impl Patch {
    fn expected(&self, unit: ReferenceUnit) -> usize {
        match (self.extra, unit) {
            (Some((kernel, more)), ReferenceUnit::EventWavefront(other)) if kernel == other => {
                self.count + more
            }
            _ => self.count,
        }
    }
}

/// The signature update after the arm of a bounce (megakernel).
const MEGAKERNEL_SIG_UPDATE: &str = concat!(
    "if (r == PHASE_NEXT) { r = bounce_run_arm(&s, &h, rc, bounce, seed0, observer); }\n",
    "    if (r == PHASE_NEXT) { ref_sig = ref_sig_event(ref_sig, h.hit.facet_idx, ",
    "h.pre_bounce_inside_gem, s.inside_gem, s.is_extraordinary); }"
);

/// The generic arm's exact match: the hero (channel 0) always matches, a companion only if every
/// index that enters its refracted direction is bitwise the hero's. Mirrors the CPU's
/// `exact_hero_indices` (incident index, mode-selected transmitted index, ordinary index) plus the
/// extraordinary index for a uniaxial entry and the principal indices for a biaxial one.
const GENERIC_EXACT_MATCH: &str = concat!(
    "let direction_matches = k == 0u\n",
    "                || (ref_bits_eq(n1k, n1)\n",
    "                    && ref_bits_eq(n2k, select(n2, select(n_o_hero, n_biax_a_hero, is_biaxial), ",
    "entering_anisotropic && !use_extraordinary))\n",
    "                    && ref_bits_eq(n_o_hoisted[k], n_o_hero)\n",
    "                    && (!(entering_anisotropic && use_extraordinary && !is_biaxial) ",
    "|| ref_bits_eq(idx.n_e_raw_ch[k], n_e_hero))\n",
    "                    && (!(entering_anisotropic && is_biaxial)\n",
    "                        || (ref_bits_eq(n_alpha_ch[k], n_alpha_ch[0])\n",
    "                            && ref_bits_eq(n_beta_ch[k], n_beta_ch[0])\n",
    "                            && ref_bits_eq(n_gamma_ch[k], n_gamma_ch[0]))));"
);

/// The uniaxial entry's exact match (`08d` and `ew_arm_uni_entry_transmit`).
const ENTRY_EXACT_MATCH: &str = concat!(
    "let direction_matches_entry = k == 0u\n",
    "                || (ref_bits_eq(n2k_dir, n2_hero_dir_e) && ref_bits_eq(n_o_k, n_o_hero)\n",
    "                    && (!use_extraordinary_entry || ref_bits_eq(n_e_raw_ch[k], n_e_hero)));"
);

/// The finalize hook, ahead of the spectral integration.
const FINALIZE_HOOK: &str = concat!(
    "ref_apply_kept(idx, lambdas, path_pdf, radiance);\n",
    "    var xyz = integrate_channels_to_xyz_family(*radiance, lambdas, path_pdf, compat);"
);

/// The event wavefront's signature update, ahead of the tail phase of a facet event.
const TAIL_SIG_UPDATE: &str = concat!(
    "ray_sig[i] = ref_sig_event(ray_sig[i], h.hit.facet_idx, h.pre_bounce_inside_gem, ",
    "s.inside_gem, s.is_extraordinary);\n",
    "    let r = bounce_tail(&s, &h, rc, ew.bounce, seed0);"
);

/// Every edit of the variant, in application order. See the module documentation.
const PATCHES: &[Patch] = &[
    Patch {
        what: "narrow_compat: companion pairs compare bitwise (compat is unused by the plain integration)",
        anchor: "matches_ab = directions_match(dirs[a], dirs[b]);",
        replacement: "matches_ab = all(dirs[a] == dirs[b]);",
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08e uniaxial internal exit: n_medium_ch[k] vs the hero's incident index",
        anchor: "let direction_matches_i = directions_match(refr_wave_dir_k, hero_refr_dir_i);",
        replacement: "let direction_matches_i = k == 0u || ref_bits_eq(n_inc_k, n1);",
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08e uniaxial exit mismatch: path_pdf = 0, not the transmit factor",
        anchor: "(*s).path_pdf[k] = prefix_path_pdf_k * t_unpol_k_mm;",
        replacement: "(*s).path_pdf[k] = 0.0;",
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08e uniaxial exit mismatch: no split staged",
        anchor: "if (original_stokes_i > 0.0) {",
        replacement: "if (false) {",
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08e generic arm: indices n1k, n2k, n_o, n_e (uniaxial) or principals (biaxial) vs the hero's",
        anchor: "let direction_matches = directions_match(final_dir_k, final_refr_dir);",
        replacement: GENERIC_EXACT_MATCH,
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08e generic mismatch: path_pdf = 0, not the transmit factor",
        anchor: "(*s).path_pdf[k] = prefix_path_pdf_k * (1.0 - ct.r_unpol_k);",
        replacement: "(*s).path_pdf[k] = 0.0;",
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08e generic exit mismatch: no split staged",
        anchor: "if (is_exit_event && original_stokes_k.x > 0.0) {",
        replacement: "if (false) {",
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08e: the split_mask write sites (two) set nothing",
        anchor: "(*h).split_mask = (*h).split_mask | (1u << k);",
        replacement: "(*h).split_mask = (*h).split_mask;",
        count: 2,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08d and ew_arm_uni_entry_transmit: uniaxial entry match by indices n2, n_o, n_e",
        anchor: "let direction_matches_entry = directions_match(final_dir_k, final_refr_dir_e);",
        replacement: ENTRY_EXACT_MATCH,
        count: 1,
        extra: Some((EwKernel::ArmUniEntryTransmit, 1)),
        scope: Scope::Both,
    },
    Patch {
        what: "08d and ew_arm_uni_entry_transmit: entry mismatch path_pdf = 0",
        anchor: "(*s).path_pdf[k] = (*s).path_pdf[k] * t_unpol_k_mm;",
        replacement: "(*s).path_pdf[k] = 0.0;",
        count: 1,
        extra: Some((EwKernel::ArmUniEntryTransmit, 1)),
        scope: Scope::Both,
    },
    Patch {
        what: "05_nee_env_sampling: the families integration is renamed; the plain one takes its name",
        anchor: "fn integrate_channels_to_xyz_family(",
        replacement: "fn integrate_channels_to_xyz_family_shipped(",
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "09_finalize: the kept hook before the spectral integration",
        anchor: "var xyz = integrate_channels_to_xyz_family(*radiance, lambdas, path_pdf, compat);",
        replacement: FINALIZE_HOOK,
        count: 1,
        extra: None,
        scope: Scope::Both,
    },
    Patch {
        what: "08f transport_bounce_step: the signature after the arm (megakernel)",
        anchor: "if (r == PHASE_NEXT) { r = bounce_run_arm(&s, &h, rc, bounce, seed0, observer); }",
        replacement: MEGAKERNEL_SIG_UPDATE,
        count: 1,
        extra: None,
        scope: Scope::Megakernel,
    },
    Patch {
        what: "ew_generate: the signature buffer starts at the seed",
        anchor: "ray_core[i].ray_meta.w = i;",
        replacement: "ray_core[i].ray_meta.w = i;\n    ray_sig[i] = BRANCH_SIG_SEED;",
        count: 1,
        extra: None,
        scope: Scope::Kernel(EwKernel::Generate),
    },
    Patch {
        what: "ew_tail: the signature at every facet event (all arms pass the tail)",
        anchor: "let r = bounce_tail(&s, &h, rc, ew.bounce, seed0);",
        replacement: TAIL_SIG_UPDATE,
        count: 1,
        extra: None,
        scope: Scope::Kernel(EwKernel::Tail),
    },
];

/// The event-wavefront kernels' signature buffer declaration and accessor.
fn ew_sig_decl() -> String {
    format!(
        "\n// Reference variant: the branch signature of every ray.\n@group(0) @binding({RAY_SIG_BINDING}) var<storage, read_write> ray_sig: array<vec2<u32>>;\n\nfn ref_sig_for(idx: u32) -> vec2<u32> {{\n    return ray_sig[idx];\n}}\n"
    )
}

/// `(what, anchor, expected count)` of every anchor `unit`'s variant checks, for the tests and for
/// reporting the patch set.
#[must_use]
pub fn anchors_of(unit: ReferenceUnit) -> Vec<(&'static str, &'static str, usize)> {
    PATCHES
        .iter()
        .filter(|patch| patch.scope.applies(unit))
        .map(|patch| (patch.what, patch.anchor, patch.expected(unit)))
        .collect()
}

/// `source` with the reference patches applied and further code appended.
///
/// `source` is the shipped generated text of `unit`. The result also has the
/// branch signature code, the shared helpers and the unit's signature storage appended. `None`
/// when any anchor does not occur exactly its expected number of times.
#[must_use]
pub fn reference_variant(unit: ReferenceUnit, source: &str) -> Option<String> {
    let mut text = source.to_owned();
    for patch in PATCHES.iter().filter(|patch| patch.scope.applies(unit)) {
        if text.matches(patch.anchor).count() != patch.expected(unit) {
            return None;
        }
        text = text.replace(patch.anchor, patch.replacement);
    }
    text.push('\n');
    text.push_str(BRANCH_SIG_SRC);
    text.push('\n');
    text.push_str(REFERENCE_PATCH_SRC);
    match unit {
        ReferenceUnit::Megakernel => text.push_str(MEGAKERNEL_SIG_DECL),
        ReferenceUnit::EventWavefront(_) => text.push_str(&ew_sig_decl()),
    }
    Some(text)
}

/// The `beam_kept` stub of `reference_patch.wgsl`, exactly as shipped there.
const BEAM_KEPT_STUB: &str = "fn beam_kept(pixel: u32, image_point: vec2<f32>, sig: vec2<u32>, lambda: f32) -> bool { return false; }";

/// The real `beam_kept`: the kept test of `kept.wgsl` on the real pixel's `(x, y)`.
const BEAM_KEPT_BODY: &str = concat!(
    "fn beam_kept(pixel: u32, image_point: vec2<f32>, sig: vec2<u32>, lambda: f32) -> bool {\n",
    "    let width = u32(camera.width);\n",
    "    return beam_kept_test(vec2<u32>(pixel % width, pixel / width), image_point, sig, lambda);\n",
    "}"
);

/// The pixel line of `ref_image_point` and `ref_apply_kept` (it occurs twice in
/// `reference_patch.wgsl`): the pixel of the dispatch-local tuple index.
const PATCH_PIXEL_LINE: &str = "let pixel = params.pixel_offset + idx / camera.num_samples;";

/// [`PATCH_PIXEL_LINE`] through the live pixel list: in a live variant the tuple index names a
/// virtual pixel, but the jitter and the kept test need the real one.
const PATCH_PIXEL_LIVE_LINE: &str =
    "let pixel = live_pixels[params.pixel_offset + idx / camera.num_samples];";

/// How often [`PATCH_PIXEL_LINE`] occurs in a reference variant (`ref_image_point`,
/// `ref_apply_kept`).
const PATCH_PIXEL_LINE_COUNT: usize = 2;

/// The storage buffers of the live reference megakernel: the megakernel's own, the live pixel list
/// and the kept buffers (the megakernel keeps `ref_sig` private and needs no signature buffer).
pub const REFERENCE_LIVE_STORAGE_BUFFERS: u32 =
    MEGAKERNEL_STORAGE_BUFFERS + 1 + KEPT_STORAGE_BUFFERS;

/// Whether an adapter allowing `available` storage buffers per shader stage can bind the live
/// reference megakernel (guide section 14.7 item 5); below that the batch's residual runs on the
/// CPU.
#[must_use]
pub const fn reference_live_fits(available: u32) -> bool {
    available >= REFERENCE_LIVE_STORAGE_BUFFERS
}

/// [`reference_variant`] with the real kept test: `beam_kept` calls `beam_kept_test` and the kept
/// unit (`kept_wgsl()`, bindings 210-213) is appended.
///
/// `None` when the reference variant is, or when the stub does not occur exactly once.
#[must_use]
pub fn reference_kept_variant(unit: ReferenceUnit, source: &str) -> Option<String> {
    let text = reference_variant(unit, source)?;
    if text.matches(BEAM_KEPT_STUB).count() != 1 {
        return None;
    }
    let mut text = text.replacen(BEAM_KEPT_STUB, BEAM_KEPT_BODY, 1);
    text.push('\n');
    text.push_str(kept_wgsl());
    Some(text)
}

/// The kept reference megakernel with its two pixel lines read through `live_pixels`, ready for
/// `live_source::live_variant` (which declares the list and swaps the generator's own lookup).
///
/// `None` when the kept variant is, or when the pixel line does not occur exactly
/// `PATCH_PIXEL_LINE_COUNT` times.
#[must_use]
pub fn reference_kept_megakernel_for_live() -> Option<String> {
    let text = reference_kept_variant(ReferenceUnit::Megakernel, SHADER_SRC)?;
    if text.matches(PATCH_PIXEL_LINE).count() != PATCH_PIXEL_LINE_COUNT {
        return None;
    }
    Some(text.replace(PATCH_PIXEL_LINE, PATCH_PIXEL_LIVE_LINE))
}

/// The reference variant of the megakernel (`transport_main`), built once per process.
///
/// `None` if any anchor drifted; callers decline.
#[must_use]
pub fn reference_megakernel_source() -> Option<String> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| reference_variant(ReferenceUnit::Megakernel, SHADER_SRC))
        .clone()
}

/// The reference variants of every event-wavefront kernel, `(entry point, source)` in
/// `EwKernel::ALL` order, built once per process.
///
/// `None` if any anchor of any kernel drifted; callers decline.
#[must_use]
pub fn reference_event_wavefront_sources() -> Option<Vec<(String, String)>> {
    static SOURCES: OnceLock<Option<Vec<(String, String)>>> = OnceLock::new();
    SOURCES
        .get_or_init(|| {
            EwKernel::ALL
                .iter()
                .map(|&kernel| {
                    reference_variant(ReferenceUnit::EventWavefront(kernel), kernel.source())
                        .map(|text| (kernel.entry_point().to_owned(), text))
                })
                .collect()
        })
        .clone()
}
