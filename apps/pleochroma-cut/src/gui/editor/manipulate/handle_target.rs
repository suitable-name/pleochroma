//! The handle target of the provisional slice tier at exact tangency, where it has no facet
//! centroid yet.

use super::{CAMERA_FOV_DEG, HANDLE_LENGTH_FRACTION, Target, tangent_contact_anchor, tier_label};
use crate::gui::solid_preview::{facet_map::FacetMap, preview_state::FrameGeometry};
use glam::Vec3;
use pleochroma::optics::raytracer::Camera;
use pleochroma_cut_core::ConstraintTier;
use pleochroma_editor::manipulate::{FacetFrame, ScreenSize, handle_layout};

/// The handle target of the provisional tier `tier` (data `tier_data`, a scale-reference tier
/// of solved mast `mast`): its handles sit on the contact of the tangent plane with the stone.
/// The remembered facet is tried first, then the tier's facets in map order. `None` when no
/// candidate faces the camera and lands on screen.
pub(super) fn provisional_target(
    geometry: &FrameGeometry,
    map: &FacetMap,
    design: &pleochroma_cut_core::Design,
    tier: usize,
    tier_data: &ConstraintTier,
    mast: f64,
    remembered: Option<u32>,
) -> Option<Target> {
    let pose = geometry.camera;
    let camera =
        Camera::new(pose.yaw, pose.pitch, pose.distance, CAMERA_FOV_DEG).with_roll(pose.roll);
    let size = ScreenSize::new(geometry.size.0 as f32, geometry.size.1 as f32);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "handle length is a small world-space distance held as f32 by the camera math"
    )]
    let length = (HANDLE_LENGTH_FRACTION * geometry.bounding_radius) as f32;
    let mut candidate_ids = map.facets_of_tier(tier).to_vec();
    if let Some(id) = remembered.filter(|id| candidate_ids.contains(id)) {
        candidate_ids.retain(|candidate| *candidate != id);
        candidate_ids.insert(0, id);
    }
    for candidate in candidate_ids {
        let index_on_gear = f64::from(map.index_on_gear(candidate as usize));
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the reference angle is stored as f32 in FacetFrame, matching the facet map"
        )]
        let direction = FacetFrame::from_tier_with_reference(
            tier_data,
            index_on_gear,
            design.meta.gear_reference_angle as f32,
            design.meta.gear_teeth_abs(),
            Vec3::ZERO,
        );
        let Some(anchor) = tangent_contact_anchor(direction.normal, mast, &geometry.corner_points)
        else {
            continue;
        };
        // A back-facing candidate would put the controls on the hidden side of the
        // stone. Facet ids are walked in stable map order after the remembered id.
        if direction.normal.dot(camera.origin - anchor) <= 0.0 {
            continue;
        }
        let mut frame = direction;
        frame.centroid = anchor;
        let Some(layout) = handle_layout(&frame, &camera, size, length) else {
            continue;
        };
        if layout.anchor.x < 0.0
            || layout.anchor.y < 0.0
            || layout.anchor.x >= size.width
            || layout.anchor.y >= size.height
        {
            continue;
        }
        return Some(Target {
            tier,
            frame,
            layout,
            label: tier_label(tier_data, tier),
            provisional: true,
            diagram: None,
        });
    }
    None
}
