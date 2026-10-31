//! Which zone holds a point: [`ZoneKernel::zone_index_at`] and its f32 twin.
//!
//! The segment kernels answer "how long is this segment in each zone"; this answers the same
//! question for a single point, with the same semantics (see the notes in `kernels.rs`):
//!
//! * sharp boundaries: the LAST zone whose shape contains the point wins, else the base;
//! * soft boundaries: the zone with the largest blended weight, the weights being the ones the
//!   segment kernel integrates (`w_j * prod_{k > j} (1 - w_k)`, the base getting
//!   `prod (1 - w_k)`), which sum to 1;
//! * a mesh shell contains the point when the crossing count of a ray from the point to
//!   infinity is odd (f64 only, sharp only).
//!
//! The point is given in the OUTER frame, exactly as the kernels take segment points. The
//! index is the length-array index: 0 is the base, then the shaped zones in order.

use super::{MAX_ZONES, ZoneKernel, ZoneKernelF32};
use glam::{DVec3, Vec3};

/// The soft-boundary pick for one float type: the argmax of the blended zone weights.
/// `depths[j]` is the signed depth into shaped zone `j`. A tie goes to the later zone, like
/// the sharp rule (a point on a boundary is inside).
macro_rules! soft_pick {
    ($name:ident, $f:ty) => {
        fn $name(depths: &[$f], softness: $f) -> usize {
            let inv_width = 1.0 / softness;
            let mut weights: [$f; MAX_ZONES] = [0.0; MAX_ZONES];
            for (w, &sd) in weights.iter_mut().zip(depths) {
                let x = sd.mul_add(inv_width, 0.5).clamp(0.0, 1.0);
                let two: $f = 2.0;
                *w = x * x * two.mul_add(-x, 3.0);
            }
            let mut blended: [$f; MAX_ZONES + 1] = [0.0; MAX_ZONES + 1];
            let mut rest: $f = 1.0;
            for j in (0..depths.len()).rev() {
                blended[j + 1] = weights[j] * rest;
                rest *= (1.0 - weights[j]).max(0.0);
            }
            blended[0] = rest;
            let mut best = 0;
            for (i, &b) in blended.iter().enumerate().take(depths.len() + 1) {
                if b >= blended[best] {
                    best = i;
                }
            }
            best
        }
    };
}

soft_pick!(soft_pick_f64, f64);
soft_pick!(soft_pick_f32, f32);

impl ZoneKernel {
    /// The zone holding the outer-frame point `p`: index 0 is the base zone, then the shaped
    /// zones in order (the length-array index of [`ZoneKernel::lengths`]).
    ///
    /// Sharp boundaries: the last zone whose shape contains `p`, else 0. Soft boundaries: the
    /// zone with the largest blended weight. A mesh shell contains `p` by the crossing parity
    /// of a ray from `p` to infinity. A point exactly on a boundary counts as inside.
    #[must_use]
    pub fn zone_index_at(&self, p: DVec3) -> usize {
        let n = self.zone_count();
        let softness = self.softness();
        if softness > 0.0 {
            let mut depths = [f64::MIN; MAX_ZONES];
            for (j, depth) in depths.iter_mut().enumerate().take(n) {
                *depth = self.depth_at(j, p);
            }
            return soft_pick_f64(&depths[..n], softness);
        }
        let mut index = 0;
        for j in 0..n {
            let inside = self
                .mesh_contains(j, p)
                .unwrap_or_else(|| self.depth_at(j, p) >= 0.0);
            if inside {
                index = j + 1;
            }
        }
        index
    }
}

impl ZoneKernelF32 {
    /// As [`ZoneKernel::zone_index_at`], in `f32` (no mesh shells).
    #[must_use]
    pub fn zone_index_at(&self, p: Vec3) -> usize {
        let n = self.zone_count();
        let softness = self.softness();
        if softness > 0.0 {
            let mut depths = [f32::MIN; MAX_ZONES];
            for (j, depth) in depths.iter_mut().enumerate().take(n) {
                *depth = self.depth_at(j, p);
            }
            return soft_pick_f32(&depths[..n], softness);
        }
        let mut index = 0;
        for j in 0..n {
            if self.depth_at(j, p) >= 0.0 {
                index = j + 1;
            }
        }
        index
    }
}
