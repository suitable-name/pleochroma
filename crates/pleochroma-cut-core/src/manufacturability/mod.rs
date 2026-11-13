//! Manufacturability checks flag a design that solves geometrically but cannot be cut.
//!
//! Six checks, each reported as a warning **on** the design, never as an edit that
//! silently "fixes" anything:
//!
//! 1. [`mesh_checks::check_vanishing_facets`]: a later cut erased an earlier facet entirely.
//! 2. [`mesh_checks::check_undersized_facets`]: a facet survives but is too small to
//!    reliably cut and polish.
//! 3. [`authored_checks::check_gear_quantization`]: an authored index-wheel position does not
//!    land on a real gear tooth.
//! 4. [`authored_checks::check_cut_order`]: a tier's stated meet target comes at or after
//!    the tier itself in the schedule. The solver resolves such a forward reference
//!    numerically, but the result depends on file order (see
//!    `pleochroma::geometry::meet_solver`), and the target facet does not exist yet when
//!    the tier is cut.
//! 5. [`authored_checks::check_meet_name_asc_safety`]: a tier's meet target name would not
//!    survive a plain `.asc` export and re-import, which would lose or mis-target the
//!    reference.
//! 6. [`mesh_checks::check_concave_tools`]: only for a design with concave tiers. It
//!    reports a tool that misses the stone, lies wholly inside it, breaks through it,
//!    removes a meet or hull vertex, overlaps another tool, or leaves a sliver. It is the
//!    costliest check (many plane-arrangement meshes), so a UI thread should run it on a
//!    worker.
//!
//! [`check_manufacturability`] runs every check over an **already-solved**
//! [`crate::design::Design`] (see its doc comment) and returns the warnings in a fixed,
//! deterministic order: check order, then tier order within each. No `HashMap` or
//! `HashSet` appears in the decision path.
//!
//! # Why these checks don't need a second solve
//!
//! `Design::planes` and `Design::status` each call `Design::solve`, which is slow for a
//! design with meet-derived structure. [`check_manufacturability`] instead takes the
//! `SolvedTier`s the caller already has, builds the plane arrangement with
//! `Design::planes_from_solved`, and meshes it with
//! [`pleochroma::geometry::stone_metrics::build_solid_mesh`]. Checks 3, 4 and 5 read the
//! authored angle, index and name state, so they run even when the design has never been
//! solved.
//!
//! # Module layout
//!
//! [`warning`] is the one [`ManufacturabilityWarning`] type every check reports
//! through; [`mesh_checks`] is [`check_manufacturability`] itself plus checks 1-2
//! (the mesh-dependent pair); [`authored_checks`] is checks 3-5 (the mast-free
//! trio).

mod authored_checks;
mod mesh_checks;
#[cfg(test)]
mod tests;
mod tool_cull;
mod warning;

use crate::design::Design;
use pleochroma::geometry::meet_solver::SolvedTier;

pub use authored_checks::{check_cut_order, check_gear_quantization, check_meet_name_asc_safety};
pub use mesh_checks::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, check_concave_tools, check_manufacturability,
    check_manufacturability_with_geometry, concave_tool_warnings, degenerate_suspects,
};
pub use warning::ManufacturabilityWarning;

/// Cumulative facet-plane count per tier prefix -- see
/// [`mesh_checks::facet_plane_boundaries`]'s own doc comment. `pub(crate)`
/// only: [`crate::design::Design::tier_for_plane_index`] is the public way to
/// use this (turning a combined-arrangement plane index into a tier index),
/// so the raw boundaries never need to leave this crate.
pub(crate) use mesh_checks::facet_plane_boundaries;

/// Runs every check this crate can given whatever solve state is actually
/// available, rather than requiring a full solve up front the way
/// [`check_manufacturability`] itself does.
///
/// The three mast-free checks
/// ([`check_gear_quantization`]/[`check_cut_order`]/[`check_meet_name_asc_safety`])
/// always run. The mesh-based checks (1, 2 and 6, reached through
/// [`check_manufacturability`]) run only when `solved` is `Some`.
///
/// For a caller that must still report the mast-free checks on a design that has not
/// solved, such as one with a [`crate::design::MissingAnchor`]. Returning an empty list
/// on a solve error would drop those checks too.
#[must_use]
pub fn check_manufacturability_available(
    design: &Design,
    solved: Option<&[SolvedTier]>,
    min_facet_area_fraction_of_w2: f64,
) -> Vec<ManufacturabilityWarning> {
    solved.map_or_else(
        || {
            let mut warnings = check_gear_quantization(design);
            warnings.extend(check_cut_order(design));
            warnings.extend(check_meet_name_asc_safety(design));
            warnings
        },
        |solved| check_manufacturability(design, solved, min_facet_area_fraction_of_w2),
    )
}
