//! [`ManufacturabilityWarning`], the one type every check reports through. See the parent
//! module's doc comment for the checks and why each is a warning.

use crate::design::TierId;

/// One manufacturability problem found in a [`crate::design::Design`]. The mesh-based
/// variants need an already-solved design.
///
/// Always a warning for the user, never acted on by this crate: `History` is the sole
/// mutator of `Design`, and this module does not touch one.
#[derive(Debug, Clone, PartialEq)]
pub enum ManufacturabilityWarning {
    /// A facet plane this tier describes never reaches the solid's surface --
    /// some later tier's cut removed it entirely. `vanished` counts how many
    /// of the tier's own facet-plane copies (its index-wheel orbit) are
    /// affected; `total` is how many that tier contributes in total (`1` for
    /// a tier with no listed indices).
    VanishingFacet {
        tier_index: usize,
        /// The stable id of the tier at `tier_index` when this warning was built
        /// (the tier table badges rows directly instead of parsing 'tier N' out
        /// of the message). See [`Self::tier_id`].
        tier_id: TierId,
        tier_name: String,
        vanished: usize,
        total: usize,
    },
    /// A facet survives (it has a real polygon on the solid's surface) but
    /// that polygon's area falls under the configured threshold -- see
    /// [`super::DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2`]. `facet_plane_index` is the
    /// index into [`crate::design::Design::planes`]'s combined list, for a caller that
    /// wants to highlight the exact facet (e.g. via
    /// [`pleochroma::geometry::stone_metrics::SolidMesh::rings`]). `area` and
    /// `threshold` stay in the design's own arbitrary "mast unit" scale (never
    /// meaningful to a cutter on their own -- see [`Self`]'s `Display` impl,
    /// which reports both as a fraction of `width_axis` instead).
    UndersizedFacet {
        tier_index: usize,
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
        tier_name: String,
        facet_plane_index: usize,
        area: f64,
        threshold: f64,
        /// The solid's own measured width along its widest axis, in the same
        /// "mast unit" scale as `area`/`threshold` -- lets [`Self`]'s `Display`
        /// impl report the facet's size as a cutter-meaningful percentage of
        /// stone width (`sqrt(area) / width_axis`) instead of a raw area no
        /// design-independent number could ever be compared against.
        width_axis: f64,
    },
    /// One of this tier's authored index-wheel positions does not land on a
    /// real gear tooth. `achievable` is the nearest real tooth position;
    /// `azimuth_error_deg` is the resulting azimuthal error if the lapidary
    /// cuts at `achievable` instead of the requested (unreachable) position.
    /// Never silently rounded -- this is reported, not corrected.
    FractionalIndex {
        tier_index: usize,
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
        tier_name: String,
        requested: f64,
        achievable: f64,
        azimuth_error_deg: f64,
    },
    /// This tier's [`pleochroma::geometry::meet_solver::MeetConstraint::MeetNamed`] resolves
    /// (see [`pleochroma::geometry::meet_solver::MeetNameResolver`]) to a target at or after
    /// this tier in the schedule. The solve is valid, since the solver ignores file order,
    /// but the target facet does not exist yet when this tier is cut.
    OutOfOrderMeet {
        tier_index: usize,
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
        tier_name: String,
        target_tier_index: usize,
        target_tier_name: String,
    },
    /// This tier's [`pleochroma::geometry::meet_solver::MeetConstraint::MeetNamed`] names at
    /// least one target that [`crate::design::meet_name_is_asc_safe`] rejects. Such a name
    /// would not re-parse to itself from a plain `.asc` `"Meet <names>"` export (see that
    /// function's doc for the rules). `unsafe_names` lists the failures in authored order.
    MeetNameNotAscSafe {
        tier_index: usize,
        /// See [`Self::VanishingFacet`]'s own `tier_id` doc comment.
        tier_id: TierId,
        tier_name: String,
        unsafe_names: Vec<String>,
    },
    /// A concave tool removes nothing: its volume lies outside the stone (or only
    /// grazes it), so the tier cuts no facet there. `tier` indexes
    /// [`crate::design::Design::concave_tiers`] and `placement` that tier's
    /// `indices`; both concave variants below use the same pair.
    ToolMissesStone { tier: usize, placement: usize },
    /// A concave tool cuts clean through the stone: it reaches two flat facets
    /// facing opposite ways, leaving a hole rather than a facet. `facet` is the
    /// lowest-numbered of the touching facets that has such an opposite, as an
    /// index into [`crate::design::Design::planes`]'s combined list. A
    /// reciprocating tool's stroke ends do not count (they leave through the
    /// facets at either end of its axis by design).
    ToolBreaksThrough {
        tier: usize,
        placement: usize,
        facet: usize,
    },
    /// A concave tool cuts away a vertex where three or more of the schedule's flat
    /// facets meet (`vertex` indexes the flat stone's deduplicated vertex list), so
    /// the meet point those facets were cut to no longer exists.
    ToolRemovesMeet {
        tier: usize,
        placement: usize,
        vertex: usize,
    },
    /// Two concave tools remove overlapping volume from the stone. `a` and `b` are
    /// positions in [`crate::design::Design::concave_tools_from_solved`]'s output
    /// (`a < b`), not tier indices; the placement list maps them back.
    ToolsOverlap { a: usize, b: usize },
    /// A facet survives the concave cuts but is a sliver: a flat facet whose area
    /// the tools reduce below the minimum, or a tool surface that small. `facet` is
    /// an index into the flat planes followed by the tools (a tool's facet id is
    /// `planes.len() + k`), the numbering `build_solid_mesh_geom` uses.
    ConcaveSliver { facet: usize },
    /// A concave tool cuts away an extreme vertex of the stone (one that sets its
    /// width, length or height), so the finished stone is smaller than the flat
    /// schedule implies.
    ToolRemovesHullVertex { tier: usize, placement: usize },
    /// A concave tool sits wholly inside the stone: it removes volume but never reaches
    /// a flat facet, so it would be an internal void, which no cutter can reach. The
    /// tool's position or size is wrong (a displacement or depth beyond the stone, or a
    /// tool too small for the depth asked of it).
    ToolEnclosed { tier: usize, placement: usize },
    /// The design has concave tiers and the file format being written (`.asc`,
    /// `.gem`, `.gcs`) cannot represent them, so they are left out of the tier
    /// records (`.asc` keeps two footnotes per tier; the native `.psdd` file
    /// stays the lossless carrier). About the design as a whole, so it names no
    /// tier: [`Self::tier_index`] reports `0` and [`Self::tier_id`] `TierId(0)` for
    /// it, and it belongs to the export notice, never to a tier-table row.
    ConcaveTiersOmittedFromExport {
        /// How many concave tiers are left out.
        count: usize,
    },
}

