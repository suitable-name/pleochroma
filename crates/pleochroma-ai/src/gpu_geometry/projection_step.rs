//! Adapted from cut-core optimize/projection_step.rs: bounded QR correction, CPU reference.
//!
//! Redundant contacts and production-plane rounding make a finite-difference
//! Jacobian inconsistent away from the solution. Solve its least-squares problem
//! instead of rejecting such a linearization as an impossible authored design.

/// Damped QR in physical trust-region units, without forming J^T J. Derivatives
/// are never normalized by their own magnitude: doing so amplifies rounding-only
/// directions. Final geometry validation remains the caller's responsibility.
pub(super) fn correction(
    columns: &[Vec<f64>],
    residual: &[f64],
    parameter_scales: &[f64],
) -> Option<Vec<f64>> {
    const DAMPING: f64 = 1e-8;
    let n = columns.len();
    let m = residual.len();
    if n == 0 || parameter_scales.len() != n || residual.iter().any(|r| !r.is_finite()) {
        return None;
    }
    let mut basis: Vec<Vec<f64>> = Vec::with_capacity(n);
    let mut triangular = vec![vec![0.0; n]; n];
    let mut rhs = Vec::with_capacity(n);
    for (j, column) in columns.iter().enumerate() {
        if column.len() != m || column.iter().any(|v| !v.is_finite()) {
            return None;
        }
        let scale = parameter_scales[j];
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let mut q = vec![0.0; m + n];
        for (dst, value) in q.iter_mut().zip(column) {
            *dst = value * scale;
        }
        q[m + j] = DAMPING.sqrt();
        // Reorthogonalization avoids losing small independent directions.
        for _ in 0..2 {
            for (i, previous) in basis.iter().enumerate() {
                let dot: f64 = q.iter().zip(previous).map(|(a, b)| a * b).sum();
                triangular[i][j] += dot;
                for (value, component) in q.iter_mut().zip(previous) {
                    *value -= dot * component;
                }
            }
        }
        let norm = q.iter().map(|v| v * v).sum::<f64>().sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return None;
        }
        triangular[j][j] = norm;
        for value in &mut q {
            *value /= norm;
        }
        rhs.push(-q.iter().zip(residual).map(|(a, b)| a * b).sum::<f64>());
        basis.push(q);
    }
    let mut step = vec![0.0; n];
    for i in (0..n).rev() {
        let later: f64 = ((i + 1)..n).map(|j| triangular[i][j] * step[j]).sum();
        step[i] = (rhs[i] - later) / triangular[i][i];
    }
    for (value, scale) in step.iter_mut().zip(parameter_scales) {
        *value *= scale;
    }
    step.iter().all(|v| v.is_finite()).then_some(step)
}

/// Shorten the whole direction to the intersection of the original trust box and
/// actual authored bounds. Keep every coupled angle/depth ratio intact.
pub(super) fn bound_correction(
    step: &mut [f64],
    current: &[f64],
    origin: &[f64],
    limits: &[f64],
    bounds: &[(f64, f64)],
) -> Option<()> {
    if [current.len(), origin.len(), limits.len(), bounds.len()]
        .into_iter()
        .any(|n| n != step.len())
    {
        return None;
    }
    let mut fraction = 1.0_f64;
    for ((((delta, value), start), limit), &(lo, hi)) in
        step.iter().zip(current).zip(origin).zip(limits).zip(bounds)
    {
        if !delta.is_finite()
            || !value.is_finite()
            || !start.is_finite()
            || !limit.is_finite()
            || *limit <= 0.0
            || lo.is_nan()
            || hi.is_nan()
            || lo > hi
            || *value < lo
            || *value > hi
        {
            return None;
        }
        if *delta != 0.0 {
            let room = if *delta > 0.0 {
                (start + limit).min(hi) - value
            } else {
                value - (start - limit).max(lo)
            };
            fraction = fraction.min(room.max(0.0) / delta.abs());
        }
    }
    if fraction <= 0.0 {
        return None;
    }
    for value in step {
        *value *= fraction;
    }
    Some(())
}

/// A one-sided finite difference inside the *actual* legal interval. Use the
/// represented f64 displacement as the divisor, including at narrow boundaries.
/// Keep the usual production-plane-sized step whenever the interval permits it;
/// clipping is preferable to silently turning a movable variable into a zero column.
pub(super) fn difference_step(value: f64, nominal: f64, bounds: (f64, f64)) -> Option<f64> {
    let (lo, hi) = bounds;
    if !value.is_finite()
        || !nominal.is_finite()
        || nominal == 0.0
        || lo.is_nan()
        || hi.is_nan()
        || value < lo
        || value > hi
        || lo > hi
    {
        return None;
    }
    let mut next = (value + nominal).clamp(lo, hi);
    if next == value {
        next = if nominal > 0.0 {
            value.next_up()
        } else {
            value.next_down()
        };
    }
    let delta = next - value;
    (next.is_finite()
        && next >= lo
        && next <= hi
        && delta.is_finite()
        && delta != 0.0
        && delta.is_sign_positive() == nominal.is_sign_positive())
    .then_some(delta)
}

/// Ordered dyadic backtracking. Additional small fractions are used only after
/// the existing four candidates fail; bounded work and represented-state checks
/// prevent an endless search below production-plane/f64 resolution.
pub(super) const BACKTRACK_STEPS: usize = 24;

