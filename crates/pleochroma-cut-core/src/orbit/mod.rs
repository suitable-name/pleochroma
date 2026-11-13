//! Deriving a tier's *orbit* and editing through that derivation.
//!
//! An orbit is the index-wheel positions one physical facet occupies once a
//! schedule's `symmetry_order`/`mirror` finish rotating (and, if mirrored,
//! reflecting) it around the index gear. It is computed from a tier's raw
//! `indices` list and never stored separately from it.
//!
//! # Irregular tiers
//!
//! A tier may fold several complete orbits that share an angle and mast (see
//! `pleochroma_formats::asc`). That needs no special edit handling, because the folded
//! facets already share the tier's angle and mast. A tier may also hold an incomplete
//! orbit, such as a facet whose symmetric partner is missing. [`orbit_units`] reports
//! both exactly as found (an `OrbitUnit` with `members.len() < expected_len`). Nothing
//! in this crate rewrites a design's `indices` to repair one: a non-orbit design must
//! stay editable and must never be silently corrected into a symmetry its author did
//! not write. Granularity matters only for membership, which
//! [`crate::design::Design::add_orbit_member`] and
//! [`crate::design::Design::remove_orbit_member`] operate on.
//!
//! # The orbit model
//!
//! A facet at azimuth `base` under `symmetry_order`-fold rotation repeats at
//! `base + k*step` for `k = 0..symmetry_order`, where
//! `step = gear_teeth_abs / symmetry_order`; every member of that rotational
//! orbit shares the same residue `base mod step`. When `mirror` is set, the
//! facet's mirror image sits at azimuth `-base mod gear_teeth_abs`, which
//! lands on a *different* residue unless `base` is already on the mirror
//! axis (residue `0` or `step/2`, where the mirror maps the rotational
//! orbit onto itself). So one facet's complete orbit is:
//! - one residue cluster of `symmetry_order` members, if `base` is on-axis
//!   or the schedule isn't mirrored, or
//! - two residue clusters of `symmetry_order` members each (`base`'s own
//!   cluster and its mirror image's), if `mirror` is set and `base` is
//!   off-axis.
//!
//! [`orbit_units`] groups a tier's raw `indices` into exactly these units;
//! [`OrbitUnit::is_complete`] checks that a unit holds its full orbit.
//!
//! # Module layout
//!
//! [`model`] is the pure orbit math (`OrbitUnit`/`orbit_units`/`expected_orbit`);
//! [`edit`] is [`crate::design::Design`]'s own orbit-editing methods built on top
//! of it.

mod edit;
mod model;
#[cfg(test)]
mod tests;

pub use model::{
    INDEX_TOLERANCE, OrbitUnit, expected_orbit, mirror_indices, orbit_units, rotate_indices,
};
