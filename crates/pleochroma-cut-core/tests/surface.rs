//! Surface finish and the render-physics effects through the public API of
//! `pleochroma-cut-core`: the undoable tier edit, the native `.psdd` round trip, the packed
//! facet words the renderer is handed, and the `[render_effects]` subtables for inclusions,
//! play-of-color and abraded edges. The oracle for "a design that does not use them changes
//! nothing" is byte equality with what the same design wrote before: an empty surface map and
//! default effects must add no key, no table, no plane word and no version bump.

use pleochroma::optics::{
    inclusions::bragg::{DEFAULT_SEED, StructurePattern},
    raytracer::surface::{FacetSurface, PolishPreset, SurfaceKind},
};
use pleochroma_cut_core::{
    Design, EdgeAbrasionEffects, Edit, History, InclusionEffects, MaterialSelection, PreformSpec,
    RemapRounding, ScheduleState, StoneRenderEffects, StructureEffects, SurfaceFinish,
    SurfaceGrade, TierFrosting, TierSurface,
    native::{DesignExtras, design_from_str, design_to_string},
};

/// The five-tier round brilliant of the frosting tests: tier 1 is `-42` degrees at `0 4 8 12`,
/// tier 2 is `-38` at `2 6 10 14`, tier 3 is `32` at `0 4 8 12`, tier 4 is `40` at `2 6 10 14`.
fn fixture() -> Design {
    const ASC: &str = "GemCad 5.0\n\
         g 16 0.0\n\
         y 4 n\n\
         I 1.54\n\
         a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
         a -42.000000 0.60000000 0 4 8 12 P1 Set stone size\n\
         a -38.000000 0.55000000 2 6 10 14 P2 Set stone size\n\
         a 32.000000 0.45000000 0 4 8 12 C1 Set stone size\n\
         a 40.000000 0.40000000 2 6 10 14 C2 Set stone size\n";
    let schedule = pleochroma_formats::asc::parse_asc(ASC).expect("fixture .asc must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.54),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    design.ensure_tier_ids();
    design
}

const fn grade(grade: SurfaceGrade) -> SurfaceFinish {
    SurfaceFinish::of_grade(grade)
}

const fn tier_of(grade_: SurfaceGrade) -> TierSurface {
    TierSurface::of_tier(grade(grade_))
}

fn set(design: &mut Design, index: usize, surface: Option<TierSurface>) -> Edit {
    design
        .apply_edit(Edit::SetTierSurface { index, surface })
        .expect("the tier exists")
}

fn native_text(design: &Design) -> String {
    design_to_string(design, None, &DesignExtras::default()).expect("the design saves")
}

fn version_lines(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| line.starts_with("version"))
        .collect()
}

fn reload(text: &str) -> Design {
    design_from_str(text).expect("loads").design
}

// ---- the edit ----

#[test]
fn setting_a_tier_surface_is_one_undo_step_and_redoes() {
    let mut design = fixture();
    let before = design.clone();
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetTierSurface {
                index: 2,
                surface: Some(tier_of(SurfaceGrade::Commercial)),
            },
        )
        .expect("applies");
    assert_eq!(
        design.tier_surface(2),
        Some(&tier_of(SurfaceGrade::Commercial))
    );
    assert_eq!(design.tier_surface(1), None);
    assert!(design.has_surface());
    assert_eq!(design.tier_surface_text(2), "Commercial");
    assert_eq!(design.tier_surface_text(1), "");
    let after = design.clone();

    assert!(history.undo(&mut design).expect("undoes"));
    assert_eq!(design, before);
    assert!(!design.has_surface());
    assert!(history.redo(&mut design).expect("redoes"));
    assert_eq!(design, after);
}

