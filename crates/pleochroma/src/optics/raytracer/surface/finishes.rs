//! The facet finish list and the facet surface list name the ground arm once.
//!
//! The renderer reads two inputs per facet: the coarse [`FacetFinish`] (the list the next-event
//! estimators and the glare rule read) and the [`FacetSurface`] word (the ground lobe and the
//! polish). A Ground facet of the word IS a frosted facet: [`merged_facet_finishes`] reports it as
//! `FacetFinish::Frosted` in the list, so the word is the single source of truth and no consumer
//! of the list has to know about the word.

use std::borrow::Cow;

use super::facet_surface::FacetSurface;
use crate::optics::raytracer::camera::FacetFinish;

/// `finishes` with every facet whose surface is a ground kind (Frosted or Ground) reported as
/// `FacetFinish::Frosted`. The list is extended with polished entries when the surface list is
/// longer.
///
/// Borrows `finishes` untouched (no allocation) when nothing needs to change, which is every
/// scene without ground surface words, and every scene whose list was already merged.
#[must_use]
pub fn merged_facet_finishes<'a>(
    finishes: &'a [FacetFinish],
    surfaces: &[FacetSurface],
) -> Cow<'a, [FacetFinish]> {
    let frosted_in_list =
        |i: usize| finishes.get(i).copied().unwrap_or_default() == FacetFinish::Frosted;
    if surfaces
        .iter()
        .enumerate()
        .all(|(i, surface)| !surface.is_ground() || frosted_in_list(i))
    {
        return Cow::Borrowed(finishes);
    }
    let mut merged = finishes.to_vec();
    if merged.len() < surfaces.len() {
        merged.resize(surfaces.len(), FacetFinish::default());
    }
    for (slot, surface) in merged.iter_mut().zip(surfaces) {
        if surface.is_ground() {
            *slot = FacetFinish::Frosted;
        }
    }
    Cow::Owned(merged)
}
