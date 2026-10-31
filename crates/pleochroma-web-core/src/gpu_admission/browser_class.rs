//! The kernel variant the browser compiles and traces for a material class.
//!
//! The desktop compiles a stone's kernels with every feature the stone can reach
//! (`classify_material`: kind plus the `NO_*` feature bits). A browser scene can never reach
//! oriented inclusions or color zoning (the page refuses effects and zoned stones), and the
//! `Scatter` kernel with those estimators compiled in takes over 20 s on a Chrome/AMD adapter,
//! past the compile deadline. So the browser compiles one smaller variant per class and traces
//! with exactly that variant: [`browser_pipeline_class`] names it, [`browser_pass_class`] checks
//! that a stone's own features fit inside it.
//!
//! The constants repeat `renderer::gpu::frame::material_class` (the desktop crate's `gpu`
//! feature is not a dependency of this crate); a test compares them.

use super::UNIAXIAL_CLASS;

/// The bits of a class value that name the kind (`material_class::KIND_MASK`).
const KIND_MASK: u32 = 3;
/// The oriented and Bragg estimators are compiled out (`material_class::NO_ORIENTED`).
const NO_ORIENTED: u32 = 4;
/// The zoned oriented estimator is compiled out (`material_class::NO_ZONED_ORIENTED`).
const NO_ZONED_ORIENTED: u32 = 8;
/// The extraordinary-ray dispersion curve is compiled out
/// (`material_class::NO_EXTRAORDINARY_DISPERSION`).
const NO_EXTRAORDINARY_DISPERSION: u32 = 32;
/// The features the browser variant may compile out; a stone that needs one of them cannot use
/// the variant.
const REMOVABLE: u32 = NO_ORIENTED | NO_ZONED_ORIENTED | NO_EXTRAORDINARY_DISPERSION;

/// The kind of a class value: 1, 2 or 3 for a material class (feature bits dropped), and the
/// value itself for anything else (generic 0, the Ray-Traced class).
#[must_use]
pub const fn class_kind(class: u32) -> u32 {
    let kind = class & KIND_MASK;
    if kind == 0 { class } else { kind }
}

/// The class value the browser compiles and traces for material class `class`.
///
/// It is the kind plus `NO_ORIENTED | NO_ZONED_ORIENTED` for every class, and plus
/// `NO_EXTRAORDINARY_DISPERSION` for isotropic and biaxial. Uniaxial keeps the extraordinary
/// curve compiled in (the worst reachable case; a stone without a curve skips it at run time).
/// The scattering NEE stays compiled in. A value that names no material class (generic 0, the
/// Ray-Traced class) is returned unchanged.
#[must_use]
pub const fn browser_pipeline_class(class: u32) -> u32 {
    let kind = class & KIND_MASK;
    if kind == 0 {
        return class;
    }
    let mut variant = kind | NO_ORIENTED | NO_ZONED_ORIENTED;
    if kind != UNIAXIAL_CLASS {
        variant |= NO_EXTRAORDINARY_DISPERSION;
    }
    variant
}

/// The class value to trace a stone with, given `scene_class` (what the desktop's
/// `classify_material` returns for it), or `None` when the browser variant lacks a feature the
/// stone needs.
///
/// The answer is always [`browser_pipeline_class`] of the stone's kind, the variant that was
/// compiled and tested. `None` means the stone needs oriented inclusions, color zones or an
/// extraordinary curve that the variant compiled out, so the pass must not run on it.
#[must_use]
pub const fn browser_pass_class(scene_class: u32) -> Option<u32> {
    let kind = scene_class & KIND_MASK;
    if kind == 0 {
        return None;
    }
    let variant = browser_pipeline_class(kind);
    let compiled_out = variant & REMOVABLE;
    let needed = !scene_class & REMOVABLE;
    if compiled_out & needed == 0 {
        Some(variant)
    } else {
        None
    }
}
