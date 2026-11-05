//! Adopts source-stated meets only when they reproduce the original geometry.

use pleochroma::geometry::{
    meet_solver::{MeetConstraint, MeetNameResolver, SolveStrategy, SolvedTier, TokenResolution},
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use pleochroma_cut_core::{
    Design,
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
};
use pleochroma_formats::asc::{AscTier, MeetInstruction};

/// The core's meet reproduction tolerance, as a fraction of original extent.
const GEOMETRY_TOLERANCE: f64 = 2e-6;
const ADOPTION_POLICY: PreservationPolicy = PreservationPolicy {
    max_junction_displacement: GEOMETRY_TOLERANCE,
    max_aspect_change: GEOMETRY_TOLERANCE,
    max_table_percent_change: 100.0 * GEOMETRY_TOLERANCE,
    max_depth_change: GEOMETRY_TOLERANCE,
    max_girdle_change: GEOMETRY_TOLERANCE,
    fixed_outline: true,
    lock_girdle_gauge: true,
    bounds_follow_turn: false,
};

#[derive(Default)]
pub struct Enrichment {
    pub adopted: usize,
    pub candidates: usize,
    pub notes: Vec<String>,
}

pub fn adopt_meets(design: &mut Design) -> Enrichment {
    let mut result = Enrichment::default();
    let candidates = candidates(design, &mut result.notes);
    result.candidates = candidates.len();
    if candidates.is_empty() {
        return result;
    }
    let certificate = match OriginalGeometry::capture(design) {
        Ok(certificate) => certificate,
        Err(reason) => {
            result.notes.push(format!(
                "Kept all {} source-stated meets pinned: {reason}",
                candidates.len()
            ));
            return result;
        }
    };

    // Try the complete set first: explicitly named meets can depend on each
    // other and need to be solved as a group. This never changes scale anchors.
    let mut together = design.clone();
    for &index in &candidates {
        adopt_constraint(&mut together, index);
    }
    if certificate.check(&together).is_ok() {
        *design = together;
        result.adopted = candidates.len();
    } else {
        adopt_individually(design, &candidates, &certificate, &mut result);
    }
    if result.adopted != 0 {
        result.notes.push(format!(
            "Adopted {}/{} source-stated meets; every accepted design reproduces the immutable original geometry within 2e-6 of its extent and passes the original meet gate. Original instructions are retained.",
            result.adopted, result.candidates
        ));
    }
    result
}

fn candidates(design: &Design, notes: &mut Vec<String>) -> Vec<usize> {
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    let mut candidates = Vec::new();
    for (index, tier) in design.tiers.iter().enumerate() {
        if !matches!(tier.constraint, MeetConstraint::ScaleReference(_))
            || design.tier_target(index).is_some()
        {
            continue;
        }
        let source = AscTier {
            notes: tier.original_notes.clone().unwrap_or_default(),
            ..AscTier::default()
        };
        let instruction = source.meet_instruction();
        if matches!(
            instruction,
            Some(
                MeetInstruction::Meet(_)
                    | MeetInstruction::CutToCenterpoint
                    | MeetInstruction::GirdleMeetPoint
            )
        ) && !positive_directive(&source.notes, &resolver)
        {
            notes.push(format!(
                "Tier {} ({:?}) stays pinned: its source meet wording is negative, conditional, ambiguous, or unresolved.",
                index + 1,
                tier.name
            ));
            continue;
        }
        let stated = match instruction {
            Some(MeetInstruction::Meet(names)) => {
                let lookup = resolver.resolve_names(&names);
                if !lookup.fully || lookup.refs.contains(&index) {
                    notes.push(format!(
                        "Tier {} ({:?}) stays pinned: its source meet names are unresolved or self-referential.",
                        index + 1,
                        tier.name
                    ));
                    continue;
                }
                MeetConstraint::MeetNamed(names)
            }
            Some(MeetInstruction::CutToCenterpoint | MeetInstruction::GirdleMeetPoint) => {
                MeetConstraint::MeetExisting
            }
            // Import also assigns MeetExisting to blank/unknown instructions.
            // That classification alone is not evidence of authored intent.
            _ => continue,
        };
        if tier.imported_meet.as_ref() == Some(&stated) {
            candidates.push(index);
        } else {
            notes.push(format!(
                "Tier {} ({:?}) stays pinned: its stored imported meet does not match the source instruction.",
                index + 1,
                tier.name
            ));
        }
    }
    candidates
}

/// Accept complete positive directives, not keyword matches inside free prose.
fn positive_directive(notes: &str, resolver: &MeetNameResolver<'_>) -> bool {
    let text = notes.trim();
    let text = text.strip_suffix('.').unwrap_or(text).trim();
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = normalized.to_ascii_lowercase();
    if let Some(destination) = lower.strip_prefix("cut to the ") {
        return matches!(
            destination,
            "centerpoint" | "center point" | "girdle meet point"
        );
    }
    let destination = lower.strip_prefix("cut to ").unwrap_or(&lower);
    if matches!(
        destination,
        "centerpoint" | "center point" | "tcp" | "pcp" | "gmp" | "girdle meet point"
    ) {
        return true;
    }
    positive_named_directive(text, resolver)
}

fn positive_named_directive(text: &str, resolver: &MeetNameResolver<'_>) -> bool {
    let mut tokens = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|token| !token.is_empty());
    if !tokens
        .next()
        .is_some_and(|token| token.eq_ignore_ascii_case("meet"))
    {
        return false;
    }
    let mut references = 0;
    let mut conjunction = false;
    let mut article = false;
    for token in tokens {
        let lower = token.to_ascii_lowercase();
        match lower.as_str() {
            "and" if references != 0 && !conjunction && !article => conjunction = true,
            "the" if !article => article = true,
            "and" | "the" => return false,
            // Even a coincidentally named tier cannot turn an ambiguous sentence
            // into an unconditional instruction. "or" is discarded by the
            // ordinary ASC parser, so inspect the raw wording before using it.
            _ => {
                // No explanatory clauses, parentheses, operators or punctuation
                // that the lenient importer could silently strip away.
                if !unambiguous_reference(token, resolver) {
                    return false;
                }
                references += 1;
                conjunction = false;
                article = false;
            }
        }
    }
    references != 0 && !conjunction && !article
}

