//! The 16x16 self-check scene of each material class.
//!
//! The isotropic class keeps the Spinel scene of `gpu_admission::self_check_scene`; a uniaxial
//! and a biaxial class get the same stone, camera and light with a built-in material of their
//! class. The GPU Worker compares those two renders with the CPU's (`pleochroma_web_gpu`'s
//! `self_check`) before the class is admitted, because a polarization bug draws a plausible
//! picture.

use pleochroma::optics::materials::GemMaterial;
use pleochroma_web_core::{
    gpu_admission::{
        self, BIAXIAL_CLASS, ISOTROPIC_CLASS, SELF_CHECK_SPP, UNIAXIAL_CLASS, class_label,
    },
    scene::{MaterialSpec, SceneSpec},
};

/// The uniaxial stone of the self-check.
const UNIAXIAL_STONE: &str = "Zircon";
/// The biaxial stone of the self-check, when the built-ins still hold it.
const BIAXIAL_STONE: &str = "Topaz";
/// Samples per pixel of an anisotropic self-check: enough that the frame totals of the GPU
/// and the CPU render agree within the tolerances of the Worker's comparison.
const ANISOTROPIC_SPP: u32 = 16;

/// The built-in biaxial stone the self-check draws: [`BIAXIAL_STONE`], else the first biaxial
/// built-in.
fn biaxial_stone() -> String {
    let built_in = GemMaterial::built_in();
    built_in
        .iter()
        .find(|material| {
            material.name == BIAXIAL_STONE && material.biaxial_delta_beta_alpha.is_some()
        })
        .or_else(|| {
            built_in
                .iter()
                .find(|material| material.biaxial_delta_beta_alpha.is_some())
        })
        .map_or_else(
            || BIAXIAL_STONE.to_owned(),
            |material| material.name.clone(),
        )
}

/// The catalogue name of the stone the self-check of `class` draws.
fn stone_for_class(class: u32) -> Option<String> {
    match class {
        UNIAXIAL_CLASS => Some(UNIAXIAL_STONE.to_owned()),
        BIAXIAL_CLASS => Some(biaxial_stone()),
        _ => None,
    }
}

/// The self-check scene of `class`: Spinel for an isotropic (or unknown) class.
pub(super) fn scene_for_class(class: u32) -> SceneSpec {
    let mut scene = gpu_admission::self_check_scene();
    if let Some(stone) = stone_for_class(class) {
        scene.material = MaterialSpec::catalogue(stone);
    }
    scene
}

/// Samples per pixel of the self-check of `class`.
pub(super) const fn spp_for_class(class: u32) -> u32 {
    if class == ISOTROPIC_CLASS {
        SELF_CHECK_SPP
    } else {
        ANISOTROPIC_SPP
    }
}

/// The reason text of a self-check frame the pool reports as invalid.
pub(super) fn invalid_frame_reason(class: u32) -> String {
    if class == ISOTROPIC_CLASS {
        "Self-check render produced invalid pixels".to_owned()
    } else {
        format!(
            "Self-check render of the {} class was blank, invalid or differed from the CPU render",
            class_label(class)
        )
    }
}
