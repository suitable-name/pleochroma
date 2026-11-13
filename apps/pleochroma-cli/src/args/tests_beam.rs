//! The beam-tracing flags of `render` and `tilt-video`: parsing, the settings they stand for, the
//! ranges, the refusals and the help pages.

use super::{
    BeamArgs, Command, RenderJobArgs, Topic, parse,
    types::{BEAM_PERSISTENCE_AVAILABLE, keep_beams_refusal},
};
use crate::help;
use pleochroma::renderer::{
    accel::RenderAccel,
    accel_beam::{BeamSettings, RegionMode},
};

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
fn beam_tracing_is_off_by_default() {
    for line in ["render a.job.json", "tilt-video v.job.json"] {
        let args = render_args(line);
        assert_eq!(args.beam, BeamArgs::default(), "{line}");
        assert_eq!(args.beam.settings(), None, "{line}");
    }
}

#[test]
fn beam_alone_turns_it_on_with_the_default_settings() {
    for line in ["render a.job.json --beam", "tilt-video v.job.json --beam"] {
        let args = render_args(line);
        assert!(args.beam.enabled, "{line}");
        assert_eq!(
            args.beam.settings(),
            Some(BeamSettings::default()),
            "{line}"
        );
    }
}

#[test]
fn each_override_is_read_and_implies_beam() {
    let spp = render_args("render a.job.json --beam-residual-min-spp 32").beam;
    assert!(spp.enabled);
    assert_eq!(spp.residual_min_spp, Some(32));
    let tiles = render_args("render a.job.json --beam-batch-tiles 8").beam;
    assert!(tiles.enabled);
    assert_eq!(tiles.batch_tiles, Some(8));
    let mode = render_args("tilt-video v.job.json --beam-region-mode per-node").beam;
    assert!(mode.enabled);
    assert_eq!(mode.region_mode, Some(RegionMode::PerNode));
    let nodes = render_args("render a.job.json --beam-nodes 96").beam;
    assert!(nodes.enabled);
    assert_eq!(nodes.nodes, Some(96));
}

#[test]
fn the_overrides_change_only_their_own_setting() {
    let settings = render_args(
        "render a.job.json --beam --beam-residual-min-spp 128 --beam-batch-tiles 16 \
         --beam-region-mode per-node --beam-nodes 64",
    )
    .beam
    .settings()
    .expect("beam tracing is on");
    assert_eq!(
        settings,
        BeamSettings {
            spectral_nodes: 64,
            residual_min_spp: 128,
            region_mode: RegionMode::PerNode,
            batch_tiles: 16,
            ..BeamSettings::default()
        }
    );
    let one = render_args("render a.job.json --beam-nodes 8")
        .beam
        .settings()
        .expect("beam tracing is on");
    assert_eq!(
        one,
        BeamSettings {
            spectral_nodes: 8,
            ..BeamSettings::default()
        }
    );
}

#[test]
fn the_flags_win_over_the_jobs_own_setting() {
    // The flags build the whole setting from the defaults, so a job's `accel.beam` never leaks
    // into a render that was asked for on the command line; without flags `settings` is `None`
    // and the job's own applies.
    let job = RenderAccel {
        beam: Some(BeamSettings {
            spectral_nodes: 96,
            ..BeamSettings::default()
        }),
        ..RenderAccel::default()
    };
    let flagged = render_args("render a.job.json --beam").beam.settings();
    assert_eq!(flagged, Some(BeamSettings::default()));
    assert_ne!(flagged, job.beam);
    assert_eq!(render_args("render a.job.json").beam.settings(), None);
}

#[test]
fn the_flags_may_stand_anywhere_and_combine_with_the_others() {
    let args = render_args(
        "render --beam-nodes 16 a.job.json --path-cache --beam --adaptive --quiet --optics-tables",
    );
    assert_eq!(args.job, std::path::PathBuf::from("a.job.json"));
    assert_eq!(args.beam.nodes, Some(16));
    assert!(args.path_cache.enabled);
    assert!(args.adaptive.enabled);
    assert!(args.quiet);
}

#[test]
fn bad_values_are_refused_with_the_flag_named() {
    for line in [
        "render a.job.json --beam-residual-min-spp 0",
        "render a.job.json --beam-residual-min-spp 65537",
        "render a.job.json --beam-residual-min-spp -4",
        "render a.job.json --beam-residual-min-spp many",
        "render a.job.json --beam-batch-tiles 0",
        "render a.job.json --beam-batch-tiles 4097",
        "render a.job.json --beam-batch-tiles 1.5",
        "render a.job.json --beam-nodes 0",
        "render a.job.json --beam-nodes 12",
        "render a.job.json --beam-nodes 4",
        "render a.job.json --beam-nodes 264",
        "render a.job.json --beam-nodes 47.5",
        "render a.job.json --beam-region-mode fuzzy",
    ] {
        let message = error(line);
        let flag = line.split_whitespace().nth(2).expect("the line has a flag");
        assert!(message.contains(flag), "{line}: {message}");
    }
    // The ends of the ranges are fine.
    for line in [
        "render a.job.json --beam-residual-min-spp 1",
        "render a.job.json --beam-residual-min-spp 65536",
        "render a.job.json --beam-batch-tiles 1",
        "render a.job.json --beam-batch-tiles 4096",
        "render a.job.json --beam-nodes 8",
        "render a.job.json --beam-nodes 256",
        "render a.job.json --beam-region-mode end-intersection",
    ] {
        assert!(parse(&argv(line)).is_ok(), "{line}");
    }
}