#[test]
fn a_surface_is_normalised_and_clearing_it_is_the_inverse() {
    let mut design = fixture();
    let before = design.clone();
    let custom = SurfaceFinish {
        rq_nm: 500.0,
        aniso: 99.0,
        lap_deg: Some(200.0),
        ..grade(SurfaceGrade::Custom)
    };
    let inverse = set(
        &mut design,
        1,
        Some(TierSurface {
            finish: tier_of(SurfaceGrade::Fine).finish,
            // Out of order, repeated and not finite: ascending, merged, dropped.
            facets: vec![
                (8.0, grade(SurfaceGrade::Prepolish)),
                (0.0, custom),
                (8.0, grade(SurfaceGrade::Mirror)),
                (f64::NAN, grade(SurfaceGrade::Mirror)),
            ],
        }),
    );
    let stored = design.tier_surface(1).expect("stored").clone();
    assert_eq!(stored.facets.len(), 2);
    assert_eq!(stored.facets[0].0.to_bits(), 0.0f64.to_bits());
    assert_eq!(stored.facets[1].0.to_bits(), 8.0f64.to_bits());
    assert_eq!(
        stored.facets[1].1.grade,
        SurfaceGrade::Mirror,
        "the later entry wins"
    );
    // Custom values are brought into range: 100 nm, 15, and 20 degrees (200 mod 180).
    let clamped = stored.facets[0].1;
    assert_eq!(clamped.rq_nm.to_bits(), 100.0f64.to_bits());
    assert_eq!(clamped.aniso.to_bits(), 15.0f64.to_bits());
    assert_eq!(clamped.lap_deg.map(f64::to_bits), Some(20.0f64.to_bits()));

    design.apply_edit(inverse).expect("the inverse applies");
    assert_eq!(design, before);
    // A surface that states nothing is a clear.
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Ideal)));
    assert_eq!(design.tier_surface(1), None);
    assert!(!design.has_surface());
}

#[test]
fn an_unknown_tier_is_an_error_and_changes_nothing() {
    let mut design = fixture();
    let before = design.clone();
    let result = design.apply_edit(Edit::SetTierSurface {
        index: 99,
        surface: Some(tier_of(SurfaceGrade::Fine)),
    });
    assert!(result.is_err());
    assert_eq!(design, before);
}

#[test]
fn removing_a_finished_tier_and_undoing_restores_its_surface() {
    let mut design = fixture();
    let with_facet = tier_of(SurfaceGrade::Commercial)
        .with_facet(2.0, Some(grade(SurfaceGrade::Prepolish)))
        .expect("states a finish");
    set(&mut design, 2, Some(with_facet.clone()));
    set(&mut design, 3, Some(tier_of(SurfaceGrade::Mirror)));
    let before = design.clone();
    let mut history = History::new();

    history
        .apply(&mut design, Edit::RemoveTier { index: 2 })
        .expect("removes");
    // The one that was tier 3 is tier 2 now and keeps its own (keyed by the stable tier id).
    assert_eq!(design.tier_surface(2), Some(&tier_of(SurfaceGrade::Mirror)));
    assert_eq!(design.tier_surfaces.len(), 1);

    assert!(history.undo(&mut design).expect("undoes"));
    assert_eq!(design, before);
    assert_eq!(design.tier_surface(2), Some(&with_facet));
}

#[test]
fn moving_a_tier_keeps_its_surface_with_it() {
    let mut design = fixture();
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Fine)));
    let id = design.tier_ids[1];
    design
        .apply_edit(Edit::MoveTier { from: 1, to: 3 })
        .expect("moves");
    let at = design.tier_ids.iter().position(|&t| t == id).expect("kept");
    assert_eq!(at, 3);
    assert_eq!(design.tier_surface(at), Some(&tier_of(SurfaceGrade::Fine)));
    assert_eq!(design.tier_surfaces.len(), 1);
}

#[test]
fn remapping_the_gear_remaps_the_facet_positions_and_undo_restores_them() {
    let mut design = fixture();
    let surface = tier_of(SurfaceGrade::Commercial)
        .with_facet(2.0, Some(grade(SurfaceGrade::Prepolish)))
        .and_then(|s| s.with_facet(6.0, Some(grade(SurfaceGrade::Mirror))))
        .expect("states a finish");
    set(&mut design, 2, Some(surface));
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Fine)));
    let before = design.clone();
    let inverse = design
        .apply_edit(Edit::RemapIndices {
            from_gear: 16,
            to_gear: 32,
            rounding: RemapRounding::Nearest,
        })
        .expect("a remap never fails");
    let moved = design.tier_surface(2).expect("kept");
    let positions: Vec<f64> = moved.facets.iter().map(|(at, _)| *at).collect();
    assert_eq!(
        positions,
        vec![4.0, 12.0],
        "the positions double with the gear"
    );
    assert_eq!(design.tier_surface(1), Some(&tier_of(SurfaceGrade::Fine)));
    for position in positions {
        assert!(design.tiers[2].indices.contains(&position));
    }
    design.apply_edit(inverse).expect("the inverse applies");
    assert_eq!(design, before);
}

