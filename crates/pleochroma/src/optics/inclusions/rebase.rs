//! Turning a resolved family set into another crystal frame.
//!
//! A custom material's families are resolved once, in the frame of its stored orientation. A design
//! that sets its own Crystal Axis (and Second Axis) turns the crystal, and the silk turns with it:
//! every family direction is a fixed combination of the frame's three axes (`c`, `a1`, `c x a1`),
//! so the same combination in the design's frame is exactly what resolving the unresolved families
//! there would give.

use super::{CrystalFrame, InclusionSet};
use glam::Vec3;

impl InclusionSet {
    /// The set turned into `to`: each family keeps its coordinates in the frame it was resolved in
    /// ([`CrystalFrame::resolve`]) and takes them to `to`'s axes. A set that remembers no frame
    /// (built from world vectors) is returned as it is, and so is one already in `to`.
    #[must_use]
    pub fn rebased(&self, to: &CrystalFrame) -> Self {
        let Some(from) = self.frame else {
            return *self;
        };
        if from == *to {
            return *self;
        }
        let turn = |d: Vec3| to.c * d.dot(from.c) + to.a1 * d.dot(from.a1) + to.a2 * d.dot(from.a2);
        let mut out = *self;
        out.frame = Some(*to);
        for family in &mut out.families[..usize::from(self.len)] {
            family.dir = turn(family.dir);
        }
        // A Bragg lamella normal (labradorite) is fixed in the crystal frame like a family
        // direction; an opal's random normals (zero) have nothing to turn.
        if let Some(structure) = &mut out.structure
            && structure.normal != Vec3::ZERO
        {
            structure.normal = turn(structure.normal);
        }
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
                kind: FamilyKind::Needle,
                dir: CrystalDir::Tilted {
                    tilt_deg: 60.0,
                    turn_deg: 30.0,
                },
                sigma: 0.1,
                p: 2,
                lobe: 0.05,
                rho: 0.0,
            },
        ]
    }

    /// Turning a resolved set is the same as resolving the families in the new frame.
    #[test]
    fn rebasing_equals_resolving_in_the_new_frame() {
        let from = CrystalFrame::new(Vec3::Y, Some(Vec3::X));
        let to = CrystalFrame::new(Vec3::new(1.0, 1.0, 0.0), Some(Vec3::Z));
        let rebased = from.resolve(&families()).rebased(&to);
        let direct = to.resolve(&families());
        for (a, b) in rebased.families().iter().zip(direct.families()) {
            assert!(
                (a.dir - b.dir).length() < 1e-5,
                "{:?} vs {:?}",
                a.dir,
                b.dir
            );
        }
        assert_eq!(rebased.families().len(), direct.families().len());
    }

    /// A lamella normal turns like a family direction; an opal's random (zero) normal stays zero.
    /// Fails without the structure turn: the normal was copied unchanged.
    #[test]
    fn rebasing_turns_a_lamella_normal_and_leaves_an_opal_alone() {
        use super::super::bragg::BraggStructure;
        let from = CrystalFrame::new(Vec3::Y, Some(Vec3::X));
        let to = CrystalFrame::new(Vec3::new(1.0, 1.0, 0.0), Some(Vec3::Z));
        let dir = CrystalDir::Tilted {
            tilt_deg: 79.0,
            turn_deg: 90.0,
        };
        let structure =
            |normal: Vec3| BraggStructure::new(7, 0.2, (450.0, 650.0), 0.05, 0.8, 0.5, 0.1, normal);
        let lamella = from
            .resolve(&families())
            .with_structure(structure(from.direction(dir)))
            .rebased(&to);
        let normal = lamella.structure().expect("structure").normal;
        let expected = to.direction(dir);
        assert!(
            (normal - expected).length() < 1e-5,
            "{normal:?} vs {expected:?}"
        );

        let opal = from
            .resolve(&families())
            .with_structure(structure(Vec3::ZERO))
            .rebased(&to);
        assert_eq!(opal.structure().expect("structure").normal, Vec3::ZERO);
    }

    /// A set without a remembered frame, or already in the frame, is untouched.
    #[test]
    fn a_frameless_or_aligned_set_is_unchanged() {
        let frame = CrystalFrame::new(Vec3::Y, None);
        let set = frame.resolve(&families());
        assert_eq!(set.rebased(&frame), set);
        let bare = InclusionSet::new(set.families());
        let to = CrystalFrame::new(Vec3::X, None);
        assert_eq!(bare.rebased(&to), bare);
    }
}
