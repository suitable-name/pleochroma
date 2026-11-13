//! Tests of the `snap-meets` command line: the limit, the output and the lines that are refused.

use super::{Command, SnapLimit, Topic, parse};
use pleochroma_cut_core::DEFAULT_SNAP_MAX_MOVE_FRACTION;
use std::path::PathBuf;

fn argv(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn parsed(line: &str) -> Command {
    parse(&argv(line)).unwrap_or_else(|e| panic!("{line:?} should parse: {e}"))
}

fn error(line: &str) -> String {
    parse(&argv(line)).expect_err("this line should not parse")
}

#[test]
fn snap_meets_defaults_to_the_shared_fraction_of_the_stone() {
    let Command::SnapMeets(args) = parsed("snap-meets a.asc --out a.psdd") else {
        panic!("expected snap-meets");
    };
    assert_eq!(args.design, PathBuf::from("a.asc"));
    assert_eq!(args.out, Some(PathBuf::from("a.psdd")));
    assert_eq!(
        args.limit,
        SnapLimit::Fraction(DEFAULT_SNAP_MAX_MOVE_FRACTION)
    );
    assert!(!args.dry_run);
    assert_eq!(args.db, None);
}

#[test]
fn snap_meets_takes_a_limit_in_millimeters_or_as_a_fraction() {
    let Command::SnapMeets(mm) = parsed("snap-meets a.psdd --max-move 0.15 --out b.psdd") else {
        panic!("expected snap-meets");
    };
    assert_eq!(mm.limit, SnapLimit::Millimeters(0.15));
    let Command::SnapMeets(fraction) =
        parsed("snap-meets a.psdd --max-move-fraction 0.05 --out b.psdd --db lib.sqlite")
    else {
        panic!("expected snap-meets");
    };
    assert_eq!(fraction.limit, SnapLimit::Fraction(0.05));
    assert_eq!(fraction.db, Some(PathBuf::from("lib.sqlite")));
}

#[test]
fn snap_meets_refuses_both_limits_and_limits_out_of_range() {
    assert_eq!(
        error("snap-meets a.asc --out b.psdd --max-move 0.1 --max-move-fraction 0.02"),
        "give one of --max-move MM and --max-move-fraction F, not both"
    );
    assert!(error("snap-meets a.asc --out b.psdd --max-move 0").contains("above 0"));
    assert!(error("snap-meets a.asc --out b.psdd --max-move -0.1").contains("above 0"));
    assert!(error("snap-meets a.asc --out b.psdd --max-move-fraction 0").contains("above 0"));
    assert!(error("snap-meets a.asc --out b.psdd --max-move-fraction 1.5").contains("at most 1"));
    assert!(error("snap-meets a.asc --out b.psdd --max-move far").contains("needs a number"));
    assert_eq!(
        error("snap-meets a.asc --out b.psdd --max-move"),
        "--max-move needs a value (see --help)"
    );
}

#[test]
fn snap_meets_needs_an_output_unless_it_is_a_dry_run() {
    let message = error("snap-meets a.asc");
    assert!(message.contains("needs --out FILE.psdd"), "{message}");
    assert!(message.contains("--dry-run"), "{message}");
    let Command::SnapMeets(dry) = parsed("snap-meets a.asc --dry-run") else {
        panic!("expected snap-meets");
    };
    assert!(dry.dry_run);
    assert_eq!(dry.out, None);
    // A dry run with --out is taken, and writes nothing (the command's test checks that).
    let Command::SnapMeets(both) = parsed("snap-meets a.asc --dry-run --out b.psdd") else {
        panic!("expected snap-meets");
    };
    assert!(both.dry_run);
    assert_eq!(both.out, Some(PathBuf::from("b.psdd")));
    assert!(error("snap-meets --out b.psdd").contains("needs a design file"));
    assert_eq!(
        error("snap-meets a.asc --dry-run --frob"),
        "unknown flag \"--frob\" for \"snap-meets\" (see --help)"
    );
}

#[test]
fn snap_meets_takes_several_designs_with_an_out_dir() {
    let Command::Batch(batch) = parsed("snap-meets a.asc sub/b.gem --out-dir out") else {
        panic!("expected a batch");
    };
    let outs: Vec<Option<PathBuf>> = batch
        .items
        .iter()
        .map(|item| match item {
            Command::SnapMeets(args) => args.out.clone(),
            other => panic!("expected snap-meets, got {other:?}"),
        })
        .collect();
    assert_eq!(
        outs,
        [
            Some(PathBuf::from("out").join("a.psdd")),
            Some(PathBuf::from("out").join("b.psdd"))
        ]
    );
    let message = error("snap-meets a.asc b.asc --out r.psdd");
    assert!(
        message.starts_with("--out names one file, but 2 design files"),
        "{message}"
    );
    // A dry run of several designs needs no folder.
    let Command::Batch(dry) = parsed("snap-meets a.asc b.asc --dry-run") else {
        panic!("expected a batch");
    };
    assert_eq!(dry.items.len(), 2);
}

#[test]
fn snap_meets_has_a_help_page_and_is_a_known_command() {
    assert_eq!(parsed("snap-meets --help"), Command::Help(Topic::SnapMeets));
    assert_eq!(parsed("help snap-meets"), Command::Help(Topic::SnapMeets));
    assert_eq!(Topic::of("snap-meets"), Topic::SnapMeets);
    let message = error("snap-meet a.asc");
    assert!(message.contains("snap-meets"), "{message}");
}