#[test]
fn replacing_the_schedule_keeps_the_surface_of_surviving_tiers_and_undo_restores_all() {
    let mut design = fixture();
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Fine)));
    set(&mut design, 3, Some(tier_of(SurfaceGrade::Prepolish)));
    let before = design.clone();

    let mut state = ScheduleState::of(&design);
    assert_eq!(
        state.tier_surfaces.len(),
        2,
        "the state carries the surface"
    );
    state.tiers.remove(1);
    let dropped = state.tier_ids.remove(1);
    state.tier_surfaces.remove(&dropped);
    let mut history = History::new();
    history
        .apply(&mut design, Edit::ReplaceSchedule(Box::new(state)))
        .expect("the state applies");
    assert_eq!(design.tier_surfaces.len(), 1);
    assert_eq!(
        design.tier_surface(2),
        Some(&tier_of(SurfaceGrade::Prepolish))
    );

    assert!(history.undo(&mut design).expect("undoes"));
    assert_eq!(design, before);
}

#[test]
fn a_surface_edit_describes_itself() {
    let mut design = fixture();
    let edit = Edit::SetTierSurface {
        index: 1,
        surface: Some(tier_of(SurfaceGrade::Commercial)),
    };
    let text = edit.describe(&design);
    assert!(text.starts_with("Set surface finish of "), "{text}");
    assert!(text.ends_with("to Commercial"), "{text}");
    let inverse = design.apply_edit(edit).expect("applies");
    assert!(
        inverse
            .describe(&design)
            .starts_with("Clear surface finish of ")
    );
}

// ---- the native file ----

#[test]
fn a_design_without_a_surface_writes_no_surface_key_and_the_same_version() {
    let design = fixture();
    let text = native_text(&design);
    assert!(!text.contains("surface"), "no key for an ideal design");
    assert!(!text.contains("ground_alpha"));
    assert!(!text.contains("render_effects"), "no effects table");
    let again = reload(&text);
    assert_eq!(native_text(&again), text);
}

#[test]
fn a_surface_round_trips_through_the_native_file_without_a_version_bump() {
    let plain = native_text(&fixture());
    let mut design = fixture();
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Commercial)));
    let custom = SurfaceFinish {
        rq_nm: 12.5,
        aniso: 2.5,
        lap_deg: Some(30.0),
        no_sweep: true,
        ..grade(SurfaceGrade::Custom)
    };
    let ground = SurfaceFinish {
        ground_alpha: Some(0.42),
        ..grade(SurfaceGrade::Ideal)
    };
    let on_a_tier = tier_of(SurfaceGrade::Fine)
        .with_facet(2.0, Some(custom))
        .and_then(|s| s.with_facet(10.0, Some(grade(SurfaceGrade::Ideal))))
        .expect("states a finish");
    set(&mut design, 4, Some(on_a_tier));
    set(
        &mut design,
        3,
        Some(TierSurface::of_tier(SurfaceFinish {
            lap_deg: None,
            ..ground
        })),
    );
    let text = native_text(&design);
    assert!(text.contains("surface = \"Commercial\""), "{text}");
    assert!(text.contains("surface_facets"), "{text}");
    assert!(text.contains("ground_alpha = 0.42"), "{text}");
    assert_eq!(version_lines(&text), version_lines(&plain));

    let loaded = reload(&text);
    assert_eq!(loaded.tier_surfaces.len(), 3);
    for index in 0..design.tiers.len() {
        assert_eq!(
            loaded.tier_surface(index),
            design.tier_surface(index),
            "tier {index} comes back exactly"
        );
    }
    assert_eq!(native_text(&loaded), text, "saving again is byte-identical");
}