fn unambiguous_reference(token: &str, resolver: &MeetNameResolver<'_>) -> bool {
    token
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && matches!(resolver.resolve_token(token), TokenResolution::Tiers(_))
        && token.split('-').all(|part| {
            // A compound reference must name real facets in every component;
            // the ordinary resolver also accepts ignored prose inside compounds.
            !matches!(
                part.to_ascii_lowercase().as_str(),
                "or" | "not"
                    | "no"
                    | "never"
                    | "avoid"
                    | "if"
                    | "unless"
                    | "when"
                    | "then"
                    | "else"
                    | "otherwise"
                    | "except"
                    | "without"
                    | "instead"
                    | "optional"
                    | "optionally"
                    | "maybe"
                    | "only"
                    | "before"
                    | "after"
                    | "until"
                    | "do"
                    | "may"
                    | "might"
                    | "should"
                    | "could"
                    | "would"
                    | "but"
                    | "can"
                    | "needed"
            ) && matches!(resolver.resolve_token(part), TokenResolution::Tiers(_))
        })
}

fn adopt_constraint(design: &mut Design, index: usize) {
    if let Some(constraint) = design.tiers[index].imported_meet.clone() {
        design.tiers[index].constraint = constraint;
    }
}

fn adopt_individually(
    design: &mut Design,
    candidates: &[usize],
    certificate: &OriginalGeometry,
    result: &mut Enrichment,
) {
    let mut pending = candidates.to_vec();
    loop {
        let mut refused = Vec::new();
        let mut accepted = 0;
        for &index in &pending {
            let mut candidate = design.clone();
            adopt_constraint(&mut candidate, index);
            match certificate.check(&candidate) {
                Ok(()) => {
                    *design = candidate;
                    accepted += 1;
                }
                Err(reason) => refused.push((index, reason)),
            }
        }
        result.adopted += accepted;
        if accepted == 0 || refused.is_empty() {
            for (index, reason) in refused {
                result.notes.push(format!(
                    "Tier {} ({:?}) stays pinned: {reason}",
                    index + 1,
                    design.tiers[index].name
                ));
            }
            break;
        }
        // A later adoption may make an earlier dependency solvable. Successful
        // tiers leave the queue permanently, so these passes always terminate.
        pending = refused.into_iter().map(|(index, _)| index).collect();
    }
}