impl ManufacturabilityWarning {
    /// The flat tier this warning is about, so a caller such as the editor's tier table can
    /// attribute it to a row. Concave-tool and export warnings report `0`; see
    /// [`Self::concave_tier`].
    #[must_use]
    pub const fn tier_index(&self) -> usize {
        match self {
            Self::VanishingFacet { tier_index, .. }
            | Self::UndersizedFacet { tier_index, .. }
            | Self::FractionalIndex { tier_index, .. }
            | Self::OutOfOrderMeet { tier_index, .. }
            | Self::MeetNameNotAscSafe { tier_index, .. } => *tier_index,
            Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolMissesStone { .. }
            | Self::ToolBreaksThrough { .. }
            | Self::ToolRemovesMeet { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. }
            | Self::ToolRemovesHullVertex { .. }
            | Self::ToolEnclosed { .. } => 0,
        }
    }

    /// The stable [`TierId`] of the flat tier this warning is about, as of the solve it was
    /// built from. Unlike a position, it survives add, remove, move and undo, so a row can be
    /// badged by id. Concave-tool and export warnings report `TierId(0)`.
    #[must_use]
    pub const fn tier_id(&self) -> TierId {
        match self {
            Self::VanishingFacet { tier_id, .. }
            | Self::UndersizedFacet { tier_id, .. }
            | Self::FractionalIndex { tier_id, .. }
            | Self::OutOfOrderMeet { tier_id, .. }
            | Self::MeetNameNotAscSafe { tier_id, .. } => *tier_id,
            Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolMissesStone { .. }
            | Self::ToolBreaksThrough { .. }
            | Self::ToolRemovesMeet { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. }
            | Self::ToolRemovesHullVertex { .. }
            | Self::ToolEnclosed { .. } => TierId(0),
        }
    }

    /// The concave tier (index into [`crate::design::Design::concave_tiers`]) this
    /// warning is about, for the variants that name one. [`Self::tier_index`] and
    /// [`Self::tier_id`] are about flat tiers and report `0` for these, so a
    /// tier-table row never inherits a concave tool's warning by accident.
    #[must_use]
    pub const fn concave_tier(&self) -> Option<usize> {
        match self {
            Self::ToolMissesStone { tier, .. }
            | Self::ToolBreaksThrough { tier, .. }
            | Self::ToolRemovesMeet { tier, .. }
            | Self::ToolRemovesHullVertex { tier, .. }
            | Self::ToolEnclosed { tier, .. } => Some(*tier),
            Self::VanishingFacet { .. }
            | Self::UndersizedFacet { .. }
            | Self::FractionalIndex { .. }
            | Self::OutOfOrderMeet { .. }
            | Self::MeetNameNotAscSafe { .. }
            | Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. } => None,
        }
    }

    /// The facet-plane index a caller should highlight in the 3D view, when this
    /// warning names one -- only [`Self::UndersizedFacet`] does; every other
    /// variant is about a tier as a whole (a vanished facet has no surviving
    /// polygon left to highlight, and the index/cut-order checks are about
    /// authored state, not a specific facet).
    #[must_use]
    pub const fn facet_plane_index(&self) -> Option<usize> {
        match self {
            Self::UndersizedFacet {
                facet_plane_index, ..
            }
            | Self::ToolBreaksThrough {
                facet: facet_plane_index,
                ..
            } => Some(*facet_plane_index),
            Self::VanishingFacet { .. }
            | Self::FractionalIndex { .. }
            | Self::OutOfOrderMeet { .. }
            | Self::MeetNameNotAscSafe { .. }
            | Self::ConcaveTiersOmittedFromExport { .. }
            | Self::ToolMissesStone { .. }
            | Self::ToolRemovesMeet { .. }
            | Self::ToolsOverlap { .. }
            | Self::ConcaveSliver { .. }
            | Self::ToolRemovesHullVertex { .. }
            | Self::ToolEnclosed { .. } => None,
        }
    }
}