#[test]
fn a_file_from_before_surface_finish_reads_ideal() {
    let mut design = fixture();
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Commercial)));
    set(
        &mut design,
        4,
        Some(
            tier_of(SurfaceGrade::Ideal)
                .with_facet(2.0, Some(grade(SurfaceGrade::Fine)))
                .expect("states a finish"),
        ),
    );
    let text = native_text(&design);
    // What a build that predates the keys wrote: the same file without them.
    let mut table: toml::Table = toml::from_str(&text).expect("the file is TOML");
    let tiers = table
        .get_mut("tiers")
        .and_then(toml::Value::as_array_mut)
        .expect("tiers");
    for tier in tiers {
        let tier = tier.as_table_mut().expect("a tier table");
        tier.retain(|key, _| !key.starts_with("surface") && key != "ground_alpha");
    }
    let old = toml::to_string(&table).expect("serialises");
    assert!(!old.contains("surface"));
    let loaded = reload(&old);
    assert!(!loaded.has_surface());
    assert_eq!(loaded.tiers.len(), design.tiers.len());
}

#[test]
fn an_unknown_grade_name_reads_as_ideal_and_the_file_still_loads() {
    let mut design = fixture();
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Commercial)));
    let text = native_text(&design).replace("\"Commercial\"", "\"Satin\"");
    assert!(!reload(&text).has_surface());
}

// ---- the words the renderer reads ----

#[test]
fn a_design_without_a_surface_gives_no_facet_surface_words() {
    let design = fixture();
    let solved = design.solve().expect("the fixture solves");
    assert_eq!(
        design.facet_surfaces_from_solved(&solved),
        [] as [pleochroma::optics::raytracer::surface::FacetSurface; 0]
    );
    let planes = design.planes_from_solved(&solved);
    assert_eq!(
        design.facet_surfaces_in(&solved, &planes),
        [] as [pleochroma::optics::raytracer::surface::FacetSurface; 0]
    );
}

#[test]
fn the_planes_of_a_finished_tier_carry_its_words() {
    let mut design = fixture();
    let solved = design.solve().expect("the fixture solves");
    let preform_len = design.preform.planes().len();
    let planes = design.planes_from_solved(&solved);

    // Tier 3's facets are 0 4 8 12; the one at 4 is Prepolish, the others Commercial.
    let tier = tier_of(SurfaceGrade::Commercial)
        .with_facet(4.0, Some(grade(SurfaceGrade::Prepolish)))
        .expect("states a finish");
    set(&mut design, 3, Some(tier));
    let words = design.facet_surfaces_from_solved(&solved);
    let marked: Vec<(usize, FacetSurface)> = words
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, word)| *word != FacetSurface::IDEAL)
        .collect();
    assert_eq!(marked.len(), 4, "four facets, four planes");
    assert!(
        marked.iter().all(|(i, _)| *i >= preform_len),
        "the preform's own planes are never finished"
    );
    let commercial = FacetSurface::from_preset(PolishPreset::Commercial, 0.3);
    let prepolish = FacetSurface::from_preset(PolishPreset::Prepolish, 0.3);
    assert_eq!(marked.iter().filter(|(_, w)| *w == commercial).count(), 3);
    assert_eq!(marked.iter().filter(|(_, w)| *w == prepolish).count(), 1);
    assert!(
        marked
            .iter()
            .all(|(_, w)| w.kind() == SurfaceKind::Polished)
    );

    // A partial arrangement (the whole stone here) gets the same words, parallel to its planes.
    let in_planes = design.facet_surfaces_in(&solved, &planes);
    assert_eq!(in_planes.len(), planes.len());
    assert_eq!(
        in_planes
            .iter()
            .filter(|word| **word != FacetSurface::IDEAL)
            .count(),
        4
    );
    // A subset of the planes finds the finished ones by their bits.
    let finished_plane = planes[marked[0].0];
    let subset = design.facet_surfaces_in(&solved, &[planes[0], finished_plane]);
    assert_eq!(subset, vec![FacetSurface::IDEAL, marked[0].1]);
}