#[test]
fn each_override_needs_its_value() {
    for flag in [
        "--beam-residual-min-spp",
        "--beam-batch-tiles",
        "--beam-region-mode",
        "--beam-nodes",
    ] {
        let message = error(&format!("render a.job.json {flag}"));
        assert!(message.contains(flag), "{message}");
        assert!(message.contains("needs a value"), "{message}");
    }
}

#[test]
fn the_ray_traced_engine_has_no_residual_for_beams() {
    for line in [
        "render a.job.json --beam --engine ray",
        "render a.job.json --engine ray --beam",
        "tilt-video v.job.json --engine ray --beam-nodes 16",
        "render a.job.json --beam-batch-tiles 4 --engine ray",
    ] {
        let message = error(line);
        assert!(message.contains("--beam"), "{line}: {message}");
        assert!(message.contains("Path-Traced"), "{line}: {message}");
    }
    // Without the beam flags the Ray Traced engine is accepted with no beam tracing.
    assert!(parse(&argv("render a.job.json --engine ray")).is_ok());
    // The path-traced engine, spelled out, is fine.
    assert!(
        render_args("render a.job.json --engine path --beam")
            .beam
            .enabled
    );
}

#[test]
fn the_flags_belong_to_the_job_commands_only() {
    for command in ["solve", "optimize"] {
        assert!(!error(&format!("{command} --beam")).is_empty(), "{command}");
    }
}

#[test]
fn the_pages_describe_the_beam_flags() {
    for topic in [Topic::Render, Topic::TiltVideo] {
        let page = help::text(topic);
        for flag in [
            "--beam",
            "--beam-residual-min-spp",
            "--beam-batch-tiles",
            "--beam-region-mode",
            "--beam-nodes",
            "end-intersection|per-node",
        ] {
            assert!(page.contains(flag), "{topic:?} page lacks {flag}");
        }
        assert!(
            page.contains("[--beam] [--beam-residual-min-spp N] [--beam-batch-tiles N]"),
            "{topic:?} usage lacks them"
        );
    }
}

#[test]
fn keep_beams_turns_beam_tracing_on_and_asks_for_the_kept_leaves() {
    for line in [
        "render a.job.json --keep-beams",
        "tilt-video v.job.json --keep-beams",
    ] {
        let args = render_args(line);
        assert!(args.beam.enabled, "{line}");
        assert!(args.beam.keep, "{line}");
        assert_eq!(
            args.beam.settings(),
            Some(BeamSettings {
                keep_leaves: true,
                ..BeamSettings::default()
            }),
            "{line}"
        );
    }
    // Nothing is kept unless the flag is given, whatever else is.
    let plain = render_args("render a.job.json --beam --beam-nodes 16");
    assert!(!plain.beam.keep);
    assert!(
        !plain
            .beam
            .settings()
            .expect("beam tracing is on")
            .keep_leaves
    );
    // It stands anywhere and combines with the overrides.
    let combined = render_args("render --keep-beams a.job.json --beam-nodes 16 --path-cache");
    assert_eq!(
        combined.beam.settings(),
        Some(BeamSettings {
            spectral_nodes: 16,
            keep_leaves: true,
            ..BeamSettings::default()
        })
    );
    assert!(combined.path_cache.enabled);
}

#[test]
fn keep_beams_is_refused_where_the_build_cannot_store_them() {
    assert_eq!(keep_beams_refusal(false, false), None);
    assert_eq!(keep_beams_refusal(false, true), None);
    assert_eq!(keep_beams_refusal(true, true), None);
    let why = keep_beams_refusal(true, false).expect("no persistence refuses the flag");
    assert!(why.contains("--keep-beams"), "{why}");
    assert!(why.contains("path-cache-disk"), "{why}");
    // This build stores beams, so the parser lets the flag through.
    assert_eq!(keep_beams_refusal(true, BEAM_PERSISTENCE_AVAILABLE), None);
    assert!(parse(&argv("render a.job.json --keep-beams")).is_ok());
}

#[test]
fn keep_beams_is_for_the_path_traced_engine_and_the_job_commands() {
    let message = error("render a.job.json --keep-beams --engine ray");
    assert!(message.contains("--beam"), "{message}");
    assert!(message.contains("Path-Traced"), "{message}");
    for command in ["solve", "optimize"] {
        assert!(
            !error(&format!("{command} --keep-beams")).is_empty(),
            "{command}"
        );
    }
}

#[test]
fn the_pages_describe_keep_beams() {
    for topic in [Topic::Render, Topic::TiltVideo] {
        let page = help::text(topic);
        assert!(page.contains("[--keep-beams]"), "{topic:?} usage");
        assert!(
            page.contains("  --keep-beams            "),
            "{topic:?} lacks the flag's entry"
        );
    }
}
