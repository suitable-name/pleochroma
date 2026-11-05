//! The labelled states of the imitation rounds (`DAgger`) and the exploration noise that visits them.
//!
//! Every round rolls the current policy out with a little noise, asks the simulator for the
//! finite-difference labels of a share of the visited states ([`pleochroma_ai::env::TeacherLabel`])
//! and adds them to one growing aggregate, [`LabelledStates`]. The next update trains on the whole
//! aggregate, so the model keeps learning from the states of its earlier, worse selves. The
//! aggregate retains a mixture of recent and older states under row and byte limits.

use std::collections::VecDeque;

use pleochroma_ai::{
    env::{N_COMPONENTS, TeacherLabel},
    model::Prng,
    training::ImitationBatch,
};

use super::config::MAX_LABEL_BYTES;

/// One labelled state: the flat observation, the free-tier mask and the label rows the
/// imitation loss reads (see [`pleochroma_ai::training::ImitationBatch`]).
#[derive(Clone, Debug, PartialEq)]
pub struct LabelledSample {
    /// The flat observation, `[global | padded tier tokens]`.
    state: Vec<f32>,
    /// Which real tiers the environment moves; its length is the tier count.
    free: Vec<bool>,
    /// Per real tier, the seven component slopes; all NaN for a tier without a label.
    sensitivities: Vec<[f32; N_COMPONENTS]>,
    /// Per real tier, the score slope; NaN for a tier without a label.
    score_slopes: Vec<f32>,
    /// Verified raw action, or a wholly NaN row when the search found no improving action.
    action_targets: Vec<f32>,
    /// The oracle step's score drop; NaN when the label has none.
    oracle_gain: f32,
}

impl LabelledSample {
    fn retained_bytes(&self) -> usize {
        self.state.capacity() * std::mem::size_of::<f32>()
            + self.free.capacity() * std::mem::size_of::<bool>()
            + self.sensitivities.capacity() * std::mem::size_of::<[f32; N_COMPONENTS]>()
            + (self.score_slopes.capacity() + self.action_targets.capacity())
                * std::mem::size_of::<f32>()
            + std::mem::size_of::<Self>()
    }
    /// The sample of `state` (flat observation) with free-tier mask `free` and `label`.
    ///
    /// The tier count is `free.len()`. A tier the label has no entry for (`None`, or beyond the
    /// label's length) gets NaN rows, and so does a missing oracle gain: NaN means "no label"
    /// for the loss.
    #[must_use]
    pub fn new(state: Vec<f32>, free: Vec<bool>, label: &TeacherLabel) -> Self {
        let tier = |k: usize| label.tiers.get(k).copied().flatten();
        let sensitivities = (0..free.len())
            .map(|k| tier(k).map_or([f32::NAN; N_COMPONENTS], |t| t.components))
            .collect();
        let score_slopes = (0..free.len())
            .map(|k| tier(k).map_or(f32::NAN, |t| t.score))
            .collect();
        let action_targets = label
            .action_target
            .as_ref()
            .filter(|row| row.len() == free.len() && row.iter().all(|value| value.is_finite()))
            .cloned()
            .unwrap_or_else(|| vec![f32::NAN; free.len()]);
        let mut state = state;
        let compact_len = pleochroma_ai::env::flat_len(free.len());
        if state.len() >= compact_len && state[compact_len..].iter().all(|v| v.to_bits() == 0) {
            state.truncate(compact_len);
            state.shrink_to_fit();
        }
        Self {
            state,
            free,
            sensitivities,
            score_slopes,
            action_targets,
            oracle_gain: label.oracle_gain.unwrap_or(f32::NAN),
        }
    }
}

/// Whether `label` teaches anything: some tier has a finite component slope, or the
/// oracle gain or a complete verified action is finite. A state without these is not worth a place in the
/// aggregate (the loss would skip every entry of it).
#[must_use]
pub fn label_is_informative(label: &TeacherLabel) -> bool {
    label.oracle_gain.is_some_and(f32::is_finite)
        || label.action_target.as_ref().is_some_and(|row| {
            row.len() == label.tiers.len() && !row.is_empty() && row.iter().all(|v| v.is_finite())
        })
        || label
            .tiers
            .iter()
            .flatten()
            .any(|tier| tier.components.iter().any(|c| c.is_finite()))
}

