//! "Follows Crystal Axis": the zone frame a pattern takes from the stone's Crystal Axis.
//!
//! A pattern written in its own frame has its axis along `+Y`. A pattern whose axis is
//! [`PatternAxis::FollowsCrystalAxis`](super::PatternAxis::FollowsCrystalAxis) is stored with the
//! table-normal frame; the application turns it with [`follow_c_axis`] wherever it knows the
//! stone's axis, so a Two-Color zone's Along and Across colors and its geometry stay on the
//! crystal when the axis moves.
//!
//! The frame takes `+Y` to the axis line (the canonical, upper-hemisphere direction of
//! `CrystalAxis`) and keeps the pattern's `+X` in the girdle plane
//! ([`ZoneFrame::along_axis`], the same frame the render gives a following material), so a
//! boundary in the pattern's `x = 0` plane runs along the axis line face up. A stone with the axis
//! through the table keeps the identity frame (the stored default, bit for bit).

use glam::Vec3;
use pleochroma::optics::{
    materials::GemMaterial,
    zoning::{ZoneFrame, ZoneShape, ZonedAbsorption},
};

use super::{ColorPattern, TemplateId};

/// The zone frame that carries a pattern's `+Y` axis onto the Crystal Axis `c_axis`
/// ([`ZoneFrame::along_axis`]).
///
/// The axis is a line: its sign does not matter, and a vector that is not a usable direction
/// gives the table normal. The identity, exactly, for an axis through the table.
#[must_use]
pub fn crystal_axis_frame(c_axis: Vec3) -> ZoneFrame {
    ZoneFrame::along_axis(c_axis.as_dvec3())
}

/// Whether `pattern` follows the Crystal Axis and has a boundary across it: a layer or a half space
/// whose normal is the pattern axis (a tourmaline's Bicolor or Tricolor, a topaz's Bicolor).
///
/// Such boundaries are perpendicular to the c axis. While the axis runs through the table they lie
/// parallel to it, one color in the crown and another in the pavilion, and face up the eye looks
/// through all of them at once; the application then suggests laying the axis in the table plane.
#[must_use]
pub fn layers_across_axis(pattern: &ColorPattern) -> bool {
    pattern.axis.follows_crystal_axis()
        && pattern.zones.iter().any(|zone| match zone.shape {
            ZoneShape::Slab { normal, .. } | ZoneShape::HalfSpace { normal, .. } => {
                normal.length() > 0.0 && normal.normalize().y.abs() > 0.99
            }
            _ => false,
        })
}

/// Whether `pattern` shows face up only with the Crystal Axis through the table.
///
/// These are the rings around the axis of a tourmaline's Watermelon and Reverse Watermelon. With
/// the axis in the table plane (where a tourmaline's own axis lies) the eye looks across the
/// rings, through rim, core and rim, and pink and green blend to orange; watermelons are cut as
/// slices across c, so the application turns the axis through the table when it applies one of
/// these patterns.
#[must_use]
pub const fn wants_axis_through_table(pattern: &ColorPattern) -> bool {
    pattern.axis.follows_crystal_axis()
        && matches!(
            pattern.template,
            Some(TemplateId::Watermelon | TemplateId::ReverseWatermelon)
        )
}

/// Points the zones of `zoning` along the Crystal Axis `c_axis` ([`crystal_axis_frame`]).
/// Returns whether the frame changed (`false`: it already was that frame, so a caller can skip
/// restarting a render).
pub fn follow_c_axis(zoning: &mut ZonedAbsorption, c_axis: Vec3) -> bool {
    let frame = crystal_axis_frame(c_axis);
    if zoning.frame == frame {
        return false;
    }
    zoning.frame = frame;
    true
}

