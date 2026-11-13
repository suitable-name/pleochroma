#[test]
fn form_parsing_is_pinned() {
    let got = (
        fnv1a(tier_form_dump().as_bytes()),
        fnv1a(preform_and_material_dump().as_bytes()),
        fnv1a(material_search_dump().as_bytes()),
    );
    // The material dump lists every built-in by name and index, including the zoned Ametrine.
    // The lists are sorted by name (`builtin_preset_names`, `design_material_options`), so
    // every index names one material: a new built-in shifts the indices after it and moves the
    // second hash. The search ranks the nearest material first.
    assert_eq!(
        got,
        (
            1_545_794_316_997_941_305,
            3_954_546_335_237_923_982,
            12_801_622_518_707_986_904
        )
    );
}

fn nudge(state: &mut EditorState, index: usize, delta: f64, out: &mut String) {
    let current = state.design.tiers[index].angle_deg;
    let outcome = state.apply_coalescing(
        Edit::RetargetAngles {
            changes: vec![(index, current, current + delta)],
        },
        angle_nudge_coalesce_key(&[index]),
    );
    let _ = writeln!(out, "nudge {outcome:?}");
}

/// The scripted sequence: tiers added, coalescing nudges, undo, redo, mirror and a complete
/// orbit.
///
/// It goes through the same `EditorState` entry points the desktop's callbacks use.
fn scripted_sequence() -> (String, String) {
    let mut log = String::new();
    let mut state = EditorState::fresh();
    let eight = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    for (index, t) in [
        tier("G1", 90.0, &eight, MeetConstraint::ScaleReference(1.0)),
        tier(
            "P1",
            -40.0,
            &eight,
            MeetConstraint::MeetNamed(vec!["G1".to_string()]),
        ),
        tier(
            "C1",
            34.5,
            &[0.0, 12.0, 24.0],
            MeetConstraint::MeetNamed(vec!["G1".to_string()]),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let outcome = state.apply(Edit::AddTier { index, tier: t });
        let _ = writeln!(log, "add {outcome:?}");
    }
    for _ in 0..3 {
        nudge(&mut state, 1, -0.25, &mut log);
    }
    let _ = writeln!(log, "undo {:?}", state.undo());
    let _ = writeln!(log, "redo {:?}", state.redo());
    let mirrored = state
        .design
        .mirror_indices(2)
        .and_then(|edit| state.apply(edit));
    let _ = writeln!(log, "mirror {mirrored:?}");
    let units = state.design.orbit_units(2);
    let anchors: Vec<f64> = units
        .iter()
        .flatten()
        .filter(|unit| !unit.is_complete())
        .filter_map(|unit| unit.members.first().copied())
        .collect();
    for position in anchors {
        let outcome = state
            .design
            .add_orbit_member(2, position)
            .and_then(|edit| state.apply(edit));
        let _ = writeln!(log, "orbit {position} {outcome:?}");
    }
    let toml = design_to_string(&state.design, None, &DesignExtras::default())
        .unwrap_or_else(|e| e.to_string());
    let generation = state.generation.load(std::sync::atomic::Ordering::Relaxed);
    let _ = writeln!(log, "generation {generation} dirty {}", state.is_dirty());
    let mut depth = 0;
    while matches!(state.undo(), Ok(true)) {
        depth += 1;
    }
    let mut redo_depth = 0;
    while matches!(state.redo(), Ok(true)) {
        redo_depth += 1;
    }
    let replayed = design_to_string(&state.design, None, &DesignExtras::default())
        .unwrap_or_else(|e| e.to_string());
    let _ = writeln!(
        log,
        "depth {depth} redo {redo_depth} replay_equal {}",
        replayed == toml
    );
    (toml, log)
}

#[test]
fn scripted_edit_sequence_is_pinned() {
    let (toml, log) = scripted_sequence();
    assert_eq!(
        (
            fnv1a(toml.as_bytes()),
            toml.len(),
            fnv1a(log.as_bytes()),
            log.as_str()
        ),
        (
            // The pinned text is the `.psdd` design file (`design_to_string`), whose header is
            // `format = "pleochroma-design"`.
            10_873_386_827_430_338_000,
            945,
            6_969_999_423_672_735_711,
            "add Ok(())\nadd Ok(())\nadd Ok(())\nnudge Ok(())\nnudge Ok(())\nnudge Ok(())\n\
             undo Ok(true)\nredo Ok(true)\nmirror Ok(())\norbit -0 Ok(())\n\
             generation 10 dirty true\ndepth 6 redo 6 replay_equal true\n"
        )
    );
}

#[test]
#[ignore = "over 60 s in the workspace run (2026-11-08); run on request: scripts/run_tests_on_request.sh"]
fn cutting_sheet_and_diagram_bytes_are_pinned() {
    let customs = custom_materials();
    let dir =
        std::env::temp_dir().join(format!("pleochroma-cut-editor-pins-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut got: Vec<(usize, u64)> = Vec::new();
    for (i, design) in fixtures().iter().take(3).enumerate() {
        let s = solved(design).expect("templates solve");
        let html = dir.join(format!("sheet{i}.html"));
        write_cutting_sheet_html(design, &s, &html, &customs).unwrap();
        let bytes = std::fs::read(&html).unwrap();
        got.push((bytes.len(), fnv1a(&bytes)));
        let png = dir.join(format!("diagram{i}.png"));
        write_diagram_png(design, &s, &png).unwrap();
        let bytes = std::fs::read(&png).unwrap();
        got.push((bytes.len(), fnv1a(&bytes)));
    }
    let _ = std::fs::remove_dir_all(&dir);
    // Fixture 0 has one pin per platform: its facet normals and diagram layout go through the
    // platform math library, so a last-digit difference can move a pixel or a label. The
    // sheets of fixtures 1 and 2 are the same bytes on both platforms.
    #[cfg(windows)]
    let fixture_0: [(usize, u64); 2] = [
        (606_562, 11_736_902_947_696_155_507),
        (141_288, 11_413_831_736_985_919_569),
    ];
    // TODO: re-pin the `cfg(windows)` fixture-0 pair from a Windows run (it predates the
    // meet-point changes).
    #[cfg(not(windows))]
    let fixture_0: [(usize, u64); 2] = [
        (582_647, 16_733_543_605_631_979_729),
        (132_123, 10_401_093_412_681_418_834),
    ];
    assert_eq!(
        got,
        [
            fixture_0[0],
            fixture_0[1],
            (579_656, 4_642_195_192_289_503_140),
            (129_953, 9_602_304_345_387_847_692),
            (596_089, 4_230_517_347_391_912_512),
            (134_764, 4_111_638_237_344_875_132),
        ]
    );
}

fn resolved(
    selection: &MaterialSelection,
    customs: &[GemMaterial],
) -> pleochroma_cut_core::ResolvedMaterial {
    let lookup = EditorMaterialLookup::new(customs);
    let resolved = selection.resolve(&lookup);
    let gem = resolved_gem_material(selection, &lookup);
    pleochroma_cut_core::ResolvedMaterial { gem, ..resolved }
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "pinned to the Windows math library's rounding in the proposal angles and the Optimize scoring"
)]
fn retarget_output_is_pinned() {
    let customs = custom_materials();
    let targets = [
        MaterialSelection {
            name: Some("Sapphire".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        },
        MaterialSelection {
            name: Some("My Garnet".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        },
        MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: Some(2.0),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        },
    ];
    let crowns = [
        // The default follows the pavilion; `fixed` pins the "crown stays" rule too.
        CrownShift::default(),
        CrownShift::fixed(),
        CrownShift {
            fraction: 0.5,
            ..CrownShift::fixed()
        },
        CrownShift {
            scale_by_ratio: true,
            ..CrownShift::fixed()
        },
    ];
    let mut got = Vec::new();
    for design in fixtures().iter().take(3) {
        let mut dump = String::new();
        for target in &targets {
            let target = resolved(target, &customs);
            for crown in crowns {
                match retarget::build_proposal(
                    design,
                    &target,
                    crown,
                    RetargetMode::Shift,
                    &customs,
                ) {
                    Ok(proposal) => {
                        let _ = writeln!(
                            dump,
                            "{proposal:?} {:?}",
                            retarget::apply(design, &proposal)
                        );
                    }
                    Err(e) => {
                        let _ = writeln!(dump, "{e}");
                    }
                }
            }
            let optimize = retarget::build_proposal(
                design,
                &target,
                CrownShift::default(),
                RetargetMode::Optimize(pleochroma_cut_core::OptimizeConfig::default()),
                &customs,
            );
            let _ = writeln!(dump, "{:?}", optimize.err());
        }
        got.push(fnv1a(without_zoning_field(&dump).as_bytes()));
    }
    // Each fixture's hash covers the Shift proposals with their applied designs, and the
    // Optimize error text. The guard note names a held angle as a magnitude, and a relation
    // note names its tiers by display name.
    assert_eq!(got, RETARGET_DUMP_HASHES);
}

/// The dump with `GemMaterial`'s `zoning` field projected out.
///
/// The proposal's `target: ResolvedMaterial` holds a `GemMaterial`, whose derived `Debug` prints
/// `, zoning: None` for a material without zones (`inclusions` follows it). No other dumped type
/// has a field of that name. Removing exactly that text yields the pinned bytes. A material that
/// really carries zones prints `zoning: Some(..)`, stays in the dump and fails the comparison.
fn without_zoning_field(dump: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(dump.replace(", zoning: None", ""))
}

/// The hashes of `retarget_output_is_pinned` (through [`without_zoning_field`]).
///
/// The hashes of `retarget_output_is_pinned`, one per fixture (not per target).
///
/// The middle one is fixture 1 (template 1, girdle 6.5 mm, on the custom "My Garnet"). Dumped
/// materials end with `, inclusions: None }`, after [`without_zoning_field`] has run.
const RETARGET_DUMP_HASHES: [u64; 3] = [
    5_470_440_730_861_629_630,
    6_320_538_651_456_192_433,
    16_840_339_363_415_434_246,
];

fn sample_outcome(cancelled: bool, polish: usize) -> OptimizeOutcome {
    OptimizeOutcome {
        provenance: None,
        before: ObjectiveComponents {
            windowing_pct: 12.5,
            extinction_pct: 8.0,
            tilt_brilliance_pct: 60.0,
        },
        before_score: 20.0,
        before_yield_loss_pct: 30.0,
        after: ObjectiveComponents {
            windowing_pct: 9.25,
            extinction_pct: 11.0,
            tilt_brilliance_pct: 60.0,
        },
        after_score: 15.0,
        after_yield_loss_pct: 31.5,
        evaluations: 42,
        changes: vec![
            AngleChange {
                index: 1,
                from_deg: -40.0,
                to_deg: -41.5,
            },
            AngleChange {
                index: 9,
                from_deg: 30.0,
                to_deg: 31.0,
            },
        ],
        cancelled,
        polish_evaluations: polish,
        polish_improvement: 0.42,
    }
}

#[test]
fn optimize_views_are_pinned() {
    let design = template_design(0);
    let mut dump = String::new();
    for (cancelled, polish) in [(false, 0), (true, 31)] {
        let outcome = sample_outcome(cancelled, polish);
        let _ = writeln!(
            dump,
            "{:?} {} {:?}",
            optimize_result_rows(&outcome),
            optimize_status_text(&outcome),
            optimize_change_rows(&outcome, &design)
        );
        let preview = build_optimize_preview_design(&design, &outcome);
        assert!(
            preview.is_err(),
            "uncertified sample changes cannot be previewed"
        );
        let _ = writeln!(dump, "{:?}", preview.map(|d| d.tiers));
    }
    let mut empty = sample_outcome(false, 0);
    empty.changes.clear();
    let _ = writeln!(dump, "{}", optimize_status_text(&empty));
    for (w, e, t, y) in [
        ("1", "2.5", "0", 0.0),
        ("x", "1", "1", 0.4),
        ("1", "-0.5", "1", 0.4),
        ("NaN", "1", "1", 0.0),
    ] {
        // Prints the four weights of the original pin in its Debug shape (the tone fields are
        // left out), so the hash keeps guarding the parser.
        let line = match parse_optimize_weights(w, e, t, y, 0.0) {
            Ok(v) => format!(
                "Ok(ObjectiveWeights {{ windowing: {:?}, extinction: {:?}, tilt_brilliance: {:?}, yield_weight: {:?} }})",
                v.windowing, v.extinction, v.tilt_brilliance, v.yield_weight
            ),
            Err(message) => format!("Err({message:?})"),
        };
        let _ = writeln!(dump, "{line}");
    }
    // `optimize_change_rows` shows both angles as magnitudes and the change as the difference
    // of the two magnitudes. The preview is a `Result`: uncertified sample changes are
    // rejected.
    assert_eq!(fnv1a(dump.as_bytes()), 2_818_803_981_942_378_801);
}
