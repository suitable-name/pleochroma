//! Deterministic per-texel and per-chunk random streams.

const MAP_SEED: u64 = 0x8e72_5b19_6a4d_c3f1;
const STRIDE: u64 = 0x9e37_79b9_7f4a_7c15;

/// Independent deterministic stream for one texel or photon chunk.
pub(super) struct SplitMix64(u64);

impl SplitMix64 {
    /// Seeds a photon-chunk stream by its stable chunk index.
    pub(super) const fn for_chunk(index: u64) -> Self {
        Self(MAP_SEED ^ index.wrapping_mul(STRIDE))
    }
    /// Seeds a gather stream by the texel's row-major index.
    pub(super) const fn for_texel(index: u64) -> Self {
        Self::for_chunk(index)
    }
    const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(STRIDE);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Returns a uniform value in [0, 1) using 24 random bits.
    pub(super) const fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }
    /// Returns a uniform value in [0, 1) using 53 random bits.
    pub(super) const fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}
