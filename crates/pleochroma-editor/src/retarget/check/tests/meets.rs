//! A retarget keeps a meet-point design meet-point ([`crate::retarget::keep_meets`]): every tier
//! that met a point of the stone cut so far still meets one after the check's candidate is
//! applied, and the round brilliant template's pin-prick culet stays a facet.

use super::*;
use crate::retarget::{keep_meets::meets_kept, plan::build_plan_from, validity::analyze};
use pleochroma_cut_core::{CutMeet, ResolvedMaterial, templates::TEMPLATES};

/// PC 45.140, Cam's Portuguese, by Greg Ninedorf (2014), assigned to the public domain by its
/// own header ("This design works for R.I. 1.76 - 2.16"): a true meet-point design.
const CAMS_PORTUGUESE: &str = "\
GemCad 5.0_2017
g 96 0.0
y 12 y
I 2.16
H PC 45.140  Cam's Portuguese
H by Greg Ninedorf, 2014
H Copyright assigned to the public domain.
a -90.00000 1.07325 96 n 1 88 80 72 64 56 48 40 32 24 16 8 G Set the size and shape
a -61.71792 0.79638 96 n 2 88 80 72 64 56 48 40 32 24 16 8 G Set the girdle line
a -51.97569 0.68189 92 n 3 84 76 68 60 52 44 36 28 20 12 4 G Meet the girdle line
a -47.01841 0.65290 96 n 4 88 80 72 64 56 48 40 32 24 16 8 G Meet 2
a -42.00000 0.64937 92 n 5 84 76 68 60 52 44 36 28 20 12 4 G Meet 3
a 32.00000 0.88823 96 n a 8 16 24 32 40 48 56 64 72 80 88 G Set the girdle width
a 28.00000 0.85428 4 n b 12 20 28 36 44 52 60 68 76 84 92 G Meet the girdle
a 23.00000 0.83175 96 n c 8 16 24 32 40 48 56 64 72 80 88 G Meet a
a 0.00000 0.68812 96 n t G Meet b
F This design works for R.I. 1.76 - 2.16
";

/// A material given by its refractive index alone.
fn index_material(n_d: f64) -> MaterialSelection {
    MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(n_d),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    }
}

/// The tiers of `design` that meet a point of the stone cut so far.
fn meeting_tiers(design: &Design) -> Vec<usize> {
    let solved = design.solve().expect("the design solves");
    let meets = design.cut_meets(&solved).expect("the stone closes");
    meets
        .tiers
        .iter()
        .enumerate()
        .filter(|(_, meet)| matches!(meet, CutMeet::Meets(_)))
        .map(|(tier, _)| tier)
        .collect()
}

/// `design`, designed for index `n_from`, retargeted to `target` by the check the Retarget
/// dialog runs (the default crown policy, as a template's adaptation uses), and its Apply edit
/// applied through the history.
fn retargeted(design: &Design, n_from: f64, target: &ResolvedMaterial) -> Design {
    let plan = build_plan_from(design, n_from, target, CrownShift::default());
    let result = check_retarget(&CheckInputs {
        girdle: None,
        design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
    });
    assert_eq!(
        result.validity.status,
        ValidityStatus::Valid,
        "{}",
        result.validity.headline()
    );
    let edit = result
        .validated_edit(design)
        .expect("the candidate validates");
    let mut retargeted = design.clone();
    History::new()
        .apply(&mut retargeted, edit)
        .expect("the retarget applies");
    assert_ne!(retargeted.tiers, design.tiers, "the angles moved");
    retargeted
}

/// Cam's Portuguese retargeted from 1.76 to 2.16 and back: after each step every tier that met
/// a point of the stone cut so far (tiers 3, 4, 5, b, c and t) still meets one.
#[test]
fn a_retarget_there_and_back_keeps_cams_portuguese_meeting() {
    let mut design = crate::loading::design_from_asc_text("cams.asc", CAMS_PORTUGUESE, None)
        .expect("the fixture parses")
        .design;
    design.material = index_material(1.76);
    let meeting = meeting_tiers(&design);
    assert_eq!(meeting, [2, 3, 4, 6, 7, 8]);
    let solved = design.solve().expect("pinned");
    let targets = design.meet_targets(&solved).expect("the stone closes");

    let mut there = retargeted(
        &design,
        1.76,
        &index_material(2.16).resolve(&BuiltinMaterials),
    );
    let now = meeting_tiers(&there);
    assert!(
        meeting.iter().all(|tier| now.contains(tier)),
        "at 2.16 the tiers {meeting:?} met, now {now:?}"
    );
    assert!(meets_kept(&there, &targets));

    there.material = index_material(2.16);
    let back = retargeted(
        &there,
        2.16,
        &index_material(1.76).resolve(&BuiltinMaterials),
    );
    let now = meeting_tiers(&back);
    assert!(
        meeting.iter().all(|tier| now.contains(tier)),
        "back at 1.76 the tiers {meeting:?} met, now {now:?}"
    );
}

/// The meet-point Standard Round Brilliant template, adapted from diamond to quartz as a template
/// created in quartz is: every tier that met still meets, and the culet, a pin-prick 5e-5 above
/// the mains' point, is still a facet of its own (it keeps its signed miss), not cut away.
#[test]
fn the_round_brilliant_template_adapted_to_quartz_keeps_its_meets_and_its_culet() {
    let spec = &TEMPLATES[0];
    let mut design = Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers());
    design.material = spec.default_material_selection();
    assert_eq!(design.tiers[3].name, "Culet");
    let meeting = meeting_tiers(&design);
    assert!(meeting.contains(&3), "the culet meets the mains' point");

    let adapted = retargeted(
        &design,
        spec.design_ri,
        &selection("Quartz").resolve(&BuiltinMaterials),
    );
    let now = meeting_tiers(&adapted);
    assert!(
        meeting.iter().all(|tier| now.contains(tier)),
        "the tiers {meeting:?} met, now {now:?}"
    );
    let analysis = analyze(&adapted, false).expect("the adapted stone closes");
    assert!(
        analysis.facets.tiers[3].alive >= 1,
        "the culet was cut away: {:?}",
        analysis.facets.tiers[3]
    );
}
