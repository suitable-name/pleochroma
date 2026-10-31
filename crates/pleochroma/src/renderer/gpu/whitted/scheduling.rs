//! Integer chunk timing avoids making raster/sample coordinates pass through f32.
#[derive(Default)]
pub(super) struct ChunkTiming {
    scene: Option<u64>,
    ns_per_tuple: Option<u128>,
    previous_pixels: u32,
}

impl ChunkTiming {
    pub(super) fn chunk_pixels(&mut self, scene: u64, spp: u32, limit: u32) -> u32 {
        if self.scene != Some(scene) {
            *self = Self {
                scene: Some(scene),
                ..Self::default()
            };
        }
        // Eight milliseconds per chunk, two chunks per FIFO turn. Initial work is
        // conservative; subsequent turns grow at most twofold. The EMA includes
        // queue/readback overhead and therefore errs toward shorter GPU occupancy.
        let maximum = limit.clamp(1, super::CHUNK_PIXELS);
        let initial = (16_384 / spp.max(1)).max(1);
        let estimate = self.ns_per_tuple.map_or(initial, |ns| {
            let pixels = 8_000_000 / ns.max(1) / u128::from(spp.max(1));
            u32::try_from(pixels.min(u128::from(maximum)))
                .unwrap_or(maximum)
                .max(1)
        });
        let growth = self
            .previous_pixels
            .checked_mul(2)
            .unwrap_or(maximum)
            .max(initial);
        estimate.min(growth).clamp(1, maximum)
    }

    pub(super) fn observe(&mut self, ns: u128, tuples: u128, chunk: u32) {
        if tuples == 0 {
            return;
        }
        let sample = (ns / tuples).max(1);
        self.ns_per_tuple = Some(
            self.ns_per_tuple
                .map_or(sample, |old| (old * 3 + sample) / 4),
        );
        self.previous_pixels = chunk;
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkTiming;

    #[test]
    fn chunk_timing_is_bounded_adaptive_and_resets_for_new_uploads() {
        let mut timing = ChunkTiming::default();
        assert_eq!(timing.chunk_pixels(1, 1, u32::MAX), 16_384);
        timing.observe(1_000_000, 16_384, 16_384);
        assert_eq!(timing.chunk_pixels(1, 1, u32::MAX), 32_768);
        assert_eq!(timing.chunk_pixels(1, 1, 77), 77);
        assert_eq!(timing.chunk_pixels(1, 64, 77), 77);
        assert_eq!(timing.chunk_pixels(2, 4, u32::MAX), 4096);
        timing.observe(100_000_000, 4096 * 4, 4096);
        assert!(timing.chunk_pixels(2, 4, u32::MAX) < 4096);
        assert_eq!(timing.chunk_pixels(2, u32::MAX, 0), 1);
    }
}
