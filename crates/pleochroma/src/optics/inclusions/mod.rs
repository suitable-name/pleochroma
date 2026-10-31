//! Oriented inclusions: silk, needles and platelets tied to the crystal frame.
//!
//! Silk, needles and adularescent platelets are oriented scatterers inside a homogeneous host.
//! A [`Family`] describes one set of them in the CRYSTAL frame (a needle axis or a platelet
//! normal as a [`CrystalDir`], a strength, a spectral exponent and a lobe width). The frame is
//! the stone's own: [`CrystalFrame`] turns the c axis, and the reference a axis a uniaxial
//! stone keeps as its stored Second Axis turn (the "Silk Turn", which the optics never read for
//! a uniaxial material), into world directions, so the silk follows the Crystal Axis in every
//! consumer. [`InclusionSet`] is the resolved result the tracer reads
//! (`GemMaterial::inclusions`); a material without one renders bit for bit as before.
//!
//! * [`catalogue`]: the built-in presets, keyed by material name.
//! * [`spec`]: the per-design choice (preset, density, turn, sheen, color) and its resolution.
//! * [`needle`], [`platelet`]: the two phase functions (sampling and evaluation agree exactly).
//! * [`spectral`]: the `(550 nm / lambda)^p` factor.
//! * [`bragg`], [`domains`]: the domain structure of precious opal and labradorite (structural
//!   color by Bragg reflection), carried in the same [`InclusionSet`] and tracked by
//!   `optics::raytracer::scattering::delta_tracking`.
//!
//! The estimator that uses the set lives in `optics::raytracer::scattering::oriented`.

pub mod bragg;
#[cfg(test)]
mod bragg_tests;
pub mod catalogue;
pub mod domains;
pub mod needle;
pub mod platelet;
mod rebase;
pub mod spec;
pub mod spectral;
#[cfg(test)]
mod tests;

use crate::optics::materials::GemMaterial;
use bragg::BraggStructure;
use glam::Vec3;

/// Most families a stone carries.
pub const MAX_FAMILIES: usize = 4;

/// The smallest needle lobe a sanitised family keeps (see `ResolvedFamily::sanitised`).
pub const NEEDLE_LOBE_FLOOR: f32 = 1e-3;

/// The shape of a family of scatterers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum FamilyKind {
    /// Needles of one axis: scatter on the cone about the axis, extinction `~ sin(theta)`.
    Needle,
    /// Platelets of one normal: mirror-like reflection, extinction `~ |cos(theta)|`.
    Platelet,
    /// Needles of every orientation (rutilated quartz): extinction independent of direction.
    RandomNeedle,
}

/// A direction in the crystal frame.
///
/// The frame is `(a1, a2, c)`: `c` the principal axis, `a1` the reference a axis (for a biaxial
/// material the `n_alpha` direction), `a2 = c x a1`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum CrystalDir {
    /// Along the c axis.
    AlongC,
    /// In the basal plane, `turn_deg` right-handed about c from the reference a axis.
    Basal {
        /// Turn from the reference a axis, degrees.
        turn_deg: f32,
    },
    /// Along the `n_alpha` direction (the reference axis itself). For chrysoberyl and
    /// alexandrite the crystallographic c axis is `n_alpha` in the app's frame.
    AlphaAxis,
    /// A general direction: `tilt_deg` from c, then `turn_deg` about it from the reference axis.
    Tilted {
        /// Angle from the c axis, degrees.
        tilt_deg: f32,
        /// Turn about c from the reference axis, degrees.
        turn_deg: f32,
    },
}

/// One family of oriented scatterers, in the crystal frame.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Family {
    /// The shape.
    pub kind: FamilyKind,
    /// The needle axis or platelet normal; unused for [`FamilyKind::RandomNeedle`].
    pub dir: CrystalDir,
    /// Extinction per model unit at 550 nm, at maximum projected area.
    pub sigma: f32,
    /// Spectral exponent: one of `0, 1, 2, 4` (Grey, Slightly Blue, Blue, Rayleigh).
    pub p: u8,
    /// Needle: the logistic scale of the cone's blur. Platelet: the GGX width.
    pub lobe: f32,
    /// Needle azimuthal forwardness (wrapped Cauchy), `0` uniform; unused for platelets.
    pub rho: f32,
}