pub(super) fn backtrack_factor(index: usize) -> f64 {
    2.0_f64.powi(-(index as i32))
}

#[cfg(test)]
mod tests {
    use super::{BACKTRACK_STEPS, backtrack_factor, bound_correction, correction, difference_step};

    #[test]
    fn dependent_contacts_with_rounding_disagreement_still_have_a_correction() {
        let step = correction(
            &[vec![1.0, 1.0], vec![1.0, 1.0]],
            &[-1.0, -1.00001],
            &[1.0, 1.0],
        )
        .expect("an inconsistent linearization is not an infeasibility verdict");
        assert!((step[0] - 0.500_002_5).abs() < 1e-5);
        assert!((step[1] - 0.500_002_5).abs() < 1e-5);
        let remaining: f64 = [-1.0, -1.00001]
            .iter()
            .map(|r| (r + step[0] + step[1]).powi(2))
            .sum();
        assert!(remaining < 1e-9);
    }

    #[test]
    fn locked_columns_stay_zero_and_parameter_units_do_not_lose_rank() {
        let step = correction(
            &[vec![0.0, 0.0], vec![0.1, 0.0], vec![0.0, 10.0]],
            &[-0.2, -0.5],
            &[5.0, 5.0, 0.1],
        )
        .expect("a locked variable does not prevent other variables moving");
        assert_eq!(step[0], 0.0);
        assert!((step[1] - 2.0).abs() < 1e-5);
        assert!((step[2] - 0.05).abs() < 1e-6);
    }

    #[test]
    fn rounding_only_direction_does_not_overwhelm_a_useful_correction() {
        let step = correction(
            &[vec![1e-9, 0.0], vec![0.0, 1.0]],
            &[-1e-6, -0.02],
            &[5.0, 0.1],
        )
        .unwrap();
        assert!(
            step[0].abs() < 1e-3,
            "rounding noise must not request a huge angle move"
        );
        assert!((step[1] - 0.02).abs() < 1e-6);
    }

    #[test]
    fn correction_uses_remaining_trust_room_without_changing_its_direction() {
        let mut step = [20.0, -0.2];
        bound_correction(
            &mut step,
            &[3.0, 0.0],
            &[0.0, 0.0],
            &[5.0, 0.1],
            &[(-5.0, 5.0), (-0.1, 0.1)],
        )
        .unwrap();
        assert!((step[0] - 2.0).abs() < 1e-12);
        assert!((step[1] + 0.02).abs() < 1e-12);
    }

    #[test]
    fn correction_respects_narrow_actual_box_without_changing_coupled_ratios() {
        let mut step = [1.0, -0.1];
        bound_correction(
            &mut step,
            &[2.0, 1.0],
            &[2.0, 1.0],
            &[5.0, 0.1],
            &[(1.0, 2.01), (0.9, 1.1)],
        )
        .unwrap();
        assert!(step[0] <= 0.01 && step[0] > 0.0);
        assert!((step[1] / step[0] + 0.1).abs() < 1e-12);
        assert!(2.0 + step[0] <= 2.01);
    }

    #[test]
    fn differences_use_available_room_and_represented_changes() {
        let value = 45.0_f64;
        let high = value + 0.0001;
        let delta = difference_step(value, 0.002, (value, high)).unwrap();
        assert_eq!(delta.to_bits(), (high - value).to_bits());
        assert_eq!(value + delta, high);
        assert!(difference_step(high, 0.002, (value, high)).is_none());
        assert!(difference_step(high, -0.002, (value, high)).is_some());
        assert!(difference_step(value, 0.002, (value, value)).is_none());
        let adjacent = value.next_up();
        assert_eq!(
            difference_step(value, 1e-30, (value, adjacent)),
            Some(adjacent - value)
        );
    }

    #[test]
    fn backtracking_includes_sub_eighth_steps_in_deterministic_order() {
        let factors: Vec<_> = (0..BACKTRACK_STEPS).map(backtrack_factor).collect();
        assert_eq!(&factors[..5], &[1.0, 0.5, 0.25, 0.125, 0.0625]);
        assert!(
            factors
                .windows(2)
                .all(|pair| pair[0] > pair[1] && pair[1] > 0.0)
        );
    }

    #[test]
    fn diagnostic_active_bound_blocks_the_whole_current_unconstrained_direction() {
        let mut step =
            correction(&[vec![1.0, 0.0], vec![0.0, 1.0]], &[-1.0, 1.0], &[5.0, 5.0]).unwrap();
        let bounds = [(0.0, 1.0), (-5.0, 5.0)];
        assert!(step[0] > 0.0 && step[1] < 0.0);
        assert!(
            bound_correction(&mut step, &[1.0, 0.0], &[0.0, 0.0], &[5.0, 5.0], &bounds).is_none()
        );
        // This documents the existing limitation, rather than changing the search:
        // the second coordinate still admits a valid bounded descent direction.
        let mut feasible = [0.0, -1.0];
        assert!(
            bound_correction(
                &mut feasible,
                &[1.0, 0.0],
                &[0.0, 0.0],
                &[5.0, 5.0],
                &bounds
            )
            .is_some()
        );
    }
}
