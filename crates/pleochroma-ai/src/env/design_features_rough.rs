//! Complete bounded rough/tool geometry and the immutable preservation policy.

use pleochroma_cut_core::{
    Design,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    preform::PreformShape,
};

use super::{MAX_CONCAVE_TIERS, MAX_ORBIT_POSITIONS, MAX_ROUGH_PLANES, flag, program};

const HEADER: usize = 25;
const TOOL_HEADER: usize = 12;
pub(super) const GLOBAL_DESIGN_DIM: usize =
    HEADER + MAX_ROUGH_PLANES * 4 + MAX_CONCAVE_TIERS * (TOOL_HEADER + MAX_ORBIT_POSITIONS) + 1;

pub(super) fn validate(design: &Design) -> Result<(), String> {
    let rough = &design.preform;
    let dimensions = [rough.half_width, rough.length_over_width, rough.depth];
    if !dimensions
        .iter()
        .all(|value| value.is_finite() && *value > 0.0 && (*value as f32).is_finite())
        || !(design.preform_y_offset as f32).is_finite()
        || design
            .girdle_diameter_mm
            .is_some_and(|value| !value.is_finite() || value <= 0.0 || !(value as f32).is_finite())
    {
        return Err("rough dimensions, offset or physical stone width are invalid".into());
    }
    if let PreformShape::Planes(set) = &rough.shape {
        if set.planes.len() > MAX_ROUGH_PLANES {
            return Err(format!(
                "rough has {} planes; observation limit is {MAX_ROUGH_PLANES}",
                set.planes.len()
            ));
        }
        if !set.planes.iter().all(|(normal, mast)| {
            normal.is_finite()
                && normal.as_vec3().is_finite()
                && (*mast as f32).is_finite()
                && normal.length_squared() > 0.0
        }) {
            return Err("rough contains an invalid half-space".into());
        }
    }
    if design.concave_tiers.len() > MAX_CONCAVE_TIERS {
        return Err(format!(
            "design has {} concave tiers; observation limit is {MAX_CONCAVE_TIERS}",
            design.concave_tiers.len()
        ));
    }
    for (index, tool) in design.concave_tiers.iter().enumerate() {
        tool.validate(design.meta.gear_teeth)
            .map_err(|error| format!("concave tier {index}: {error}"))?;
        program::orbit(design, &tool.indices, 0.0)?;
    }
    Ok(())
}

pub(super) fn capture(
    design: &Design,
    baseline: &MeetPreservationBaseline,
    policy: PreservationPolicy,
) -> Result<Vec<f32>, String> {
    let width = baseline.width();
    let rough = &design.preform;
    let (kind, sides, planes) = match &rough.shape {
        PreformShape::Block => (1.0 / 3.0, 0, &[][..]),
        PreformShape::Cylinder { sides } => (2.0 / 3.0, (*sides).max(3), &[][..]),
        PreformShape::Planes(set) => (1.0, 0, set.planes.as_slice()),
    };
    let structure = baseline.structure();
    let mut values = vec![
        kind,
        (rough.half_width / width) as f32,
        rough.length_over_width as f32,
        (rough.depth / width) as f32,
        sides as f32 / 720.0,
        flag(design.girdle_diameter_mm.is_some()),
        (design.girdle_diameter_mm.unwrap_or(0.0) / 100.0) as f32,
        (design.preform_y_offset / width) as f32,
        width.ln_1p() as f32,
        baseline.identify_radius() as f32,
        baseline.max_slack() as f32,
        policy.max_junction_displacement as f32,
        policy.max_aspect_change as f32,
        (policy.max_table_percent_change / 100.0) as f32,
        policy.max_depth_change as f32,
        policy.max_girdle_change as f32,
        flag(policy.fixed_outline),
        flag(policy.lock_girdle_gauge),
        flag(policy.bounds_follow_turn),
        planes.len() as f32 / MAX_ROUGH_PLANES as f32,
        design.concave_tiers.len() as f32 / MAX_CONCAVE_TIERS as f32,
        structure.junctions as f32 / 10_000.0,
        structure.edges as f32 / 10_000.0,
        structure.faces as f32 / 10_000.0,
        structure.reach_fallbacks_possible as f32 / 128.0,
    ];
    for (normal, mast) in planes {
        values.extend([
            normal.x as f32,
            normal.y as f32,
            normal.z as f32,
            (*mast / width) as f32,
        ]);
    }
    values.resize(HEADER + MAX_ROUGH_PLANES * 4, 0.0);
    for (index, tool) in design.concave_tiers.iter().enumerate() {
        let orbit = program::orbit(design, &tool.indices, 0.0)?;
        values.extend(tool_header(tool, orbit.len()));
        values.extend(orbit);
        values.resize(
            HEADER + MAX_ROUGH_PLANES * 4 + (index + 1) * (TOOL_HEADER + MAX_ORBIT_POSITIONS),
            0.0,
        );
    }
    values.resize(GLOBAL_DESIGN_DIM - 1, 0.0);
    // Girdle frosting also applies to exposed preform walls, which have no tier token.
    values.push(flag(design.render_effects.girdle_frosted));
    if !values.iter().all(|value| value.is_finite()) {
        return Err(
            "rough or preservation context is not representable as finite f32 values".into(),
        );
    }
    Ok(values)
}

fn tool_header(tool: &ConcaveTier, orbit_len: usize) -> [f32; TOOL_HEADER] {
    let shape = match tool.tool {
        ConcaveTool::Cylinder => 1.0,
        ConcaveTool::Cone => 2.0,
        ConcaveTool::Circle => 3.0,
        ConcaveTool::Disc => 4.0,
        ConcaveTool::Sphere => 5.0,
    };
    [
        shape / 5.0,
        (tool.angle_deg / 90.0) as f32,
        (tool.tool_azimuth_deg / 360.0) as f32,
        tool.displacement[0] as f32,
        tool.displacement[1] as f32,
        tool.displacement[2] as f32,
        tool.diameter_ratio as f32,
        (tool.tool_angle_deg.unwrap_or(0.0) / 180.0) as f32,
        flag(tool.tool_angle_deg.is_some()),
        flag(tool.motion == ToolMotion::Plunge),
        orbit_len as f32 / MAX_ORBIT_POSITIONS as f32,
        1.0,
    ]
}
