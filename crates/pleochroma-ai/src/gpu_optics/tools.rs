//! CUDA tool-piece packing, copied from `pleochroma/src/geometry/tool.rs::pieces`.
//! Shape and sweep decomposition preserve source order (normal ties use first piece).
use glam::Vec3;
use pleochroma::geometry::tool::ToolPrimitive;

/// Six fixed records per tool: kind,c[3],axis[3],half_length,r0,r1,stroke,sweep[3].
pub(super) fn pack(tools: &[ToolPrimitive]) -> Vec<f32> {
    let mut out = Vec::with_capacity(tools.len() * 85);
    for tool in tools {
        let pieces = pieces(tool);
        out.push(pieces.len() as f32);
        for piece in &pieces {
            out.extend(piece);
        }
        out.resize(out.len() + (6 - pieces.len()) * 14, 0.0);
    }
    if out.is_empty() {
        out.push(0.0);
    }
    out
}

fn piece(kind: f32, c: Vec3, a: Vec3, profile: [f32; 4], d: Vec3) -> [f32; 14] {
    [
        kind, c.x, c.y, c.z, a.x, a.y, a.z, profile[0], profile[1], profile[2], profile[3], d.x,
        d.y, d.z,
    ]
}

fn pieces(t: &ToolPrimitive) -> Vec<[f32; 14]> {
    let c = Vec3::from_slice(&t.origin);
    let a = Vec3::from_slice(&t.axis);
    let d = Vec3::from_slice(&t.sweep_dir);
    let hl = t.axis[3];
    let rim = t.origin[3];
    let stroke = t.profile[2];
    let swept = t.sweep_kind != 0 && stroke > 0.0;
    let mut out = Vec::with_capacity(6);
    if t.kind == 0 {
        if swept {
            let dir = if t.sweep_kind == 1 { a } else { d };
            out.push(piece(0.0, c - dir * stroke, a, [0.0, rim, rim, 0.0], d));
            out.push(piece(0.0, c + dir * stroke, a, [0.0, rim, rim, 0.0], d));
            out.push(piece(1.0, c, dir, [stroke, rim, rim, 0.0], d));
        } else {
            out.push(piece(0.0, c, a, [0.0, rim, rim, 0.0], d));
        }
        return out;
    }
    let rn = t.profile[0];
    let rp = t.profile[1];
    let (segments, peak_z, peak_r) = match t.kind {
        1 => (vec![[-hl, hl, rim, rim]], hl, rim),
        2 => (
            vec![[-hl, hl, rn, rp]],
            if rp >= rn { hl } else { -hl },
            rn.max(rp),
        ),
        _ => (vec![[-hl, 0.0, rn, rim], [0.0, hl, rim, rp]], 0.0, rim),
    };
    for [lo, hi, r0, r1] in segments {
        let centre = c + a * (0.5 * (lo + hi));
        let half = 0.5 * (hi - lo);
        if !swept {
            out.push(piece(1.0, centre, a, [half, r0, r1, 0.0], d));
        } else if t.sweep_kind == 1 {
            let shift = if r1 >= r0 { -a * stroke } else { a * stroke };
            out.push(piece(1.0, centre + shift, a, [half, r0, r1, 0.0], d));
        } else {
            out.push(piece(1.0, centre - d * stroke, a, [half, r0, r1, 0.0], d));
            out.push(piece(1.0, centre + d * stroke, a, [half, r0, r1, 0.0], d));
            out.push(piece(2.0, centre, a, [half, r0, r1, stroke], d));
        }
    }
    if swept && t.sweep_kind == 1 {
        out.push(piece(
            1.0,
            c + a * peak_z,
            a,
            [stroke, peak_r, peak_r, 0.0],
            d,
        ));
    }
    out
}
