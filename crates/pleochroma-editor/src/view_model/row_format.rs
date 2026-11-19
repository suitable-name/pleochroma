//! Small per-tier text/formatting helpers.
//!
//! The pieces [`super::rows`]'s row builders share, plus the helpers other editor actions
//! call directly (multi-select, the symmetry-preview and meet-name-resolution checks).

use super::{IndexChip, TierRow, rows::tidy_text};
use pleochroma::geometry::meet_solver::{
    Block, MeetConstraint, MeetNameResolver, TokenResolution, classify_blocks,
};
use pleochroma_cut_core::{Design, OrbitUnit, TierTarget};

/// Splits a tier's [`MeetConstraint`] AND its [`TierTarget`] (if any) into the
/// `(constraint_kind, constraint_text)` pair [`TierRow`] carries and the
/// tier-edit form round-trips through its `LineEdit` -- the inverse of
/// `super::loading::parse_tier_form`'s constraint parsing and
/// `super::loading::parse_tier_target`'s target parsing, combined.
///
/// `target` wins whenever it is `Some`. A target-bearing tier stores a `ScaleReference(0.0)`
/// placeholder as its constraint, so reading the constraint would show a meaningless "0".
pub(super) fn constraint_kind_and_text(
    constraint: &MeetConstraint,
    target: Option<TierTarget>,
) -> (i32, String) {
    if let Some(target) = target {
        return match target {
            TierTarget::DepthMm(mm) => (3, tidy_text(mm)),
            TierTarget::GirdleThicknessMm(mm) => (4, tidy_text(mm)),
            TierTarget::TableWidthMm(mm) => (5, tidy_text(mm)),
        };
    }
    match constraint {
        MeetConstraint::MeetExisting => (0, String::new()),
        MeetConstraint::MeetNamed(names) => (1, names.join(", ")),
        // Rust's shortest round-trippable `f64` Display, not a fixed decimal count --
        // this feeds back into the form's `LineEdit`, so it must read back exactly
        // what `solve` (or the user) produced.
        // (noise below 1e-8 is dropped: `0.6525000000000001` reads `0.6525`)
        MeetConstraint::ScaleReference(value) => (2, tidy_text(*value)),
    }
}

/// The tier table's ANGLE cell text, always two decimals.
///
/// A nudge leaves `f64` noise (e.g. `-40.300000000000004`) that would read as a broken number.
/// Only the display is rounded, never the stored value. The inline editor and the tier-edit form
/// re-seed from [`TierRow::angle_full`] instead, so a re-save cannot round a value the design
/// holds exactly.
#[must_use]
pub fn format_angle_cell(angle_deg: f64) -> String {
    format!("{angle_deg:.2}")
}

/// One index-wheel position's display text: an integer on a whole tooth, else two decimals.
///
/// Integral-aware like [`format_angle_cell`], so `"24"` rather than `"24.00"`. A genuinely
/// fractional index (see `ManufacturabilityWarning::FractionalIndex`) keeps its fraction.
#[must_use]
pub fn format_index_value(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Builds one [`IndexChip`] per entry in `indices`, flagging the occurrences also present in
/// `detached`.
///
/// The chip row is kept apart from [`TierRow`], which the desktop builds on a worker thread;
/// the Slint chip model is `Rc`-backed and cannot cross threads.
///
/// A plain epsilon comparison is enough: `detached` holds values copied verbatim from `indices`,
/// never wrapped, so no gear-wraparound normalization is needed.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the Slint chip position precision; index positions are small numbers"
)]
pub fn index_chip_items(indices: &[f64], detached: &[f64]) -> Vec<IndexChip> {
    indices
        .iter()
        .map(|&position| IndexChip {
            position: position as f32,
            label: format_index_value(position),
            detached: detached.iter().any(|&d| (d - position).abs() < 1e-9),
        })
        .collect()
}

/// Display text for a tier's `imported_meet`: what the source `.asc` file's `G` field claimed.
/// `""` when there is nothing to adopt, which also decides whether the Adopt button shows.
pub(super) fn imported_meet_text(imported_meet: Option<&MeetConstraint>) -> String {
    match imported_meet {
        Some(MeetConstraint::MeetExisting) => "meets an unspecified vertex".to_string(),
        Some(MeetConstraint::MeetNamed(names)) => format!("meets {}", names.join(", ")),
        // `Design::from_asc_schedule` never actually stores this variant here, but a
        // future non-exhaustive addition should degrade to "nothing to show," not panic.
        None | Some(MeetConstraint::ScaleReference(_)) => String::new(),
    }
}

