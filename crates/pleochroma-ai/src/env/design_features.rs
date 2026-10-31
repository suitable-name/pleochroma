//! Bounded, explicit geometry and constraint context for the policy.
//!
//! The original signed planes, full facet orbits and preform identify the operation-start
//! arrangement on which meet preservation depends. They are not a low-order moment sketch.
//! Values are converted to f32 for training: this is not a bit-exact encoding of f64 geometry.
//! Static suffixes are shared across rollout observations; padding is created only when packing.

use std::{collections::BTreeMap, sync::Arc};

use pleochroma::geometry::meet_solver::{MeetConstraint, MeetNameResolver, SolvedTier};
use pleochroma_cut_core::{
    Design,
    design::{FacetSide, TierHinge, TierTarget},
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
};
use serde::{Deserialize, Serialize};

#[path = "design_features_program.rs"]
mod program;
#[path = "design_features_rough.rs"]
mod rough;
#[cfg(test)]
#[path = "design_features_tests.rs"]
mod tests;

/// Maximum explicit placements per planar or concave tier; larger orbits are rejected.
pub const MAX_ORBIT_POSITIONS: usize = 128;
/// Maximum prefix expression nodes in one angle relation; larger programs are rejected.
pub const MAX_RELATION_NODES: usize = 32;
/// Maximum resolved named-meet participants in one tier.
pub const MAX_NAMED_MEETS: usize = 16;
/// Maximum explicit half-spaces for a custom rough (analytic cylinders need no plane list).
pub const MAX_ROUGH_PLANES: usize = 32;
/// Maximum concave tools whose geometry fits the global context.
pub const MAX_CONCAVE_TIERS: usize = 8;

const TIER_HEADER: usize = 20;
const TIER_STATIC_DIM: usize =
    TIER_HEADER + MAX_ORBIT_POSITIONS * 2 + MAX_NAMED_MEETS + MAX_RELATION_NODES * 2;

/// Geometry suffix for one tier, with cheap mutable mast readback.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "SerializedTierDesignFeatures")]
pub struct TierDesignFeatures {
    static_features: Arc<[f32]>,
    /// Current signed solved mast in operation-start stone-width units.
    pub current_mast: f32,
    /// Current authored scale anchor in the same units (zero for a meet-derived tier).
    pub current_anchor: f32,
}

#[derive(Deserialize)]
struct SerializedTierDesignFeatures {
    static_features: Arc<[f32]>,
    current_mast: f32,
    current_anchor: f32,
}

impl TryFrom<SerializedTierDesignFeatures> for TierDesignFeatures {
    type Error = String;

    fn try_from(raw: SerializedTierDesignFeatures) -> Result<Self, Self::Error> {
        let features = Self {
            static_features: raw.static_features,
            current_mast: raw.current_mast,
            current_anchor: raw.current_anchor,
        };
        if !features.valid_width()
            || !features
                .static_features
                .iter()
                .all(|value| value.is_finite())
            || !features.current_mast.is_finite()
            || !features.current_anchor.is_finite()
        {
            return Err("invalid geometry tier suffix width or non-finite input".into());
        }
        Ok(features)
    }
}

impl TierDesignFeatures {
    /// Geometry, dependencies, authored facet frosting, and two live masts.
    pub const FEATURE_DIM: usize = TIER_STATIC_DIM + 2;

    /// Whether this suffix is absent (legacy/default synthetic observation) or complete.
    #[must_use]
    pub fn valid_width(&self) -> bool {
        self.static_features.is_empty() || self.static_features.len() == TIER_STATIC_DIM
    }

    /// Packs this suffix without ever truncating a populated representation.
    ///
    /// # Panics
    /// Panics if an internal caller violates the suffix-width invariant.
    #[must_use]
    pub fn to_feature_vector(&self) -> Vec<f32> {
        assert!(self.valid_width(), "invalid design tier suffix width");
        let mut values = self.static_features.to_vec();
        values.resize(TIER_STATIC_DIM, 0.0);
        values.extend([self.current_mast, self.current_anchor]);
        values
    }
}

/// Immutable rough, tool and preservation context shared by an episode's observations.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(try_from = "SerializedGlobalDesignFeatures")]
pub struct GlobalDesignFeatures {
    features: Arc<[f32]>,
    /// Current optical-score width divided by its episode-start value.
    pub current_width_ratio: f32,
}

