//! The row facts cache: where each job computes and whether it is Ray-Traced.
//!
//! The database row holds neither, so the controller reads them from the job's snapshot, once
//! per job id, with a small serde probe that skips the rest of the text. A save, a copy or a
//! bulk change replaces or drops the entry; ids no longer listed are dropped. Showing the rows
//! (`apply_view`) never decodes a snapshot: it reads this cache only.

use super::super::rows::{FactsMap, JobFacts};
use pleochroma::renderer::engine::EngineSpec;
use pleochroma_render_jobs::ComputeChoice;
use serde::Deserialize;
use std::collections::BTreeSet;

/// The two keys of a job file that the row needs.
#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    engine: EngineSpec,
    compute: ProbeCompute,
}

/// `compute`, read for its target only.
#[derive(Deserialize)]
struct ProbeCompute {
    target: ComputeChoice,
}

/// The facts of a job file's text, or `None` when it cannot be read.
pub fn read_facts(snapshot: &str) -> Option<JobFacts> {
    let probe: Probe = serde_json::from_str(snapshot).ok()?;
    Some(JobFacts {
        compute: probe.compute.target,
        is_ray: probe.engine.is_ray(),
    })
}

/// The facts read so far, by job id.
#[derive(Debug, Default)]
pub struct FactsCache {
    known: FactsMap,
    /// Jobs whose snapshot could not be read: not asked again until the job changes.
    unreadable: BTreeSet<i64>,
}

impl FactsCache {
    /// An empty cache.
    pub const fn new() -> Self {
        Self {
            known: FactsMap::new(),
            unreadable: BTreeSet::new(),
        }
    }

    /// Drops every entry whose id is not in `listed` and returns the listed ids that have no
    /// entry yet (the ones to read).
    pub fn retain_listed(&mut self, listed: &[i64]) -> Vec<i64> {
        self.known.retain(|id, _| listed.contains(id));
        self.unreadable.retain(|id| listed.contains(id));
        listed
            .iter()
            .copied()
            .filter(|id| !self.known.contains_key(id) && !self.unreadable.contains(id))
            .collect()
    }

    /// Stores what was read for `id` (`None`: the snapshot could not be read).
    pub fn insert(&mut self, id: i64, facts: Option<JobFacts>) {
        match facts {
            Some(facts) => {
                self.unreadable.remove(&id);
                self.known.insert(id, facts);
            }
            None => {
                self.known.remove(&id);
                self.unreadable.insert(id);
            }
        }
    }

    /// Forgets `id`, so the next refresh reads it again (after an edit).
    pub fn forget(&mut self, id: i64) {
        self.known.remove(&id);
        self.unreadable.remove(&id);
    }

    /// Replaces the compute choice of a known job (a bulk change), without a read.
    pub fn set_compute(&mut self, id: i64, compute: ComputeChoice) {
        if let Some(facts) = self.known.get_mut(&id) {
            facts.compute = compute;
        }
    }

    /// The facts the rows are built from.
    pub const fn map(&self) -> &FactsMap {
        &self.known
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ray_text() -> &'static str {
        r#"{"engine":{"engine":"RayTraced"},"format":8,"compute":{"target":"both","transfer":"full_data"},"output":"x.png"}"#
    }

    fn path_text() -> &'static str {
        r#"{"format":8,"compute":{"target":"local","transfer":"full_data"}}"#
    }

    #[test]
    fn the_probe_reads_compute_and_engine_and_nothing_else() {
        assert_eq!(
            read_facts(ray_text()),
            Some(JobFacts {
                compute: ComputeChoice::Both,
                is_ray: true
            })
        );
        assert_eq!(
            read_facts(path_text()),
            Some(JobFacts {
                compute: ComputeChoice::Local,
                is_ray: false
            })
        );
        assert_eq!(read_facts("not json"), None);
        assert_eq!(read_facts(r#"{"format":8}"#), None);
    }

    /// Reads every listed id that has no entry, noting each read.
    fn sync(cache: &mut FactsCache, reads: &mut Vec<i64>, listed: &[i64]) {
        for id in cache.retain_listed(listed) {
            reads.push(id);
            cache.insert(id, read_facts(path_text()));
        }
    }

    /// The cache asks for each id once, drops ids that left the list, and asks again after
    /// `forget`.
    #[test]
    fn the_cache_reads_once_per_id_and_follows_an_edit() {
        let mut cache = FactsCache::new();
        let mut reads = Vec::new();
        sync(&mut cache, &mut reads, &[1, 2]);
        sync(&mut cache, &mut reads, &[1, 2]);
        sync(&mut cache, &mut reads, &[1, 2, 3]);
        assert_eq!(reads, [1, 2, 3]);

        cache.set_compute(2, ComputeChoice::Remote);
        assert_eq!(cache.map()[&2].compute, ComputeChoice::Remote);
        sync(&mut cache, &mut reads, &[1, 2, 3]);
        assert_eq!(reads, [1, 2, 3]);

        // An edit drops the entry; the next refresh reads that id again.
        cache.forget(2);
        sync(&mut cache, &mut reads, &[1, 2, 3]);
        assert_eq!(reads, [1, 2, 3, 2]);

        // An id that left the list is dropped, and read again if it comes back.
        sync(&mut cache, &mut reads, &[1]);
        assert_eq!(cache.map().len(), 1);
        sync(&mut cache, &mut reads, &[1, 3]);
        assert_eq!(reads, [1, 2, 3, 2, 3]);
    }

    #[test]
    fn an_unreadable_job_is_not_asked_again_until_it_changes() {
        let mut cache = FactsCache::new();
        assert_eq!(cache.retain_listed(&[4]), [4]);
        cache.insert(4, None);
        assert!(cache.retain_listed(&[4]).is_empty());
        assert!(cache.map().is_empty());
        cache.forget(4);
        assert_eq!(cache.retain_listed(&[4]), [4]);
    }
}
