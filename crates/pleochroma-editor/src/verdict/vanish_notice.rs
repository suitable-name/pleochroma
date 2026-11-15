//! The vanished-facet notice: what an edit cut away, compared across two verdicts.
//!
//! [`vanish_snapshot`] reads the vanished facets off a design's verdict inputs, one
//! [`VanishEntry`] per tier that has any. [`vanish_notice`] compares the snapshot from before an
//! edit with the one after it, by [`TierId`] (a tier keeps its id when it moves in the schedule,
//! so a reorder is not mistaken for a new loss), and words what grew. Both are pure, so a
//! desktop or web caller keeps the baseline snapshot and decides when to compare.

use super::{VerdictInputs, label_of};
use pleochroma_cut_core::{Design, ManufacturabilityWarning, TierId};

/// The vanished facets of one verdict, per tier.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VanishSnapshot {
    /// The tiers that have at least one vanished facet, in schedule order.
    pub tiers: Vec<VanishEntry>,
}

/// One tier's vanished facets in a [`VanishSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VanishEntry {
    /// The tier's stable id, the key the before/after comparison matches on.
    pub id: TierId,
    /// The tier's position in `design.tiers` when the snapshot was taken.
    pub index: usize,
    /// What a person reads for the tier: its display name, its own name, or `tier N`.
    pub label: String,
    /// How many of the tier's facets are cut away by later tiers.
    pub vanished: usize,
    /// How many facets the tier has.
    pub total: usize,
}

/// The notice for newly vanished facets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VanishNotice {
    /// The sentence the banner shows.
    pub text: String,
    /// The position of the first affected tier, for "Select".
    pub select_tier: usize,
    /// That tier's label, the `{tier}` of the button text `Select {tier}`.
    pub select_label: String,
}

/// The vanished facets of `design` as `inputs` report them
/// ([`ManufacturabilityWarning::VanishingFacet`]).
///
/// The entry's id is the design's id for the tier's position, falling back to the id the finding
/// carries; its label follows the verdict's naming ([`VerdictInputs::tier_names`]), with an
/// unnamed tier reading `tier N`. Entries are in schedule order.
#[must_use]
pub fn vanish_snapshot(design: &Design, inputs: &VerdictInputs) -> VanishSnapshot {
    let mut tiers: Vec<VanishEntry> = inputs
        .warnings
        .iter()
        .filter_map(|warning| {
            let ManufacturabilityWarning::VanishingFacet {
                tier_index,
                tier_id,
                tier_name,
                vanished,
                total,
            } = warning
            else {
                return None;
            };
            if *vanished == 0 {
                return None;
            }
            let shown = inputs
                .tier_names
                .get(*tier_index)
                .filter(|shown| !shown.trim().is_empty());
            let label = shown.map_or_else(
                || {
                    if tier_name.trim().is_empty() {
                        // `label_of` gives "Tier N"; the notice reads mid-sentence.
                        label_of(tier_name, *tier_index).to_lowercase()
                    } else {
                        tier_name.clone()
                    }
                },
                Clone::clone,
            );
            Some(VanishEntry {
                id: design.tier_id_at(*tier_index).unwrap_or(*tier_id),
                index: *tier_index,
                label,
                vanished: *vanished,
                total: *total,
            })
        })
        .collect();
    tiers.sort_by_key(|entry| entry.index);
    VanishSnapshot { tiers }
}

/// The notice for what vanished between `before` and `after`: the tiers whose `vanished` count
/// grew, matched by id (a tier absent from `before` counts from zero).
///
/// `None` when nothing new vanished: no tier grew, because the set is unchanged, shrank, or only
/// lost tiers that were removed.
///
/// The wording counts what this edit cut away, `after.vanished - before.vanished` per tier:
///
/// - one tier, every facet gone: `This cut away P2.` (one facet) or
///   `This cut away P2: all 8 of its facets.`;
/// - one tier, some facets: `This cut away 2 of the 8 facets of P2.`;
/// - two or three tiers: `This cut away P2, P3 and P4.`;
/// - more: `This cut away P2, P3 and 3 more tiers.`
#[must_use]
pub fn vanish_notice(before: &VanishSnapshot, after: &VanishSnapshot) -> Option<VanishNotice> {
    let grown: Vec<(&VanishEntry, usize)> = after
        .tiers
        .iter()
        .filter_map(|entry| {
            let was = before
                .tiers
                .iter()
                .find(|old| old.id == entry.id)
                .map_or(0, |old| old.vanished);
            entry
                .vanished
                .checked_sub(was)
                .filter(|gain| *gain > 0)
                .map(|gain| (entry, gain))
        })
        .collect();
    let (first, first_gain) = *grown.first()?;
    let text = if grown.len() == 1 {
        one_tier_text(first, first_gain)
    } else {
        let names: Vec<&str> = grown
            .iter()
            .map(|(entry, _)| entry.label.as_str())
            .collect();
        many_tiers_text(&names)
    };
    Some(VanishNotice {
        text,
        select_tier: first.index,
        select_label: first.label.clone(),
    })
}

/// The sentence for one tier that lost `gain` facets in this edit.
fn one_tier_text(entry: &VanishEntry, gain: usize) -> String {
    let label = &entry.label;
    if entry.vanished >= entry.total {
        if entry.total <= 1 {
            format!("This cut away {label}.")
        } else {
            format!("This cut away {label}: all {} of its facets.", entry.total)
        }
    } else {
        format!(
            "This cut away {gain} of the {} facets of {label}.",
            entry.total
        )
    }
}

/// The sentence for two or more tiers: every name for up to three, else two and a count.
fn many_tiers_text(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => format!("This cut away {only}."),
        [a, b] => format!("This cut away {a} and {b}."),
        [a, b, c] => format!("This cut away {a}, {b} and {c}."),
        [a, b, rest @ ..] => format!("This cut away {a}, {b} and {} more tiers.", rest.len()),
    }
}
