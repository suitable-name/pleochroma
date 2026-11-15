//! The stone pose of a scene, applied once per request.
//!
//! A scene travels with its planes as designed plus [`SceneState::stone_pose`]. Every consumer of
//! the scene (the tray shadow and mirror, the tracers, the finish and surface inputs) reads the
//! stone as it stands on the tray, so a request that traces turns its scene into a posed scene
//! first ([`apply_pose`]) and everything after it stays unchanged:
//!
//! * the planes and tools are rotated, `d` unchanged;
//! * `material` becomes the posed traced material (crystal axes, zones and the inclusions, which
//!   stay in memory beside the material);
//! * the effects that would build or re-apply an unrotated inclusion set are cleared;
//! * the girdle frosting is folded into explicit `frosted_planes`, computed on the planes as
//!   designed (the girdle finish classifier reads the stone's own vertical);
//! * the scratch basis rides in `effects.scratch`;
//! * `stone_pose` is the identity, so nothing applies it twice.
//!
//! The pose is applied after validation, wherever a request owns its scene and is about to trace
//! it: the stream tracer jobs (a plain worker's request and a coordinator's own lane), batch
//! items, live passes, beam tiles, queued jobs and the `render` command. A coordinator forwards
//! the scene as received (a joined worker poses it itself, and the inclusions would not survive a
//! second trip over the wire), and tilt requests ignore the pose ([`clear_pose`]).

use pleochroma::{
    optics::raytracer::{FacetFinish, surface::ScratchBasis},
    render_setup::{StonePose, pose_stone},
};
use pleochroma_net::SceneState;
use std::borrow::Cow;

/// The pose `scene` carries, clamped. A non-finite component reads as 0 (the worker refuses such a
/// scene in `validate_scene` before any request gets here).
#[must_use]
pub fn pose_of(scene: &SceneState) -> StonePose {
    StonePose::from_array(scene.stone_pose)
}

/// Turns `scene` into its posed scene (see the module docs). A scene whose pose is the identity
/// keeps every value as it is and only has its raw pose written as `[0, 0, 0]`.
pub fn apply_pose(scene: &mut SceneState) {
    let pose = pose_of(scene);
    scene.stone_pose = [0.0, 0.0, 0.0];
    if pose.is_identity() {
        return;
    }
    // The finishes are resolved on the planes as designed: the girdle classifier reads the
    // stone's own vertical, which the rotation would move.
    let frosted: Vec<u32> = super::resolve_facet_finishes(scene)
        .iter()
        .enumerate()
        .filter(|(_, finish)| **finish == FacetFinish::Frosted)
        .filter_map(|(index, _)| u32::try_from(index).ok())
        .collect();
    let posed = {
        let traced = scene.traced_material();
        pose_stone(pose, &scene.planes, &scene.tools, &traced)
    };
    scene.planes = posed.planes;
    scene.tools = posed.tools;
    scene.material = posed.material;
    scene.girdle_frosted = false;
    scene.frosted_planes = frosted;
    // The posed material holds the inclusions and the play-of-color structure already rotated;
    // anything left in the effects would build or re-apply an unrotated set.
    scene.effects.inclusions = None;
    scene.effects.inclusion_second_axis = None;
    scene.effects.structure = None;
    scene.effects.material_inclusions = None;
    scene.effects.scratch = ScratchBasis::new(posed.up, posed.side);
}

/// `scene` as a posed scene: borrowed untouched when its pose is the identity, otherwise a copy
/// run through [`apply_pose`].
#[must_use]
pub fn posed_scene(scene: &SceneState) -> Cow<'_, SceneState> {
    if pose_of(scene).is_identity() {
        return Cow::Borrowed(scene);
    }
    let mut posed = scene.clone();
    apply_pose(&mut posed);
    Cow::Owned(posed)
}

/// Clears the pose of `scene`: the stone is the one as designed. Tilt requests (the tilt curves
/// are a property of the design) call it next to the other view-only features they drop.
pub const fn clear_pose(scene: &mut SceneState) {
    scene.stone_pose = [0.0, 0.0, 0.0];
}

/// `scene` with its pose cleared: borrowed when it carries none.
#[must_use]
pub fn without_pose(scene: &SceneState) -> Cow<'_, SceneState> {
    if scene.stone_pose == [0.0, 0.0, 0.0] {
        return Cow::Borrowed(scene);
    }
    let mut cleared = scene.clone();
    clear_pose(&mut cleared);
    Cow::Owned(cleared)
}

#[cfg(test)]
#[path = "pose_tests.rs"]
mod tests;
