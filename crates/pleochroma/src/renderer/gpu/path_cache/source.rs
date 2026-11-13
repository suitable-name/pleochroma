//! The path cache's two shader units.
//!
//! WGSL has no include, so both units are built from the shipped megakernel text, as the adaptive
//! live variant patches it (`gpu::frame::live_source`):
//! - the phase B unit: the megakernel plus `shaders/path_cache/phase_b.wgsl`. The appended text
//!   only adds declarations and one entry point, so the megakernel's entry points are untouched;
//! - the completion unit: the megakernel with seven counted one-line edits that make
//!   `transport_main` trace the fired samples of phase B's list, then `phase_b.wgsl`,
//!   `shaders/beam/branch_sig.wgsl` and `shaders/path_cache/complete.wgsl` appended.
//!
//! Any anchor count other than one returns `None`: a drifted megakernel declines (the host then
//! completes the fired samples) instead of being mis-patched. The shipped unit and the pipelines
//! of ordinary passes are unchanged.

use std::sync::OnceLock;

use crate::renderer::gpu::{MEGAKERNEL_STORAGE_BUFFERS, frame::SHADER_SRC};

/// `shaders/path_cache/phase_b.wgsl`: the kernel appended to the megakernel unit.
pub const PHASE_B_SHADER_SRC: &str = include_str!("../../shaders/path_cache/phase_b.wgsl");

/// `shaders/path_cache/complete.wgsl`: the cover test and the hooks the completion edits call.
pub const COMPLETE_SHADER_SRC: &str = include_str!("../../shaders/path_cache/complete.wgsl");

/// `shaders/beam/branch_sig.wgsl`: `branch_step`, the branch signature the cover test folds.
///
/// This module's own constant, because the path cache builds without the `beam` feature.
pub const BRANCH_SIG_SHADER_SRC: &str = include_str!("../../shaders/beam/branch_sig.wgsl");

/// The completion entry point: the megakernel's own `transport_main`, patched.
pub const COMPLETE_ENTRY: &str = "transport_main";

/// The bindings the completion entry point of the shipped base reaches, ascending.
///
/// Every binding of the megakernel (0 to 14) and phase B's own (180 to 184). The optics-table base
/// adds `OPTICS_TABLES_BINDING` (170).
pub const COMPLETE_BINDINGS: [u32; 20] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 180, 181, 182, 183, 184,
];

/// Storage buffers the completion entry point binds on the shipped base.
///
/// The megakernel's, then the slab words, the tile offsets, the pixel list and the fired list. The
/// optics-table base binds one more.
pub const COMPLETE_STORAGE_BUFFERS: u32 = MEGAKERNEL_STORAGE_BUFFERS + 4;

/// Uniform buffers the completion entry point binds.
///
/// The camera, the transport parameters, the dimensions of the HDR map and of its distribution,
/// and phase B's pass parameters.
pub const COMPLETE_UNIFORM_BUFFERS: u32 = 5;

/// One counted edit of the completion variant.
struct CompletionPatch {
    /// What the edit does, for the tests.
    what: &'static str,
    /// The exact one-line text replaced; it must occur exactly once in the base.
    anchor: &'static str,
    /// The replacement.
    replacement: &'static str,
}

