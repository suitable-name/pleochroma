//! The render physics of a design as the render inputs every consumer takes.
//!
//! A design stores its choices in two places: the stone effects (`Design::render_effects`:
//! oriented inclusions, play-of-color, abraded edges) and the tier surface finish
//! (`Design::tier_surfaces`). The user's faceting machine is a preference of the person at the
//! keyboard, never part of a design. [`design_scene_effects`] puts the three together into the one
//! wire record ([`SceneEffects`]) that the local tracer, the exports, the tilt sweeps, the
//! batch thumbnails and a remote worker all take, so they render exactly the same stone:
//!
//! * the material the tracer traces is `effects.apply_to_material(material)`, which applies the
//!   inclusion choice and then the play-of-color choice, in that order;
//! * the surface input of the trace is `effects.surface_scene(&effects.surfaces())`.
//!
//! A design with nothing set gives [`SceneEffects::default`]: the material is borrowed untouched,
//! the surface scene is ideal, and the render is bit-identical to one without this module.

use glam::Vec3;
use pleochroma::{
    geometry::{GpuFacetPlane, meet_solver::SolvedTier},
    optics::{
        materials::GemMaterial,
        raytracer::surface::{FacetSurface, FacetingMachine},
    },
};
use pleochroma_cut_core::{Design, StoneRenderEffects};
use pleochroma_editor::stone_orientation::orientation_override;
use pleochroma_net::scene::effects::{EdgeSpec, MachineSpec, SceneEffects};

/// The stored Second Axis a uniaxial stone's silk reads as its turn, for the design's Crystal
/// Axis override: `None` unless the override is on and `material` is not biaxial (a biaxial stone
/// carries its own Second Axis, which the optics read).
#[must_use]
pub fn inclusion_second_axis(effects: &StoneRenderEffects, material: &GemMaterial) -> Option<Vec3> {
    if material.biaxial_delta_beta_alpha.is_some() {
        return None;
    }
    orientation_override(effects, material).and_then(|(_, alpha)| alpha)
}

/// The facet surface finish of `design`'s whole stone as a table: each plane of the finished
/// stone that departs from ideal, as the f32 plane the tracer is given, with its packed word.
/// Empty when the design has no surface finish, when nothing it states departs from ideal, or
/// when `solved` is `None` (the design does not solve). Runs no solve of its own.
///
/// The table is independent of which planes get traced: the viewport (which may show a Cut slider
/// position), an export of the finished stone and a tilt sweep each ask [`words_for_planes`] for
/// the words of their own plane list.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the GPU plane precision, converted exactly as the Cut slider converts its planes"
)]
pub fn surface_table(design: &Design, solved: Option<&[SolvedTier]>) -> Vec<(GpuFacetPlane, u32)> {
    if !design.has_surface() {
        return Vec::new();
    }
    let Some(masts) = solved else {
        return Vec::new();
    };
    design
        .planes_from_solved(masts)
        .into_iter()
        .zip(design.facet_surfaces_from_solved(masts))
        .filter(|(_, word)| *word != FacetSurface::IDEAL)
        .map(|((normal, offset), word)| {
            (
                GpuFacetPlane::new(normal.as_vec3(), -offset as f32),
                word.packed(),
            )
        })
        .collect()
}

/// The packed words of `planes`, parallel to them, from `table` ([`surface_table`]): a plane takes
/// the word of the equal plane in the table and is ideal otherwise. Empty when no plane has a
/// word, which is every stone without a surface finish (the table is empty).
#[must_use]
pub fn words_for_planes(table: &[(GpuFacetPlane, u32)], planes: &[GpuFacetPlane]) -> Vec<u32> {
    if table.is_empty() {
        return Vec::new();
    }
    let ideal = FacetSurface::IDEAL.packed();
    let words: Vec<u32> = planes
        .iter()
        .map(|plane| {
            table
                .iter()
                .find(|(whole, _)| whole == plane)
                .map_or(ideal, |(_, word)| *word)
        })
        .collect();
    if words.iter().all(|word| *word == ideal) {
        Vec::new()
    } else {
        words
    }
}

