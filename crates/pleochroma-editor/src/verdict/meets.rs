//! Two things a cutter meets at the wheel that the finished stone alone does not show: a tier
//! with no point to cut to when it is cut ([`pleochroma_cut_core::Design::cut_meets`], "no meet
//! point"), and a girdle that tiers cut into ([`pleochroma_cut_core::Design::girdle_level`]).
//!
//! [`meet_facts`] and [`girdle_fact`] read the facts off the design for [`super::gather()`];
//! [`meet_point_reason`] and [`girdle_level_reason`] turn them into reasons for
//! [`super::evaluate`]. Lengths are written in millimetres when the design's girdle diameter
//! gives a scale ([`VerdictInputs::mm_per_unit`]), otherwise as percentages: a miss of the
//! stone's size, a girdle thickness of its width.

use super::{FixAction, Level, Reason, ReasonKind, VerdictInputs, plural, reason};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{CutMeets, DEFAULT_SNAP_MAX_MOVE_FRACTION, Design, GirdleLevel};

/// A tier the cutter has no point to cut to ([`ReasonKind::NoMeetPoint`]).
#[derive(Debug, Clone, PartialEq)]
pub struct FloatingTier {
    /// The tier's position in `design.tiers`.
    pub tier: usize,
    /// The tier's name (may be empty).
    pub name: String,
    /// How far its plane misses the nearest point already on the stone when it is cut, in design
    /// (mast) units.
    pub miss: f64,
    /// Whether [`FixAction::SnapToMeets`] can move it: an explicit-depth
    /// ([`MeetConstraint::ScaleReference`]) tier, with no authored target, whose miss is at most
    /// [`DEFAULT_SNAP_MAX_MOVE_FRACTION`] of the stone's size.
    pub snappable: bool,
}

/// What the cut-so-far meets say about the design's tiers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MeetFacts {
    /// The tiers with no point to cut to, in cutting order. Empty when every tier has one.
    pub floating: Vec<FloatingTier>,
    /// The finished stone's size (its largest extent), in design units: what a miss is a
    /// percentage of when there are no millimetres.
    pub size: f64,
}

/// A girdle that is not level ([`ReasonKind::GirdleNotLevel`]).
#[derive(Debug, Clone, PartialEq)]
pub struct GirdleLevelFact {
    /// The range the girdle band's thickness lies in at its line corners (where girdle faces
    /// meet, and the ends of level edges), thinnest and thickest, in design units
    /// ([`GirdleLevel::thickness_range`]).
    pub thickness: (f64, f64),
    /// The stone's width, in design units: what the thickness is a percentage of when there are
    /// no millimetres.
    pub width: f64,
    /// The tiers that cut into the girdle, in cutting order: each tier's position in
    /// `design.tiers` and its name. Never empty.
    pub cut_in: Vec<(usize, String)>,
}

/// The name of tier `tier` of `design` (empty for a position the design lacks).
fn name_of(design: &Design, tier: usize) -> String {
    design
        .tiers
        .get(tier)
        .map_or_else(String::new, |found| found.name.clone())
}

/// Whether tier `tier` of `design` is placed by an explicit depth of its own, the only kind
/// [`FixAction::SnapToMeets`] moves: a [`MeetConstraint::ScaleReference`] tier without an
/// authored target (a target, not the mast, sets such a tier's depth, so the snap leaves it).
fn has_explicit_depth(design: &Design, tier: usize) -> bool {
    design
        .tiers
        .get(tier)
        .is_some_and(|found| matches!(found.constraint, MeetConstraint::ScaleReference(_)))
        && design.tier_target(tier).is_none()
}

/// The facts [`meet_point_reason`] reads, from `design`'s cut-so-far `meets`.
pub(super) fn meet_facts(design: &Design, meets: &CutMeets) -> MeetFacts {
    let limit = DEFAULT_SNAP_MAX_MOVE_FRACTION * meets.size;
    MeetFacts {
        floating: meets
            .floating(design)
            .into_iter()
            .map(|(tier, miss)| FloatingTier {
                tier,
                name: name_of(design, tier),
                miss,
                snappable: has_explicit_depth(design, tier) && miss <= limit,
            })
            .collect(),
        size: meets.size,
    }
}

