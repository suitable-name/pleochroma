//! Per-scene render physics, separate from the stable scene payload (like the engine).

use super::{FromWorker, WorkerHandler, WorkerRole};
use crate::scene::EffectsSpec;

impl WorkerHandler {
    pub(super) fn set_effects(
        &mut self,
        scene_id: u64,
        effects: EffectsSpec,
    ) -> Option<FromWorker> {
        if let Err(reply) = self.require(WorkerRole::Render, "SetEffects") {
            return Some(*reply);
        }
        let Some(cached) = self
            .scene
            .as_mut()
            .filter(|cached| cached.scene_id == scene_id)
        else {
            return Some(FromWorker::SceneError {
                scene_id,
                message: "Render effects belong to an unavailable scene.".to_owned(),
            });
        };
        if let Err(message) = cached.scene.set_effects(effects) {
            self.scene = None;
            return Some(FromWorker::SceneError { scene_id, message });
        }
        self.denoiser.clear();
        None
    }
}
