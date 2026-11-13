//! The `--optics-tables` flag of `render` and `tilt-video`: parsing, the refusal with the Ray
//! Traced engine and the help pages. The flag's effect (the process default of the tables) is
//! applied by the command and never by these tests, which only parse.

use super::{Command, RenderJobArgs, Topic, parse};
use crate::help;
use pleochroma::optics::tables::OpticsTables;

fn argv(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn render_args(line: &str) -> RenderJobArgs {
    match parse(&argv(line)) {
        Ok(Command::Render(args) | Command::TiltVideo(args)) => args,
        other => panic!("{line:?} should parse as a render command, got {other:?}"),
    }
}

fn error(line: &str) -> String {
    parse(&argv(line)).expect_err("this line should not parse")
}

#[test]
fn the_optics_tables_are_off_by_default() {
    for line in ["render a.job.json", "tilt-video v.job.json"] {
        assert_eq!(render_args(line).optics_tables, OpticsTables::Off, "{line}");
    }
}

#[test]
fn the_flag_turns_them_on_for_both_job_commands() {
    for line in [
        "render a.job.json --optics-tables",
        "tilt-video v.job.json --optics-tables",
    ] {
        assert_eq!(render_args(line).optics_tables, OpticsTables::On, "{line}");
    }
}

#[test]
fn the_flag_may_stand_anywhere_and_changes_nothing_else() {
    let plain = render_args("render a.job.json --quiet --sample-offset 16");
    let tabled = render_args("render --optics-tables a.job.json --quiet --sample-offset 16");
    assert_eq!(tabled.optics_tables, OpticsTables::On);
    assert_eq!(
        RenderJobArgs {
            optics_tables: OpticsTables::Off,
            ..tabled
        },
        plain
    );
}

#[test]
fn the_ray_traced_engine_does_not_use_the_tables() {
    for line in [
        "render a.job.json --optics-tables --engine ray",
        "tilt-video v.job.json --engine ray --optics-tables",
    ] {
        let message = error(line);
        assert!(message.contains("--optics-tables"), "{line}: {message}");
        assert!(message.contains("path-traced"), "{line}: {message}");
    }
    // Without the flag the Ray Traced engine is accepted with the tables off.
    assert!(parse(&argv("render a.job.json --engine ray")).is_ok());
}

#[test]
fn the_flag_belongs_to_the_job_commands_only() {
    for command in ["solve", "optimize", "info"] {
        let message = error(&format!("{command} --optics-tables"));
        assert!(!message.is_empty(), "{command}");
    }
}

#[test]
fn the_pages_describe_the_flag() {
    for topic in [Topic::Render, Topic::TiltVideo] {
        let page = help::text(topic);
        assert!(page.contains("[--optics-tables]"), "{topic:?} usage");
        assert!(
            page.contains("  --optics-tables         Read the Fresnel"),
            "{topic:?} lacks the entry"
        );
    }
}
