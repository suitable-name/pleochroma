// Adapted from pleochroma geometry/stone_metrics/{vertices,mod}.rs; MIT.
// Copyright (c) 2026 Niklas Gussek. Full license: LICENSE-MIT.
use glam::DVec3;
const VERTEX_DEDUP: f64 = 1e-6;
/// One deduplicated vertex of the solid.
pub(super) struct SolidVertex {
    pub(super) v: DVec3,
}

/// Accepted vertices, in first-seen order, plus an index over them sorted by
/// `x` so a new candidate's duplicate check only has to scan the vertices
/// that could possibly be within [`VERTEX_DEDUP`] of it on every axis instead
/// of the full accepted set.
///
/// `verts`' order is exactly the push order of [`insert_if_new`](Self::insert_if_new)
/// calls (the divergence-theorem sum in production `measure_solid`
/// depends on it); `by_x` is a lookup structure only, never observed by
/// callers.
#[derive(Default)]
pub(super) struct VertexAccumulator {
    pub(super) verts: Vec<SolidVertex>,
    /// Indices into `verts`, kept sorted ascending by `verts[i].v.x`.
    by_x: Vec<usize>,
}

impl VertexAccumulator {
    /// Inserts `v` unless some already-accepted vertex is within
    /// [`VERTEX_DEDUP`] of it on every axis -- identical to a linear scan
    /// testing `(s.v - v).abs().max_element() < VERTEX_DEDUP` against every
    /// prior vertex, just restricted up front to the `x`-sorted window that
    /// could possibly match (any vertex outside `[v.x - VERTEX_DEDUP, v.x +
    /// VERTEX_DEDUP]` fails the `x`-axis check alone, so narrowing to that
    /// window changes no accept/reject decision).
    pub(super) fn insert_if_new(&mut self, v: DVec3) {
        let verts = &self.verts;
        let lo = self
            .by_x
            .partition_point(|&i| verts[i].v.x < v.x - VERTEX_DEDUP);
        let hi = self
            .by_x
            .partition_point(|&i| verts[i].v.x <= v.x + VERTEX_DEDUP);
        for &i in &self.by_x[lo..hi] {
            if (self.verts[i].v - v).abs().max_element() < VERTEX_DEDUP {
                return;
            }
        }
        let idx = self.verts.len();
        self.verts.push(SolidVertex { v });
        let pos = self.by_x.partition_point(|&i| self.verts[i].v.x < v.x);
        self.by_x.insert(pos, idx);
    }
}

/// Drops duplicate planes so a tier that lists the same index twice can't
/// double-count its face's area. The first occurrence wins.
///
/// Two planes are duplicates when their normals coincide under
/// [`normals_coincide`] (the rule the `.asc` plane builder uses, which holds for
/// `f32`-normalised inputs widened to `f64`) and their offsets differ by less
/// than `1e-9`.
pub(super) fn dedup_planes(planes: &[(DVec3, f64)]) -> Vec<(DVec3, f64)> {
    let mut out: Vec<(DVec3, f64)> = Vec::with_capacity(planes.len());
    for &(n, m) in planes {
        let dup = out
            .iter()
            .any(|&(n2, m2)| normals_coincide(n, n2) && (m - m2).abs() < 1e-9);
        if !dup {
            out.push((n, m));
        }
    }
    out
}

pub(super) fn dedup_origin_indices(
    original: &[(DVec3, f64)],
    deduped: &[(DVec3, f64)],
) -> Vec<usize> {
    let same = |a: (DVec3, f64), b: (DVec3, f64)| {
        a.0.x.to_bits() == b.0.x.to_bits()
            && a.0.y.to_bits() == b.0.y.to_bits()
            && a.0.z.to_bits() == b.0.z.to_bits()
            && a.1.to_bits() == b.1.to_bits()
    };
    let mut out = Vec::with_capacity(deduped.len());
    let mut cursor = 0usize;
    for &d in deduped {
        while cursor < original.len() && !same(original[cursor], d) {
            cursor += 1;
        }
        // `cursor == original.len()` here would mean `deduped` contains a
        // plane `dedup_planes` could not have produced from `original` --
        // an invariant violation, not a real runtime case. Clamping instead
        // of panicking keeps this diagnostic helper infallible even if that
        // invariant is ever broken by a future edit to `dedup_planes`.
        out.push(cursor.min(original.len().saturating_sub(1)));
        cursor += 1;
    }
    out
}

pub(super) fn pow2_scale_norm(scale: f64) -> f64 {
    const MANTISSA: u64 = (1 << 52) - 1;
    /// 2^64, lifts a subnormal into the normal range.
    const SUBNORMAL_LIFT: f64 = 18_446_744_073_709_551_616.0;
    if !(scale.is_finite() && scale > 0.0) {
        return 1.0;
    }
    // `scale` is positive, so the sign bit is clear and `bits >> 52` is the biased exponent.
    let raw = scale.to_bits();
    let (bits, lift) = if raw >> 52 == 0 {
        ((scale * SUBNORMAL_LIFT).to_bits(), 64_i64)
    } else {
        (raw, 0_i64)
    };
    let biased = i64::from(u16::try_from(bits >> 52).unwrap_or(1023));
    let mantissa = f64::from_bits((bits & MANTISSA) | (1023_u64 << 52));
    let mut exponent = biased - 1023 - lift;
    if mantissa >= std::f64::consts::SQRT_2 {
        exponent += 1;
    }
    let biased_out = u64::try_from((exponent + 1023).clamp(1, 2046)).unwrap_or(1023);
    f64::from_bits(biased_out << 52)
}

fn normals_coincide(a: DVec3, b: DVec3) -> bool {
    let (a_hat, b_hat) = (a.normalize_or_zero(), b.normalize_or_zero());
    1.0 - a_hat.dot(b_hat) <= 2e-7
}