/// The render physics of a design: `effects` (inclusions, play-of-color, abraded edge), the
/// stone's facet `surface_words` and the user's `machine`.
///
/// `material` is the material before the inclusion choice is applied (it only decides which Second
/// Axis the silk reads); `None` leaves the Second Axis unset. The machine is written only when the
/// stone has a surface finish, so a stone without one is exactly [`SceneEffects::default`].
#[must_use]
pub fn design_scene_effects(
    effects: &StoneRenderEffects,
    material: Option<&GemMaterial>,
    surface_words: Vec<u32>,
    machine: &FacetingMachine,
) -> SceneEffects {
    let second_axis = effects
        .inclusions
        .as_ref()
        .and(material)
        .and_then(|material| inclusion_second_axis(effects, material))
        .map(|axis| axis.to_array());
    let mut scene = SceneEffects {
        inclusions: effects.inclusion_spec(),
        inclusion_second_axis: second_axis,
        structure: effects.structure_spec(),
        facet_surfaces: surface_words,
        machine: MachineSpec::default(),
        edge: EdgeSpec::from_abrasion(&effects.edge_abrasion()),
        // The material's own set rides in only with a captured scene (`scene_snapshot`).
        material_inclusions: None,
    };
    if scene.has_surface() {
        scene.machine = MachineSpec::from_machine(machine);
    }
    scene
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{
        EdgeAbrasionEffects, Edit, InclusionEffects, SurfaceFinish, SurfaceGrade, TierSurface,
    };
    use pleochroma_editor::EditorSession;

    #[test]
    fn a_design_with_nothing_set_gives_the_default_record_whatever_the_machine() {
        let session = EditorSession::fresh();
        let machine = FacetingMachine {
            arm_offset_deg: 20.0,
            ..FacetingMachine::DEFAULT
        };
        let scene = design_scene_effects(
            &session.design.render_effects,
            Some(&GemMaterial::sapphire()),
            words_for_planes(&surface_table(&session.design, None), &[]),
            &machine,
        );
        assert_eq!(scene, SceneEffects::default());
        assert!(scene.is_none());
    }

    #[test]
    fn the_inclusion_choice_and_the_edge_reach_the_record_and_the_machine_follows_a_surface() {
        let effects = StoneRenderEffects {
            inclusions: InclusionEffects::of_preset("Fine Silk"),
            edge_abrasion: EdgeAbrasionEffects {
                width: 0.01,
                alpha: 0.3,
                halo_rq_nm: 0.0,
            },
            ..StoneRenderEffects::default()
        };
        let machine = FacetingMachine {
            arm_offset_deg: 20.0,
            ..FacetingMachine::DEFAULT
        };
        let scene = design_scene_effects(
            &effects,
            Some(&GemMaterial::sapphire()),
            Vec::new(),
            &machine,
        );
        assert_eq!(
            scene.inclusions.as_ref().map(|s| s.preset.as_str()),
            Some("Fine Silk")
        );
        assert!(scene.edge.width > 0.0);
        assert_eq!(
            scene.machine,
            MachineSpec::from_machine(&machine),
            "an abraded edge reads the machine"
        );
        // Silk with no surface at all keeps the default machine.
        let silk_only = StoneRenderEffects {
            edge_abrasion: EdgeAbrasionEffects::default(),
            ..effects
        };
        let scene = design_scene_effects(&silk_only, None, Vec::new(), &machine);
        assert_eq!(scene.machine, MachineSpec::default());
    }

    #[test]
    fn a_surface_tier_gives_its_words_for_whichever_planes_are_traced() {
        const ASC: &str = "GemCad 5.0\n\
             g 16 0.0\n\
             y 4 n\n\
             I 1.54\n\
             a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
             a -42.000000 0.60000000 0 4 8 12 P1 Set stone size\n\
             a 32.000000 0.45000000 0 4 8 12 C1 Set stone size\n";
        let schedule = pleochroma_formats::asc::parse_asc(ASC).expect("fixture .asc must parse");
        let mut design = Design::from_asc_schedule(
            pleochroma_cut_core::PreformSpec::block(2.0, 1.0, 2.0),
            &schedule,
        );
        design.ensure_tier_ids();
        let solved = design.solve().expect("the fixture solves");
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the tracer's planes are f32, converted as the Cut slider converts them"
        )]
        let planes: Vec<GpuFacetPlane> = design
            .planes_from_solved(&solved)
            .into_iter()
            .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
            .collect();
        assert_eq!(surface_table(&design, Some(&solved)).len(), 0);
        let surface = TierSurface::of_tier(SurfaceFinish::of_grade(SurfaceGrade::Fine));
        design
            .apply_edit(Edit::SetTierSurface {
                index: 1,
                surface: Some(surface),
            })
            .expect("the surface applies");
        let table = surface_table(&design, Some(&solved));
        assert!(
            !table.is_empty(),
            "a finished tier puts planes in the table"
        );
        let words = words_for_planes(&table, &planes);
        assert_eq!(words.len(), planes.len(), "one word per plane");
        assert!(
            words
                .iter()
                .any(|word| *word != FacetSurface::IDEAL.packed())
        );
        assert_eq!(surface_table(&design, None).len(), 0);
    }
}
