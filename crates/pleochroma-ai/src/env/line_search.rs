//! Ordered candidate iteration that can share spare capacity during a line search.

use super::{CandidateOutcome, ContinuousTierAction, EnvCore};

/// Recheck capacity before every unstarted scale. An indexed parallel suffix retains
/// scale order; its caller stops consuming at the first accepted outcome, so reported
/// evaluation counts retain their sequential meaning despite speculative computation.
pub(crate) fn candidates<'a>(
    core: &'a EnvCore,
    action: &'a ContinuousTierAction,
    scales: &'a [f64],
    parallel: &'a (dyn Fn() -> bool + Sync),
) -> impl Iterator<Item = (f64, CandidateOutcome)> + 'a {
    let mut next = 0;
    let mut pending: Option<std::vec::IntoIter<CandidateOutcome>> = None;
    std::iter::from_fn(move || {
        let &scale = scales.get(next)?;
        if pending.is_none() {
            let can_share = parallel();
            if can_share && scales.len() - next > 1 {
                pending = Some(
                    core.try_scales_parallel(action, &scales[next..])
                        .into_iter(),
                );
            }
        }
        let outcome = match &mut pending {
            Some(outcomes) => outcomes.next()?,
            None => core.try_candidate(action, scale),
        };
        next += 1;
        Some((scale, outcome))
    })
}
