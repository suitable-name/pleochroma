use super::{Command, ImageCompareArgs, Topic, parse};
use std::path::PathBuf;

fn argv(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn parsed(line: &str) -> ImageCompareArgs {
    match parse(&argv(line)) {
        Ok(Command::ImageCompare(args)) => args,
        other => panic!("{line:?} should parse as image-compare, got {other:?}"),
    }
}

fn error(line: &str) -> String {
    parse(&argv(line)).expect_err("this line should not parse")
}

#[test]
fn two_images_are_the_whole_command() {
    let args = parsed("image-compare ref.plraw run.plraw");
    assert_eq!(args.reference, PathBuf::from("ref.plraw"));
    assert_eq!(args.run, PathBuf::from("run.plraw"));
    assert_eq!(args.ref2, None);
    assert_eq!(args.tonemap_from, None);
    assert_eq!(args.error_png, None);
    assert!(!args.json);
    assert_eq!(args.out, None);
}

#[test]
fn every_option_is_taken_and_may_stand_anywhere() {
    let args = parsed(
        "image-compare --ref2 ref2.plraw ref.plraw --tonemap-from a.job.json run.plraw \
         --error-png err.png --json --out report.json",
    );
    assert_eq!(args.reference, PathBuf::from("ref.plraw"));
    assert_eq!(args.run, PathBuf::from("run.plraw"));
    assert_eq!(args.ref2, Some(PathBuf::from("ref2.plraw")));
    assert_eq!(args.tonemap_from, Some(PathBuf::from("a.job.json")));
    assert_eq!(args.error_png, Some(PathBuf::from("err.png")));
    assert!(args.json);
    assert_eq!(args.out, Some(PathBuf::from("report.json")));
}

#[test]
fn both_images_are_required_and_no_more_than_two_are_taken() {
    for line in [
        "image-compare",
        "image-compare only.plraw",
        "image-compare --json only.plraw",
        "image-compare a.plraw b.plraw c.plraw",
    ] {
        let message = error(line);
        assert!(
            message.contains("exactly two image files"),
            "{line}: {message}"
        );
    }
}

#[test]
fn an_option_without_its_value_is_refused() {
    for flag in ["--ref2", "--tonemap-from", "--error-png", "--out"] {
        let message = error(&format!("image-compare a.plraw b.plraw {flag}"));
        assert_eq!(message, format!("{flag} needs a value (see --help)"));
    }
}

#[test]
fn an_unknown_option_names_the_command() {
    assert_eq!(
        error("image-compare a.plraw b.plraw --frob"),
        "unknown flag \"--frob\" for \"image-compare\" (see --help)"
    );
}

#[test]
fn help_wins_over_missing_images() {
    assert_eq!(
        parse(&argv("image-compare --help")),
        Ok(Command::Help(Topic::ImageCompare))
    );
    assert_eq!(
        parse(&argv("help image-compare")),
        Ok(Command::Help(Topic::ImageCompare))
    );
    assert!(
        crate::help::text(Topic::ImageCompare).starts_with("Usage: pleochroma-cli image-compare ")
    );
}