#[test]
fn a_custom_angle_and_no_sweep_reach_the_word() {
    let mut design = fixture();
    let solved = design.solve().expect("the fixture solves");
    let custom = SurfaceFinish {
        rq_nm: 8.0,
        aniso: 3.0,
        lap_deg: Some(45.0),
        no_sweep: true,
        ..grade(SurfaceGrade::Custom)
    };
    set(&mut design, 2, Some(TierSurface::of_tier(custom)));
    let words = design.facet_surfaces_from_solved(&solved);
    let word = words
        .iter()
        .copied()
        .find(|word| *word != FacetSurface::IDEAL)
        .expect("a custom finish is a word");
    assert_eq!(word.kind(), SurfaceKind::Polished);
    assert!((word.rq_nm() - 8.0).abs() < 0.11);
    assert!((word.anisotropy() - 3.0).abs() < 0.07);
    assert!((word.custom_angle_deg().expect("fixed") - 45.0).abs() < 0.2);
    assert!(word.no_sweep());
}

#[test]
fn a_frosted_tier_takes_the_roughness_the_design_states_and_none_otherwise() {
    let mut design = fixture();
    let solved = design.solve().expect("the fixture solves");
    design
        .apply_edit(Edit::SetTierFrosting {
            index: 1,
            frosting: Some(TierFrosting::Tier),
        })
        .expect("frosts");
    // A polish on a frosted tier changes nothing: the frosted finish already names the default
    // ground arm.
    set(&mut design, 1, Some(tier_of(SurfaceGrade::Commercial)));
    assert_eq!(
        design.facet_surfaces_from_solved(&solved),
        [] as [pleochroma::optics::raytracer::surface::FacetSurface; 0]
    );

    let rough = SurfaceFinish {
        ground_alpha: Some(0.4),
        ..grade(SurfaceGrade::Commercial)
    };
    set(&mut design, 1, Some(TierSurface::of_tier(rough)));
    let words = design.facet_surfaces_from_solved(&solved);
    let ground: Vec<FacetSurface> = words
        .iter()
        .copied()
        .filter(|word| *word != FacetSurface::IDEAL)
        .collect();
    assert_eq!(ground.len(), 4);
    assert!(ground.iter().all(|word| word.is_ground()));
    assert!(
        ground
            .iter()
            .all(|word| (word.ground_alpha() - 0.4).abs() < 0.001)
    );
}

// ---- the render effects ----

fn effects_with_everything() -> StoneRenderEffects {
    StoneRenderEffects {
        inclusions: Some(InclusionEffects {
            density: 0.5,
            turn_deg: Some(30.0),
            ..InclusionEffects::of_preset("Fine Silk").expect("a preset")
        }),
        structure: Some(StructureEffects {
            pattern: StructurePattern::Harlequin,
            seed: 777,
            size: Some(0.3),
            color_low_nm: 450.0,
            color_high_nm: 650.0,
            purity: 0.07,
            coverage: 0.6,
        }),
        edge_abrasion: EdgeAbrasionEffects {
            width: 0.02,
            alpha: 0.35,
            halo_rq_nm: 12.0,
        },
        ..StoneRenderEffects::default()
    }
}

fn set_effects(design: &mut Design, effects: StoneRenderEffects) -> Edit {
    design
        .apply_edit(Edit::SetRenderEffects { effects })
        .expect("effects always apply")
}

#[test]
fn a_design_with_default_effects_writes_no_effects_table() {
    let text = native_text(&fixture());
    assert!(!text.contains("render_effects"));
    assert!(!text.contains("edge_abrasion"));
}

#[test]
fn inclusions_structure_and_edges_round_trip_through_the_native_file() {
    let mut design = fixture();
    set_effects(&mut design, effects_with_everything());
    let text = native_text(&design);
    assert!(text.contains("[render_effects.inclusions]"), "{text}");
    assert!(text.contains("preset = \"Fine Silk\""), "{text}");
    assert!(text.contains("[render_effects.structure]"), "{text}");
    assert!(text.contains("pattern = \"Harlequin\""), "{text}");
    assert!(text.contains("seed = 777"), "{text}");
    assert!(text.contains("edge_abrasion_width = 0.02"), "{text}");
    assert!(text.contains("edge_abrasion_alpha = 0.35"), "{text}");
    // Unused parts are not written.
    assert!(!text.contains("sheen"), "{text}");
    assert_eq!(
        version_lines(&text),
        version_lines(&native_text(&fixture())),
        "no version bump"
    );

    let loaded = reload(&text);
    assert_eq!(loaded.render_effects, design.render_effects);
    assert_eq!(native_text(&loaded), text, "saving again is byte-identical");
}

