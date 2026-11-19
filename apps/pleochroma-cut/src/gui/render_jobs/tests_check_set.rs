//! The material check set without a window: its plan, its labels and its distinct outputs.

use super::{
    capture_still::plan_still_outputs,
    check_set::{EXTRA_MATERIALS, SHOTS, check_label, extra_materials, job_count},
};
use pleochroma::render_setup::StonePose;
use std::{collections::HashSet, path::Path};

/// Every job of the set for a design whose material is `own`: the four stills, each in `own` and
/// then in the extra materials, as `(label, material)`.
fn planned(own: &str) -> Vec<(String, String)> {
    let mut planned = Vec::new();
    for shot in SHOTS {
        planned.push((check_label("Pentalume", shot, own), own.to_string()));
        for material in extra_materials(own) {
            planned.push((check_label("Pentalume", shot, &material), material));
        }
    }
    planned
}

#[test]
fn a_design_in_its_own_material_gets_twelve_jobs() {
    assert_eq!(job_count("Sapphire"), 12);
    assert_eq!(planned("Sapphire").len(), 12);
}

#[test]
fn a_design_in_one_of_the_extra_materials_gets_eight_jobs() {
    for own in EXTRA_MATERIALS {
        assert_eq!(job_count(own), 8, "{own}");
        assert_eq!(planned(own).len(), 8, "{own}");
    }
    assert_eq!(job_count("quartz"), 8, "the names compare without case");
    assert_eq!(
        extra_materials("Quartz"),
        vec!["Cubic Zirconia".to_string()]
    );
    assert_eq!(
        extra_materials("Cubic Zirconia"),
        vec!["Quartz".to_string()]
    );
}

#[test]
fn the_four_stills_are_table_up_and_tilted_in_two_lightings() {
    let up = SHOTS.iter().filter(|shot| !shot.tilted).count();
    assert_eq!((up, SHOTS.len() - up), (2, 2));
    let lights: HashSet<&str> = SHOTS.iter().map(|shot| shot.light).collect();
    assert_eq!(lights, HashSet::from(["Light Tent", "Daylight"]));
    let distinct: HashSet<String> = SHOTS
        .iter()
        .map(|shot| format!("{} {}", shot.pose_word(), shot.light))
        .collect();
    assert_eq!(distinct.len(), 4);
}

#[test]
fn the_tilted_pose_leans_ten_degrees_and_the_other_is_the_table_up() {
    for shot in SHOTS {
        let pose = shot.pose();
        if shot.tilted {
            assert!((pose.tilt_deg - 10.0).abs() < f32::EPSILON);
            assert!(!pose.is_identity());
        } else {
            assert_eq!(pose, StonePose::IDENTITY);
        }
    }
}

#[test]
fn every_label_of_the_set_is_distinct_and_names_the_design_and_the_material_last() {
    let jobs = planned("Sapphire");
    let labels: HashSet<&str> = jobs.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(labels.len(), 12);
    for (label, material) in &jobs {
        assert!(
            label.starts_with("Pentalume \u{b7} check \u{b7} "),
            "{label}"
        );
        assert!(label.ends_with(&format!(" \u{b7} {material}")), "{label}");
    }
}

#[test]
fn twelve_jobs_with_one_template_name_get_twelve_distinct_outputs() {
    // The export template does not name the pose, so the names can clash; the reservation numbers them.
    let names = vec!["Pentalume.png".to_string(); 12];
    let outputs = plan_still_outputs(Path::new("out"), &names, &[], &|_| false);
    assert_eq!(outputs.len(), 12);
    assert_eq!(outputs.iter().collect::<HashSet<_>>().len(), 12);

    let names = vec!["Pentalume.png".to_string(); 8];
    let taken = vec![Path::new("out").join("Pentalume.png")];
    let outputs = plan_still_outputs(Path::new("out"), &names, &taken, &|_| false);
    assert_eq!(outputs.iter().collect::<HashSet<_>>().len(), 8);
    assert!(
        !outputs.contains(&taken[0]),
        "a queued output is not reused"
    );
}