/// The edits of the completion variant, in application order.
const COMPLETION_PATCHES: [CompletionPatch; 7] = [
    CompletionPatch {
        what: "transport_main: the dispatch covers the fired entries of the chunk",
        anchor: "let total = params.num_pixels * camera.num_samples;",
        replacement: "let total = pcc_fired_total();",
    },
    CompletionPatch {
        what: "transport_generate_ray: the real pixel of the fired entry",
        anchor: "let pixel = idx / camera.num_samples + params.pixel_offset;",
        replacement: "let pixel = pcc_pixel(idx);",
    },
    CompletionPatch {
        what: "transport_generate_ray: the absolute sample of the fired entry (seed and hero)",
        anchor: "let sample_num = local_sample + params.sample_offset;",
        replacement: "let sample_num = pcc_sample(idx);",
    },
    CompletionPatch {
        what: "transport_generate_ray: the jitter in x of the position's sample index",
        anchor: "let jx = cranley_patterson_rotate(low_discrepancy_base2(sample_num), rot_jx) - 0.5;",
        replacement: "let jx = cranley_patterson_rotate(low_discrepancy_base2(pcc_jitter_index(idx)), rot_jx) - 0.5;",
    },
    CompletionPatch {
        what: "transport_generate_ray: the jitter in y of the position's sample index",
        anchor: "let jy = cranley_patterson_rotate(radical_inverse_base(sample_num, 3u), rot_jy) - 0.5;",
        replacement: "let jy = cranley_patterson_rotate(radical_inverse_base(pcc_jitter_index(idx), 3u), rot_jy) - 0.5;",
    },
    CompletionPatch {
        what: "transport_bounce_step: the branch signature of every facet event (PathEvents::facet)",
        anchor: "if (r == PHASE_NEXT) { r = bounce_run_arm(&s, &h, rc, bounce, seed0, observer); }",
        replacement: "if (r == PHASE_NEXT) { r = bounce_run_arm(&s, &h, rc, bounce, seed0, observer); pcc_facet(h.hit.facet_idx, h.pre_bounce_inside_gem, s.inside_gem, s.is_extraordinary); }",
    },
    CompletionPatch {
        what: "transport_main: the cover test and the add into the fired sample's tuple slot",
        anchor: "transport_finalize_ray(idx, lambdas, &radiance, split_radiance, path_pdf, compat, path_escaped, nee_xyz);",
        replacement: "pcc_finalize(idx, lambdas, &radiance, split_radiance, path_pdf, compat, path_escaped, nee_xyz, current_dir);",
    },
];

/// `base` with the seven completion edits, or `None` when an anchor does not occur exactly once.
fn patched_megakernel(base: &str) -> Option<String> {
    let mut text = base.to_owned();
    for patch in &COMPLETION_PATCHES {
        let count = text.matches(patch.anchor).count();
        if count != 1 {
            tracing::warn!(
                edit = patch.what,
                count,
                "path cache completion: an anchor does not occur exactly once"
            );
            return None;
        }
        text = text.replacen(patch.anchor, patch.replacement, 1);
    }
    Some(text)
}

/// The completion unit of `base`: the shipped megakernel unit, or its optics-table variant.
///
/// `base` with the seven edits, then `phase_b.wgsl`, `branch_sig.wgsl` and `complete.wgsl`
/// appended. `None` when an anchor of `base` does not occur exactly once.
#[must_use]
pub fn completion_variant(base: &str) -> Option<String> {
    let mut text = patched_megakernel(base)?;
    text.push('\n');
    text.push_str(PHASE_B_SHADER_SRC);
    text.push('\n');
    text.push_str(BRANCH_SIG_SHADER_SRC);
    text.push('\n');
    text.push_str(COMPLETE_SHADER_SRC);
    Some(text)
}

/// The phase B entry point.
pub const PHASE_B_ENTRY: &str = "pcb_phase_b_main";

/// The bindings the phase B entry point reads or writes, ascending.
///
/// They are the megakernel's camera (0), transport parameters (1), material (2, the absorption
/// bands), per-tuple XYZ output (4) and HDR map (10, 11), then its own slab words (180), tile
/// offsets (181), pixel list (182), pass parameters (183) and fired list (184).
pub const PHASE_B_BINDINGS: [u32; 11] = [0, 1, 2, 4, 10, 11, 180, 181, 182, 183, 184];

/// The megakernel unit with the phase B kernel appended, built once.
#[must_use]
pub fn phase_b_source() -> &'static str {
    static SOURCE: OnceLock<String> = OnceLock::new();
    SOURCE.get_or_init(|| format!("{SHADER_SRC}\n{PHASE_B_SHADER_SRC}"))
}