#[derive(Deserialize)]
struct SerializedGlobalDesignFeatures {
    features: Arc<[f32]>,
    current_width_ratio: f32,
}

impl TryFrom<SerializedGlobalDesignFeatures> for GlobalDesignFeatures {
    type Error = String;

    fn try_from(raw: SerializedGlobalDesignFeatures) -> Result<Self, Self::Error> {
        let features = Self {
            features: raw.features,
            current_width_ratio: raw.current_width_ratio,
        };
        if !features.valid_width()
            || !features.features.iter().all(|value| value.is_finite())
            || !features.current_width_ratio.is_finite()
        {
            return Err("invalid geometry global suffix width or non-finite input".into());
        }
        Ok(features)
    }
}

impl GlobalDesignFeatures {
    /// Rough header, custom half-spaces and every supported concave placement.
    pub const FEATURE_DIM: usize = rough::GLOBAL_DESIGN_DIM + 1;

    /// Whether a suffix is absent or has the complete current width.
    #[must_use]
    pub fn valid_width(&self) -> bool {
        self.features.is_empty() || self.features.len() == rough::GLOBAL_DESIGN_DIM
    }

    /// Packs the complete suffix; an absent synthetic fixture gets zero context.
    ///
    /// # Panics
    /// Panics if an internal caller violates the suffix-width invariant.
    #[must_use]
    pub fn to_feature_vector(&self) -> Vec<f32> {
        assert!(self.valid_width(), "invalid design global suffix width");
        let mut values = self.features.to_vec();
        values.resize(rough::GLOBAL_DESIGN_DIM, 0.0);
        values.push(self.current_width_ratio);
        values
    }
}

