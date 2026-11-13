//! The shadow's receiving plane and the Tray Reflection's plane are one number, bit for bit.
use super::geometry::StoneFrame;
use crate::{
    geometry::{
        cuts::StandardGemCuts,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::raytracer::environment::tray_plane_y,
};
use glam::Vec3;

#[test]
fn a_plane_only_stone_has_the_shadow_plane() {
    for planes in [
        StandardGemCuts::standard_round_brilliant(),
        StandardGemCuts::emerald_cut(),
    ] {
        let geom = StoneGeometry::planes_only(&planes);
        let frame = StoneFrame::new(geom).expect("valid stone");
        let plane = tray_plane_y(geom).expect("valid stone");
        assert_eq!(plane.to_bits(), frame.plane_y.to_bits());
        assert!(plane.is_finite());
    }
}

#[test]
fn a_stone_with_a_tool_has_the_shadow_plane() {
    let planes = StandardGemCuts::standard_round_brilliant();
    // A ball on the culet cuts the tip away, so the true solid's lowest vertex rises.
    let plane_only_y = tray_plane_y(StoneGeometry::planes_only(&planes)).expect("valid stone");
    let tools = [ToolPrimitive::ball(Vec3::new(0.0, plane_only_y, 0.0), 0.1)];
    let geom = StoneGeometry {
        planes: &planes,
        tools: &tools,
    };
    let frame = StoneFrame::new(geom).expect("valid stone with a small tool");
    let plane = tray_plane_y(geom).expect("valid stone with a small tool");
    assert_eq!(plane.to_bits(), frame.plane_y.to_bits());
    assert!(
        plane > plane_only_y,
        "the tool lifts the plane: {plane} vs {plane_only_y}"
    );
}

#[test]
fn a_degenerate_stone_has_no_plane() {
    assert!(tray_plane_y(StoneGeometry::planes_only(&[])).is_none());
}
