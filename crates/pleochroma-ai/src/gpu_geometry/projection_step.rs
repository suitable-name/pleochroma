//! Adapted from cut-core optimize/projection_step.rs: bounded QR correction, CPU reference.
//!
//! Redundant contacts and production-plane rounding make a finite-difference
//! Jacobian inconsistent away from the solution. Solve its least-squares problem
//! instead of rejecting such a linearization as an impossible authored design.

/// Damped QR in physical trust-region units, without forming J^T J.
///
/// Derivatives are never normalized by their own magnitude: doing so amplifies rounding-only
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

/// Computes a damped-QR step in the subspace allowed by active bounds.
///
/// Zeroes Jacobian columns whose step components point outside an active boundary
/// (room <= 1e-12), re-solving
/// the coupled least-squares system on the remaining free variables, and then scaling
/// the resulting feasible direction to the trust-region box.
///
/// Returns `Ok(step)` on success, `Err(false)` for an invalid correction, and
/// `Err(true)` when no feasible direction remains.
pub(super) fn bounded_qr_correction(
    columns: &[Vec<f64>],
    residual: &[f64],
    current: &[f64],
    origin: &[f64],
    limits: &[f64],
    bounds: &[(f64, f64)],
) -> Result<Vec<f64>, bool> {
    let n = columns.len();
    if [current.len(), origin.len(), limits.len(), bounds.len()]
        .into_iter()
        .any(|len| len != n)
    {
        return Err(false);
    }
    let mut active_columns = columns.to_vec();
    let mut pinned = vec![false; n];
    // Every nonterminal pass pins at least one additional coordinate, so at
    // most n pinning passes and one final free-subspace solve are needed.
    for _pass in 0..=n {
        let Some(mut step) = correction(&active_columns, residual, limits) else {
            return Err(false);
        };
        for j in 0..n {
            if pinned[j] {
                step[j] = 0.0;
            }
        }
        let mut newly_pinned = false;
        for j in 0..n {
            let delta = step[j];
            if delta == 0.0 || pinned[j] {
                continue;
            }
            let value = current[j];
            let start = origin[j];
            let limit = limits[j];
            let (lo, hi) = bounds[j];
            let room = if delta > 0.0 {
                (start + limit).min(hi) - value
            } else {
                value - (start - limit).max(lo)
            };
            if room <= 1e-12 {
                pinned[j] = true;
                active_columns[j].fill(0.0);
                newly_pinned = true;
            }
        }
        if !newly_pinned {
            if bound_correction(&mut step, current, origin, limits, bounds).is_some() {
                return Ok(step);
            }
            return Err(true);
        }
        if pinned.iter().all(|&p| p) {
            return Err(true);
        }
    }
    Err(true)
}

/// A one-sided finite difference inside the *actual* legal interval.
///
/// Use the represented f64 displacement as the divisor, including at narrow boundaries.
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

/// Ordered dyadic backtracking.
///
/// Additional small fractions are used only after
/// the existing four candidates fail; bounded work and represented-state checks
/// prevent an endless search below production-plane/f64 resolution.
pub(super) const BACKTRACK_STEPS: usize = 24;

pub(super) fn backtrack_factor(index: usize) -> f64 {
    2.0_f64.powi(-(index as i32))
}

#[cfg(test)]
mod tests {
    use super::{
        BACKTRACK_STEPS, backtrack_factor, bound_correction, bounded_qr_correction, correction,
        difference_step,
    };

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
        // Uniform clipping alone cannot recover this direction. The active-set
        // solver below must retain the second coordinate's bounded descent.
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

    #[test]
    fn bounded_qr_correction_pins_active_bound_and_descends_on_free_variable() {
        let columns = [vec![1.0, 0.0], vec![0.0, 1.0]];
        let residual = [-1.0, 1.0];
        let current = [1.0, 0.0];
        let origin = [0.0, 0.0];
        let limits = [5.0, 5.0];
        let bounds = [(0.0, 1.0), (-5.0, 5.0)];
        let step = bounded_qr_correction(&columns, &residual, &current, &origin, &limits, &bounds)
            .expect("feasible direction on unpinned coordinate");
        assert_eq!(step[0], 0.0);
        assert!(step[1] < 0.0);
    }

    #[test]
    fn bounded_correction_preserves_descent_with_many_blocked_coordinates() {
        let n = 9;
        let columns: Vec<Vec<f64>> = (0..n)
            .map(|i| {
                let mut column = vec![0.0; n];
                column[i] = 1.0;
                column
            })
            .collect();
        let residual = vec![-1.0; n];
        let current = vec![0.0; n];
        let limits = vec![1.0; n];
        let mut bounds = vec![(-1.0, 0.0); n];
        bounds[n - 1] = (-1.0, 1.0);
        let step = bounded_qr_correction(&columns, &residual, &current, &current, &limits, &bounds)
            .unwrap();
        assert!(step[..n - 1].iter().all(|&value| value == 0.0));
        assert!(step[n - 1] > 0.0);
        bounds[n - 1] = (-1.0, 0.0);
        assert_eq!(
            bounded_qr_correction(&columns, &residual, &current, &current, &limits, &bounds),
            Err(true)
        );
    }

    #[test]
    fn bounded_correction_resolves_more_than_six_sequential_bounds() {
        // A^T A = I + 11^T. Pinning a negative component shifts every free
        // component down by the same amount. Place each successive component
        // between the previous and next shift, so only one crosses zero at a
        // time. The final coordinate remains free and has to be re-solved.
        let n = 9;
        let mut target = vec![0.0; n];
        target[0] = -1.0;
        let mut pinned_sum = 1.0;
        let mut previous_shift = 0.0;
        let mut shift = 1.0 / n as f64;
        for (i, value) in target.iter_mut().enumerate().take(n - 1).skip(1) {
            *value = (previous_shift + shift) / 2.0;
            pinned_sum -= *value;
            previous_shift = shift;
            shift = pinned_sum / (n - i) as f64;
        }
        target[n - 1] = 1.0;
        let columns: Vec<_> = (0..n)
            .map(|i| {
                let mut column = vec![0.0; n + 1];
                column[i] = 1.0;
                column[n] = 1.0;
                column
            })
            .collect();
        let mut residual: Vec<_> = target.iter().map(|value| -value).collect();
        residual.push(-target.iter().sum::<f64>());
        let limits = vec![10_000.0; n];
        let mut active = columns.clone();
        for pin in 0..n - 1 {
            let step = correction(&active, &residual, &limits).unwrap();
            let outward: Vec<_> = (0..n - 1)
                .filter(|&i| !active[i].iter().all(|&value| value == 0.0) && step[i] < 0.0)
                .collect();
            assert_eq!(outward, vec![pin], "pinning pass {pin}");
            active[pin].fill(0.0);
        }
        let free_solution = correction(&active, &residual, &limits).unwrap();
        let current = vec![0.0; n];
        let mut bounds = vec![(0.0, 10_000.0); n];
        bounds[n - 1] = (-10_000.0, 10_000.0);
        let actual =
            bounded_qr_correction(&columns, &residual, &current, &current, &limits, &bounds)
                .unwrap();
        assert!(actual[..n - 1].iter().all(|&value| value == 0.0));
        assert_eq!(actual[n - 1], free_solution[n - 1]);
        assert!(actual[n - 1] > 0.0);
    }
}
