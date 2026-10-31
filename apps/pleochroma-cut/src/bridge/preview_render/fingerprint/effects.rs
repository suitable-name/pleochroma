//! The render physics a fingerprint names: inclusions and play-of-color, the surface finish of
//! the facets and the abraded edge.
//!
//! Each key is `0` for a design that sets nothing, and then adds nothing to the fingerprint
//! text, so every picture cached before the effects existed keeps its fingerprint. A key is
//! never `0` for anything set.

use pleochroma::optics::{
    inclusions::{bragg::StructureSpec, spec::InclusionSpec},
    raytracer::surface::{EdgeAbrasion, FacetingMachine},
};
use pleochroma_vault::model::surface_finish::StoredSurfaceFinish;

use super::fnv1a;

/// The three keys a fingerprint can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct EffectsKey {
    /// [`inclusions_hash`]: `;incl=<hash>`.
    pub inclusions: u64,
    /// [`surfaces_hash`]: `;surf=<hash>`.
    pub surfaces: u64,
    /// [`edge_hash`]: `;edge=<hash>`.
    pub edge: u64,
}

impl EffectsKey {
    /// A design that sets nothing.
    pub const NONE: Self = Self {
        inclusions: 0,
        surfaces: 0,
        edge: 0,
    };

    /// The keys of a library design's stored surface finish (`diagram_surface_finish` and
    /// `diagram_edge_abrasion`, both read from the design file at import), with the user's
    /// faceting machine: `0` for a design that states none, whatever the machine. A library
    /// scan reads only those rows, never the design file, so this is the key the scan and the
    /// writer of a library picture can both build; the inclusion key stays `0` here
    /// ([`Self::with_stored_inclusions`] adds it).
    #[must_use]
    pub fn of_stored(finish: &StoredSurfaceFinish, machine: &FacetingMachine) -> Self {
        let surfaces = if finish.tiers.is_empty() {
            0
        } else {
            fnv1a(format!("{machine:?}|{:?}", finish.tiers).as_bytes()).max(1)
        };
        let edge = finish
            .edge
            .as_ref()
            .filter(|edge| !edge.is_off())
            .map_or(0, |edge| fnv1a(format!("{edge:?}").as_bytes()).max(1));
        Self {
            inclusions: 0,
            surfaces,
            edge,
        }
    }

    /// `self` with the inclusion key of the choice text a library design's file states
    /// (`Database::diagram_inclusion_effects`); `0` for a design that states none. Like the
    /// surface keys, a library scan and the writer of a library picture both read this one row.
    #[must_use]
    pub fn with_stored_inclusions(mut self, text: Option<&str>) -> Self {
        self.inclusions = text.map_or(0, |text| fnv1a(text.as_bytes()).max(1));
        self
    }

    /// Appends the keys that are set, in the order `;incl=`, `;surf=`, `;edge=`.
    pub(super) fn append_to(&self, part: &mut String) {
        use std::fmt::Write as _;
        if self.inclusions != 0 {
            let _ = write!(part, ";incl={:016x}", self.inclusions);
        }
        if self.surfaces != 0 {
            let _ = write!(part, ";surf={:016x}", self.surfaces);
        }
        if self.edge != 0 {
            let _ = write!(part, ";edge={:016x}", self.edge);
        }
    }
}

/// A hash of a design's inclusion choice and play-of-color choice (seed included). `0` when
/// neither is set.
///
/// Hashes the `Debug` text of the specs: every `f32` prints its shortest round-trip form, so the
/// text is deterministic and covers every field, including ones added later.
#[must_use]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the key parts are built here and checked by the fingerprint tests; production keys come from `EffectsKey`"
    )
)]
pub fn inclusions_hash(
    inclusions: Option<&InclusionSpec>,
    structure: Option<&StructureSpec>,
) -> u64 {
    if inclusions.is_none() && structure.is_none() {
        return 0;
    }
    fnv1a(format!("{inclusions:?}|{structure:?}").as_bytes()).max(1)
}

/// A hash of a design's packed facet surface words (`FacetSurface::packed`, in plane order) and
/// the faceting machine the scratch directions come from. `0` when every word is ideal, so a
/// design with no surface override keeps its fingerprint whatever the user's machine says.
#[must_use]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the key parts are built here and checked by the fingerprint tests; production keys come from `EffectsKey`"
    )
)]
pub fn surfaces_hash(words: &[u32], machine: &FacetingMachine) -> u64 {
    use std::fmt::Write as _;
    if words.iter().all(|&word| word == 0) {
        return 0;
    }
    let mut text = format!("{machine:?}|");
    for word in words {
        let _ = write!(text, "{word:08x}");
    }
    fnv1a(text.as_bytes()).max(1)
}

/// A hash of the abraded-edge setting. `0` when it changes nothing.
#[must_use]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the key parts are built here and checked by the fingerprint tests; production keys come from `EffectsKey`"
    )
)]
pub fn edge_hash(edge: &EdgeAbrasion) -> u64 {
    if !edge.active() {
        return 0;
    }
    fnv1a(format!("{edge:?}").as_bytes()).max(1)
}
