//! The stone pose in the live loop: what the loop traces, shadows and mirrors when the Live
//! Render tab shows the stone posed on the tray.
//!
//! The pose reaches the loop already filtered by the tab (`RenderContext::effective_stone_pose`
//! is the identity on the Edit tab), so every helper here takes the pose as it is and returns its
//! inputs untouched for the identity. The optical metrics, the tilt paths and the overlay never
//! read these helpers: they keep the stone as designed.

use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::materials::GemMaterial,
    render_setup::{PosedStone, StonePose, pose_stone},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// How long the Stone Shadow stays paused after a pose drag or a pose slider is released, so the
/// map is built once for the pose the cutter settled on and not for every pose on the way.
pub(super) const SHADOW_SETTLE: Duration = Duration::from_millis(250);

/// Decides when the Stone Shadow may start a build around a pose drag: never while the drag is
/// held, and again only [`SHADOW_SETTLE`] after its release.
#[derive(Debug, Default)]
pub(super) struct PoseDragHold {
    /// The instant the pause ends, set while a drag is held and after its release.
    until: Option<Instant>,
}

impl PoseDragHold {
    /// Whether a shadow build may start at `now`. `held` is `RenderContext::pose_drag_held`; the
    /// loop calls this once per iteration, so the release is seen on the next one.
    pub(super) fn builds_allowed(&mut self, held: bool, now: Instant) -> bool {
        if held {
            self.until = now.checked_add(SHADOW_SETTLE);
            return false;
        }
        match self.until {
            Some(until) if now < until => false,
            _ => {
                self.until = None;
                true
            }
        }
    }
}

/// The planes, tools and traced material of a shadow request for the stone in `pose`. The shared
/// planes and tools are kept as they are (no copy) for the identity; otherwise they are the posed
/// stone's, and `material` is the posed material.
pub(super) fn posed_shadow_stone(
    pose: StonePose,
    planes: &Arc<Vec<GpuFacetPlane>>,
    tools: &Arc<Vec<ToolPrimitive>>,
    material: GemMaterial,
) -> (
    Arc<Vec<GpuFacetPlane>>,
    Arc<Vec<ToolPrimitive>>,
    GemMaterial,
) {
    if pose.is_identity() {
        return (Arc::clone(planes), Arc::clone(tools), material);
    }
    let posed = pose_stone(pose, planes, tools, &material);
    (
        Arc::new(posed.planes),
        Arc::new(posed.tools),
        posed.material,
    )
}

/// The planes and tools of the stone in `pose`, for the tray mirror, which reads only the stone's
/// lowest vertex. The shared planes and tools are kept as they are for the identity.
pub(super) fn posed_geometry(
    pose: StonePose,
    planes: &Arc<Vec<GpuFacetPlane>>,
    tools: &Arc<Vec<ToolPrimitive>>,
) -> (Arc<Vec<GpuFacetPlane>>, Arc<Vec<ToolPrimitive>>) {
    if pose.is_identity() {
        return (Arc::clone(planes), Arc::clone(tools));
    }
    // The material takes no part in the geometry; any material turns the same planes and tools.
    let posed = pose_stone(pose, planes, tools, &GemMaterial::diamond());
    (Arc::new(posed.planes), Arc::new(posed.tools))
}

/// The posed stone the traced frame uses, kept for the scene generation it was built for: the
/// generation covers the planes, tools, material, engine and pose, so the loop turns the stone
/// once per scene and not once per frame.
#[derive(Default)]
pub(super) struct PosedFrameCache {
    entry: Option<(u64, Arc<PosedStone>)>,
}

