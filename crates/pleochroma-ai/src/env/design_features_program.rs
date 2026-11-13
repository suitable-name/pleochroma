//! Bounded encoding of facet orbits and angle relations for the policy.

use pleochroma_cut_core::{
    Design,
    design::{Expr, RelationExpr, TierFrosting},
};

use super::{MAX_ORBIT_POSITIONS, MAX_RELATION_NODES};

pub(super) fn orbit(design: &Design, indices: &[f64], cheater: f64) -> Result<Vec<f32>, String> {
    if indices.len() > MAX_ORBIT_POSITIONS {
        return Err(format!(
            "orbit has {} placements; observation limit is {MAX_ORBIT_POSITIONS}",
            indices.len()
        ));
    }
    let indices = if indices.is_empty() {
        &[0.0][..]
    } else {
        indices
    };
    indices
        .iter()
        .map(|&index| {
            super::super::features::index_azimuth(
                index,
                design.meta.gear_teeth,
                design.meta.gear_reference_angle,
                cheater,
            )
            .map(|angle| (angle.rem_euclid(std::f64::consts::TAU) / std::f64::consts::TAU) as f32)
            .ok_or_else(|| "orbit has an invalid angle, reference, cheater, or gear".into())
        })
        .collect()
}

// Authored frosting flag per orbit slot, before physical-plane alias merging. Mirrors
// `Design::frosted_planes_from_solved`: an implicit index-zero facet (empty `indices`) is
// frosted only by `TierFrosting::Tier`, not by `Facets([0])`.
pub(super) fn frosting(design: &Design, index: usize) -> Vec<f32> {
    let indices = &design.tiers[index].indices;
    let finish = design.tier_frosting(index);
    if indices.is_empty() {
        return vec![super::flag(matches!(finish, Some(TierFrosting::Tier)))];
    }
    indices
        .iter()
        .map(|&position| super::flag(finish.is_some_and(|frosting| frosting.covers(position))))
        .collect()
}

// Operators are prefix-ordered. References are tier positions rather than native IDs, so
// renumbering IDs does not change the encoding.
pub(super) fn relation(design: &Design, index: usize) -> Result<Vec<[f32; 2]>, String> {
    let Some(relation) = design
        .tier_id_at(index)
        .and_then(|id| design.tier_relations.get(&id))
    else {
        return Ok(Vec::new());
    };
    let mut stack = vec![&relation.angle];
    let mut nodes = Vec::new();
    while let Some(node) = stack.pop() {
        if nodes.len() == MAX_RELATION_NODES {
            return Err(format!(
                "tier {index} relation exceeds {MAX_RELATION_NODES} observation nodes"
            ));
        }
        let (opcode, value) = match node {
            Expr::Number(value) => {
                if !value.is_finite() {
                    return Err(format!(
                        "tier {index} relation contains a non-finite constant"
                    ));
                }
                // Signed log compression maps constants up to 1e9 onto [-1, 1], keeping their
                // order so that large literals do not dominate attention.
                (
                    1.0,
                    (value.signum() * value.abs().ln_1p() / 1e9_f64.ln_1p()) as f32,
                )
            }
            Expr::Ref(id) => {
                let driver = design
                    .index_of_tier_id(*id)
                    .ok_or_else(|| format!("tier {index} relation references a missing tier"))?;
                (2.0, (driver + 1) as f32 / (design.tiers.len() + 1) as f32)
            }
            Expr::Neg(child) => {
                stack.push(child);
                (3.0, 0.0)
            }
            Expr::Add(left, right) => binary(&mut stack, left, right, 4.0),
            Expr::Sub(left, right) => binary(&mut stack, left, right, 5.0),
            Expr::Mul(left, right) => binary(&mut stack, left, right, 6.0),
            Expr::Div(left, right) => binary(&mut stack, left, right, 7.0),
        };
        nodes.push([opcode / 7.0, value]);
    }
    Ok(nodes)
}

fn binary<'a>(
    stack: &mut Vec<&'a RelationExpr>,
    left: &'a RelationExpr,
    right: &'a RelationExpr,
    opcode: f32,
) -> (f32, f32) {
    stack.push(right);
    stack.push(left);
    (opcode, 0.0)
}