struct OriginalGeometry {
    pinned: Design,
    solved: Vec<SolvedTier>,
    baseline: MeetPreservationBaseline,
    mast_tolerance: f64,
}

impl OriginalGeometry {
    fn capture(design: &Design) -> Result<Self, String> {
        let solved = solve_checked(design)?;
        let planes = design.planes_from_solved(&solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            return Err("the original design does not form a closed planar solid".to_owned());
        };
        let Some(&first) = mesh.positions.first() else {
            return Err("the original solid has no vertices".to_owned());
        };
        let (lo, hi) = mesh
            .positions
            .iter()
            .fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let extent = (hi - lo).max_element();
        if !extent.is_finite() || extent <= 0.0 {
            return Err("the original solid has an invalid extent".to_owned());
        }
        let pinned = pinned_witness(design, &solved);
        if pinned.planes_from_solved(&solve_checked(&pinned)?) != planes {
            return Err("the pinned witness does not reproduce the original planes".to_owned());
        }
        let baseline = MeetPreservationBaseline::capture(&pinned)
            .map_err(|error| format!("the original meet network cannot be certified: {error}"))?;
        Ok(Self {
            pinned,
            solved,
            baseline,
            mast_tolerance: extent * GEOMETRY_TOLERANCE,
        })
    }

    fn check(&self, candidate: &Design) -> Result<(), String> {
        let solved = solve_checked(candidate)?;
        if solved.len() != self.solved.len() {
            return Err("adoption changed the tier count".to_owned());
        }
        for (index, (original, adopted)) in self.solved.iter().zip(&solved).enumerate() {
            if (original.mast - adopted.mast).abs() > self.mast_tolerance {
                return Err(format!(
                    "adoption would move tier {} beyond the original geometry tolerance",
                    index + 1
                ));
            }
        }

        // The core gate rejects changed authored constraints as stale. Certify
        // a pinned witness with the original authoring instead. Exact equality
        // of every production plane proves it is the actual adopted geometry;
        // the immutable baseline still checks every original meet and boundary.
        // This is not a recapture of the candidate, nor a relaxed meet policy.
        let witness = pinned_witness(&self.pinned, &solved);
        if witness.planes_from_solved(&solve_checked(&witness)?)
            != candidate.planes_from_solved(&solved)
        {
            return Err("the adopted geometry differs from its pinned witness".to_owned());
        }
        self.baseline
            .validate(&witness, ADOPTION_POLICY)
            .map(|_| ())
            .map_err(|error| format!("the original meet gate refused adoption: {error}"))
    }
}

fn pinned_witness(design: &Design, solved: &[SolvedTier]) -> Design {
    let mut pinned = design.clone();
    // These targets are retained on the actual design. Only the private witness
    // uses already-solved depths, so a target cannot override a witness mast.
    pinned.tier_targets.clear();
    for (tier, solved) in pinned.tiers.iter_mut().zip(solved) {
        tier.constraint = MeetConstraint::ScaleReference(solved.mast);
    }
    pinned
}