/// The aggregate of labelled states, holding at most `cap` of them.
/// The newest quarter stays ordered; older rows may be reordered during eviction.
///
/// The seven queues are parallel (one entry per state), in the layout of
/// [`pleochroma_ai::training::ImitationBatch`].
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct LabelledStates {
    cap: usize,
    #[serde(default)]
    eviction_counter: u64,
    byte_limit: usize,
    bytes: usize,
    row_bytes: VecDeque<usize>,
    states: VecDeque<Vec<f32>>,
    n_tiers: VecDeque<usize>,
    free_tiers: VecDeque<Vec<bool>>,
    sensitivities: VecDeque<Vec<[f32; N_COMPONENTS]>>,
    score_slopes: VecDeque<Vec<f32>>,
    action_targets: VecDeque<Vec<f32>>,
    oracle_gain: VecDeque<f32>,
}

impl LabelledStates {
    /// Checks the parallel replay queues before installing a recovery snapshot.
    pub(super) fn validate_recovery(
        &self,
        cap: usize,
        max_tiers: usize,
        byte_limit: usize,
    ) -> Result<(), super::RunnerError> {
        let rows = self.states.len();
        let lengths = [
            self.row_bytes.len(),
            self.n_tiers.len(),
            self.free_tiers.len(),
            self.sensitivities.len(),
            self.score_slopes.len(),
            self.action_targets.len(),
            self.oracle_gain.len(),
        ];
        if self.cap != cap
            || self.byte_limit != byte_limit
            || rows > cap
            || self.bytes > self.byte_limit
            || lengths.iter().any(|n| *n != rows)
            || self
                .row_bytes
                .iter()
                .try_fold(0usize, |sum, n| sum.checked_add(*n))
                != Some(self.bytes)
        {
            return Err(super::RunnerError::Data(
                "invalid saved replay queue lengths or limits".into(),
            ));
        }
        let width = pleochroma_ai::env::flat_len(max_tiers);
        for row in 0..rows {
            let tiers = self.n_tiers[row];
            if tiers == 0
                || tiers > max_tiers
                || (self.states[row].len() != width
                    && self.states[row].len() != pleochroma_ai::env::flat_len(tiers))
                || self.states[row].iter().any(|v| !v.is_finite())
                || self.free_tiers[row].len() != tiers
                || self.sensitivities[row].len() != tiers
                || self.score_slopes[row].len() != tiers
                || self.action_targets[row].len() != tiers
                || self.sensitivities[row]
                    .iter()
                    .flatten()
                    .any(|v| v.is_infinite())
                || self.score_slopes[row]
                    .iter()
                    .chain(&self.action_targets[row])
                    .any(|v| v.is_infinite())
                || !(self.action_targets[row].iter().all(|v| v.is_finite())
                    || self.action_targets[row].iter().all(|v| v.is_nan()))
                || self.oracle_gain[row].is_infinite()
            {
                return Err(super::RunnerError::Data(format!(
                    "invalid saved replay row {row}"
                )));
            }
        }
        Ok(())
    }

