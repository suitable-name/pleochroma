//! Conservative geometric families for leakage-resistant dataset splits.
//!
//! This is deliberately coarser than design equality. Depth, rough, material, target programs
//! and tier labels cannot put two angularly identical cuts into different splits. Reordered
//! tiers, equivalent gears, uniform scaling, rotation and reflection share one family.

use pleochroma_cut_core::Design;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn normalized_geometry_key(design: &Design) -> Vec<u8> {
    let gear = f64::from(design.meta.gear_teeth.unsigned_abs().max(1));
    let driven: BTreeMap<_, _> = design
        .evaluate_relations()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut normals = Vec::new();
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let angle = driven.get(&tier_index).copied().unwrap_or(tier.angle_deg);
        let indices = if tier.indices.is_empty() {
            &[0.0][..]
        } else {
            &tier.indices
        };
        let cheater = design.cheater_offset_deg(tier_index).unwrap_or(0.0);
        for &index in indices {
            let azimuth = if angle == 0.0 {
                0.0
            } else {
                (360.0 * (index + design.meta.gear_reference_angle) / gear + cheater)
                    .rem_euclid(360.0)
            };
            normals.push((angle, azimuth));
        }
    }
    // Removing duplicates also joins native alias tiers to their single-plane ASC export.
    normals.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
    normals.dedup();
    // One rare inclination group supplies all necessary rotation origins. Choosing it by
    // count then angle is invariant to reorder/reflection/rotation, unlike taking N facets.
    // Repeated azimuths never trigger duplicate full-key sorts.
    let mut inclinations = BTreeMap::<u64, BTreeSet<u64>>::new();
    for &(angle, azimuth) in &normals {
        if angle != 0.0 {
            inclinations
                .entry(quantized(angle))
                .or_default()
                .insert(azimuth.to_bits());
        }
    }
    let anchors = inclinations
        .iter()
        .min_by_key(|(angle, azimuths)| (azimuths.len(), **angle))
        .map_or_else(
            || vec![0.0],
            |(_, azimuths)| azimuths.iter().map(|bits| f64::from_bits(*bits)).collect(),
        );
    let mut best: Option<Vec<(u64, u64)>> = None;
    for anchor in anchors {
        for sign in [-1.0, 1.0] {
            let mut key: Vec<_> = normals
                .iter()
                .map(|&(angle, azimuth)| {
                    let az = if angle == 0.0 {
                        0.0
                    } else {
                        (sign * (azimuth - anchor)).rem_euclid(360.0)
                    };
                    (quantized(angle), quantized(az))
                })
                .collect();
            key.sort_unstable();
            key.dedup();
            if best.as_ref().is_none_or(|current| key < *current) {
                best = Some(key);
            }
        }
    }
    // Omitting concave parameters intentionally groups alternate carving programs with their
    // parent flat cut: holding one such program out beside its parent would leak the family.
    best.unwrap_or_default()
        .into_iter()
        .flat_map(|(a, b)| a.to_le_bytes().into_iter().chain(b.to_le_bytes()))
        .collect()
}

fn quantized(value: f64) -> u64 {
    // File formats serialize decimal angles; ignore sub-microdegree round-trip noise.
    let rounded = (value * 1e6).round();
    if rounded == 0.0 || rounded == 360_000_000.0 {
        0
    } else {
        rounded.to_bits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reordered_rotated_reflected_scaled_and_equivalent_gears_share_a_family() {
        let mut design = Design::concave_fixture();
        design.meta.gear_teeth = 96;
        let expected = normalized_geometry_key(&design);
        let mut variant = design.clone();
        variant.tiers.reverse();
        variant.meta.gear_teeth = 192;
        for tier in &mut variant.tiers {
            tier.indices = tier
                .indices
                .iter()
                .map(|index| 2.0 * (12.0 - index))
                .collect();
        }
        variant.preform.half_width *= 2.0;
        variant.preform.depth *= 2.0;
        variant.girdle_diameter_mm = Some(22.0);
        assert_eq!(expected, normalized_geometry_key(&variant));
    }
}
