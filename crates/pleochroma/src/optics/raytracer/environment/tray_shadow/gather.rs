//! Geometric blocked fraction at a tray texel.
use super::{
    geometry::{StoneFrame, StoneHit},
    lights::LightTable,
    rng::SplitMix64,
};
use glam::Vec3;

/// Estimates the geometric fraction blocked at one texel using stratified CDF samples.
pub(super) fn blocked_fraction(
    frame: &StoneFrame,
    stone: &StoneHit<'_>,
    table: &LightTable,
    texel: (u32, u32),
    resolution: u32,
    n_dirs: u32,
) -> f32 {
    let p = frame.texel_center(texel, resolution) + Vec3::Y * frame.lift;
    let index = u64::from(texel.1) * u64::from(resolution) + u64::from(texel.0);
    let mut rng = SplitMix64::for_texel(index);
    let xi = rng.next_f64();
    let mut blocked = 0_u32;
    for k in 0..n_dirs {
        let dir = table.sample((f64::from(k) + xi) / f64::from(n_dirs), &mut rng);
        if frame.hits_sphere(p, dir) && stone.hits(p, dir) {
            blocked += 1;
        }
    }
    blocked as f32 / n_dirs as f32
}
