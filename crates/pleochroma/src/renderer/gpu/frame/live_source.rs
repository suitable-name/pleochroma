//! Shader sources of the adaptive (live-tile) passes.
//!
//! An adaptive pass traces only the pixels of the live tiles. The transport kernels already
//! draw every ray from `(pixel, sample)` alone, and they get the pixel from ONE line of
//! `transport_generate_ray` (`transport_bounce/09_finalize_and_ray_gen.wgsl`):
//! `idx / camera.num_samples + params.pixel_offset`. A live pass therefore compiles the
//! UNCHANGED unit with that line replaced by a lookup in a storage array of real pixel
//! indices ([`LIVE_PIXELS_BINDING`]): the dispatch then covers the "virtual pixels" `0..n` of
//! the live list, every kernel that calls `transport_generate_ray` (the megakernel's
//! `transport_main`, the event wavefront's `ew_generate`) draws the same stratified jitter,
//! wavelengths and seeds it would for that real pixel in an ordinary pass, and nothing else
//! in the unit changes.
//!
//! The variant is built from the shipped source text at run time, so the generated units, their
//! hashes and the pipelines of ordinary passes stay byte-identical (guide section 2.3: off
//! means identical). [`live_variant`] refuses a source whose lookup line moved, and the tests
//! below fail on that, so a drift cannot silently turn the live pass into a plain one.

use std::sync::OnceLock;

use super::SHADER_SRC;

/// `@binding` of the live pixel list (`array<u32>`, real pixel indices) in a live variant.
///
/// 140 is used by no shader unit in the shipped sources.
pub const LIVE_PIXELS_BINDING: u32 = 140;

/// The pixel lookup line of `transport_generate_ray`, exactly as shipped.
const PIXEL_LOOKUP: &str = "let pixel = idx / camera.num_samples + params.pixel_offset;";

/// The same line reading the real pixel from the live list.
const LIVE_PIXEL_LOOKUP: &str =
    "let pixel = live_pixels[idx / camera.num_samples + params.pixel_offset];";

/// The declaration appended to a live variant (WGSL module scope is order-independent).
const LIVE_PIXELS_DECL: &str = "\n// Adaptive pass: the real pixel index of every live \"virtual\" pixel.\n@group(0) @binding(140) var<storage, read> live_pixels: array<u32>;\n";

/// `reduce_xyz_live.wgsl`: the per-pixel sample sum plus the odd-sample half-buffer sum.
pub const LIVE_REDUCE_SHADER_SRC: &str = include_str!("../../shaders/reduce_xyz_live.wgsl");

/// The entry point of [`LIVE_REDUCE_SHADER_SRC`].
pub const LIVE_REDUCE_ENTRY: &str = "reduce_xyz_live_main";

/// `source` with its pixel lookup replaced by the live-list lookup and the list's binding
/// declared, or `None` when `source` does not hold the lookup line exactly once.
#[must_use]
pub fn live_variant(source: &str) -> Option<String> {
    if source.matches(PIXEL_LOOKUP).count() != 1 {
        return None;
    }
    let mut text = source.replacen(PIXEL_LOOKUP, LIVE_PIXEL_LOOKUP, 1);
    text.push_str(LIVE_PIXELS_DECL);
    Some(text)
}

/// The megakernel unit's live variant (`transport_main` over the live list), built once.
///
/// `None` only if the shipped unit no longer holds the lookup line, which
/// `the_shipped_units_hold_the_pixel_lookup_exactly_once` rejects.
#[must_use]
pub fn megakernel_live_source() -> Option<&'static str> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE.get_or_init(|| live_variant(SHADER_SRC)).as_deref()
}

/// The reference megakernel's live variant (beam residual passes): the megakernel patched with the
/// exact-match reference edits, the signature lanes and the kept test, over the live pixel list,
/// built once. `None` when any anchor of the reference variant or the lookup line drifted.
#[cfg(feature = "beam")]
#[must_use]
pub fn reference_live_megakernel_source() -> Option<&'static str> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| {
            crate::renderer::gpu::beam::reference_source::reference_kept_megakernel_for_live()
                .and_then(|text| live_variant(&text))
        })
        .as_deref()
}

/// The `ew_generate` unit's live variant of the event wavefront, built once.
#[must_use]
pub fn event_wavefront_live_generate_source() -> Option<&'static str> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| {
            live_variant(crate::renderer::gpu::event_wavefront::EwKernel::Generate.source())
        })
        .as_deref()
}

#[cfg(test)]
mod tests {
    use naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    use super::{
        LIVE_PIXEL_LOOKUP, LIVE_PIXELS_BINDING, LIVE_PIXELS_DECL, LIVE_REDUCE_ENTRY,
        LIVE_REDUCE_SHADER_SRC, PIXEL_LOOKUP, event_wavefront_live_generate_source, live_variant,
        megakernel_live_source,
    };
    use crate::renderer::gpu::{event_wavefront::EwKernel, frame::SHADER_SRC};

