//! Exact-state certificates from the original production gate, retained only
//! inside one immutable episode projection. No tolerance or hash-based key match.
#[cfg(any(feature = "cuda", test))]
use super::network::Plane;
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::{
    Design,
    meet_preservation::{PreservationFacts, PreservationPolicy, ShapeMeasures},
};
use std::{collections::VecDeque, sync::Mutex};

const CAPACITY: usize = 8;

/// Only completed CUDA arrangement refutations belong here.
///
/// The owning projection fixes all proof metadata. Keys retain production plane
/// order and every coefficient bit, including signed zero. Eight 256-plane keys
/// retain at most 64 KiB of coefficient payload per episode.
#[cfg(any(feature = "cuda", test))]
#[derive(Default)]
pub(super) struct ArrangementRefutations(Mutex<VecDeque<Vec<[u64; 4]>>>);

#[cfg(any(feature = "cuda", test))]
fn plane_bits((normal, distance): &Plane) -> [u64; 4] {
    [
        normal.x.to_bits(),
        normal.y.to_bits(),
        normal.z.to_bits(),
        distance.to_bits(),
    ]
}

#[cfg(any(feature = "cuda", test))]
impl ArrangementRefutations {
    pub fn contains(&self, planes: &[Plane]) -> bool {
        self.0.lock().is_ok_and(|entries| {
            entries.iter().rev().any(|entry| {
                entry.len() == planes.len()
                    && entry.iter().copied().eq(planes.iter().map(plane_bits))
            })
        })
    }