/// A short label for `units` (`orbit::orbit_units`'s decomposition of one tier's
/// `indices`) plus whether it should be flagged amber. Empty/`false` for a tier with
/// nothing to link (0 or 1 occurrence).
pub(super) fn orbit_status_text(units: &[OrbitUnit]) -> (String, bool) {
    if units.iter().all(|u| u.members.len() <= 1) {
        return (String::new(), false);
    }
    match units {
        [one] => {
            if one.is_complete() {
                (format!("orbit x{}", one.members.len()), false)
            } else {
                (
                    format!("{}/{} orbit", one.members.len(), one.expected_len),
                    true,
                )
            }
        }
        // Every unit incomplete is a `mixed_fold` (not symmetric); a partly complete set is
        // a common, benign partial orbit, so it keeps its count.
        many => {
            let incomplete = many.iter().filter(|u| !u.is_complete()).count();
            if incomplete == 0 {
                (format!("{} orbits", many.len()), false)
            } else if incomplete == many.len() {
                ("not symmetric".to_string(), true)
            } else {
                (
                    format!("{} orbits ({incomplete} incomplete)", many.len()),
                    true,
                )
            }
        }
    }
}

/// Counts the tiers that would have incomplete orbits under a proposed symmetry order and mirror.
///
/// Works on a copy of `design.meta` with only the two symmetry fields replaced, so `design` is
/// not touched. Each tier is counted by the same rule as its orbit badge ([`orbit_status_text`]),
/// so the count agrees with the rows once the change is applied.
#[must_use]
pub fn tiers_incomplete_under_proposed_symmetry(
    design: &Design,
    symmetry_order: u32,
    mirror: bool,
) -> usize {
    let mut proposed_meta = design.meta.clone();
    proposed_meta.symmetry_order = symmetry_order;
    proposed_meta.mirror = mirror;
    design
        .tiers
        .iter()
        .filter(|tier| {
            let units = pleochroma_cut_core::orbit_units(&tier.indices, &proposed_meta);
            orbit_status_text(&units).1
        })
        .count()
}

/// One tier's display label for a validation-banner mention.
///
/// `"tier 5 (Girdle)"` when named, else `"tier 5"`. 1-based to match the tier table's own
/// `#` column, which is what a cutter actually reads off screen.
#[must_use]
pub fn tier_label(design: &Design, tier_index: usize) -> String {
    let Some(tier) = design.tiers.get(tier_index) else {
        return format!("tier {}", tier_index + 1);
    };
    if tier.name.is_empty() || pleochroma_cut_core::is_legacy_123_abc(&tier.name) {
        let labels = pleochroma_cut_core::compute_tier_labels(&design.tiers);
        labels.get(tier_index).map_or_else(
            || format!("tier {}", tier_index + 1),
            |l| format!("tier {} ({})", tier_index + 1, l.code),
        )
    } else {
        format!("tier {} ({})", tier_index + 1, tier.name)
    }
}

/// Sets [`TierRow::multi_selected`] on every row from `multi_selected`.
///
/// The row builders always clear the flag, so a caller that rebuilds the whole tier list applies
/// this afterwards. Toggling a selection changes nothing in `Design`, so the rows already shown
/// are patched rather than rebuilt.
pub fn apply_multi_selection(
    rows: &mut [TierRow],
    multi_selected: &std::collections::BTreeSet<usize>,
) {
    for row in rows {
        row.multi_selected =
            usize::try_from(row.index).is_ok_and(|index| multi_selected.contains(&index));
    }
}

/// The first name in `names` (a `MeetConstraint::MeetNamed` constraint's typed list) that
/// does not resolve against `design`'s current tiers.
///
/// Uses the same [`MeetNameResolver`] as the solver, so a name the solver would silently drop
/// is caught when the tier is saved.
///
/// `None` when every name resolves to a tier or is a meet-point word or connective
/// (`TokenResolution::Ignorable`).
#[must_use]
pub fn first_unresolved_meet_name(design: &Design, names: &[String]) -> Option<String> {
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    names
        .iter()
        .find(|name| matches!(resolver.resolve_token(name), TokenResolution::Unresolved))
        .cloned()
}