impl std::fmt::Display for ManufacturabilityWarning {
    #[expect(
        clippy::too_many_lines,
        reason = "one arm per warning variant; splitting would only scatter the user-facing texts"
    )]
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Tier numbers in these strings are 1-based, as the tier table shows them. The
        // fields and `Self::tier_index` stay 0-based, since they index `Design::tiers`.
        match self {
            Self::VanishingFacet {
                tier_index,
                tier_name,
                vanished,
                total,
                ..
            } => write!(
                f,
                "tier {} ({tier_name}): {vanished}/{total} facet(s) cut away entirely \
                 by a later tier",
                tier_index + 1
            ),
            Self::UndersizedFacet {
                tier_index,
                tier_name,
                area,
                threshold,
                width_axis,
                ..
            } => {
                // Shown as a percentage of stone width (`sqrt(area) / width_axis`), since a raw
                // design-scale area means nothing to a cutter.
                let facet_pct = if *width_axis > 0.0 {
                    100.0 * area.sqrt() / width_axis
                } else {
                    f64::NAN
                };
                let threshold_pct = if *width_axis > 0.0 {
                    100.0 * threshold.sqrt() / width_axis
                } else {
                    f64::NAN
                };
                write!(
                    f,
                    "tier {} ({tier_name}): facet spans only {facet_pct:.2}% of the stone's \
                     width, below the {threshold_pct:.2}% minimum",
                    tier_index + 1
                )
            }
            Self::FractionalIndex {
                tier_index,
                tier_name,
                requested,
                achievable,
                azimuth_error_deg,
                ..
            } => write!(
                f,
                "tier {} ({tier_name}): index {requested} does not land on a gear \
                 tooth; nearest achievable is {achievable} ({azimuth_error_deg:+.4} deg azimuth \
                 error)",
                tier_index + 1
            ),
            Self::OutOfOrderMeet {
                tier_index,
                tier_name,
                target_tier_index,
                target_tier_name,
                ..
            } => write!(
                f,
                "tier {} ({tier_name}): meets tier {} \
                 ({target_tier_name}), which is not cut until later in the schedule",
                tier_index + 1,
                target_tier_index + 1
            ),
            Self::MeetNameNotAscSafe {
                tier_index,
                tier_name,
                unsafe_names,
                ..
            } => write!(
                f,
                "tier {} ({tier_name}): meet target name(s) {} would not survive a plain \
                 .asc export/re-import -- rename without spaces, commas or semicolons, and \
                 without leading/trailing punctuation",
                tier_index + 1,
                unsafe_names.join(", ")
            ),
            Self::ToolMissesStone { tier, placement } => write!(
                f,
                "concave tier {} placement {}: the tool lies outside the stone and cuts nothing",
                tier + 1,
                placement + 1
            ),
            Self::ToolBreaksThrough {
                tier,
                placement,
                facet,
            } => write!(
                f,
                "concave tier {} placement {}: the tool cuts clean through the stone \
                 (it reaches facet {} and the facet opposite it)",
                tier + 1,
                placement + 1,
                facet + 1
            ),
            Self::ToolRemovesMeet {
                tier,
                placement,
                vertex,
            } => write!(
                f,
                "concave tier {} placement {}: the tool cuts away the meet point of vertex {}",
                tier + 1,
                placement + 1,
                vertex + 1
            ),
            Self::ToolsOverlap { a, b } => write!(
                f,
                "concave tools {} and {} remove overlapping volume",
                a + 1,
                b + 1
            ),
            Self::ConcaveSliver { facet } => write!(
                f,
                "facet {} is left a sliver by the concave cuts",
                facet + 1
            ),
            Self::ToolRemovesHullVertex { tier, placement } => write!(
                f,
                "concave tier {} placement {}: the tool cuts away a vertex that sets the \
                 stone's width, length or height",
                tier + 1,
                placement + 1
            ),
            Self::ToolEnclosed { tier, placement } => write!(
                f,
                "concave tier {} placement {}: the tool sits entirely inside the stone and never \
                 reaches its surface, so it cannot be cut",
                tier + 1,
                placement + 1
            ),
            Self::ConcaveTiersOmittedFromExport { count } => write!(
                f,
                "{count} concave tier(s) cannot be written to this format and are left out of \
                 the tier list; the .asc keeps them as footnotes, the .psdd file keeps \
                 them in full"
            ),
        }
    }
}
