//! Adapter-free checks of the sorted wavefront's generated units (`renderer::gpu::frame::sorted`).
//!
//! Both parse and validate with `naga`. The `sorted_*.wgsl` sources initialise every `var` where
//! it is declared, and their one function-scope array is the radiance of `sw_finalize`, which
//! `transport_finalize_ray` takes by pointer. The bindings each entry point reaches are compared
//! with the host's table in `frame::sorted_tests`, which needs the `gpu` feature.

use naga::{
    front::wgsl,
    valid::{Capabilities, ValidationFlags, Validator},
};

/// The transport unit: the megakernel's prelude and bounce text, the event wavefront's ray
/// records and surface phase, the queue layout and the kernels.
const TRANSPORT: &str = include_str!(concat!(env!("OUT_DIR"), "/sorted_wavefront.generated.wgsl"));

/// The prepare unit: the queue layout and the two one-thread kernels.
const PREPARE: &str = include_str!(concat!(env!("OUT_DIR"), "/sorted_prepare.generated.wgsl"));

/// The sources as written, by name.
const SOURCES: [(&str, &str); 3] = [
    (
        "sorted_queues.wgsl",
        include_str!("../shaders/sorted_queues.wgsl"),
    ),
    (
        "sorted_prepare.wgsl",
        include_str!("../shaders/sorted_prepare.wgsl"),
    ),
    (
        "sorted_wavefront.wgsl",
        include_str!("../shaders/sorted_wavefront.wgsl"),
    ),
];

#[test]
fn both_sorted_units_parse_and_validate() {
    for (name, source) in [
        ("sorted_wavefront.generated.wgsl", TRANSPORT),
        ("sorted_prepare.generated.wgsl", PREPARE),
    ] {
        let module = wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{name}: {}", error.emit_to_string(source)));
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    }
}

#[test]
fn the_new_wgsl_initialises_every_var_and_declares_one_array() {
    let mut arrays = Vec::new();
    for (name, source) in SOURCES {
        for (index, line) in source.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default().trim();
            let Some(declaration) = code
                .strip_prefix("var ")
                .or_else(|| code.strip_prefix("for (var "))
            else {
                continue;
            };
            assert!(
                declaration.contains('='),
                "{name}:{}: a var without an initialiser: {line}",
                index + 1
            );
            if declaration
                .split('=')
                .next()
                .is_some_and(|ty| ty.contains("array<"))
            {
                arrays.push(format!("{name}:{}", index + 1));
            }
        }
    }
    assert_eq!(
        arrays.len(),
        1,
        "the only function-scope array is the radiance of `sw_finalize`: {arrays:?}"
    );
}
