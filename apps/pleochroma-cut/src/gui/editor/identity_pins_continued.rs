#[test]
fn form_parsing_is_pinned() {
    let got = (
        fnv1a(tier_form_dump().as_bytes()),
        fnv1a(preform_and_material_dump().as_bytes()),
        fnv1a(material_search_dump().as_bytes()),
    );
    // The material dump lists every built-in by name and index, including the zoned Ametrine.
    // Re-pinned 2026-10-22: four new built-ins (Precious Opal (White), Precious Opal (Black),
    // Labradorite, Moonstone) joined the list (owner's run).
    // 2026-10-27: the material lists are alphabetical now (`builtin_preset_names` and
    // `design_material_options` sort by name, so every index in the dump names another material).
    // Re-pinned from the owner's Windows run 2026-10-27_010333: the SECOND hash moved with the
    // alphabetical lists; the THIRD (`material_search_dump`, RI -> nearest built-in) moved with
    // the 2026-10-26 color-review data refits (Peridot n_D 1.651_549, RI override also clearing
    // the axis dispersions). The search itself still ranks nearest first.
    // Re-pinned from the owner's Windows run 2026-11-04_154834: the SECOND hash moved because
    // Titanite (Sphene) joined the alphabetical built-in list on 2026-11-03 (every later index
    // names another material).
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

/// The scripted sequence: three tiers added, three coalescing nudges, undo, redo,
/// mirror, complete orbit -- through the same `EditorState` entry points the
/// desktop's callbacks use.
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
            // The pinned text is the `.psdd` design file text (`design_to_string`); re-pinned
            // 2026-11-02 for the rename: its header says `format = "pleochroma-design"` (files
            // tagged `"indicatrix-design"` still read).
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
    // Re-recorded 2026-10-05. Since 2026-10-04 (18:19, `cut_sheet::diagram::render_cut_diagram`)
    // the printed diagram stamps every facet's canonical label (`P1 0`, `C2 3`, ...,
    // `pleochroma_cut_core::design::labelling`) through `DiagramStyle::facet_labels`. The ink is
    // fixed-size 5x7 text at scale 1, so the standalone PNG (1800x720) grew by about 23-24 KB and
    // the sheet's embedded 900x360 PNG by about 21 KB, which base64 and the HTML text turn into the
    // ~28 KB the three sheets grew by; the sheet carries one diagram, not two. The facet planes did
    // not move: `solid_status_is_pinned` and the yield pins above still pass unchanged.
    //
    // Fixture 0 (the Standard Round Brilliant) is pinned once per platform. Two Windows runs
    // on 2026-10-05 (12:46, before the round-2 lanes, and 16:27, after them) gave the first
    // pair; the owner's workspace run of the same day gave the second pair for it and the SAME
    // bytes as Windows for fixtures 1 and 2. The two pairs differ by a few diagram pixels (the
    // standalone PNG by 5 bytes), which fits the platform split named in this file's module
    // doc: the facet normals go through `sin` and `cos`, glibc rounds a few arguments
    // differently from the Windows runtime, and a last-digit change moves a pixel or a label
    // only where a facet centre sits on a pixel boundary. By file times no source on the
    // sheet's path changed between the 16:27 run and the owner's, apart from the pure file
    // splits. Not proven: if a Windows run ever shows the second pair, the cause is a source
    // change and not the platform.
    //
    // Re-recorded 2026-10-06 (lanes L1 and L2), on both platforms. L1: the printed sheet
    // follows `Design::cutting_order()` for a planar design too (pavilion and girdle first,
    // table last), shows each tier's code in its label column with the descriptive name at
    // the front of the instruction (`Crown Main: Meet P1, P2, G1`), and prints index lists as
    // `96-08-16`, `03.50`, `-`; the three template fixtures store their tiers in cutting
    // order. L2: `write_cutting_sheet_html` is `cutting_sheet_document` with
    // `SheetDetails::from_design`, laid out like the fantasy-cut template: heading "Cutting
    // instructions", Facet Data, Size Data, Design Data, FOUR single-panel views (each
    // 560x440, instead of the one 900x360 three-panel picture), then a Pavilion and a Crown
    // section in place of the one tier table. The sheets therefore grew from about 109 KB to
    // about 186 KB. The standalone diagram PNG moved by its labels only (numbered per letter
    // in cutting order, and the table's label is `T`, no longer `Table`); the facet planes
    // did not move.
    //
    // Re-pinned 2026-10-27 (owner's Linux run; the `cfg(windows)` fixture-0 pair below was
    // re-pinned from the lead's Windows run the same day, fixtures 1 and 2 already matched the
    // Linux pins there). Cause: the R4-F layout
    // lane (2026-10-26, `pleochroma_solid::diagram2d`, `layout.rs`) gave every frame at least
    // `LARGE_FRAME_MIN_HEIGHT` (300 px) tall its own layout: the tick ring shrinks from 0.86 to
    // `LARGE_WHEEL_FRAC` (0.74) of the panel box, the caption moves from the top row to under its
    // panel, and the picture is boxed by 8 px gaps instead of a 6 %-of-height caption band. The
    // lane kept the small-frame pins safe, but the printed diagram (1800x720) and the sheet's four
    // single-panel views (560x440) are large frames too, so the pictures themselves changed:
    // the standalone PNGs got about 4 KB smaller and the sheets 6.0 to 6.7 KB (the base64 of the
    // four embedded PNGs); the facet planes did not move. Inferred from the code and the size
    // pattern, not bisected by a run.
    //
    // Windows fixture 0 re-pinned 2026-10-27 from the lead's workspace run: the same R4-F
    // diagram layout change. The sheet is 179_454 bytes, exactly the Linux length (the 2026-10-06
    // pair was 186_214), and the standalone diagram PNG is 141_288 bytes against Linux's 141_286.
    // Both hashes still differ from Linux's, so the Windows pictures are not byte-identical to
    // the Linux ones for this frame (presumably the platform math library in the layout, as for
    // the facet planes; not bisected). Fixtures 1 and 2 match the Linux pins.
    //
    // The three sheets re-pinned 2026-11-05 from the lead's Windows run (quick-release). Cause:
    // the cutting-instructions campaign of 2026-11-04 (lanes VP and M). The sheet's main part
    // is now the official fantasy-cut template: four inline SVG line drawings
    // (`pleochroma_solid::diagram2d::view_to_svg`), index 0 printed as the gear's tooth count,
    // and computed meets in the instruction column (`meet_instruction`). An "Additional
    // information" section follows, with the four shaded renderings at 1600x1257 (were 560x440),
    // the masts and the carat estimate. Those renderings draw their facet labels at the
    // frame's text scale (`DiagramStyle::label_scale`, 2 at 1257 px), like the tooth numbers.
    // The sheets grew from about 179 KB to about 600 KB. The standalone diagram PNGs did not
    // move (the raster diagram keeps `label_scale` 1), nor did the facet planes. The
    // `cfg(not(windows))` fixture-0 sheet entry was re-pinned from the owner's Linux run the
    // same day: 603_737 bytes, 16 fewer than Windows (the platform split above); fixtures 1
    // and 2 match Windows byte for byte.
    //
    // Re-pinned 2026-11-05 again from the owner's Linux run (the three sheets; the diagram
    // PNGs did not move). Two causes, both in the sheet's text:
    // - the HTML's screen presentation and extended additional information (lane Y: a
    //   screen-only bar, paper sheets on grey, a Stone summary, restyled masts and
    //   orientation; `cut_sheet/summary.rs`);
    // - the template convention in the instruction column (`Design::sizing_tier`,
    //   "Set girdle width.").
    // The template part's print layout is unchanged. The `cfg(windows)` fixture-0 sheet entry
    // is from the owner's Windows run the same day (`owner_runs/windows/2026-11-05_094501`):
    // 606_562 bytes, 16 more than Linux as before; that run gave the same fixture-1 and
    // fixture-2 sheets and the same tier-row hashes as Linux.
    #[cfg(windows)]
    let fixture_0: [(usize, u64); 2] = [
        (606_562, 11_736_902_947_696_155_507),
        (141_288, 11_413_831_736_985_919_569),
    ];
    #[cfg(not(windows))]
    let fixture_0: [(usize, u64); 2] = [
        (606_546, 588_139_891_447_678_699),
        (141_286, 2_017_440_248_910_210_068),
    ];
    assert_eq!(
        got,
        [
            fixture_0[0],
            fixture_0[1],
            (600_894, 8_452_203_179_921_548_134),
            (139_660, 9_075_363_107_119_437_507),
            (595_830, 11_078_173_895_711_783_295),
            (141_463, 343_495_985_279_007_824),
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
        // The default now follows the pavilion; `fixed` keeps the old "crown stays" rule
        // pinned too.
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
    // Re-recorded 2026-10-06 (Windows), three causes. Lane W1-RT (2026-10-05, deliberate
    // behaviour change): the Shift proposal no longer carries the table or the culet (a
    // retarget never tilts a flat facet, so the old table row that crossed to the pavilion
    // side is gone), a shifted angle is guarded (never past the horizontal, 1 to 89.5
    // degrees, with a note), crown rows read the crown-window margin and risk instead of the
    // pavilion formula, and the notes gained the flat-facet and guard lines. The dump above
    // therefore differs for every fixture that has a table. The Optimize half of the dump is
    // untouched.
    //
    // Lane L3 (2026-10-06, angles read positive, tiers read by code): the guard note names the
    // held angle as a magnitude (`held at 89.50°` for a pavilion row, never `-89.50°`), and a
    // relation note names its tiers by display name (code for an empty or old-style name).
    //
    // Lane L1 (2026-10-06, cutting order): the three template fixtures now store their tiers
    // in the order they are cut, so the proposal rows and the dump of the applied design list
    // the pavilion first and the table last.
    //
    // Lane RT1 (2026-10-07, deliberate behaviour change, re-recorded 2026-10-07): the default
    // crown policy follows the pavilion's stretch (crown rows move, a note names the stretch),
    // and `CrownShift::fixed()` joined the list, so the dump differs for every fixture.
    //
    // Re-pinned 2026-10-08 (Windows): the proposal's `target: ResolvedMaterial` holds a
    // `GemMaterial`, whose Debug dump gained the `absorption_unit` field; angles and scores are
    // unchanged.
    assert_eq!(got, RETARGET_DUMP_HASHES);
}

/// The dump with `GemMaterial`'s `zoning` field projected out.
///
/// The proposal's `target: ResolvedMaterial` holds a `GemMaterial`, which derives `Debug` with
/// `zoning` as its LAST field, so each material prints a trailing `, zoning: None }` that the
/// pinned constants predate. No other dumped type has a field of that name. Removing exactly
/// that text yields the pinned bytes. A material that really carries zones prints
/// `zoning: Some(..)`, stays in the dump and fails the comparison.
fn without_zoning_field(dump: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Owned(dump.replace(", zoning: None", ""))
}

/// The hashes of `retarget_output_is_pinned` (through [`without_zoning_field`]).
///
/// Re-pinned 2026-10-27 (lead's Windows workspace run): only the SECOND hash moved, from
/// `7_632_287_026_653_762_141` to `6_320_538_651_456_192_433`. The hashes are per FIXTURE (design),
/// not per target; the middle one is fixture 1 (template 1, girdle 6.5 mm, on the custom "My
/// Garnet"). Probable cause, inferred from the code and NOT confirmed by a run: that dump is the
/// only one that holds a `GemMaterial` `Debug` text. Fixture 1's own index (1.74, the custom
/// material) equals the "My Garnet" target's, so its Shift proposal moves nothing and is the one
/// the angle-only meet check accepts; the other two fixtures (1.54 and the 1.66 override) retarget
/// by a real shift and, as far as the unchanged hashes show, print only error texts.
/// `GemMaterial` (derived `Debug`) gained a trailing `inclusions: Option<InclusionSet>` field
/// after the 2026-10-08 Windows pin, and `without_zoning_field` strips only `, zoning: None`, so
/// every dumped material now ends `, inclusions: None }`. Ruled out: `MODEL_UNIT_FACE_UP_PATH`
/// 2.52 -> 2.83 (read only when a design has no girdle diameter, which fixture 1 has, and the
/// Optimize half prints only its error), the 2026-10-26 color refits (not Diamond, Sapphire or the
/// RI-2.0 gem, and they would move fixtures 0 and 2 too), the alphabetical lists (lookups are by
/// name) and `serde_json` `float_roundtrip` (no JSON in the dump).
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
        // The pin predates the tone fields of `ObjectiveWeights`: print the four it recorded,
        // in the Debug shape it recorded them in, so the hash below keeps guarding the parser.
        let line = match parse_optimize_weights(w, e, t, y, 0.0) {
            Ok(v) => format!(
                "Ok(ObjectiveWeights {{ windowing: {:?}, extinction: {:?}, tilt_brilliance: {:?}, yield_weight: {:?} }})",
                v.windowing, v.extinction, v.tilt_brilliance, v.yield_weight
            ),
            Err(message) => format!("Err({message:?})"),
        };
        let _ = writeln!(dump, "{line}");
    }
    // Re-recorded 2026-10-06 (lanes L3 and L1): `optimize_change_rows`
    // now shows both angles as magnitudes and the change as the difference of the two
    // magnitudes, so the sample change `-40.0 -> -41.5` that this dump held as
    // `"-40.00°"`, `"-41.50°"`, `"-1.50°"` is now `"40.00°"`, `"41.50°"`, `"+1.50°"`, and the
    // `30.0 -> 31.0` change keeps `"+1.00°"`. The tier names in the dump are unchanged for
    // this template (its tiers are named by code already) unless one is empty or old-style.
    // Lane L1 (2026-10-06, cutting order) adds a second cause: template 0 now stores its tiers
    // in the order they are cut (pavilion first, table last), so the sample change at index 1
    // names the Girdle where it named a different tier, and the preview design's tier list is
    // in the new order. Nothing else in the dump moved.
    // Re-recorded from the owner's 2026-10-19 run: preview now returns Result and
    // rejects these uncertified, stale sample changes instead of displaying a
    // partially edited design. The UI rows and weight-parser contract stay pinned.
    // 2026-10-27: the row and status text now print the 0-100 score (`100 - loss`,
    // `display_score`) instead of the loss, so this value moved; re-pinned from the owner's
    // Windows run 2026-10-26_234253 (nothing but the printed number changes in the dump).
    assert_eq!(fnv1a(dump.as_bytes()), 2_818_803_981_942_378_801);
}
