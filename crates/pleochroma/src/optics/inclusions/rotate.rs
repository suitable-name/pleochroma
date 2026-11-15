//! Turning a resolved family set with the stone.
//!
//! A Stone Pose rotates the whole stone about the model origin. The silk, needles and platelets
//! are fixed in the stone, so every direction the set holds turns with the same rotation.

use super::{CrystalFrame, InclusionSet};
use glam::Mat3;

impl InclusionSet {
    /// The set turned by the rotation `r`: every family direction, the remembered crystal frame
    /// and the Bragg structure (its lamella normal and its lattice, see
    /// `BraggStructure::rotated`) are the stone's own and turn with it. A family without a
    /// direction (random needles) has nothing to turn.
    #[must_use]
    pub fn rotated(&self, r: Mat3) -> Self {
        let mut out = *self;
        for family in &mut out.families[..usize::from(self.len)] {
            family.dir = r * family.dir;
        }
        out.structure = self.structure.map(|s| s.rotated(r));
        out.frame = self.frame.map(|f| CrystalFrame {
            c: r * f.c,
            a1: r * f.a1,
            a2: r * f.a2,
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::{CrystalDir, Family, FamilyKind},
        *,
    };
    use glam::Vec3;

    fn families() -> [Family; 2] {
        [
            Family {
                kind: FamilyKind::Needle,
                dir: CrystalDir::AlongC,
                sigma: 0.2,
                p: 2,
                lobe: 0.05,
                rho: 0.0,
            },
            Family {
                kind: FamilyKind::RandomNeedle,
                dir: CrystalDir::AlongC,
                sigma: 0.1,
                p: 2,
                lobe: 0.05,
                rho: 0.0,
            },
        ]
    }

    /// Family directions and the remembered frame turn with the rotation; a random-needle family
    /// keeps its zero direction.
    #[test]
    fn family_directions_and_the_frame_turn_with_the_rotation() {
        let frame = CrystalFrame::new(Vec3::Y, Some(Vec3::X));
        let set = frame.resolve(&families());
        let r = Mat3::from_rotation_y(0.7) * Mat3::from_rotation_x(1.1);
        let turned = set.rotated(r);
        assert!((turned.families()[0].dir - r * Vec3::Y).length() < 1e-6);
        assert_eq!(turned.families()[1].dir, Vec3::ZERO);
        let f = turned.frame.expect("the frame is kept");
        assert!((f.c - r * Vec3::Y).length() < 1e-6);
        assert!((f.a1 - r * Vec3::X).length() < 1e-6);
        assert!((f.a2 - r * frame.a2).length() < 1e-6);
    }

    /// Rotating by the identity changes nothing.
    #[test]
    fn the_identity_rotation_keeps_the_set() {
        let set = CrystalFrame::new(Vec3::Y, None).resolve(&families());
        assert_eq!(set.rotated(Mat3::IDENTITY), set);
    }
}