/// The design's own representative crown/pavilion facet angles, in degrees (magnitudes).
///
/// For the crown-window margin ([`tier_margin_and_risk`]) and the proportion verdict's angle
/// metrics: the tier whose name contains "main" (case-insensitive) on each side, or else the
/// tier with the largest magnitude on that side. `None` on a side with no tier.
///
/// The standard templates name their main facets "Crown Main" and "Pavilion Main", so they
/// resolve to the main facet; a hand-authored design falls back to the largest.
///
/// Sides come from [`classify_blocks`], not the sign of `angle_deg`: a girdle tier's angle is
/// about 90 degrees, so a sign test would read it as a crown or pavilion facet.
#[must_use]
pub fn representative_crown_and_pavilion_angles_deg(design: &Design) -> (Option<f64>, Option<f64>) {
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let mut crown_main: Option<f64> = None;
    let mut crown_largest: Option<f64> = None;
    let mut pavilion_main: Option<f64> = None;
    let mut pavilion_largest: Option<f64> = None;
    for (tier, block) in design.tiers.iter().zip(&blocks) {
        let is_main = tier.name.to_lowercase().contains("main");
        match block {
            Block::Crown => {
                let angle = tier.angle_deg;
                crown_largest = Some(crown_largest.map_or(angle, |m| angle.max(m)));
                if is_main {
                    crown_main = Some(crown_main.map_or(angle, |m| angle.max(m)));
                }
            }
            Block::Pavilion => {
                let magnitude = tier.angle_deg.abs();
                pavilion_largest = Some(pavilion_largest.map_or(magnitude, |m| magnitude.max(m)));
                if is_main {
                    pavilion_main = Some(pavilion_main.map_or(magnitude, |m| magnitude.max(m)));
                }
            }
            Block::Girdle => {}
        }
    }
    (
        crown_main.or(crown_largest),
        pavilion_main.or(pavilion_largest),
    )
}

/// A tier flatter than this (in degrees) is a table or culet, not a windowing question.
pub(super) const FLAT_TIER_DEG: f64 = 0.5;

/// The margin text and risk level for one tier, by `block`.
///
/// Pavilion tiers read the table-only critical-angle margin. Crown tiers read the crown-window
/// estimate against `pavilion_partner_deg`, suffixed `" (est.)"` so it is not taken for the
/// table-only certainty of a pavilion margin. A girdle tier, a flat table or culet (magnitude
/// below [`FLAT_TIER_DEG`], where light leaves straight by design), or a crown tier with no
/// pavilion angle reads `("", -1)`: nothing to show. `n_d` is the design's effective refractive
/// index.
///
/// `block` is the caller's [`classify_blocks`] result, not inferred from the angle: a girdle
/// tier's angle is about 90 degrees, so its size does not say which side it is on.
pub(super) fn tier_margin_and_risk(
    tier_angle_deg: f64,
    block: Block,
    n_d: f64,
    pavilion_partner_deg: Option<f64>,
) -> (String, i32) {
    let risk_level_of = |risk: pleochroma_cut_core::Risk| match risk {
        pleochroma_cut_core::Risk::Safe => 0,
        pleochroma_cut_core::Risk::Marginal => 1,
        pleochroma_cut_core::Risk::Windows => 2,
    };
    if tier_angle_deg.abs() < FLAT_TIER_DEG {
        return (String::new(), -1);
    }
    match block {
        Block::Pavilion => {
            let margin = pleochroma_cut_core::tier_margin_deg(tier_angle_deg, n_d);
            let risk_level =
                risk_level_of(pleochroma_cut_core::windowing_risk(tier_angle_deg, n_d));
            (format!("{margin:+.1}\u{b0}"), risk_level)
        }
        Block::Crown => {
            let Some(pavilion_deg) = pavilion_partner_deg else {
                return (String::new(), -1);
            };
            let margin =
                pleochroma_cut_core::crown_window_margin_deg(pavilion_deg, tier_angle_deg, n_d);
            let risk_level = risk_level_of(pleochroma_cut_core::crown_windowing_risk(
                pavilion_deg,
                tier_angle_deg,
                n_d,
            ));
            (format!("{margin:+.1}\u{b0} (est.)"), risk_level)
        }
        Block::Girdle => (String::new(), -1),
    }
}