fn solve_checked(design: &Design) -> Result<Vec<SolvedTier>, String> {
    let solved = design.solve().map_err(|error| error.to_string())?;
    if solved.len() != design.tiers.len()
        || solved.iter().any(|tier| {
            !tier.mast.is_finite()
                || matches!(
                    tier.strategy,
                    SolveStrategy::Failed | SolveStrategy::LeastSquaresFallback
                )
        })
    {
        return Err(
            "the meet solver did not produce exact finite depths for every tier".to_owned(),
        );
    }
    Ok(solved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{
        PreformSpec,
        design::{ConstraintTier, ScheduleMeta},
    };

    fn octahedron() -> Design {
        let tier = |name, angle_deg, indices| ConstraintTier {
            name,
            angle_deg,
            indices,
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        };
        let mut tiers: Vec<_> = [0.0, 24.0, 48.0, 72.0]
            .into_iter()
            .enumerate()
            .map(|(i, index)| tier(format!("C{i}"), 45.0, vec![index]))
            .collect();
        tiers.push(tier("P".into(), -45.0, vec![0.0, 24.0, 48.0, 72.0]));
        Design::new(
            PreformSpec::block(4.0, 4.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            tiers,
        )
    }

    fn with_stated_meet() -> Design {
        let mut design = octahedron();
        design.tiers[1].imported_meet = Some(MeetConstraint::MeetNamed(vec![
            "C0".into(),
            "C2".into(),
            "C3".into(),
        ]));
        design.tiers[1].original_notes = Some("Meet C0, C2, C3".into());
        design
    }

    #[test]
    fn unspecified_import_classification_is_not_authored_intent() {
        for notes in [None, Some(""), Some("Polish carefully")] {
            let mut design = octahedron();
            design.tiers[1].imported_meet = Some(MeetConstraint::MeetExisting);
            design.tiers[1].original_notes = notes.map(str::to_owned);
            let original = design.clone();
            let report = adopt_meets(&mut design);
            assert_eq!(report.candidates, 0);
            assert_eq!(report.adopted, 0);
            assert_eq!(design, original);
        }
    }

    #[test]
    fn negative_or_conditional_source_prose_never_unlocks_a_tier() {
        for notes in [
            "Do not cut to centerpoint",
            "Avoid GMP",
            "Cut to centerpoint if needed",
            "Cut to GMP unless the girdle is thin",
            "Centerpoint (optional)",
            "Meet C0 or C2",
            "Meet C0-or-C2",
            "Meet C0 and C2 if needed",
            "Meet C0, not C2",
            "Meet C0 and C2; leave C3",
            "Meet C0/C2",
        ] {
            let mut design = with_stated_meet();
            let source = AscTier {
                notes: notes.into(),
                ..AscTier::default()
            };
            // Reproduce the lenient importer's classification. Its recognition
            // alone must not be enough to authorize constraint adoption.
            design.tiers[1].imported_meet = Some(match source.meet_instruction() {
                Some(MeetInstruction::Meet(names)) => MeetConstraint::MeetNamed(names),
                _ => MeetConstraint::MeetExisting,
            });
            design.tiers[1].original_notes = Some(notes.into());
            let original = design.clone();
            let report = adopt_meets(&mut design);
            assert_eq!(report.candidates, 0, "{notes}");
            assert_eq!(report.adopted, 0, "{notes}");
            assert_eq!(design, original, "{notes}");
            assert!(
                report.notes.iter().any(|note| note.contains("wording")),
                "{notes}"
            );
        }
    }

    #[test]
    fn direct_positive_instructions_remain_eligible() {
        let design = octahedron();
        let inputs = design.meet_tier_inputs();
        let resolver = MeetNameResolver::new(&inputs);
        for notes in [
            "Centerpoint",
            "Center point.",
            "TCP",
            "PCP",
            "GMP",
            "Girdle meet point",
            "Cut to centerpoint",
            "Cut to center point",
            "Cut to TCP",
            "Cut to PCP",
            "Cut to GMP",
            "Cut to girdle meet point",
            "Cut to the centerpoint",
            "Cut to the center point",
            "Cut to the girdle meet point",
            "Meet C0, C2, C3",
            "Meet C0 C2 C3.",
            "Meet C0 and C2",
            "  MEET  C0, C2 and the C3.  ",
            "Meet C0-C2-C3",
        ] {
            assert!(positive_directive(notes, &resolver), "{notes}");
        }
    }

    #[test]
    fn a_negative_word_cannot_be_laundered_through_a_matching_tier_name() {
        let mut design = octahedron();
        design.tiers[2].name = "not".into();
        let inputs = design.meet_tier_inputs();
        let resolver = MeetNameResolver::new(&inputs);
        assert!(!positive_directive("Meet C0, not C3", &resolver));
        assert!(!positive_directive("Meet C0-not-C3", &resolver));
    }

    #[test]
    fn unknown_named_support_is_not_silently_dropped() {
        let mut design = with_stated_meet();
        design.tiers[1].imported_meet = Some(MeetConstraint::MeetNamed(vec![
            "C0".into(),
            "MissingSupport".into(),
        ]));
        design.tiers[1].original_notes = Some("Meet C0, MissingSupport".into());
        let original = design.clone();
        let report = adopt_meets(&mut design);
        assert_eq!(report.adopted, 0);
        assert_eq!(design, original);
        assert!(report.notes.iter().any(|note| note.contains("unresolved")));
    }

    #[test]
    fn a_source_dimension_keeps_its_authoritative_target() {
        let mut design = with_stated_meet();
        design.tier_targets.insert(
            design.tier_ids[1],
            pleochroma_cut_core::design::TierTarget::DepthMm(3.0),
        );
        let original = design.clone();
        let report = adopt_meets(&mut design);
        assert_eq!(report.candidates, 0);
        assert_eq!(design, original);
    }

    #[test]
    fn explicit_meet_is_adopted_without_moving_the_original_stone() {
        let mut design = with_stated_meet();
        let original = design.clone();
        let certificate = OriginalGeometry::capture(&original).unwrap();
        let report = adopt_meets(&mut design);
        assert_eq!(report.adopted, 1, "{:?}", report.notes);
        assert_eq!(report.candidates, 1);
        assert!(matches!(
            design.tiers[1].constraint,
            MeetConstraint::MeetNamed(_)
        ));
        certificate.check(&design).unwrap();
        let mut expected = original;
        expected.tiers[1].constraint = design.tiers[1].constraint.clone();
        assert_eq!(design, expected);
        assert_eq!(adopt_meets(&mut design).adopted, 0);
    }

    #[test]
    fn a_changed_depth_cannot_be_certified_against_a_new_baseline() {
        let design = octahedron();
        let certificate = OriginalGeometry::capture(&design).unwrap();
        let mut moved = design;
        moved.tiers[0].constraint = MeetConstraint::ScaleReference(0.51);
        assert!(certificate.check(&moved).is_err());
    }

    #[test]
    fn an_inconsistent_stated_meet_preserves_the_recorded_depth() {
        let mut design = with_stated_meet();
        // The three referenced 45-degree planes still meet at the old apex.
        // A 30-degree plane through that apex needs a different mast than 0.5.
        design.tiers[1].angle_deg = 30.0;
        let original = design.clone();
        let report = adopt_meets(&mut design);
        assert_eq!(report.candidates, 1);
        assert_eq!(report.adopted, 0);
        assert_eq!(design, original);
        assert!(!report.notes.is_empty());
    }

    #[test]
    fn invalid_original_leaves_all_constraints_untouched() {
        let mut design = with_stated_meet();
        design.tiers[0].angle_deg = f64::INFINITY;
        let original = design.clone();
        let report = adopt_meets(&mut design);
        assert_eq!(report.adopted, 0);
        assert_eq!(design, original);
        assert!(!report.notes.is_empty());
    }
}
