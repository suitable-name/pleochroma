//! Shader sources of the optics-table variants of the transport kernels.
//!
//! `optics_tables_variant` routes the Fresnel and absorption sites of a unit through the table
//! lookups and appends the lookups with their storage buffer at `OPTICS_TABLES_BINDING`.
//! A pipeline layout only includes the bindings an entry point reaches, and which entry points
//! reach a routed helper depends on the call graph. [`tables_entry_variant`] therefore also
//! makes the entry point read one table word as its first statement, so every entry point of
//! a table variant reaches the buffer. The host binds it exactly when the class carries
//! [`material_class::OPTICS_TABLES`] (see the `*_uses_tables` functions below). The read is a
//! single load of a word the kernel then ignores.
//!
//! The variants are built from the shipped source text at run time, so a class without the bit
//! compiles byte-identical sources. A unit whose routed statements moved has no variant
//! (`None`); the renderers then drop the bit from the class instead of compiling a plain kernel
//! under a table class, and the tests below fail on such a drift.

use std::sync::OnceLock;

use super::{SHADER_SRC, live_source::live_variant, specialisation::material_class};
use crate::{optics::tables::optics_tables_variant, renderer::gpu::event_wavefront::EwKernel};

/// The megakernel's entry point.
pub const MEGAKERNEL_ENTRY: &str = "transport_main";

/// The read that keeps the table binding in an entry point's layout.
const REACH_FUNCTION: &str = "
// Optics tables: one table word read by the first statement of the entry point, so the entry point
// reaches the table binding whatever helpers it calls.
fn ot_reach() -> f32 {
    return optics_tables[0];
}
";

/// The first statement of an entry point of a table variant.
const REACH_STATEMENT: &str = "\n    let ot_probe = ot_reach();";

/// `source` with the optics-table routing of `optics_tables_variant` and the table read in the
/// first statement of the entry point `entry`.
///
/// `None` when `source` lacks one of the routed statements (it has drifted), or does not define
/// `entry` exactly once.
#[must_use]
pub fn tables_entry_variant(source: &str, entry: &str) -> Option<String> {
    add_reach(optics_tables_variant(source)?, entry)
}

/// `text` with the table read added to the entry point `entry`.
fn add_reach(mut text: String, entry: &str) -> Option<String> {
    let needle = format!("fn {entry}(");
    if text.matches(&needle).count() != 1 {
        return None;
    }
    let start = text.find(&needle)?;
    let body = start + text[start..].find('{')?;
    text.insert_str(body + 1, REACH_STATEMENT);
    text.push_str(REACH_FUNCTION);
    Some(text)
}

/// The megakernel's table variant, built once; `None` when the shipped unit has drifted.
#[must_use]
pub fn megakernel_tables_source() -> Option<&'static str> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| tables_entry_variant(SHADER_SRC, MEGAKERNEL_ENTRY))
        .as_deref()
}

/// The megakernel's table variant over the live pixel list of an adaptive pass, built once.
#[must_use]
pub fn megakernel_live_tables_source() -> Option<&'static str> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| megakernel_tables_source().and_then(live_variant))
        .as_deref()
}

/// The table variant of an event wavefront kernel, built once per kernel.
#[must_use]
pub fn event_wavefront_tables_source(kernel: EwKernel) -> Option<&'static str> {
    static SOURCES: [OnceLock<Option<String>>; EwKernel::ALL.len()] =
        [const { OnceLock::new() }; EwKernel::ALL.len()];
    SOURCES
        .get(kernel as usize)?
        .get_or_init(|| tables_entry_variant(kernel.source(), kernel.entry_point()))
        .as_deref()
}

/// The table variant of `ew_generate` over the live pixel list of an adaptive pass, built once.
#[must_use]
pub fn event_wavefront_live_generate_tables_source() -> Option<&'static str> {
    static SOURCE: OnceLock<Option<String>> = OnceLock::new();
    SOURCE
        .get_or_init(|| event_wavefront_tables_source(EwKernel::Generate).and_then(live_variant))
        .as_deref()
}

