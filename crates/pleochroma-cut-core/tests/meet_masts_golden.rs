//! Golden masts: today's solved meet masts, pinned bit for bit.
//!
//! Every built-in template and every `.asc` fixture the crate's own tests load is solved with
//! [`Design::solve`], and each tier's mast is recorded as the hexadecimal bit pattern of its
//! `f64`. The reader test solves the same designs again and compares the result against the
//! recorded file, so a change to the meet solver that alters any solve without a pick shows up as
//! a differing bit, not as a tolerance that happened to hold.
//!
//! The recorded file is `tests/data/meet_masts_golden_0_11_0.txt`. Each line holds
//! `name`, `tier` and `mast bits`, separated by tabs. A design whose solve fails writes one line
//! with `-` as the tier and the error text in place of the bits, so a design that stops solving
//! (or starts solving) differs from the record too.
//!
//! To record the file, run the writer once on a tree whose solver is the one to pin:
//!
//! ```text
//! cargo test -p pleochroma-cut-core --test meet_masts_golden -- --ignored write_meet_masts_golden
//! ```
//!
//! The reader test skips with a message while the file does not exist.

use pleochroma::geometry::meet_solver::{
    Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc,
};
use pleochroma_cut_core::{
    ConstraintTier, Design, PreformSpec, ScheduleMeta, templates::TEMPLATES,
};
use std::{fmt::Write as _, fs, path::PathBuf};

/// Pentalume, the published design.
const PENTALUME: &str = include_str!("../src/design/girdle_level/pentalume.asc");

/// Cam's Portuguese, verbatim.
const CAMS_PORTUGUESE: &str = include_str!("../src/design/girdle_level/cams_portuguese.asc");

/// Iris, the published design.
const IRIS: &str = include_str!("../src/design/girdle_level/iris.asc");

/// The 103-tier `CrackOtto` step cut.
const CRACKOTTO_STEP: &str = include_str!("../src/optimize_cost_probe_crackotto_step.asc");

/// "RBC-445", 12 tiers with named meet references.
const RBC_445: &str = "GemCad 5.0\ng 96 0.0\ny 3 y\nI 1.54\n\
     H PC 13.156  RBC-445\n\
     H Richard B Conley, Facets, Oct 2013 p10\n\
     a -50.400000 0.61624001 92 n 1 68 60 36 28 4 G TCP\n\
     a -48.900000 0.63552822 88 n 2 72 56 40 24 8 G TCP\n\
     a -90.000000 0.91252100 92 n 3 68 60 36 28 4\n\
     a -90.000000 0.96225045 88 n 4 72 56 40 24 8\n\
     a -47.200000 0.59908304 95 n 5 65 63 33 31 1\n\
     a -43.000000 0.64485570 86 n 6 74 54 42 22 10 G PCP\n\
     a -47.266965 0.63949242 87 n 7 73 55 41 23 9\n\
     a 31.000000 0.62509296 4 n A 28 36 60 68 92\n\
     a 29.000000 0.62477630 8 n B 24 40 56 72 88\n\
     a 28.100000 0.60078994 2 n C 30 34 62 66 94\n\
     a 20.940747 0.56272062 14 n D 18 46 50 78 82\n\
     a 0.000000 0.40674031 96 n E\n";

/// The five-tier round brilliant of `tests/surface.rs`.
const SURFACE_ROUND: &str = "GemCad 5.0\n\
     g 16 0.0\n\
     y 4 n\n\
     I 1.54\n\
     a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
     a -42.000000 0.60000000 0 4 8 12 P1 Set stone size\n\
     a -38.000000 0.55000000 2 6 10 14 P2 Set stone size\n\
     a 32.000000 0.45000000 0 4 8 12 C1 Set stone size\n\
     a 40.000000 0.40000000 2 6 10 14 C2 Set stone size\n";

/// The five-tier round brilliant of `tests/edit_roundtrip.rs`.
const EDIT_ROUND: &str = "GemCad 5.0\n\
     g 16 0.0\n\
     y 4 n\n\
     I 1.54\n\
     a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
     a -42.000000 0.60000000 0 4 8 12 G Set stone size\n\
     a -38.000000 0.55000000 2 6 10 14 G Set stone size\n\
     a 32.000000 0.45000000 0 4 8 12 G Set stone size\n\
     a 40.000000 0.40000000 2 6 10 14 G Set stone size\n";