/// Turns the zones of `material` to its own Crystal Axis when its pattern follows it.
///
/// The mode travels on the zones themselves (`ZonedAbsorption::follows_axis`, set by
/// `ColorPattern::to_zoned`), so every surface that resolves a material (the desktop, the CLI,
/// the web app, the phone) turns it the same way, and a design file's custom-material snapshot
/// carries it. Returns whether the zones were turned (`false`: no zones, a pattern that does not
/// follow, or zones already on the axis). A material that does not follow is never touched.
pub fn follow_noted_pattern(material: &mut GemMaterial) -> bool {
    let c_axis = material.c_axis;
    material
        .zoning
        .as_mut()
        .is_some_and(|zoning| zoning.follows_axis && follow_c_axis(zoning, c_axis))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        crystal_axis::CrystalAxis,
        material::pattern::{
            ColorPattern, PatternAxis, TemplateId, decode_pattern, encode_pattern, template,
        },
    };
    use glam::DVec3;

    fn close(a: DVec3, b: DVec3) -> bool {
        (a - b).length() < 1e-9
    }

    #[test]
    fn an_axis_through_the_table_is_the_identity_frame() {
        assert_eq!(crystal_axis_frame(Vec3::Y), ZoneFrame::IDENTITY);
        assert_eq!(crystal_axis_frame(-Vec3::Y), ZoneFrame::IDENTITY);
        assert_eq!(crystal_axis_frame(Vec3::ZERO), ZoneFrame::IDENTITY);
    }

    #[test]
    fn the_frame_carries_the_pattern_axis_onto_the_crystal_axis() {
        for direction in [
            Vec3::X,
            Vec3::Z,
            Vec3::new(0.3, 0.8, -0.5),
            Vec3::new(1.0, 1.0, 0.0),
        ] {
            let frame = crystal_axis_frame(direction);
            let want = CrystalAxis::from_direction(direction.as_dvec3()).direction();
            assert!(
                close(frame.direction(DVec3::Y), want),
                "{direction:?}: {:?} vs {want:?}",
                frame.direction(DVec3::Y)
            );
            // Either sign of the line is the same frame.
            assert!(close(
                frame.direction(DVec3::Y),
                crystal_axis_frame(-direction).direction(DVec3::Y)
            ));
        }
    }

    #[test]
    fn the_x_equals_zero_boundary_runs_along_the_axis_and_through_the_table() {
        // The pattern's `x = 0` plane must contain both the axis and the table normal, so face
        // up the split lies on the axis line, whichever way the axis points.
        for direction in [
            Vec3::X,
            Vec3::Z,
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.3, 0.8, -0.5),
        ] {
            let frame = crystal_axis_frame(direction);
            let normal = frame.direction(DVec3::X);
            assert!(normal.y.abs() < 1e-12, "{direction:?}: {normal:?}");
            assert!(
                normal.dot(direction.as_dvec3().normalize()).abs() < 1e-9,
                "{direction:?}"
            );
            // A proper rotation, the same for either sign of the line.
            assert!((frame.rotation.length() - 1.0).abs() < 1e-12);
            assert_eq!(frame, crystal_axis_frame(-direction), "{direction:?}");
        }
    }

    #[test]
    fn layers_across_the_axis_are_found_only_on_a_following_pattern() {
        for id in [
            TemplateId::Bicolor,
            TemplateId::Tricolor,
            TemplateId::Banded,
        ] {
            assert!(
                layers_across_axis(&template(id, PatternAxis::FollowsCrystalAxis)),
                "{id:?}"
            );
            assert!(
                !layers_across_axis(&template(id, PatternAxis::ACROSS)),
                "{id:?}: a fixed pattern stands where it was put"
            );
        }
        for id in [
            TemplateId::Split,
            TemplateId::Watermelon,
            TemplateId::Sectors,
            TemplateId::ColorBand,
            TemplateId::Uniform,
        ] {
            assert!(
                !layers_across_axis(&template(id, PatternAxis::FollowsCrystalAxis)),
                "{id:?}: its boundaries contain the axis"
            );
        }
    }

    #[test]
    fn only_the_watermelons_ask_for_the_axis_through_the_table() {
        for id in [TemplateId::Watermelon, TemplateId::ReverseWatermelon] {
            assert!(
                wants_axis_through_table(&template(id, PatternAxis::FollowsCrystalAxis)),
                "{id:?}"
            );
            assert!(
                !wants_axis_through_table(&template(id, PatternAxis::ACROSS)),
                "{id:?}: a fixed pattern stands where it was put"
            );
        }
        for id in [
            TemplateId::Bicolor,
            TemplateId::Tricolor,
            TemplateId::HexagonalBanding,
            TemplateId::CoreAndRim,
            TemplateId::Sectors,
        ] {
            assert!(
                !wants_axis_through_table(&template(id, PatternAxis::FollowsCrystalAxis)),
                "{id:?}"
            );
        }
    }

    #[test]
    fn re_framing_reports_whether_anything_changed() {
        let mut pattern = template(TemplateId::Bicolor, PatternAxis::FollowsCrystalAxis);
        for at in pattern.unsolved_colors() {
            pattern.set_color_bands(at, vec![[520.0, 40.0, 0.3]]);
        }
        let mut zoned = pattern
            .to_zoned()
            .expect("a solved pattern converts")
            .expect("a bicolor has zones");
        assert_eq!(zoned.frame, ZoneFrame::IDENTITY, "the stored default");
        assert!(
            !follow_c_axis(&mut zoned, Vec3::Y),
            "already through the table"
        );
        assert!(follow_c_axis(&mut zoned, Vec3::X));
        assert!(close(zoned.frame.direction(DVec3::Y), DVec3::X));
        assert!(!follow_c_axis(&mut zoned, Vec3::X), "the same axis again");
        assert!(
            follow_c_axis(&mut zoned, Vec3::Y),
            "back to the table normal"
        );
        assert_eq!(zoned.frame, ZoneFrame::IDENTITY);
    }

    fn zoned_material(name: &str, c_axis: Vec3, axis: PatternAxis) -> GemMaterial {
        let mut pattern = template(TemplateId::Bicolor, axis);
        for at in pattern.unsolved_colors() {
            pattern.set_color_bands(at, vec![[520.0, 40.0, 0.3]]);
        }
        let mut material = GemMaterial::diamond();
        material.name = name.to_owned();
        material.c_axis = c_axis;
        material.zoning = pattern.to_zoned().expect("converts");
        material
    }

    #[test]
    fn only_a_following_pattern_is_turned_to_the_materials_axis() {
        // A fixed pattern is never touched, whatever the material's name.
        let mut plain = zoned_material("Fan", Vec3::X, PatternAxis::ACROSS);
        let before = plain.clone();
        assert!(!plain.zoning.as_ref().expect("zoned").follows_axis);
        assert!(!follow_noted_pattern(&mut plain), "fixed: untouched");
        assert_eq!(plain, before);

        // The same name with a following pattern turns: the flag, not the name, decides.
        let mut fan = zoned_material("Fan", Vec3::X, PatternAxis::FollowsCrystalAxis);
        assert!(fan.zoning.as_ref().expect("zoned").follows_axis);
        assert!(follow_noted_pattern(&mut fan));
        let frame = fan.zoning.as_ref().expect("zoned").frame;
        assert!(close(frame.direction(DVec3::Y), DVec3::X));
        assert!(!follow_noted_pattern(&mut fan), "already on the axis");

        // Through the table: nothing to turn.
        let mut upright = zoned_material("Fan", Vec3::Y, PatternAxis::FollowsCrystalAxis);
        assert!(!follow_noted_pattern(&mut upright));
    }

    #[test]
    fn a_material_without_zones_is_left_alone() {
        let mut bare = GemMaterial::diamond();
        bare.c_axis = Vec3::X;
        assert!(!follow_noted_pattern(&mut bare));
    }

    #[test]
    fn the_flag_is_omitted_from_a_file_unless_set() {
        let fixed = zoned_material("Fixed", Vec3::Y, PatternAxis::ACROSS)
            .zoning
            .expect("zoned");
        let text = serde_json::to_string(&fixed).expect("serialises");
        assert!(
            !text.contains("follows_axis"),
            "an old file is unchanged: {text}"
        );
        let back: ZonedAbsorption = serde_json::from_str(&text).expect("reads");
        assert_eq!(back, fixed);

        let follows = zoned_material("Follows", Vec3::Y, PatternAxis::FollowsCrystalAxis)
            .zoning
            .expect("zoned");
        let text = serde_json::to_string(&follows).expect("serialises");
        assert!(text.contains("\"follows_axis\":true"), "{text}");
        let back: ZonedAbsorption = serde_json::from_str(&text).expect("reads");
        assert_eq!(back, follows);
    }

    #[test]
    fn the_mode_round_trips_and_an_old_axis_is_unchanged() {
        let follows = template(TemplateId::Watermelon, PatternAxis::FollowsCrystalAxis);
        assert!(follows.axis.follows_crystal_axis());
        let text = encode_pattern(&follows).expect("encodes");
        let back: ColorPattern = decode_pattern(&text).expect("decodes");
        assert_eq!(back, follows);

        let fixed = template(TemplateId::Watermelon, PatternAxis::ACROSS);
        assert!(!fixed.axis.follows_crystal_axis());
        let text = encode_pattern(&fixed).expect("encodes");
        assert!(
            !text.contains("FollowsCrystalAxis"),
            "an old pattern never names the new mode"
        );
        assert_eq!(decode_pattern(&text).expect("decodes"), fixed);
    }
}