/// One family resolved into world directions.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ResolvedFamily {
    /// The shape.
    pub kind: FamilyKind,
    /// Unit needle axis or platelet normal in the stone frame (zero for random needles).
    pub dir: Vec3,
    /// Extinction per model unit at 550 nm, at maximum projected area.
    pub sigma: f32,
    /// Spectral exponent, one of `0, 1, 2, 4`.
    pub p: u8,
    /// Needle cone blur or platelet GGX width.
    pub lobe: f32,
    /// Needle azimuthal forwardness.
    pub rho: f32,
}

impl ResolvedFamily {
    const EMPTY: Self = Self {
        kind: FamilyKind::Needle,
        dir: Vec3::ZERO,
        sigma: 0.0,
        p: 0,
        lobe: 0.0,
        rho: 0.0,
    };

    /// The family with every value limited to the range the estimator is valid for, or `None`
    /// when it can scatter nothing (no strength, or no usable direction).
    fn sanitised(self) -> Option<Self> {
        if !self.sigma.is_finite() || self.sigma <= 0.0 {
            return None;
        }
        let dir = if self.kind == FamilyKind::RandomNeedle {
            Vec3::ZERO
        } else {
            let d = self.dir.normalize_or_zero();
            if d == Vec3::ZERO {
                return None;
            }
            d
        };
        let lobe = if self.lobe.is_finite() {
            self.lobe
        } else {
            0.0
        };
        // A needle lobe of 0 is the ideal cone, whose density is a delta: the phase carried as the
        // MIS partner would be 0 and the escape weight would drop the cone's energy under an HDR
        // map. A floor of 1e-3 keeps a finite density (every built-in is >= 0.015, even Sharp).
        let lobe = match self.kind {
            FamilyKind::Platelet => lobe.clamp(0.02, 0.6),
            _ => lobe.clamp(NEEDLE_LOBE_FLOOR, 1.0),
        };
        let rho = if self.rho.is_finite() { self.rho } else { 0.0 };
        Some(Self {
            dir,
            lobe,
            rho: rho.clamp(0.0, needle::MAX_RHO),
            p: spectral::snap_exponent(self.p),
            ..self
        })
    }
}

/// The resolved families of a stone, in the stone frame (at most [`MAX_FAMILIES`]).
///
/// `Copy` and fixed size so it travels with the material and mirrors a fixed GPU block.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InclusionSet {
    families: [ResolvedFamily; MAX_FAMILIES],
    len: u8,
    structure: Option<BraggStructure>,
    /// The crystal frame `families` were resolved in ([`CrystalFrame::resolve`]), so a design's own
    /// Crystal Axis can turn them ([`Self::rebased`]); `None` for a set built from world vectors.
    /// Neither serialised nor part of the comparison: it is bookkeeping about how the set was made.
    #[cfg_attr(feature = "serde", serde(skip))]
    frame: Option<CrystalFrame>,
}

impl PartialEq for InclusionSet {
    fn eq(&self, other: &Self) -> bool {
        self.families == other.families
            && self.len == other.len
            && self.structure == other.structure
    }
}

impl InclusionSet {
    /// A set with no families.
    pub const EMPTY: Self = Self {
        families: [ResolvedFamily::EMPTY; MAX_FAMILIES],
        len: 0,
        structure: None,
        frame: None,
    };

    /// The set of `families`, each sanitised; families that scatter nothing are dropped and
    /// everything past [`MAX_FAMILIES`] is ignored.
    #[must_use]
    pub fn new(families: &[ResolvedFamily]) -> Self {
        let mut set = Self::EMPTY;
        for family in families.iter().filter_map(|f| f.sanitised()) {
            if usize::from(set.len) == MAX_FAMILIES {
                break;
            }
            set.families[usize::from(set.len)] = family;
            set.len += 1;
        }
        set
    }

    /// The families in order.
    #[must_use]
    pub fn families(&self) -> &[ResolvedFamily] {
        &self.families[..usize::from(self.len)]
    }