/// The recorded file, `tests/data/meet_masts_golden_0_11_0.txt` under the crate root.
fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("meet_masts_golden_0_11_0.txt")
}

/// `text` as a design with every mast pinned as the file records it.
fn pinned_design(text: &str) -> Design {
    let schedule = pleochroma_formats::asc::parse_asc(text).expect("the fixture parses");
    Design::from_asc_schedule(PreformSpec::block(3.0, 1.0, 4.0), &schedule)
}

/// `text` as a design that keeps the file's own meet references, with one bootstrapped
/// `ScaleReference` for each crown, pavilion and girdle block that the file does not anchor.
fn meet_design(text: &str) -> Design {
    let schedule = pleochroma_formats::asc::parse_asc(text).expect("the fixture parses");
    let mut inputs = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

/// Every design the golden covers, as `(name, design)` in a fixed order: the built-in templates
/// in table order, then the fixtures.
fn golden_designs() -> Vec<(String, Design)> {
    let mut designs: Vec<(String, Design)> = TEMPLATES
        .iter()
        .map(|spec| {
            (
                format!("template: {}", spec.name),
                Design::new(spec.preform.clone(), spec.schedule_meta(), spec.tiers()),
            )
        })
        .collect();
    for (name, text) in [
        ("pinned: pentalume", PENTALUME),
        ("pinned: cams_portuguese", CAMS_PORTUGUESE),
        ("pinned: iris", IRIS),
        ("pinned: crackotto_step", CRACKOTTO_STEP),
        ("pinned: rbc_445", RBC_445),
        ("pinned: surface_round", SURFACE_ROUND),
        ("pinned: edit_round", EDIT_ROUND),
    ] {
        designs.push((name.to_string(), pinned_design(text)));
    }
    for (name, text) in [
        ("meet: pentalume", PENTALUME),
        ("meet: cams_portuguese", CAMS_PORTUGUESE),
        ("meet: iris", IRIS),
        ("meet: crackotto_step", CRACKOTTO_STEP),
        ("meet: rbc_445", RBC_445),
    ] {
        designs.push((name.to_string(), meet_design(text)));
    }
    designs
}

/// The golden text for the current solver: one `name`, `tier`, `mast bits` line per solved tier.
fn golden_text() -> String {
    let mut text = String::new();
    for (name, design) in golden_designs() {
        match design.solve() {
            Ok(solved) => {
                for (tier, solved_tier) in solved.iter().enumerate() {
                    writeln!(text, "{name}\t{tier}\t{:016x}", solved_tier.mast.to_bits())
                        .expect("writing to a String cannot fail");
                }
            }
            Err(error) => {
                writeln!(text, "{name}\t-\tsolve error: {error}")
                    .expect("writing to a String cannot fail");
            }
        }
    }
    text
}

/// Writes the golden file from the current solver. Run it once on the tree whose masts are to be
/// pinned; the reader test then guards every later change.
#[test]
#[ignore = "writes tests/data/meet_masts_golden_0_11_0.txt; run it once on the unchanged solver"]
fn write_meet_masts_golden() {
    let path = golden_path();
    let dir = path.parent().expect("the golden path has a directory");
    fs::create_dir_all(dir).expect("the data directory can be created");
    let text = golden_text();
    assert!(!text.is_empty(), "no design was solved");
    fs::write(&path, text).expect("the golden file can be written");
}

/// Every golden design solves to the recorded masts, bit for bit.
#[test]
fn solved_masts_match_the_golden_bit_for_bit() {
    let path = golden_path();
    let Ok(recorded) = fs::read_to_string(&path) else {
        eprintln!(
            "skipped: {} does not exist yet; run the ignored test `write_meet_masts_golden` once \
             on the unchanged solver to record it",
            path.display()
        );
        return;
    };
    let current = golden_text();
    let recorded_lines: Vec<&str> = recorded.lines().collect();
    let current_lines: Vec<&str> = current.lines().collect();
    let mut differences = Vec::new();
    for line in 0..recorded_lines.len().max(current_lines.len()) {
        let want = recorded_lines.get(line).copied().unwrap_or("<missing>");
        let got = current_lines.get(line).copied().unwrap_or("<missing>");
        if want != got {
            differences.push(format!(
                "line {}: recorded `{want}`, solved `{got}`",
                line + 1
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "{} of {} golden lines differ:\n{}",
        differences.len(),
        recorded_lines.len(),
        differences
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