#[test]
fn only_the_parts_that_are_set_are_written() {
    let mut design = fixture();
    set_effects(
        &mut design,
        StoneRenderEffects {
            inclusions: InclusionEffects::of_preset("Rutilated"),
            ..StoneRenderEffects::default()
        },
    );
    let text = native_text(&design);
    assert!(text.contains("[render_effects.inclusions]"), "{text}");
    assert!(!text.contains("[render_effects.structure]"), "{text}");
    assert!(!text.contains("edge_abrasion"), "{text}");
    assert_eq!(
        reload(&text).render_effects.inclusions,
        InclusionEffects::of_preset("Rutilated")
    );
}

#[test]
fn an_unknown_preset_or_pattern_reads_as_not_set() {
    let mut design = fixture();
    set_effects(&mut design, effects_with_everything());
    let text = native_text(&design)
        .replace("\"Fine Silk\"", "\"Glitter Dust\"")
        .replace("\"Harlequin\"", "\"Swirl\"");
    let loaded = reload(&text);
    assert_eq!(loaded.render_effects.inclusions, None);
    assert_eq!(loaded.render_effects.structure, None);
    assert!(loaded.render_effects.edge_abrasion.to_abrasion().active());
}

#[test]
fn a_set_effects_edit_with_the_new_parts_is_one_undo_step() {
    let mut design = fixture();
    let before = design.clone();
    let mut history = History::new();
    history
        .apply(
            &mut design,
            Edit::SetRenderEffects {
                effects: effects_with_everything(),
            },
        )
        .expect("applies");
    assert!(design.render_effects.inclusion_spec().is_some());
    assert!(design.render_effects.structure_spec().is_some());
    assert!(design.render_effects.edge_abrasion().active());
    assert!(history.undo(&mut design).expect("undoes"));
    assert_eq!(design, before);
    assert!(design.render_effects.is_default());
}

#[test]
fn the_effects_are_sanitised_when_applied() {
    let mut design = fixture();
    let wild = StoneRenderEffects {
        inclusions: Some(InclusionEffects {
            density: 7.0,
            ..InclusionEffects::of_preset("Fine Silk").expect("a preset")
        }),
        structure: Some(StructureEffects {
            seed: DEFAULT_SEED,
            coverage: 9.0,
            ..effects_with_everything().structure.expect("set")
        }),
        edge_abrasion: EdgeAbrasionEffects {
            width: 9.0,
            alpha: 0.2,
            halo_rq_nm: 0.0,
        },
        ..StoneRenderEffects::default()
    };
    set_effects(&mut design, wild);
    let stored = design.render_effects;
    assert_eq!(
        stored.inclusions.expect("kept").density.to_bits(),
        1.0f64.to_bits()
    );
    assert_eq!(
        stored.structure.expect("kept").coverage.to_bits(),
        1.0f64.to_bits()
    );
    assert_eq!(stored.edge_abrasion.width.to_bits(), 0.1f64.to_bits());
}

#[test]
fn the_physics_parts_survive_a_rebuild_from_the_app_controls() {
    let design_effects = effects_with_everything();
    let from_controls =
        StoneRenderEffects::from_render_values(0.5, true, 30.0, 120.0, None, false, 0.01);
    assert_eq!(from_controls.inclusions, None);
    let kept = from_controls.with_physics_of(&design_effects);
    assert_eq!(kept.inclusions, design_effects.inclusions);
    assert_eq!(kept.structure, design_effects.structure);
    assert_eq!(kept.edge_abrasion, design_effects.edge_abrasion);
    assert!(kept.c_axis_override_enabled);
}
