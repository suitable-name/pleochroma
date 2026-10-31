//! [`DesignScene`]: a geometry scene together with the design's render physics.
//!
//! The phone traces the design's silk, play-of-color, surface finish and abraded edge the way the
//! desktop and the browser do: the record ([`EffectsSpec`], built by
//! `pleochroma_web_core::scene::design_effects_spec`) is applied to every scene built from the
//! spec ([`DesignScene::build_with_customs`]: the live preview, a local export, a remote session),
//! and a remote worker is sent the same record in the wire scene ([`wire_effects`]).

use std::{ops::Deref, sync::Arc};

use pleochroma::renderer::env_map::EnvironmentMap;
use pleochroma_net::scene::{
    effects::{EdgeSpec, MachineSpec, SceneEffects},
    material_inclusions::MaterialInclusions,
};
use pleochroma_web_core::scene::{CustomMaterialSpec, EffectsSpec, GeometrySceneSpec, OwnedScene};

/// An immutable geometry scene plus the design's render physics.
///
/// Dereferences to the [`GeometrySceneSpec`] (the planes, tools, camera and lighting); only the
/// build is its own, because it also applies the effects.
#[derive(Debug, Clone, PartialEq)]
pub struct DesignScene {
    geometry: GeometrySceneSpec,
    effects: EffectsSpec,
}

impl DesignScene {
    /// `geometry` with the design's `effects`.
    #[must_use]
    pub const fn new(geometry: GeometrySceneSpec, effects: EffectsSpec) -> Self {
        Self { geometry, effects }
    }

    /// `geometry` with no render physics: it builds exactly the scene
    /// [`GeometrySceneSpec::build_with_customs`] does.
    #[must_use]
    pub fn plain(geometry: GeometrySceneSpec) -> Self {
        Self::new(geometry, EffectsSpec::default())
    }

    /// The render physics this scene applies.
    #[must_use]
    pub const fn effects(&self) -> &EffectsSpec {
        &self.effects
    }

    /// Builds the scene ([`GeometrySceneSpec::build_with_customs`]) and applies the effects
    /// record to it; the default record changes nothing, bit for bit.
    ///
    /// # Errors
    ///
    /// The first tool or scene-builder problem, or the effects record's own
    /// (`EffectsSpec::validate`), as a sentence.
    pub fn build_with_customs(
        &self,
        hdr: Option<Arc<EnvironmentMap>>,
        customs: &[CustomMaterialSpec],
    ) -> Result<OwnedScene, String> {
        let mut scene = self.geometry.build_with_customs(hdr, customs)?;
        scene.set_effects(self.effects.clone())?;
        Ok(scene)
    }
}

impl Deref for DesignScene {
    type Target = GeometrySceneSpec;

    fn deref(&self) -> &GeometrySceneSpec {
        &self.geometry
    }
}

/// The render physics of `scene` as the wire record a remote worker is sent.
///
/// That is the effects the scene applied, and the inclusions its material traces with (a custom
/// material's stored silk and play-of-color, which the material's own serde bytes leave out).
/// The plain Ray-Traced image engine ignores all of it, so it sends the default record, like the
/// desktop.
#[must_use]
pub fn wire_effects(scene: &OwnedScene) -> SceneEffects {
    if scene.engine().is_ray() {
        return SceneEffects::default();
    }
    let spec = scene.effects();
    SceneEffects {
        inclusions: spec.inclusions.clone(),
        inclusion_second_axis: spec.inclusion_second_axis,
        structure: spec.structure,
        facet_surfaces: spec.facet_surfaces.clone(),
        machine: MachineSpec::from_machine(&spec.machine.to_machine()),
        edge: EdgeSpec::from_abrasion(&spec.edge.to_abrasion()),
        material_inclusions: MaterialInclusions::of_material(scene.material()),
    }
}