    /// An empty aggregate that keeps at most `cap` states (at least 1).
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self::with_budget(cap, MAX_LABEL_BYTES)
    }

    /// An empty aggregate with a host-memory payload budget in bytes.
    #[must_use]
    pub fn with_budget(cap: usize, byte_limit: usize) -> Self {
        Self {
            cap: cap.max(1),
            eviction_counter: 0,
            byte_limit,
            bytes: 0,
            row_bytes: VecDeque::new(),
            states: VecDeque::new(),
            n_tiers: VecDeque::new(),
            free_tiers: VecDeque::new(),
            sensitivities: VecDeque::new(),
            score_slopes: VecDeque::new(),
            action_targets: VecDeque::new(),
            oracle_gain: VecDeque::new(),
        }
    }

    /// Queue backing allocations are accounted in addition to row payloads. Row headers
    /// are included in both figures, intentionally leaving a conservative budget margin.
    fn queue_bytes(&self) -> usize {
        // Two slots per retained row cover VecDeque's geometric growth; we shrink
        // queues when eviction or recovery would leave more spare capacity than this.
        let slot = 2 * std::mem::size_of::<usize>()
            + 4 * std::mem::size_of::<Vec<f32>>()
            + std::mem::size_of::<Vec<bool>>()
            + std::mem::size_of::<Vec<[f32; N_COMPONENTS]>>()
            + std::mem::size_of::<f32>();
        self.len().saturating_mul(2).saturating_mul(slot)
    }

    pub(super) fn normalize_queue_capacity(&mut self) {
        let max_capacity = self.len().saturating_mul(2);
        if [
            self.row_bytes.capacity(),
            self.states.capacity(),
            self.n_tiers.capacity(),
            self.free_tiers.capacity(),
            self.sensitivities.capacity(),
            self.score_slopes.capacity(),
            self.action_targets.capacity(),
            self.oracle_gain.capacity(),
        ]
        .into_iter()
        .any(|n| n > max_capacity)
        {
            self.shrink_queues();
        }
    }

    /// States held now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Accounted host payload and per-row headers currently retained.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        self.bytes.saturating_add(self.queue_bytes())
    }

    /// Whether no state is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Adds a state, evicting deterministically from older rows when over budget.
    pub fn push(&mut self, sample: LabelledSample) {
        let bytes = sample.retained_bytes();
        self.bytes = self.bytes.saturating_add(bytes);
        self.row_bytes.push_back(bytes);
        self.n_tiers.push_back(sample.free.len());
        self.states.push_back(sample.state);
        self.free_tiers.push_back(sample.free);
        self.sensitivities.push_back(sample.sensitivities);
        self.score_slopes.push_back(sample.score_slopes);
        self.action_targets.push_back(sample.action_targets);
        self.oracle_gain.push_back(sample.oracle_gain);
        self.enforce_budget();
    }

    /// Apply the current conservative accounting after loading legacy recovery data.
    pub(super) fn enforce_budget(&mut self) {
        self.normalize_queue_capacity();
        while !self.states.is_empty()
            && (self.states.len() > self.cap || self.retained_bytes() > self.byte_limit)
        {
            // Preserve the newest quarter and choose deterministically among the older rows.
            // The serialized counter makes future decisions identical after recovery.
            let eligible = (self.states.len() * 3 / 4).max(1);
            let mut x = self.eviction_counter.wrapping_add(0x9e37_79b9_7f4a_7c15);
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^= x >> 31;
            self.eviction_counter = self.eviction_counter.wrapping_add(1);
            let index = (x % eligible as u64) as usize;
            // Swap the selected older row to the front, then pop it. Only the older
            // segment is reordered; every parallel queue takes the identical swap.
            self.row_bytes.swap(0, index);
            self.n_tiers.swap(0, index);
            self.states.swap(0, index);
            self.free_tiers.swap(0, index);
            self.sensitivities.swap(0, index);
            self.score_slopes.swap(0, index);
            self.action_targets.swap(0, index);
            self.oracle_gain.swap(0, index);
            self.bytes = self
                .bytes
                .saturating_sub(self.row_bytes.pop_front().unwrap_or(0));
            self.n_tiers.pop_front();
            self.states.pop_front();
            self.free_tiers.pop_front();
            self.sensitivities.pop_front();
            self.score_slopes.pop_front();
            self.action_targets.pop_front();
            self.oracle_gain.pop_front();
        }
        self.normalize_queue_capacity();
    }

    fn shrink_queues(&mut self) {
        self.row_bytes.shrink_to_fit();
        self.states.shrink_to_fit();
        self.n_tiers.shrink_to_fit();
        self.free_tiers.shrink_to_fit();
        self.sensitivities.shrink_to_fit();
        self.score_slopes.shrink_to_fit();
        self.action_targets.shrink_to_fit();
        self.oracle_gain.shrink_to_fit();
    }

    /// Drops every state.
    pub fn clear(&mut self) {
        self.bytes = 0;
        self.eviction_counter = 0;
        self.row_bytes.clear();
        self.states.clear();
        self.n_tiers.clear();
        self.free_tiers.clear();
        self.sensitivities.clear();
        self.score_slopes.clear();
        self.action_targets.clear();
        self.oracle_gain.clear();
        self.shrink_queues();
    }

    /// The whole aggregate as a training batch. Nothing is copied: the
    /// queues are made contiguous in place and the batch borrows them.
    pub fn batch(&mut self) -> ImitationBatch<'_> {
        ImitationBatch {
            states: self.states.make_contiguous(),
            n_tiers: self.n_tiers.make_contiguous(),
            free_tiers: self.free_tiers.make_contiguous(),
            sensitivities: self.sensitivities.make_contiguous(),
            score_slopes: self.score_slopes.make_contiguous(),
            action_targets: self.action_targets.make_contiguous(),
            oracle_gain: self.oracle_gain.make_contiguous(),
        }
    }
}