#[cfg(test)]
mod tests {
    use naga::{
        TypeInner,
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    use super::{
        COMPLETE_BINDINGS, COMPLETE_ENTRY, COMPLETE_SHADER_SRC, COMPLETE_STORAGE_BUFFERS,
        COMPLETE_UNIFORM_BUFFERS, COMPLETION_PATCHES, PHASE_B_BINDINGS, PHASE_B_ENTRY,
        PHASE_B_SHADER_SRC, completion_variant, patched_megakernel, phase_b_source,
    };
    use crate::renderer::{
        gpu::frame::SHADER_SRC,
        path_cache::{record, shade},
    };

    /// The bindings an entry point reaches, sorted, and its storage and uniform global counts.
    fn reached(source: &str, entry: &str) -> (Vec<u32>, u32, u32) {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("unit does not parse: {}", e.emit_to_string(source)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("unit does not validate: {}", e.emit_to_string(source)));
        let index = module
            .entry_points
            .iter()
            .position(|e| e.name == entry)
            .expect("entry point exists");
        let uses = info.get_entry_point(index);
        let mut bindings = Vec::new();
        let (mut storage, mut uniform) = (0_u32, 0_u32);
        for (handle, var) in module.global_variables.iter() {
            if uses[handle].is_empty() {
                continue;
            }
            match var.space {
                naga::AddressSpace::Storage { .. } => storage += 1,
                naga::AddressSpace::Uniform => uniform += 1,
                _ => {}
            }
            if let Some(binding) = var.binding.as_ref() {
                bindings.push(binding.binding);
            }
        }
        bindings.sort_unstable();
        (bindings, storage, uniform)
    }

    /// The literal of `const NAME: TYPE = LITERAL;` in a WGSL text.
    fn const_literal(source: &str, name: &str) -> String {
        let head = format!("const {name}:");
        let line = source
            .lines()
            .find(|l| l.starts_with(&head))
            .unwrap_or_else(|| panic!("const {name} not found"));
        let (_, value) = line.split_once('=').expect("const has a value");
        value.trim().trim_end_matches(';').trim().to_owned()
    }

    fn validated() -> (naga::Module, naga::valid::ModuleInfo) {
        let source = phase_b_source();
        let module = wgsl::parse_str(source).unwrap_or_else(|e| {
            panic!("phase B unit does not parse: {}", e.emit_to_string(source))
        });
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| {
                panic!(
                    "phase B unit does not validate: {}",
                    e.emit_to_string(source)
                )
            });
        (module, info)
    }

    #[test]
    fn the_phase_b_unit_is_the_shipped_megakernel_plus_the_kernel() {
        assert!(phase_b_source().starts_with(SHADER_SRC));
        assert!(phase_b_source().contains(PHASE_B_ENTRY));
        assert_eq!(SHADER_SRC.matches(PHASE_B_ENTRY).count(), 0);
    }

    #[test]
    fn the_phase_b_unit_validates_and_has_its_entry_point() {
        let (module, _) = validated();
        assert!(module.entry_points.iter().any(|e| e.name == PHASE_B_ENTRY));
        // The megakernel's own entry point is still there, unchanged.
        assert!(
            module
                .entry_points
                .iter()
                .any(|e| e.name == "transport_main")
        );
    }

    #[test]
    fn the_entry_point_reaches_exactly_the_bindings_the_pass_binds() {
        let (module, info) = validated();
        let index = module
            .entry_points
            .iter()
            .position(|e| e.name == PHASE_B_ENTRY)
            .expect("entry point exists");
        let uses = info.get_entry_point(index);
        let mut reached: Vec<u32> = module
            .global_variables
            .iter()
            .filter(|(handle, _)| !uses[*handle].is_empty())
            .filter_map(|(_, var)| var.binding.as_ref().map(|b| b.binding))
            .collect();
        reached.sort_unstable();
        assert_eq!(reached, PHASE_B_BINDINGS.to_vec());
    }

    #[test]
    fn the_own_bindings_are_used_by_no_shipped_unit() {
        for binding in 180..=184_u32 {
            let needle = format!("@binding({binding})");
            assert_eq!(
                SHADER_SRC.matches(&needle).count(),
                0,
                "binding {binding} is already taken in the megakernel unit"
            );
        }
    }