/// Whether the megakernel of `class` is the table variant (and so binds the table buffer).
#[must_use]
pub fn megakernel_uses_tables(class: u32) -> bool {
    material_class::uses_optics_tables(class) && megakernel_tables_source().is_some()
}

/// The megakernel source `class` compiles: the shipped unit, or its table variant.
#[must_use]
pub fn megakernel_source_for(class: u32) -> &'static str {
    if megakernel_uses_tables(class) {
        megakernel_tables_source().unwrap_or(SHADER_SRC)
    } else {
        SHADER_SRC
    }
}

/// The live (adaptive pass) megakernel source `class` compiles; `None` when the shipped unit no
/// longer holds the pixel lookup line.
#[must_use]
pub fn megakernel_live_source_for(class: u32) -> Option<&'static str> {
    if megakernel_uses_tables(class) {
        megakernel_live_tables_source()
    } else {
        super::live_source::megakernel_live_source()
    }
}

/// Whether `kernel` of `class` is the table variant (and so binds the table buffer).
#[must_use]
pub fn event_wavefront_uses_tables(kernel: EwKernel, class: u32) -> bool {
    material_class::uses_optics_tables(class) && event_wavefront_tables_source(kernel).is_some()
}

/// The source `kernel` compiles under `class`: the shipped unit, or its table variant.
#[must_use]
pub fn event_wavefront_source_for(kernel: EwKernel, class: u32) -> &'static str {
    if event_wavefront_uses_tables(kernel, class) {
        event_wavefront_tables_source(kernel).unwrap_or_else(|| kernel.source())
    } else {
        kernel.source()
    }
}

/// The live `ew_generate` source `class` compiles; `None` when the shipped unit lacks the
/// pixel lookup line.
#[must_use]
pub fn event_wavefront_live_generate_source_for(class: u32) -> Option<&'static str> {
    if event_wavefront_uses_tables(EwKernel::Generate, class) {
        event_wavefront_live_generate_tables_source()
    } else {
        super::live_source::event_wavefront_live_generate_source()
    }
}

#[cfg(test)]
mod tests {
    use naga::{
        front::wgsl,
        valid::{Capabilities, ValidationFlags, Validator},
    };

    use super::{
        MEGAKERNEL_ENTRY, event_wavefront_live_generate_source_for,
        event_wavefront_live_generate_tables_source, event_wavefront_source_for,
        event_wavefront_tables_source, event_wavefront_uses_tables, megakernel_live_source_for,
        megakernel_live_tables_source, megakernel_source_for, megakernel_tables_source,
        megakernel_uses_tables, tables_entry_variant,
    };
    use crate::{
        optics::tables::OPTICS_TABLES_BINDING,
        renderer::gpu::{
            event_wavefront::EwKernel,
            frame::{SHADER_SRC, material_class},
        },
    };

    /// The `@group(0)` bindings `entry` of `source` reaches, ascending, as naga sees them.
    fn reached_bindings(source: &str, entry: &str) -> Vec<u32> {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{entry} does not parse: {}", e.emit_to_string(source)));
        let info = Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{entry} does not validate: {}", e.emit_to_string(source)));
        let index = module
            .entry_points
            .iter()
            .position(|ep| ep.name == entry)
            .expect("the entry point exists");
        let uses = info.get_entry_point(index);
        let mut reached: Vec<u32> = module
            .global_variables
            .iter()
            .filter(|(handle, _)| !uses[*handle].is_empty())
            .filter_map(|(_, var)| var.binding.as_ref())
            .inspect(|binding| assert_eq!(binding.group, 0))
            .map(|binding| binding.binding)
            .collect();
        reached.sort_unstable();
        reached
    }

    #[test]
    fn the_shipped_units_have_table_variants() {
        assert!(megakernel_tables_source().is_some());
        assert!(megakernel_live_tables_source().is_some());
        assert!(event_wavefront_live_generate_tables_source().is_some());
        for kernel in EwKernel::ALL {
            assert!(
                event_wavefront_tables_source(kernel).is_some(),
                "{kernel:?} has no table variant"
            );
        }
    }