    fn validate(name: &str, source: &str) -> naga::Module {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{name} does not parse: {}", e.emit_to_string(source)));
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name} does not validate: {}", e.emit_to_string(source)));
        module
    }

    #[test]
    fn the_shipped_units_hold_the_pixel_lookup_exactly_once() {
        assert_eq!(SHADER_SRC.matches(PIXEL_LOOKUP).count(), 1);
        assert_eq!(EwKernel::Generate.source().matches(PIXEL_LOOKUP).count(), 1);
        assert!(megakernel_live_source().is_some());
        assert!(event_wavefront_live_generate_source().is_some());
    }

    #[test]
    fn a_source_without_exactly_one_lookup_has_no_live_variant() {
        assert!(live_variant("fn nothing() {}").is_none());
        let twice = format!("{PIXEL_LOOKUP}\n{PIXEL_LOOKUP}");
        assert!(live_variant(&twice).is_none());
    }

    #[test]
    fn the_live_variant_changes_only_the_lookup_and_adds_the_list() {
        let live = megakernel_live_source().expect("the megakernel unit holds the lookup");
        assert!(live.contains(LIVE_PIXEL_LOOKUP));
        assert!(!live.contains(PIXEL_LOOKUP));
        let (base_head, base_tail) = SHADER_SRC.split_once(PIXEL_LOOKUP).expect("lookup present");
        let expected = format!("{base_head}{LIVE_PIXEL_LOOKUP}{base_tail}{LIVE_PIXELS_DECL}");
        assert_eq!(live, expected);
    }

    #[test]
    fn the_live_megakernel_reaches_the_list_and_the_plain_one_does_not() {
        let reached = |source: &str, entry: &str| -> Vec<u32> {
            let module = validate("transport unit", source);
            let info = Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .expect("validated above");
            let index = module
                .entry_points
                .iter()
                .position(|ep| ep.name == entry)
                .expect("entry point exists");
            let uses = info.get_entry_point(index);
            module
                .global_variables
                .iter()
                .filter(|(handle, _)| !uses[*handle].is_empty())
                .filter_map(|(_, var)| var.binding.as_ref().map(|b| b.binding))
                .collect()
        };
        let plain = reached(SHADER_SRC, "transport_main");
        let live = reached(
            megakernel_live_source().expect("live source"),
            "transport_main",
        );
        assert!(!plain.contains(&LIVE_PIXELS_BINDING));
        assert!(live.contains(&LIVE_PIXELS_BINDING));
        let mut without = live;
        without.retain(|binding| *binding != LIVE_PIXELS_BINDING);
        let mut plain_sorted = plain;
        plain_sorted.sort_unstable();
        without.sort_unstable();
        assert_eq!(without, plain_sorted, "only the list binding is added");
    }

    #[cfg(all(feature = "beam", beam_tests))]
    #[test]
    fn the_live_reference_megakernel_reaches_the_list_and_the_kept_buffers() {
        use super::reference_live_megakernel_source;

        let source = reference_live_megakernel_source().expect("the reference live source");
        assert!(source.contains(LIVE_PIXEL_LOOKUP));
        assert!(!source.contains(PIXEL_LOOKUP));
        let module = validate("reference live megakernel", source);
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("validated above");
        let index = module
            .entry_points
            .iter()
            .position(|ep| ep.name == "transport_main")
            .expect("entry point exists");
        let uses = info.get_entry_point(index);
        let reached: Vec<u32> = module
            .global_variables
            .iter()
            .filter(|(handle, _)| !uses[*handle].is_empty())
            .filter_map(|(_, var)| var.binding.as_ref().map(|b| b.binding))
            .collect();
        for binding in [LIVE_PIXELS_BINDING, 210, 211, 212, 213] {
            assert!(reached.contains(&binding), "binding {binding} not reached");
        }
    }

    #[test]
    fn the_live_event_wavefront_generate_reaches_the_list() {
        let source = event_wavefront_live_generate_source().expect("live source");
        let module = validate("ew_generate live", source);
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .expect("validated above");
        let index = module
            .entry_points
            .iter()
            .position(|ep| ep.name == EwKernel::Generate.entry_point())
            .expect("entry point exists");
        let uses = info.get_entry_point(index);
        let reaches_list = module.global_variables.iter().any(|(handle, var)| {
            !uses[handle].is_empty()
                && var.binding.as_ref().map(|b| b.binding) == Some(LIVE_PIXELS_BINDING)
        });
        assert!(reaches_list);
    }

    #[test]
    fn the_live_reduce_kernel_validates_and_has_its_entry_point() {
        let module = validate("reduce_xyz_live.wgsl", LIVE_REDUCE_SHADER_SRC);
        assert!(
            module
                .entry_points
                .iter()
                .any(|ep| ep.name == LIVE_REDUCE_ENTRY)
        );
    }
}