/// The fact [`girdle_level_reason`] reads, from `design`'s girdle `level`. `None` for a level
/// girdle.
pub(super) fn girdle_fact(design: &Design, level: &GirdleLevel) -> Option<GirdleLevelFact> {
    (!level.is_level()).then(|| GirdleLevelFact {
        thickness: level.thickness_range(),
        width: level.width,
        cut_in: level
            .cut_in
            .iter()
            .map(|&tier| (tier, name_of(design, tier)))
            .collect(),
    })
}

/// `items` as a list in a sentence: `A`, `A and B`, `A, B and C`.
pub(super) fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A miss as the reason writes it: `0.12 mm` with a scale, `1.2 % of the stone's size` without.
fn miss_text(miss: f64, size: f64, mm_per_unit: Option<f64>) -> String {
    match mm_per_unit {
        Some(scale) => format!("{:.2} mm", miss * scale),
        None if size > 0.0 => format!("{:.1} % of the stone's size", 100.0 * miss / size),
        None => format!("{miss:.4} in mast units"),
    }
}

/// The reason for the tiers with no point to cut to: one line naming every one with its miss.
///
/// It offers [`FixAction::SnapToMeets`] when at least one of them can move
/// ([`FloatingTier::snappable`]), and asks first, since the fix changes the design.
pub(super) fn meet_point_reason(inputs: &VerdictInputs) -> Option<Reason> {
    let facts = inputs.meets.as_ref()?;
    let first = facts.floating.first()?;
    let tiers: Vec<String> = facts
        .floating
        .iter()
        .map(|floating| {
            format!(
                "{} ({} off)",
                inputs.label(&floating.name, floating.tier),
                miss_text(floating.miss, facts.size, inputs.mm_per_unit)
            )
        })
        .collect();
    let text = if tiers.len() == 1 {
        format!(
            "1 tier has no meet point: {}. A cutter has to set its depth by measurement.",
            listed(&tiers)
        )
    } else {
        format!(
            "{} tiers have no meet point: {}. A cutter has to set their depth by measurement.",
            tiers.len(),
            listed(&tiers)
        )
    };
    let mut found = reason(ReasonKind::NoMeetPoint, Level::Check, text);
    found.tier = Some(first.tier);
    let movable = facts
        .floating
        .iter()
        .filter(|floating| floating.snappable)
        .count();
    if movable > 0 {
        found.fix = Some(FixAction::SnapToMeets);
        found.confirm = Some(format!(
            "Move the depth of {movable} tier{} to the nearest meet point? This changes the \
             design; check the optics afterwards.",
            plural(movable)
        ));
    }
    Some(found)
}

/// The reason for a girdle that is not level: how thick it runs and which tiers cut into it.
/// There is no fix.
pub(super) fn girdle_level_reason(inputs: &VerdictInputs) -> Option<Reason> {
    let fact = inputs.girdle_level.as_ref()?;
    let &(first, _) = fact.cut_in.first()?;
    // The two ends come from different places round the girdle, so the thinner one can come out
    // below zero on a girdle that tilts; a band is never thinner than nothing.
    let thinnest = fact.thickness.0.max(0.0);
    let thickest = fact.thickness.1.max(0.0);
    let range = match inputs.mm_per_unit {
        Some(scale) => format!("{:.2} to {:.2} mm", thinnest * scale, thickest * scale),
        None if fact.width > 0.0 => format!(
            "{:.1} to {:.1} % of the stone's width",
            100.0 * thinnest / fact.width,
            100.0 * thickest / fact.width
        ),
        None => format!("{thinnest:.4} to {thickest:.4} in mast units"),
    };
    let names: Vec<String> = fact
        .cut_in
        .iter()
        .map(|(tier, name)| inputs.label(name, *tier))
        .collect();
    let verb = if names.len() == 1 { "cuts" } else { "cut" };
    let mut found = reason(
        ReasonKind::GirdleNotLevel,
        Level::Check,
        format!(
            "The girdle is not level: it is {range} thick. {} {verb} into it.",
            listed(&names)
        ),
    );
    found.tier = Some(first);
    Some(found)
}
