//! The deterministic domain map of a Bragg structure.
//!
//! Opal and labradorite are made of ordered domains: patches in which the silica spheres (opal)
//! or the exsolution lamellae (labradorite) share one orientation and one lattice spacing. The
//! map is procedural, with no texture: a jittered grid of cell size `s`, a seed point
//! `p_c = (c + h3(c, seed)) s` in every cell `c = floor(x / s)`, and the domain at `x` is the one
//! whose seed point is nearest among the 27 cells around `x`. Everything a domain owns (whether
//! it is active, its normal-incidence Bragg wavelength, its lattice normal) is a hash of its id,
//! so the map is a pure function of `(seed, x)` and can be rebuilt bit for bit anywhere, on the
//! GPU included (the hash is [`hash_u32`]; the floats are `(h >> 8) / 2^24`, exact in `f32`).
//!
//! The patterns follow from the cell size alone: a small cell is Pinfire, a large one Harlequin
//! or Broad Flash.

use super::bragg::BraggStructure;
use crate::optics::raytracer::sampling::hash_u32;
use glam::Vec3;
use std::f32::consts::TAU;

/// Hash salts: one per quantity drawn from a cell or domain, so the quantities are independent.
const SALT_JITTER_X: u32 = 0x9E37_79B9;
const SALT_JITTER_Y: u32 = 0x7F4A_7C15;
const SALT_JITTER_Z: u32 = 0xF39C_C060;
const SALT_ID: u32 = 0x85EB_CA77;
const SALT_ACTIVE: u32 = 0x68E3_1DA4;
const SALT_COLOR_A: u32 = 0xB529_7A4D;
const SALT_COLOR_B: u32 = 0x1B56_C4E9;
const SALT_NORMAL_A: u32 = 0x2C1B_3C6D;
const SALT_NORMAL_B: u32 = 0x297A_2D39;

/// One ordered patch of the structure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Domain {
    /// The domain's id: a hash of its cell and the structure's seed.
    pub id: u32,
    /// The cell whose seed point defines the domain.
    pub cell: [i32; 3],
    /// Whether the patch reflects at all (`false` leaves a clear gap).
    pub active: bool,
    /// The Bragg wavelength at normal incidence, nanometres.
    pub lambda_b0_nm: f32,
    /// The unit lattice normal (the (111) plane normal of opal, the lamella normal of
    /// labradorite).
    pub normal: Vec3,
}

/// `h` as a float in `[0, 1)`, exactly: the top 24 bits over `2^24`.
fn unit_float(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// A hash of the cell `cell`, the structure seed and a quantity `salt`.
fn cell_hash(cell: [i32; 3], seed: u32, salt: u32) -> u32 {
    let mut h = hash_u32(seed ^ salt);
    for c in cell {
        h = hash_u32(h ^ c.cast_unsigned());
    }
    h
}

/// The cell containing `p` for cells of side `cell_size`.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a cell index of a point inside a stone is far inside i32"
)]
pub fn cell_of(cell_size: f32, p: Vec3) -> [i32; 3] {
    let q = p / cell_size;
    [q.x.floor() as i32, q.y.floor() as i32, q.z.floor() as i32]
}

/// The seed point of `cell`: the cell's corner plus a hashed jitter inside it.
#[must_use]
pub fn seed_point(structure: &BraggStructure, cell: [i32; 3]) -> Vec3 {
    let jitter = Vec3::new(
        unit_float(cell_hash(cell, structure.seed, SALT_JITTER_X)),
        unit_float(cell_hash(cell, structure.seed, SALT_JITTER_Y)),
        unit_float(cell_hash(cell, structure.seed, SALT_JITTER_Z)),
    );
    (Vec3::new(cell[0] as f32, cell[1] as f32, cell[2] as f32) + jitter) * structure.cell_size
}

/// The domain with the nearest seed point to `p`, searching the 27 cells around it.
///
/// A tie (a point exactly equidistant from two seed points) goes to the cell met first in the
/// fixed `x, y, z` loop order; ties have measure zero.
#[must_use]
pub fn domain_at(structure: &BraggStructure, p: Vec3) -> Domain {
    let home = cell_of(structure.cell_size, p);
    let mut best = home;
    let mut best_d2 = f32::INFINITY;
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                let cell = [home[0] + dx, home[1] + dy, home[2] + dz];
                let d2 = (seed_point(structure, cell) - p).length_squared();
                if d2 < best_d2 {
                    best_d2 = d2;
                    best = cell;
                }
            }
        }
    }
    domain_of_cell(structure, best)
}

/// Everything a domain owns, from its cell.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    clippy::manual_midpoint,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
pub fn domain_of_cell(structure: &BraggStructure, cell: [i32; 3]) -> Domain {
    let seed = structure.seed;
    let id = cell_hash(cell, seed, SALT_ID);
    let active = unit_float(cell_hash(cell, seed, SALT_ACTIVE)) < structure.coverage;

    // The normal-incidence Bragg wavelength: Gaussian about the middle of the color range with a
    // quarter of its width as the standard deviation (Box-Muller), clamped to the range.
    let u1 = unit_float(cell_hash(cell, seed, SALT_COLOR_A)).max(1e-7);
    let u2 = unit_float(cell_hash(cell, seed, SALT_COLOR_B));
    let normal_draw = (-2.0 * u1.ln()).sqrt() * (TAU * u2).cos();
    let (low, high) = (structure.lambda_low_nm, structure.lambda_high_nm);
    let lambda_b0_nm = (0.5 * (low + high) + 0.25 * (high - low) * normal_draw).clamp(low, high);

    let normal = if structure.normal == Vec3::ZERO {
        // Opal: a uniformly random lattice orientation.
        let z = 1.0 - 2.0 * unit_float(cell_hash(cell, seed, SALT_NORMAL_A));
        let phi = TAU * unit_float(cell_hash(cell, seed, SALT_NORMAL_B));
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * phi.cos(), r * phi.sin(), z)
    } else {
        structure.normal
    };
    Domain {
        id,
        cell,
        active,
        lambda_b0_nm,
        normal,
    }
}
