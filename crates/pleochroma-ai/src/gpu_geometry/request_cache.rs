//! Bounded exact request memoization, owned by one immutable episode/policy.
//! No tolerance keys: requested travel is derived from the exact proposal, and
//! the entire angle box is part of the key. The owner must check runtime health.
use super::certificate::same_state;
use pleochroma_cut_core::Design;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Mutex,
};

const CAPACITY: usize = 16;

#[derive(Debug)]
pub(crate) struct RequestCache<T>(Mutex<VecDeque<Entry<T>>>);

#[derive(Debug)]
struct Entry<T> {
    proposal: Design,
    bounds: BTreeMap<usize, (f64, f64)>,
    result: T,
}

impl<T> Default for RequestCache<T> {
    fn default() -> Self {
        Self(Mutex::new(VecDeque::new()))
    }
}

fn same_bounds(a: &BTreeMap<usize, (f64, f64)>, b: &BTreeMap<usize, (f64, f64)>) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((i, a), (j, b))| {
            i == j && a.0.to_bits() == b.0.to_bits() && a.1.to_bits() == b.1.to_bits()
        })
}

impl<T: Clone> RequestCache<T> {
    pub(crate) fn get(&self, proposal: &Design, bounds: &BTreeMap<usize, (f64, f64)>) -> Option<T> {
        self.0
            .lock()
            .ok()?
            .iter()
            .rev()
            .find(|entry| {
                same_state(&entry.proposal, proposal) && same_bounds(&entry.bounds, bounds)
            })
            .map(|entry| entry.result.clone())
    }

    pub(crate) fn insert(
        &self,
        proposal: &Design,
        bounds: &BTreeMap<usize, (f64, f64)>,
        result: T,
    ) {
        if let Ok(mut entries) = self.0.lock() {
            if let Some(i) = entries.iter().position(|entry| {
                same_state(&entry.proposal, proposal) && same_bounds(&entry.bounds, bounds)
            }) {
                entries.remove(i);
            }
            entries.push_back(Entry {
                proposal: proposal.clone(),
                bounds: bounds.clone(),
                result,
            });
            while entries.len() > CAPACITY {
                entries.pop_front();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CAPACITY, RequestCache};
    use pleochroma_cut_core::Design;
    use std::collections::BTreeMap;

    #[test]
    fn negative_requests_require_exact_proposal_and_bounds_and_are_episode_local() {
        let design = Design::concave_fixture();
        let cache = RequestCache::default();
        let bounds = BTreeMap::from([(2, (1.0_f64, 2.0_f64))]);
        cache.insert(&design, &bounds, "refused");
        assert_eq!(cache.get(&design, &bounds), Some("refused"));
        let changed_bounds = BTreeMap::from([(2, (1.0, 2.0_f64.next_up()))]);
        assert!(cache.get(&design, &changed_bounds).is_none());
        let mut changed = design.clone();
        changed.tiers[2].angle_deg = changed.tiers[2].angle_deg.next_up();
        assert!(
            cache.get(&changed, &bounds).is_none(),
            "travel changes invalidate the key"
        );
        changed = design.clone();
        changed.tier_ids[0] = pleochroma_cut_core::TierId(10_000);
        assert!(cache.get(&changed, &bounds).is_none());
        let other_episode: RequestCache<&str> = RequestCache::default();
        assert!(other_episode.get(&design, &bounds).is_none());
    }

    #[test]
    fn request_cache_is_bounded_and_does_not_equate_signed_zero_bounds() {
        let design = Design::concave_fixture();
        let cache = RequestCache::default();
        let bounds = BTreeMap::from([(2, (0.0, 1.0))]);
        cache.insert(&design, &bounds, 0);
        assert!(
            cache
                .get(&design, &BTreeMap::from([(2, (-0.0, 1.0))]))
                .is_none()
        );
        for i in 1..=CAPACITY {
            cache.insert(
                &design,
                &BTreeMap::from([(2, (i as f64, i as f64 + 1.0))]),
                i,
            );
        }
        assert!(cache.get(&design, &bounds).is_none());
    }
}