    pub fn insert_refuted(&self, planes: &[Plane]) {
        if !(3..=256).contains(&planes.len())
            || planes
                .iter()
                .any(|(normal, distance)| !normal.is_finite() || !distance.is_finite())
        {
            return;
        }
        let key = planes.iter().map(plane_bits).collect::<Vec<_>>();
        if let Ok(mut entries) = self.0.lock() {
            if let Some(index) = entries.iter().position(|entry| *entry == key) {
                entries.remove(index);
            }
            entries.push_back(key);
            while entries.len() > CAPACITY {
                entries.pop_front();
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Certificates {
    accepted: Mutex<VecDeque<Certificate>>,
    rejected: Mutex<VecDeque<(Design, String)>>,
}

#[derive(Clone)]
struct Certificate {
    design: Design,
    facts: PreservationFacts,
    solved: Vec<SolvedTier>,
}

impl Certificates {
    pub fn insert(&self, design: &Design, facts: PreservationFacts, solved: Vec<SolvedTier>) {
        if let Ok(mut entries) = self.accepted.lock() {
            if let Some(i) = entries
                .iter()
                .position(|entry| same_state(&entry.design, design))
            {
                entries.remove(i);
            }
            entries.push_back(Certificate {
                design: design.clone(),
                facts,
                solved,
            });
            while entries.len() > CAPACITY {
                entries.pop_front();
            }
        }
    }

    pub fn facts(&self, design: &Design) -> Option<PreservationFacts> {
        self.accepted
            .lock()
            .ok()?
            .iter()
            .rev()
            .find(|entry| same_state(&entry.design, design))
            .map(|entry| entry.facts)
    }

    pub fn solved(&self, design: &Design) -> Option<Vec<SolvedTier>> {
        self.accepted
            .lock()
            .ok()?
            .iter()
            .rev()
            .find(|entry| same_state(&entry.design, design))
            .map(|entry| entry.solved.clone())
    }

    /// Only the original policy's deterministic refusal belongs here. A stricter
    /// request/travel refusal must never poison a later, wider request.
    pub fn insert_rejection(&self, design: &Design, error: String) {
        if let Ok(mut entries) = self.rejected.lock() {
            if let Some(i) = entries.iter().position(|(old, _)| same_state(old, design)) {
                entries.remove(i);
            }
            entries.push_back((design.clone(), error));
            while entries.len() > CAPACITY {
                entries.pop_front();
            }
        }
    }

    pub fn rejection(&self, design: &Design) -> Option<String> {
        self.rejected
            .lock()
            .ok()?
            .iter()
            .rev()
            .find(|(old, _)| same_state(old, design))
            .map(|(_, error)| error.clone())
    }
}

pub(crate) fn same_state(a: &Design, b: &Design) -> bool {
    a == b
        && a.tier_ids_eq(b)
        && a.tier_ids == b.tier_ids
        && a.concave_tier_ids == b.concave_tier_ids
        && a.tier_targets == b.tier_targets
        && a.tier_relations == b.tier_relations
        && a.tier_frosting == b.tier_frosting
        && a.tier_surfaces == b.tier_surfaces
        && a.tiers.iter().zip(&b.tiers).all(|(a, b)| {
            a.angle_deg.to_bits() == b.angle_deg.to_bits()
                && a.indices
                    .iter()
                    .map(|v| v.to_bits())
                    .eq(b.indices.iter().map(|v| v.to_bits()))
                && match (&a.constraint, &b.constraint) {
                    (
                        pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference(a),
                        pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference(b),
                    ) => a.to_bits() == b.to_bits(),
                    _ => true,
                }
        })
}

/// Pure arithmetic adapted from meet_preservation/shape.rs::validate and
/// network.rs::validate_solved. The production gate has already certified all
/// topology, incidence, gauge and outline requirements under the original policy.
/// Only these shape/displacement limits change when a request grants less travel
/// than the finished candidate's own turn. Rechecking them avoids a second solve
/// and arrangement walk for Retarget while retaining the exact requested policy.
pub(super) fn requested_shape_holds(
    before: ShapeMeasures,
    facts: PreservationFacts,
    policy: PreservationPolicy,
) -> bool {
    let optional = |a: Option<f64>, b: Option<f64>, limit: f64| match (a, b) {
        (Some(a), Some(b)) => a.is_finite() && b.is_finite() && (a - b).abs() <= limit,
        (None, None) => true,
        (None, Some(b)) => b.is_finite() && b.abs() <= limit,
        (Some(_), None) => false,
    };
    [
        policy.max_aspect_change,
        policy.max_table_percent_change,
        policy.max_depth_change,
        policy.max_girdle_change,
        policy.max_junction_displacement,
    ]
    .into_iter()
    .all(|limit| limit.is_finite() && limit >= 0.0)
        && facts.shape.aspect.is_finite()
        && (before.aspect - facts.shape.aspect).abs() <= policy.max_aspect_change
        && optional(
            before.table_percent,
            facts.shape.table_percent,
            policy.max_table_percent_change,
        )
        && optional(before.crown, facts.shape.crown, policy.max_depth_change)
        && optional(
            before.pavilion,
            facts.shape.pavilion,
            policy.max_depth_change,
        )
        && optional(before.girdle, facts.shape.girdle, policy.max_girdle_change)
        && facts.shape_distance <= policy.max_junction_displacement
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::meet_preservation::MeetPreservationBaseline;

    #[test]
    fn arrangement_refutations_require_exact_ordered_finite_plane_bits() {
        use glam::DVec3;
        let planes = vec![(DVec3::X, 0.0), (DVec3::Y, 1.0), (DVec3::Z, 2.0)];
        let cache = ArrangementRefutations::default();
        cache.insert_refuted(&planes);
        assert!(cache.contains(&planes));
        let mut changed = planes.clone();
        changed[0].1 = -0.0;
        assert!(!cache.contains(&changed));
        changed = planes.clone();
        changed[1].0.x = 0.0_f64.next_up();
        assert!(!cache.contains(&changed));
        changed = planes.clone();
        changed.swap(0, 1);
        assert!(!cache.contains(&changed));
        assert!(!ArrangementRefutations::default().contains(&planes));
        changed[0].1 = f64::INFINITY;
        cache.insert_refuted(&changed);
        assert!(!cache.contains(&changed));
    }

    #[test]
    fn arrangement_refutations_bound_key_count_and_plane_payload() {
        use glam::DVec3;
        let cache = ArrangementRefutations::default();
        let mut planes = vec![(DVec3::X, 0.0); 3];
        for index in 0..=CAPACITY {
            planes[0].1 = index as f64;
            cache.insert_refuted(&planes);
        }
        assert!(cache.contains(&planes));
        planes[0].1 = 0.0;
        assert!(!cache.contains(&planes));
        let oversized = vec![(DVec3::X, 1.0); 257];
        cache.insert_refuted(&oversized);
        assert!(!cache.contains(&oversized));
        assert_eq!(cache.0.lock().unwrap().len(), CAPACITY);
    }

    #[test]
    fn identity_changes_never_reuse_a_certificate_even_when_design_partial_eq_matches() {
        let mut design = Design::concave_fixture();
        design.concave_tiers.clear();
        design.concave_tier_ids.clear();
        let baseline = MeetPreservationBaseline::capture(&design).expect("baseline");
        let (facts, solved) = baseline
            .validate_and_solve_with(&design, PreservationPolicy::OPTIMIZE, None)
            .expect("original production certificate");
        let cache = Certificates::default();
        cache.insert(&design, facts, solved);
        let mut other = design.clone();
        other.tier_ids[0] = pleochroma_cut_core::TierId(10_000);
        assert_eq!(
            other, design,
            "Design PartialEq omits bookkeeping identities"
        );
        assert!(!other.tier_ids_eq(&design));
        assert!(cache.facts(&other).is_none());
        assert!(cache.solved(&other).is_none());
        assert_eq!(cache.facts(&design), Some(facts));
    }

    #[test]
    fn signed_zero_angle_is_a_different_crown_pavilion_state() {
        let mut crown = Design::concave_fixture();
        crown.tiers[0].angle_deg = 0.0;
        let mut pavilion = crown.clone();
        pavilion.tiers[0].angle_deg = -0.0;
        assert_eq!(crown, pavilion, "f64 PartialEq equates signed zero");
        assert!(!same_state(&crown, &pavilion));
    }

    #[test]
    fn request_limits_recheck_all_optional_shape_and_displacement_measures() {
        let mut design = Design::concave_fixture();
        design.concave_tiers.clear();
        design.concave_tier_ids.clear();
        let baseline = MeetPreservationBaseline::capture(&design).expect("baseline");
        let mut facts = baseline
            .validate(&design, PreservationPolicy::OPTIMIZE)
            .expect("facts");
        let before = baseline.shape();
        let policy = PreservationPolicy::RETARGET;
        facts.shape = before;
        facts.shape_distance = policy.max_junction_displacement;
        assert!(requested_shape_holds(before, facts, policy));
        facts.shape_distance = policy.max_junction_displacement.next_up();
        assert!(!requested_shape_holds(before, facts, policy));
        facts.shape_distance = 0.0;
        let mut start = before;
        start.crown = None;
        facts.shape.crown = Some(policy.max_depth_change);
        assert!(requested_shape_holds(start, facts, policy));
        facts.shape.crown = Some(policy.max_depth_change.next_up());
        assert!(!requested_shape_holds(start, facts, policy));
        start.crown = Some(0.1);
        facts.shape.crown = None;
        assert!(
            !requested_shape_holds(start, facts, policy),
            "lost measurable geometry is never zero"
        );
    }
}