    #[test]
    fn a_class_without_the_bit_compiles_the_shipped_sources_unchanged() {
        for class in [
            material_class::GENERIC,
            material_class::ISOTROPIC,
            material_class::UNIAXIAL | material_class::NO_ORIENTED,
            material_class::BIAXIAL
                | (material_class::FEATURE_MASK
                    & !material_class::OPTICS_TABLES
                    & !material_class::TRAY_SHADOW
                    & !material_class::TRAY_MIRROR),
        ] {
            assert!(!megakernel_uses_tables(class));
            // Compares the text, not the address: equal constants may live in several codegen
            // units. `assert!` rather than `assert_eq!`, so a failure does not print the unit.
            assert!(
                megakernel_source_for(class) == SHADER_SRC,
                "class {class:#x}: the megakernel source is not the shipped unit"
            );
            for kernel in EwKernel::ALL {
                assert!(!event_wavefront_uses_tables(kernel, class));
                assert!(
                    event_wavefront_source_for(kernel, class) == kernel.source(),
                    "class {class:#x}: {kernel:?} is not the shipped unit"
                );
            }
        }
    }

    #[test]
    fn a_class_with_the_bit_compiles_the_table_variants() {
        let class = material_class::ISOTROPIC | material_class::OPTICS_TABLES;
        assert!(megakernel_uses_tables(class));
        let source = megakernel_source_for(class);
        assert_ne!(source, SHADER_SRC);
        assert!(source.contains("ot_amplitudes(") && source.contains("fn ot_reach()"));
        assert!(
            megakernel_live_source_for(class)
                .is_some_and(|live| live.contains("live_pixels") && live.contains("fn ot_reach()"))
        );
        assert!(
            event_wavefront_live_generate_source_for(class)
                .is_some_and(|live| live.contains("live_pixels") && live.contains("ot_reach"))
        );
        for kernel in EwKernel::ALL {
            assert!(event_wavefront_uses_tables(kernel, class));
            assert!(event_wavefront_source_for(kernel, class).contains("fn ot_reach()"));
        }
    }

    #[test]
    fn a_unit_without_the_routed_sites_or_the_entry_has_no_variant() {
        let unit = "fn helper() -> f32 { return 1.0; }\n\
                    @compute @workgroup_size(1)\nfn kernel_main(\n    @builtin(global_invocation_id) gid: vec3<u32>,\n) {\n    helper();\n}\n";
        assert!(
            tables_entry_variant(unit, "kernel_main").is_none(),
            "a unit without the routed sites has no variant"
        );
        assert!(tables_entry_variant(SHADER_SRC, "no_such_entry").is_none());
        assert!(tables_entry_variant(SHADER_SRC, "transport_").is_none());
    }

    #[test]
    fn every_megakernel_and_wavefront_variant_reaches_the_table_binding() {
        let source = megakernel_tables_source().expect("variant");
        let plain = reached_bindings(SHADER_SRC, MEGAKERNEL_ENTRY);
        let tabled = reached_bindings(source, MEGAKERNEL_ENTRY);
        assert!(!plain.contains(&OPTICS_TABLES_BINDING));
        let mut expected = plain;
        expected.push(OPTICS_TABLES_BINDING);
        assert_eq!(tabled, expected);

        let live = megakernel_live_tables_source().expect("live variant");
        assert!(reached_bindings(live, MEGAKERNEL_ENTRY).contains(&OPTICS_TABLES_BINDING));

        for kernel in EwKernel::ALL {
            let plain = reached_bindings(kernel.source(), kernel.entry_point());
            let tabled = reached_bindings(
                event_wavefront_tables_source(kernel).expect("variant"),
                kernel.entry_point(),
            );
            let mut expected = plain;
            expected.push(OPTICS_TABLES_BINDING);
            assert_eq!(tabled, expected, "{kernel:?}");
        }
        let live = event_wavefront_live_generate_tables_source().expect("live variant");
        assert!(reached_bindings(live, "ew_generate").contains(&OPTICS_TABLES_BINDING));
    }
}