/// Rejects geometry or constraint programs that cannot be completely represented.
///
/// This check does not solve geometry and is usable by the corpus loader before scheduling
/// expensive evaluations. Native metadata with no effect on the objective or action is omitted.
///
/// # Errors
/// Returns an explicit capacity or invalid-input reason; nothing is silently truncated.
pub fn validate_design_representation(design: &Design) -> Result<(), String> {
    if design.tiers.is_empty() || design.meta.gear_teeth == 0 {
        return Err("design context needs planar tiers and a nonzero index gear".into());
    }
    rough::validate(design)?;
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    for (index, tier) in design.tiers.iter().enumerate() {
        program::orbit(
            design,
            &tier.indices,
            design.cheater_offset_deg(index).unwrap_or(0.0),
        )?;
        if let MeetConstraint::MeetNamed(names) = &tier.constraint {
            let refs = resolver.resolve_names(names);
            if refs.refs.len() > MAX_NAMED_MEETS {
                return Err(format!(
                    "tier {index} resolves {} named meets; limit is {MAX_NAMED_MEETS}",
                    refs.refs.len()
                ));
            }
        }
        program::relation(design, index)?;
        let numbers = [
            tier.angle_deg,
            match tier.constraint {
                MeetConstraint::ScaleReference(mast) => mast,
                _ => 0.0,
            },
            design.tier_target(index).map_or(0.0, target_value),
        ];
        if !numbers
            .iter()
            .all(|n| n.is_finite() && (*n as f32).is_finite())
        {
            return Err(format!(
                "tier {index} has a non-finite or f32-unrepresentable geometry input"
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(super) struct DesignFeatures {
    pub(super) tiers: Vec<TierDesignFeatures>,
    pub(super) global: GlobalDesignFeatures,
    pub(super) width: f64,
}

const fn flag(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

impl DesignFeatures {
    pub(super) fn capture(
        design: &Design,
        solved: &[SolvedTier],
        anchors: &BTreeMap<usize, TierHinge>,
        baseline: &MeetPreservationBaseline,
        policy: PreservationPolicy,
    ) -> Result<Self, String> {
        validate_design_representation(design)?;
        let width = baseline.width();
        if !width.is_finite() || width <= 0.0 || solved.len() != design.tiers.len() {
            return Err(
                "original geometry has no finite positive width or complete mast list".into(),
            );
        }
        let inputs = design.meet_tier_inputs();
        let resolver = MeetNameResolver::new(&inputs);
        let tiers = design
            .tiers
            .iter()
            .enumerate()
            .map(|(index, _)| {
                tier_context(
                    design,
                    index,
                    solved[index].mast,
                    anchors.get(&index),
                    width,
                    &resolver,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let features = rough::capture(design, baseline, policy)?;
        Ok(Self {
            tiers,
            global: GlobalDesignFeatures {
                features: features.into(),
                current_width_ratio: 1.0,
            },
            width,
        })
    }
}

const fn target_value(target: TierTarget) -> f64 {
    match target {
        TierTarget::DepthMm(value)
        | TierTarget::GirdleThicknessMm(value)
        | TierTarget::TableWidthMm(value) => value,
    }
}

fn tier_context(
    design: &Design,
    index: usize,
    mast: f64,
    hinge: Option<&TierHinge>,
    width: f64,
    resolver: &MeetNameResolver<'_>,
) -> Result<TierDesignFeatures, String> {
    let tier = &design.tiers[index];
    let orbit = program::orbit(
        design,
        &tier.indices,
        design.cheater_offset_deg(index).unwrap_or(0.0),
    )?;
    let relation = program::relation(design, index)?;
    let (refs, fully) = match &tier.constraint {
        MeetConstraint::MeetNamed(names) => {
            let resolved = resolver.resolve_names(names);
            (resolved.refs, resolved.fully)
        }
        _ => (Vec::new(), false),
    };
    let (kind, anchor) = match tier.constraint {
        MeetConstraint::MeetExisting => (0.0, 0.0),
        MeetConstraint::MeetNamed(_) => (0.5, 0.0),
        MeetConstraint::ScaleReference(value) => (1.0, value / width),
    };
    let target = design.tier_target(index);
    let target_kind = match target {
        None => 0.0,
        Some(TierTarget::DepthMm(_)) => 1.0 / 3.0,
        Some(TierTarget::GirdleThicknessMm(_)) => 2.0 / 3.0,
        Some(TierTarget::TableWidthMm(_)) => 1.0,
    };
    let mut values = vec![
        (tier.angle_deg / 90.0) as f32,
        (mast / width) as f32,
        kind,
        anchor as f32,
        flag(fully),
        target_kind,
        (target.map_or(0.0, target_value) / 100.0) as f32,
        flag(hinge.is_some()),
        hinge.map_or(0.0, |h| (h.point.x / width) as f32),
        hinge.map_or(0.0, |h| (h.point.y / width) as f32),
        hinge.map_or(0.0, |h| (h.point.z / width) as f32),
        hinge.map_or(0.0, |h| (h.azimuth_rad / std::f64::consts::TAU) as f32),
        hinge.map_or(0.0, |h| match h.side {
            FacetSide::Crown => 1.0,
            FacetSide::Pavilion => -1.0,
        }),
        hinge.map_or(0.0, |h| flag(h.wall_edge)),
        (design.meta.gear_reference_angle / f64::from(design.meta.gear_teeth.unsigned_abs()))
            as f32,
        (design.cheater_offset_deg(index).unwrap_or(0.0) / 360.0) as f32,
        design.meta.gear_teeth as f32 / 720.0,
        orbit.len() as f32 / MAX_ORBIT_POSITIONS as f32,
        refs.len() as f32 / MAX_NAMED_MEETS as f32,
        relation.len() as f32 / MAX_RELATION_NODES as f32,
    ];
    values.extend(orbit);
    values.resize(TIER_HEADER + MAX_ORBIT_POSITIONS, 0.0);
    values.extend(
        refs.into_iter()
            .map(|i| (i + 1) as f32 / (design.tiers.len() + 1) as f32),
    );
    values.resize(TIER_HEADER + MAX_ORBIT_POSITIONS + MAX_NAMED_MEETS, 0.0);
    values.extend(relation.into_iter().flatten());
    values.resize(TIER_STATIC_DIM - MAX_ORBIT_POSITIONS, 0.0);
    values.extend(program::frosting(design, index));
    values.resize(TIER_STATIC_DIM, 0.0);
    if !values.iter().all(|value| value.is_finite()) {
        return Err(format!(
            "tier {index} context cannot be represented as finite f32 values"
        ));
    }
    Ok(TierDesignFeatures {
        static_features: values.into(),
        current_mast: (mast / width) as f32,
        current_anchor: anchor as f32,
    })
}
