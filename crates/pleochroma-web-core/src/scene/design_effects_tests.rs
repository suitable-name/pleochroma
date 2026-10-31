use super::*;
use pleochroma_cut_core::{EdgeAbrasionEffects, InclusionEffects, PreformSpec};

fn design() -> Design {
    Design::fresh(PreformSpec::block(1.0, 1.0, 2.0), 96, 4, 1.62)
}

#[test]
fn a_design_with_nothing_set_gives_the_default_record() {
    let spec = design_effects_spec(&design(), Some("Sapphire"), &[], None);
    assert_eq!(spec, EffectsSpec::default());
    assert!(spec.is_none());
}

#[test]
fn the_inclusion_choice_and_the_abraded_edge_reach_the_record() {
    let mut design = design();
    design.render_effects.inclusions = InclusionEffects::of_preset("Fine Silk");
    design.render_effects.edge_abrasion = EdgeAbrasionEffects {
        width: 0.01,
        alpha: 0.3,
        halo_rq_nm: 0.0,
    };
    let spec = design_effects_spec(&design, Some("Sapphire"), &[], None);
    assert_eq!(
        spec.inclusions.as_ref().map(|s| s.preset.as_str()),
        Some("Fine Silk")
    );
    assert!(spec.edge.width > 0.0);
    assert!(spec.facet_surfaces.is_empty(), "no solve, no surface words");
    assert_eq!(spec.machine, MachineSpec::default());
    // The record applies to a material exactly as the desktop's does.
    let sapphire = GemMaterial::sapphire();
    assert!(spec.apply_to_material(&sapphire).inclusions.is_some());
}

#[test]
fn the_stored_second_axis_is_read_for_a_uniaxial_stone_and_never_for_a_biaxial_one() {
    let mut design = design();
    design.render_effects.inclusions = InclusionEffects::of_preset("Fine Silk");
    design.render_effects.c_axis_override_enabled = true;
    design.render_effects.c_axis_tilt_deg = 30.0;
    design.render_effects.c_axis_azimuth_deg = 40.0;
    design.render_effects.c_axis_second_azimuth_deg = Some(10.0);

    let sapphire = GemMaterial::sapphire();
    let expected = orientation_override(&design.render_effects, &sapphire)
        .and_then(|(_, alpha)| alpha)
        .map(|axis| axis.to_array());
    assert!(expected.is_some());
    let spec = design_effects_spec(&design, Some("Sapphire"), &[], None);
    assert_eq!(spec.inclusion_second_axis, expected);

    // A custom material is found by name among the session's (and only there).
    let mut custom = GemMaterial::sapphire();
    custom.name = "My Corundum".to_owned();
    let found = design_effects_spec(&design, Some("My Corundum"), &[custom], None);
    assert_eq!(found.inclusion_second_axis, expected);
    assert_eq!(
        design_effects_spec(&design, Some("My Corundum"), &[], None).inclusion_second_axis,
        None
    );

    // A biaxial stone carries its own Second Axis, and an unresolved name leaves it unset.
    let biaxial = design_effects_spec(&design, Some("Alexandrite"), &[], None);
    assert_eq!(biaxial.inclusion_second_axis, None);
    assert_eq!(
        design_effects_spec(&design, None, &[], None).inclusion_second_axis,
        None
    );
    assert_eq!(
        design_effects_spec(&design, Some("No Such Stone"), &[], None).inclusion_second_axis,
        None
    );
}