    /// The set with the Bragg domain structure `structure` (opal, labradorite) added; the
    /// families stay.
    #[must_use]
    pub const fn with_structure(mut self, structure: BraggStructure) -> Self {
        self.structure = Some(structure);
        self
    }

    /// The Bragg domain structure, when the stone has one.
    #[must_use]
    pub const fn structure(&self) -> Option<&BraggStructure> {
        self.structure.as_ref()
    }

    /// Whether any family or a domain structure scatters.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.len > 0 || self.structure.is_some()
    }
}

/// The stone's crystal frame in world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrystalFrame {
    /// The principal axis (c, or `n_gamma` of a biaxial stone).
    pub c: Vec3,
    /// The reference a axis, perpendicular to `c` (`n_alpha` of a biaxial stone).
    pub a1: Vec3,
    /// `c x a1`.
    pub a2: Vec3,
}

impl CrystalFrame {
    /// The frame of a stone with principal axis `c_axis` and, when known, a second direction
    /// `second` (projected perpendicular to `c_axis`). Without one, the same arbitrary
    /// completion `alpha_axis_from_turn(c, 0)` every biaxial construction uses.
    #[must_use]
    pub fn new(c_axis: Vec3, second: Option<Vec3>) -> Self {
        let c = c_axis.normalize_or_zero();
        let projected = second
            .map(|s| s - c * s.dot(c))
            .filter(|p| p.length_squared() > 1e-10)
            .map(Vec3::normalize);
        let a1 =
            projected.unwrap_or_else(|| crate::optics::birefringence::alpha_axis_from_turn(c, 0.0));
        Self {
            c,
            a1,
            a2: c.cross(a1),
        }
    }

    /// The world direction of `dir`.
    #[must_use]
    pub fn direction(&self, dir: CrystalDir) -> Vec3 {
        let in_plane = |turn_deg: f32| {
            let (sin, cos) = turn_deg.to_radians().sin_cos();
            self.a1 * cos + self.a2 * sin
        };
        match dir {
            CrystalDir::AlongC => self.c,
            CrystalDir::AlphaAxis => self.a1,
            CrystalDir::Basal { turn_deg } => in_plane(turn_deg),
            CrystalDir::Tilted { tilt_deg, turn_deg } => {
                let (sin, cos) = tilt_deg.to_radians().sin_cos();
                self.c * cos + in_plane(turn_deg) * sin
            }
        }
    }

    /// `families` resolved into the world frame.
    #[must_use]
    pub fn resolve(&self, families: &[Family]) -> InclusionSet {
        let resolved: Vec<ResolvedFamily> = families
            .iter()
            .map(|f| ResolvedFamily {
                kind: f.kind,
                dir: self.direction(f.dir),
                sigma: f.sigma,
                p: f.p,
                lobe: f.lobe,
                rho: f.rho,
            })
            .collect();
        let mut set = InclusionSet::new(&resolved);
        set.frame = Some(*self);
        set
    }
}

/// A stable orthonormal pair perpendicular to the unit vector `n`, the same branch-minimal
/// construction `scattering::frosted_orthonormal_basis` uses.
pub(crate) fn perpendicular_basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let t = (a - n * n.dot(a)).normalize_or_zero();
    let b = n.cross(t);
    (t, b)
}

impl GemMaterial {
    /// The oriented families the tracer scatters off, or `None`.
    ///
    /// `None` when the material carries no active set. A zoned material has them too: the
    /// zoned oriented estimator keeps the zones' absorption per segment.
    #[must_use]
    pub fn oriented_inclusions(&self) -> Option<&InclusionSet> {
        self.inclusions.as_ref().filter(|set| set.is_active())
    }

    /// Whether the material is a scattering medium for the tracer: the Henyey-Greenstein haze
    /// is on, or oriented inclusions are. The single gate every scattering-aware site reads
    /// (instead of `scattering_sigma_s > 0.0` alone).
    #[must_use]
    pub fn scatters(&self) -> bool {
        self.scattering_sigma_s > 0.0 || self.oriented_inclusions().is_some()
    }

    /// The material with oriented inclusions `set` (a set with no active family clears them).
    #[must_use]
    pub fn with_inclusions(mut self, set: InclusionSet) -> Self {
        self.inclusions = set.is_active().then_some(set);
        self
    }
}