    #[test]
    fn the_pass_parameters_have_the_size_the_host_struct_has() {
        let (module, _) = validated();
        let span = module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(name), TypeInner::Struct { span, .. }) if name == "PcbParams" => Some(*span),
                _ => None,
            });
        assert_eq!(span, Some(32));
    }

    #[test]
    fn every_completion_anchor_occurs_once_in_the_shipped_unit_and_is_replaced() {
        let patched = patched_megakernel(SHADER_SRC).expect("the shipped unit patches");
        for patch in &COMPLETION_PATCHES {
            assert_eq!(
                SHADER_SRC.matches(patch.anchor).count(),
                1,
                "{}",
                patch.what
            );
            assert_eq!(patched.matches(patch.anchor).count(), 0, "{}", patch.what);
            assert_eq!(
                patched.matches(patch.replacement).count(),
                1,
                "{}",
                patch.what
            );
        }
        let unit = completion_variant(SHADER_SRC).expect("the shipped unit builds");
        assert!(unit.starts_with(&patched));
        assert!(unit.ends_with(COMPLETE_SHADER_SRC));
    }

    #[test]
    fn a_drifted_unit_has_no_completion_variant() {
        let anchor = COMPLETION_PATCHES[0].anchor;
        let removed = SHADER_SRC.replacen(anchor, "", 1);
        assert!(completion_variant(&removed).is_none());
        let doubled = format!("{SHADER_SRC}\n{anchor}\n");
        assert!(completion_variant(&doubled).is_none());
        assert!(completion_variant("fn nothing() {}").is_none());
    }

    #[test]
    fn the_completion_unit_validates_and_reaches_exactly_the_bindings_the_pass_binds() {
        let unit = completion_variant(SHADER_SRC).expect("the shipped unit builds");
        let (bindings, storage, uniform) = reached(&unit, COMPLETE_ENTRY);
        assert_eq!(bindings, COMPLETE_BINDINGS.to_vec());
        assert_eq!(storage, COMPLETE_STORAGE_BUFFERS);
        assert_eq!(uniform, COMPLETE_UNIFORM_BUFFERS);
        assert!(unit.contains(PHASE_B_ENTRY));
        let module = wgsl::parse_str(&unit).expect("parses");
        assert!(module.entry_points.iter().any(|e| e.name == PHASE_B_ENTRY));
    }

    #[test]
    fn the_completion_constants_are_the_cpu_twins() {
        assert_eq!(
            const_literal(PHASE_B_SHADER_SRC, "PCB_BOUNCE_SHIFT"),
            format!("{}u", record::BOUNCE_SHIFT)
        );
        let gain: f32 = const_literal(PHASE_B_SHADER_SRC, "PCB_RESIDUAL_FIRE_GAIN")
            .parse()
            .expect("an f32 literal");
        assert_eq!(gain.to_bits(), shade::RESIDUAL_FIRE_GAIN.to_bits());
        let floor: f32 = const_literal(PHASE_B_SHADER_SRC, "PCB_RESIDUAL_FLOOR")
            .parse()
            .expect("an f32 literal");
        assert_eq!(floor.to_bits(), shade::RESIDUAL_FLOOR.to_bits());
    }

    #[test]
    fn the_record_layout_constants_are_the_cpu_twins() {
        assert_eq!(
            const_literal(PHASE_B_SHADER_SRC, "PCB_RECORD_WORDS"),
            format!("{}u", crate::renderer::path_cache::layout::RECORD_WORDS)
        );
        // The depth coefficients follow the signature word.
        assert_eq!(
            const_literal(PHASE_B_SHADER_SRC, "PCB_DEPTH_WORD"),
            format!(
                "{}u",
                std::mem::offset_of!(record::ExitRecord, depth_bits) / 4
            )
        );
        assert_eq!(std::mem::offset_of!(record::ExitRecord, flags), 24);
        assert_eq!(std::mem::offset_of!(record::ExitRecord, signature), 28);
    }

    #[test]
    fn the_completion_unit_has_exactly_one_branch_step() {
        let unit = completion_variant(SHADER_SRC).expect("the shipped unit builds");
        assert_eq!(unit.matches("fn branch_step(").count(), 1);
    }

    #[test]
    fn the_new_wgsl_has_no_function_scope_array_and_initialises_every_var() {
        for source in [PHASE_B_SHADER_SRC, COMPLETE_SHADER_SRC] {
            for line in source.lines() {
                let line = line.trim_start();
                if line.starts_with("var ") {
                    assert!(!line.contains("array<"), "function-scope array: {line}");
                    assert!(line.contains('='), "uninitialised var: {line}");
                }
            }
        }
    }
}