/// The raw (pre-squash) action of an exploration step: `mean + std * N(0, 1)` on the free
/// tiers, the mean itself on the others.
///
/// A tier that is not free draws no random number, and with `std <= 0` nothing is drawn at all:
/// the result is the deterministic policy. This is written out instead of going through
/// `GaussianPolicy::sample_raw_masked` because that sampler clamps its log-std into the policy's
/// own range, which would turn a requested standard deviation of 0 into 0.05.
#[must_use]
pub fn explore_raw(mean: &[f32], free: &[bool], std: f32, rng: &mut Prng) -> Vec<f32> {
    mean.iter()
        .enumerate()
        .map(|(k, &m)| {
            if std > 0.0 && free.get(k).copied().unwrap_or(false) {
                std.mul_add(rng.standard_normal(), m)
            } else {
                m
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use pleochroma_ai::env::{GLOBAL_DIM, TIER_DIM, TierSensitivity, flat_len};

    use super::*;

    fn sensitivity(base: f32) -> TierSensitivity {
        TierSensitivity {
            components: [
                base,
                base + 1.0,
                base + 2.0,
                base + 3.0,
                base + 4.0,
                base + 5.0,
                base + 6.0,
            ],
            score: base * 10.0,
            forward_probe: true,
        }
    }

    fn label(tiers: Vec<Option<TierSensitivity>>, gain: Option<f32>) -> TeacherLabel {
        TeacherLabel {
            tiers,
            oracle_gain: gain,
            oracle_scale: gain.map(|_| 1.0),
            action_target: None,
            evaluations: 0,
        }
    }

    /// A sample of a stone with `n` tiers whose state starts with `marker` and whose oracle
    /// gain is `marker`.
    fn sample(marker: f32, n: usize) -> LabelledSample {
        let mut state = vec![0.0; flat_len(4)];
        state[0] = marker;
        let tiers = (0..n).map(|k| Some(sensitivity(k as f32))).collect();
        LabelledSample::new(state, vec![true; n], &label(tiers, Some(marker)))
    }

    #[test]
    fn a_label_becomes_one_row_per_tier_with_nan_for_what_is_missing() {
        let tiers = vec![Some(sensitivity(1.0)), None, Some(sensitivity(3.0))];
        let s = LabelledSample::new(vec![0.5; 4], vec![true, false, true], &label(tiers, None));
        assert_eq!(s.sensitivities.len(), 3);
        assert_eq!(s.score_slopes.len(), 3);
        assert_eq!(s.sensitivities[0], [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(s.score_slopes[0], 10.0);
        // A tier without a label is all NaN, with a NaN score slope.
        assert!(s.sensitivities[1].iter().all(|v| v.is_nan()));
        assert!(s.score_slopes[1].is_nan());
        assert_eq!(s.sensitivities[2], [3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0]);
        // No oracle gain is NaN as well.
        assert!(s.oracle_gain.is_nan());
        assert_eq!(s.free, [true, false, true]);
        assert!(s.action_targets.iter().all(|v| v.is_nan()));
    }

    #[test]
    fn a_verified_action_keeps_its_zero_coordinates_through_replay_eviction() {
        let mut measured = label(vec![None; 3], None);
        measured.action_target = Some(vec![0.75, 0.0, -0.25]);
        assert!(label_is_informative(&measured));
        let mut replay = LabelledStates::new(1);
        replay.push(sample(1.0, 2));
        replay.push(LabelledSample::new(vec![], vec![true; 3], &measured));
        assert_eq!(replay.batch().action_targets, [vec![0.75, 0.0, -0.25]]);
        measured.action_target = Some(vec![0.75, f32::NAN, -0.25]);
        let invalid = LabelledSample::new(vec![], vec![true; 3], &measured);
        assert!(invalid.action_targets.iter().all(|v| v.is_nan()));
    }

    #[test]
    fn a_yield_that_cannot_be_measured_stays_a_nan_entry_of_a_labelled_tier() {
        let mut yieldless = sensitivity(2.0);
        yieldless.components[3] = f32::NAN;
        let s = LabelledSample::new(
            vec![0.0; 2],
            vec![true],
            &label(vec![Some(yieldless)], Some(0.0)),
        );
        assert_eq!(s.sensitivities[0][..3], [2.0, 3.0, 4.0]);
        assert!(s.sensitivities[0][3].is_nan());
        assert_eq!(s.oracle_gain, 0.0);
    }

    #[test]
    fn the_rows_follow_the_free_mask_not_the_labels_length() {
        // A shorter label leaves the extra tiers unlabelled; a longer one is cut.
        let short = LabelledSample::new(
            vec![0.0],
            vec![true; 3],
            &label(vec![Some(sensitivity(1.0))], None),
        );
        assert_eq!(short.sensitivities.len(), 3);
        assert!(short.score_slopes[1].is_nan() && short.score_slopes[2].is_nan());
        let long = LabelledSample::new(
            vec![0.0],
            vec![true; 1],
            &label(vec![Some(sensitivity(1.0)), Some(sensitivity(2.0))], None),
        );
        assert_eq!(long.sensitivities.len(), 1);
    }

    #[test]
    fn a_label_is_informative_with_a_finite_component_or_gain() {
        assert!(!label_is_informative(&label(vec![None, None], None)));
        assert!(!label_is_informative(&label(Vec::new(), None)));
        assert!(label_is_informative(&label(vec![None], Some(0.0))));
        assert!(label_is_informative(&label(
            vec![Some(sensitivity(1.0))],
            None
        )));
        // A gain that is not a number teaches nothing.
        assert!(!label_is_informative(&label(vec![None], Some(f32::NAN))));
        // Nor does a tier whose every figure is NaN.
        let mut blank = sensitivity(1.0);
        blank.components = [f32::NAN; N_COMPONENTS];
        blank.score = f32::NAN;
        assert!(!label_is_informative(&label(vec![Some(blank)], None)));
        // Score slopes are diagnostic only: an unverified direction is not supervision.
        let mut score_only = blank;
        score_only.score = 2.0;
        assert!(!label_is_informative(&label(vec![Some(score_only)], None)));
        // So is a single finite component.
        let mut one = blank;
        one.components[1] = 0.5;
        assert!(label_is_informative(&label(vec![Some(one)], None)));
    }

    #[test]
    fn eviction_keeps_recent_rows_and_parallel_queues_aligned() {
        let mut aggregate = LabelledStates::new(3);
        assert!(aggregate.is_empty());
        for marker in 0..5_usize {
            aggregate.push(sample(marker as f32, 2));
            assert_eq!(aggregate.len(), (marker + 1).min(3));
        }
        let batch = aggregate.batch();
        assert_eq!(batch.oracle_gain.len(), 3);
        assert_eq!(batch.oracle_gain[2], 4.0);
        let firsts: Vec<f32> = batch.states.iter().map(|s| s[0]).collect();
        assert_eq!(firsts, batch.oracle_gain);
        assert_eq!(batch.n_tiers, [2, 2, 2]);
        assert_eq!(batch.free_tiers.len(), 3);
        assert_eq!(batch.sensitivities.len(), 3);
        assert_eq!(batch.score_slopes.len(), 3);
    }

    #[test]
    fn a_capacity_of_zero_still_keeps_the_newest_state() {
        let mut aggregate = LabelledStates::new(0);
        aggregate.push(sample(1.0, 1));
        aggregate.push(sample(2.0, 1));
        assert_eq!(aggregate.len(), 1);
        assert_eq!(aggregate.batch().oracle_gain, [2.0]);
    }

    #[test]
    fn byte_eviction_keeps_parallel_rows_aligned_and_clear_releases_accounting() {
        let mut aggregate = LabelledStates::new(100);
        let row_bytes = sample(0.0, 2).retained_bytes();
        for marker in [1.0, 2.0] {
            aggregate.push(sample(marker, 2));
        }
        aggregate.byte_limit = aggregate.retained_bytes();
        aggregate.push(sample(3.0, 2));
        assert!(aggregate.retained_bytes() <= aggregate.byte_limit);
        assert_eq!(aggregate.bytes, 2 * row_bytes);
        let batch = aggregate.batch();
        assert_eq!(batch.oracle_gain.len(), 2);
        assert_eq!(batch.oracle_gain[1], 3.0);
        assert_eq!(batch.states[0][0], batch.oracle_gain[0]);
        batch.validate(GLOBAL_DIM, TIER_DIM, 4).unwrap();
        aggregate.clear();
        assert_eq!(aggregate.bytes, 0);
        assert!(aggregate.row_bytes.is_empty());
        aggregate.byte_limit = row_bytes - 1;
        aggregate.push(sample(4.0, 2));
        assert!(aggregate.is_empty());
        assert_eq!(aggregate.bytes, 0);
    }

    #[test]
    fn the_batch_has_matching_lengths_and_passes_the_trainers_own_validation() {
        let mut aggregate = LabelledStates::new(10);
        for (marker, n) in [(0.0, 1), (1.0, 3), (2.0, 4), (3.0, 2)] {
            aggregate.push(sample(marker, n));
        }
        let batch = aggregate.batch();
        assert_eq!(batch.len(), 4);
        assert!(!batch.is_empty());
        assert_eq!(batch.n_tiers, [1, 3, 4, 2]);
        for i in 0..batch.len() {
            assert_eq!(batch.free_tiers[i].len(), batch.n_tiers[i]);
            assert_eq!(batch.sensitivities[i].len(), batch.n_tiers[i]);
            assert_eq!(batch.score_slopes[i].len(), batch.n_tiers[i]);
        }
        assert_eq!(batch.states[0].len(), flat_len(1));
        batch
            .validate(GLOBAL_DIM, TIER_DIM, 4)
            .expect("the aggregate is a valid imitation batch");
    }

    #[test]
    fn a_wrapped_queue_still_yields_one_contiguous_batch_in_order() {
        // Evict and push repeatedly so the ring buffers wrap around.
        let mut aggregate = LabelledStates::new(4);
        for marker in 0..11 {
            aggregate.push(sample(marker as f32, 2));
            let _ = aggregate.batch();
        }
        assert_eq!(aggregate.batch().oracle_gain[3], 10.0);
    }

    #[test]
    fn clearing_empties_every_queue() {
        let mut aggregate = LabelledStates::new(5);
        aggregate.push(sample(1.0, 2));
        aggregate.push(sample(2.0, 2));
        aggregate.clear();
        assert!(aggregate.is_empty());
        let batch = aggregate.batch();
        assert!(batch.is_empty());
        assert!(batch.n_tiers.is_empty() && batch.oracle_gain.is_empty());
        // It can be refilled afterwards.
        aggregate.push(sample(3.0, 1));
        assert_eq!(aggregate.len(), 1);
    }

    #[test]
    fn replay_budget_includes_queue_capacity_and_eviction_keeps_rows_aligned() {
        let mut replay = LabelledStates::with_budget(10, 1024 * 1024);
        for i in 0..10 {
            replay.push(sample(i as f32, 2));
        }
        assert!(replay.retained_bytes() > replay.bytes);
        replay.byte_limit = replay.retained_bytes();
        for i in 10..100 {
            replay.push(sample(i as f32, 2));
            assert!(replay.retained_bytes() <= replay.byte_limit);
            let batch = replay.batch();
            for (state, gain) in batch.states.iter().zip(batch.oracle_gain) {
                assert_eq!(state[0], *gain);
            }
            assert_eq!(*batch.oracle_gain.last().unwrap(), i as f32);
        }
    }

    #[test]
    fn compact_rows_preserve_real_values_and_resume_eviction_sequence() {
        let mut state: Vec<f32> = (0..flat_len(2)).map(|i| i as f32).collect();
        let expected = state.clone();
        state.resize(flat_len(4), 0.0);
        let mut replay = LabelledStates::with_budget(5, 8 * 1024 * 1024);
        replay.push(LabelledSample::new(
            state,
            vec![true, false],
            &label(vec![None; 2], Some(1.0)),
        ));
        assert_eq!(replay.batch().states[0], expected);
        for i in 0..20 {
            replay.push(sample(i as f32, 2));
        }
        let saved = rmp_serde::to_vec_named(&replay).unwrap();
        let mut restored: LabelledStates = rmp_serde::from_slice(&saved).unwrap();
        for i in 20..40 {
            replay.push(sample(i as f32, 2));
            restored.push(sample(i as f32, 2));
        }
        let original = replay.batch();
        let resumed = restored.batch();
        assert_eq!(original.oracle_gain, resumed.oracle_gain);
        assert_eq!(original.states, resumed.states);
        for (state, gain) in original.states.iter().zip(original.oracle_gain) {
            assert_eq!(state[0], *gain);
        }
        assert_eq!(original.oracle_gain[original.len() - 1], 39.0);
        assert_eq!(replay.bytes, restored.bytes);
    }

    #[test]
    fn exploration_without_noise_is_the_deterministic_policy_and_draws_nothing() {
        let mean = [0.5, -1.0, 0.25];
        let mut rng = Prng::new(9);
        let before = rng.clone();
        assert_eq!(explore_raw(&mean, &[true; 3], 0.0, &mut rng), mean);
        assert_eq!(explore_raw(&mean, &[true; 3], -1.0, &mut rng), mean);
        assert_eq!(rng, before, "no random number may be drawn");
    }

    #[test]
    fn exploration_noise_is_reproducible_and_skips_tiers_that_are_not_free() {
        let mean = [0.5, -1.0, 0.25, 1.0];
        let free = [true, false, true, false];
        let a = explore_raw(&mean, &free, 0.3, &mut Prng::new(4));
        let b = explore_raw(&mean, &free, 0.3, &mut Prng::new(4));
        assert_eq!(a, b);
        assert_ne!(a[0], mean[0]);
        assert_ne!(a[2], mean[2]);
        assert_eq!((a[1], a[3]), (mean[1], mean[3]));
        // Tiers that are not free draw nothing: the free tiers see the same numbers as in a
        // stream that never met the others.
        let mut reference = Prng::new(4);
        assert_eq!(a[0], 0.3f32.mul_add(reference.standard_normal(), mean[0]));
        assert_eq!(a[2], 0.3f32.mul_add(reference.standard_normal(), mean[2]));
        // A mask shorter than the mean marks the missing tiers as not free.
        let short = explore_raw(&mean, &[true], 0.3, &mut Prng::new(4));
        assert_eq!(short[0], a[0]);
        assert_eq!(short[1..], mean[1..]);
    }

    #[test]
    fn exploration_noise_has_the_requested_spread() {
        let mut rng = Prng::new(11);
        let n = 20_000;
        let mean = vec![0.0; n];
        let draws = explore_raw(&mean, &vec![true; n], 0.3, &mut rng);
        let average = draws.iter().sum::<f32>() / n as f32;
        let variance = draws.iter().map(|d| (d - average).powi(2)).sum::<f32>() / n as f32;
        assert!(average.abs() < 0.01, "{average}");
        assert!((variance.sqrt() - 0.3).abs() < 0.01, "{}", variance.sqrt());
    }
}