impl PosedFrameCache {
    /// The stone of `generation` in `pose`, or `None` for the identity (the loop then traces the
    /// planes, tools and material it holds). `traced` is the material the engine traces.
    pub(super) fn get(
        &mut self,
        generation: u64,
        pose: StonePose,
        (planes, tools): (&[GpuFacetPlane], &[ToolPrimitive]),
        traced: &GemMaterial,
    ) -> Option<Arc<PosedStone>> {
        if pose.is_identity() {
            self.entry = None;
            return None;
        }
        if let Some((held, posed)) = &self.entry
            && *held == generation
        {
            return Some(Arc::clone(posed));
        }
        let posed = Arc::new(pose_stone(pose, planes, tools, traced));
        self.entry = Some((generation, Arc::clone(&posed)));
        Some(posed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::context::live_render_pose;
    use glam::Vec3;
    use pleochroma::geometry::cuts::StandardGemCuts;

    fn tilted() -> StonePose {
        StonePose {
            tilt_deg: 90.0,
            turn_deg: 0.0,
            spin_deg: 0.0,
        }
    }

    fn stone() -> (Arc<Vec<GpuFacetPlane>>, Arc<Vec<ToolPrimitive>>) {
        (
            Arc::new(StandardGemCuts::standard_round_brilliant()),
            Arc::new(Vec::new()),
        )
    }

    /// The Live Render tab shows the pose (clamped), the Edit tab never does.
    #[test]
    fn the_pose_applies_only_while_the_live_render_tab_is_shown() {
        let pose = StonePose {
            tilt_deg: 40.0,
            turn_deg: 30.0,
            spin_deg: 5.0,
        };
        assert_eq!(live_render_pose(true, pose), pose);
        assert!(live_render_pose(false, pose).is_identity());
        assert_eq!(
            live_render_pose(
                true,
                StonePose {
                    tilt_deg: 400.0,
                    ..pose
                }
            )
            .tilt_deg,
            180.0,
            "the pose is clamped on the way in"
        );
    }

    #[test]
    fn the_identity_keeps_the_shared_stone_and_the_material() {
        let (planes, tools) = stone();
        let (p, t, m) =
            posed_shadow_stone(StonePose::IDENTITY, &planes, &tools, GemMaterial::diamond());
        assert!(Arc::ptr_eq(&p, &planes) && Arc::ptr_eq(&t, &tools));
        assert!(m == GemMaterial::diamond(), "the material is untouched");
        let (p, t) = posed_geometry(StonePose::IDENTITY, &planes, &tools);
        assert!(Arc::ptr_eq(&p, &planes) && Arc::ptr_eq(&t, &tools));
    }

    /// A tilt of 90 degrees stands the table normal (+Y) on +Z, and the posed material's
    /// crystal axis turns with it.
    #[test]
    fn a_pose_turns_the_planes_and_the_material() {
        let (planes, tools) = stone();
        let sapphire = GemMaterial::sapphire();
        let axis = sapphire.c_axis;
        let (posed, _, material) = posed_shadow_stone(tilted(), &planes, &tools, sapphire);
        assert_eq!(posed.len(), planes.len());
        let table = planes
            .iter()
            .position(|p| (Vec3::from(p.normal) - Vec3::Y).length() < 1e-4)
            .expect("the round brilliant has a table");
        let turned = Vec3::from(posed[table].normal);
        assert!((turned - Vec3::Z).length() < 1e-4, "table normal {turned}");
        assert!((posed[table].d - planes[table].d).abs() < 1e-6, "d is kept");
        let expected = tilted().rotation() * axis;
        assert!((material.c_axis - expected).length() < 1e-4);
        let (mirror, _) = posed_geometry(tilted(), &planes, &tools);
        assert_eq!(*mirror, *posed, "the mirror reads the same posed planes");
    }

    /// The traced frame turns the stone once per scene generation.
    #[test]
    fn the_posed_frame_is_built_once_per_generation_and_never_for_the_identity() {
        let (planes, tools) = stone();
        let material = GemMaterial::diamond();
        let mut cache = PosedFrameCache::default();
        let stone = (planes.as_slice(), tools.as_slice());
        assert!(
            cache
                .get(1, StonePose::IDENTITY, stone, &material)
                .is_none()
        );
        let first = cache.get(1, tilted(), stone, &material).expect("posed");
        let again = cache.get(1, tilted(), stone, &material).expect("posed");
        assert!(Arc::ptr_eq(&first, &again), "same generation, same stone");
        let next = cache.get(2, tilted(), stone, &material).expect("posed");
        assert!(
            !Arc::ptr_eq(&first, &next),
            "a new generation turns it again"
        );
        assert!(
            cache
                .get(3, StonePose::IDENTITY, stone, &material)
                .is_none()
        );
    }

    /// The pause lasts while the drag is held and 250 ms after its release.
    #[test]
    fn the_shadow_waits_for_the_pose_drag_and_250_ms_after_it() {
        let start = Instant::now();
        let mut hold = PoseDragHold::default();
        assert!(hold.builds_allowed(false, start), "no drag, no pause");
        assert!(!hold.builds_allowed(true, start), "held");
        let held_late = start + Duration::from_secs(2);
        assert!(!hold.builds_allowed(true, held_late), "still held");
        assert!(
            !hold.builds_allowed(false, held_late + Duration::from_millis(100)),
            "released, still settling"
        );
        assert!(
            hold.builds_allowed(false, held_late + SHADOW_SETTLE),
            "250 ms after the release"
        );
        assert!(hold.builds_allowed(false, held_late + Duration::from_secs(5)));
    }
}
