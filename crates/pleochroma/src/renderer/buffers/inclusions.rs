//! The oriented-inclusion block of [`GpuGemMaterial`](super::GpuGemMaterial).
//!
//! [`GpuInclusionBlock`] is the GPU twin of `GemMaterial::inclusions` (the resolved
//! [`InclusionSet`]): up to four families of oriented scatterers, the plain haze that scatters
//! alongside them, and room for two later features that only append data to the same block, so
//! no offset moves again: the domain structure of opal and labradorite (`structure`, with its
//! seed and flag in `header`) and the abraded-edge band (`edge`).
//!
//! # Layout (224 bytes, appended at offset 2800 of the material)
//!
//! | Field | Offset | Content |
//! |---|---|---|
//! | `header` | 0 | `[family_count, structure_flag, domain_seed, flags]` |
//! | `fam_dir` | 16 | per family: world needle axis or platelet normal in `.xyz`, the kind in `.w` |
//! | `fam_params` | 80 | per family: `[sigma, p, lobe, rho]` |
//! | `haze` | 144 | `[sigma_HG, g, 0, 0]`: the Henyey-Greenstein haze, set only with inclusions |
//! | `structure` | 160 | three rows for the domain structure; zero when off |
//! | `edge` | 208 | `[width, alpha, 0, 0]`: the abraded-edge band; zero when off |
//!
//! Every field is a `u32`/`f32` quad, so the Rust offsets are the WGSL ones (the struct's
//! alignment is 16 and the whole block is a multiple of it); the asserts below pin them.
//! A material without inclusions encodes to all zeros.

use core::mem::offset_of;

use crate::optics::{
    inclusions::{FamilyKind, MAX_FAMILIES, bragg::BraggStructure},
    materials::GemMaterial,
};

/// The `kind` stored in `fam_dir[i].w`, as the float the shader switches on.
pub mod family_kind {
    /// Needles of one axis.
    pub const NEEDLE: f32 = 0.0;
    /// Platelets of one normal.
    pub const PLATELET: f32 = 1.0;
    /// Needles of every orientation.
    pub const RANDOM_NEEDLE: f32 = 2.0;
}

/// Rows of [`GpuInclusionBlock::structure`].
pub const STRUCTURE_ROWS: usize = 3;

/// The three `structure` rows of a [`BraggStructure`], the data `bragg_structure` in
/// `transport_physics/12_bragg_domains.wgsl` reads back:
///
/// | Row | Content |
/// |---|---|
/// | 0 | `[cell_size, lambda_low_nm, lambda_high_nm, purity]` |
/// | 1 | `[coverage, mosaic, kappa, majorant]` |
/// | 2 | `[normal.x, normal.y, normal.z, 0]` (zero: a random normal per domain) |
///
/// `kappa` and the majorant are computed here, by the CPU's own functions, so both sides use the
/// same bits. The domain seed travels in `header[2]`.
#[must_use]
pub fn structure_rows(structure: &BraggStructure) -> [[f32; 4]; STRUCTURE_ROWS] {
    [
        [
            structure.cell_size,
            structure.lambda_low_nm,
            structure.lambda_high_nm,
            structure.purity,
        ],
        [
            structure.coverage,
            structure.mosaic,
            structure.kappa(),
            structure.majorant(),
        ],
        [
            structure.normal.x,
            structure.normal.y,
            structure.normal.z,
            0.0,
        ],
    ]
}

/// The inclusion block, GPU-encoded. See the module documentation for the layout.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuInclusionBlock {
    /// `[family_count, structure_flag, domain_seed, flags]`. `flags` is reserved (zero).
    pub header: [u32; 4],
    /// World direction (`.xyz`, zero for random needles) and [`family_kind`] (`.w`).
    pub fam_dir: [[f32; 4]; MAX_FAMILIES],
    /// `[sigma, p, lobe, rho]`: the strength at 550 nm, the spectral exponent, the lobe width
    /// and the needle azimuthal forwardness.
    pub fam_params: [[f32; 4]; MAX_FAMILIES],
    /// `[sigma_HG, g, 0, 0]`.
    pub haze: [f32; 4],
    /// The domain structure of opal and labradorite; zero when off.
    pub structure: [[f32; 4]; STRUCTURE_ROWS],
    /// `[width, alpha, 0, 0]`, the abraded-edge band; zero when off.
    pub edge: [f32; 4],
}

const _: () = {
    assert!(offset_of!(GpuInclusionBlock, header) == 0);
    assert!(offset_of!(GpuInclusionBlock, fam_dir) == 16);
    assert!(offset_of!(GpuInclusionBlock, fam_params) == 80);
    assert!(offset_of!(GpuInclusionBlock, haze) == 144);
    assert!(offset_of!(GpuInclusionBlock, structure) == 160);
    assert!(offset_of!(GpuInclusionBlock, edge) == 208);
    assert!(size_of::<GpuInclusionBlock>() == 224);
    assert!(MAX_FAMILIES == 4);
};

impl GpuInclusionBlock {
    /// The block of `material`: its oriented families and the haze that goes with them, all
    /// zero when the material has no active inclusions (the same gate the CPU tracer reads,
    /// `GemMaterial::oriented_inclusions`).
    #[must_use]
    pub fn encode(material: &GemMaterial) -> Self {
        let mut block = Self::default();
        let Some(set) = material.oriented_inclusions() else {
            return block;
        };
        for (slot, family) in set.families().iter().take(MAX_FAMILIES).enumerate() {
            let kind = match family.kind {
                FamilyKind::Needle => family_kind::NEEDLE,
                FamilyKind::Platelet => family_kind::PLATELET,
                FamilyKind::RandomNeedle => family_kind::RANDOM_NEEDLE,
            };
            block.fam_dir[slot] = [family.dir.x, family.dir.y, family.dir.z, kind];
            block.fam_params[slot] = [family.sigma, f32::from(family.p), family.lobe, family.rho];
            block.header[0] += 1;
        }
        block.haze = [material.scattering_sigma_s, material.scattering_g, 0.0, 0.0];
        if let Some(structure) = set.structure() {
            block = block.with_structure(1, structure.seed, structure_rows(structure));
        }
        block
    }

    /// The block with the domain structure of an opal or labradorite: its flag, the seed of
    /// the domain map and the three parameter rows ([`structure_rows`] encodes them).
    #[must_use]
    pub const fn with_structure(
        mut self,
        flag: u32,
        domain_seed: u32,
        rows: [[f32; 4]; STRUCTURE_ROWS],
    ) -> Self {
        self.header[1] = flag;
        self.header[2] = domain_seed;
        self.structure = rows;
        self
    }

    /// The block with the abraded-edge band: its width in model units and its GGX roughness.
    #[must_use]
    pub const fn with_edge(mut self, width: f32, alpha: f32) -> Self {
        self.edge = [width, alpha, 0.0, 0.0];
        self
    }
}
