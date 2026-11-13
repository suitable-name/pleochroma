#[cfg(feature = "adaptive-sampling")]
use super::AdaptiveArgs;
use super::{Command, RenderJobArgs, Topic, parse};
use crate::help;
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::adaptive::{AdaptiveSettings, NoiseMetric};
use std::path::PathBuf;

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
fn the_benchmark_outputs_are_off_by_default() {
    for line in ["render a.job.json", "tilt-video v.job.json"] {
        let args = render_args(line);
        assert_eq!(args.raw_out, None, "{line}");
        assert_eq!(args.progress_raw, None, "{line}");
        assert_eq!(args.sample_offset, 0, "{line}");
    }
}

#[test]
fn render_takes_raw_out_progress_raw_and_sample_offset() {
    let args = render_args(
        "render a.job.json --raw-out ref.plraw --progress-raw steps --sample-offset 16384",
    );
    assert_eq!(args.raw_out, Some(PathBuf::from("ref.plraw")));
    assert_eq!(args.progress_raw, Some(PathBuf::from("steps")));
    assert_eq!(args.sample_offset, 16384);
}

#[test]
fn tilt_video_takes_raw_out_and_sample_offset_and_refuses_progress_raw() {
    let args = render_args("tilt-video v.job.json --raw-out raw --sample-offset 8000");
    assert_eq!(args.raw_out, Some(PathBuf::from("raw")));
    assert_eq!(args.progress_raw, None);
    assert_eq!(args.sample_offset, 8000);

    let message = error("tilt-video v.job.json --progress-raw steps");
    assert!(
        message.contains("--progress-raw is for \"render\""),
        "{message}"
    );
    assert!(message.contains("tilt-video"), "{message}");
}

#[test]
fn the_flags_may_stand_anywhere_and_zero_is_a_valid_offset() {
    let args = render_args("render --sample-offset 0 a.job.json --raw-out r.plraw");
    assert_eq!(args.job, PathBuf::from("a.job.json"));
    assert_eq!(args.raw_out, Some(PathBuf::from("r.plraw")));
    assert_eq!(args.sample_offset, 0);
}

#[test]
fn a_sample_offset_must_be_a_whole_number_from_zero_up() {
    for word in ["-1", "-16", "1.5", "many", "", "4294967296"] {
        let line = vec![
            "render".to_string(),
            "a.job.json".to_string(),
            "--sample-offset".to_string(),
            word.to_string(),
        ];
        let message = parse(&line).expect_err("this offset should be refused");
        assert!(message.contains("--sample-offset"), "{word:?}: {message}");
        assert!(message.contains("whole number"), "{word:?}: {message}");
    }
    let largest = render_args("render a.job.json --sample-offset 4294967295");
    assert_eq!(largest.sample_offset, u32::MAX);
}

#[test]
fn each_flag_needs_its_value() {
    for flag in ["--raw-out", "--progress-raw", "--sample-offset"] {
        let message = error(&format!("render a.job.json {flag}"));
        assert!(message.contains(flag), "{message}");
        assert!(message.contains("needs a value"), "{message}");
    }
    assert!(error("tilt-video v.job.json --raw-out").contains("needs a value"));
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn adaptive_sampling_is_off_by_default() {
    for line in ["render a.job.json", "tilt-video v.job.json"] {
        let args = render_args(line);
        assert_eq!(args.adaptive, AdaptiveArgs::default(), "{line}");
        assert!(!args.adaptive.enabled, "{line}");
        assert_eq!(args.adaptive.settings(512), Ok(None), "{line}");
    }
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn adaptive_alone_turns_it_on_with_the_provisional_defaults() {
    for line in [
        "render a.job.json --adaptive",
        "tilt-video v.job.json --adaptive",
    ] {
        let args = render_args(line);
        assert!(args.adaptive.enabled, "{line}");
        assert_eq!(args.adaptive.target_noise, None);
        assert_eq!(args.adaptive.min_spp, None);
        assert_eq!(args.adaptive.metric, None);
        assert_eq!(
            args.adaptive.settings(8192),
            Ok(Some(AdaptiveSettings::provisional(8192))),
            "{line}"
        );
    }
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn each_adaptive_companion_flag_implies_adaptive() {
    let target = render_args("render a.job.json --target-noise 0.02").adaptive;
    assert!(target.enabled);
    assert_eq!(target.target_noise, Some(0.02));
    let min = render_args("render a.job.json --min-spp 128").adaptive;
    assert!(min.enabled);
    assert_eq!(min.min_spp, Some(128));
    let metric = render_args("tilt-video v.job.json --noise-metric relative").adaptive;
    assert!(metric.enabled);
    assert_eq!(metric.metric, Some(NoiseMetric::RelativeLstar));
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_flags_override_the_provisional_defaults() {
    let args = render_args("render a.job.json --target-noise 0.005 --min-spp 64");
    let settings = args
        .adaptive
        .settings(4096)
        .expect("valid")
        .expect("adaptive is on");
    assert_eq!(settings.target_noise.to_bits(), 0.005_f32.to_bits());
    assert_eq!(settings.min_spp, 64);
    assert_eq!(settings.max_spp, 4096);
    assert_eq!(settings.metric, NoiseMetric::RelativeLstar);
    assert_eq!(settings.pass_spp, 16);
    assert_eq!(settings.tile, 16);
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_benchmark_flag_combinations_are_accepted() {
    // Exactly as benchmarks/render_accel/run.sh calls them.
    let relative =
        render_args("render a.job.json --adaptive --target-noise 0.01 --noise-metric relative");
    let settings = relative.adaptive.settings(8192).unwrap().unwrap();
    assert_eq!(settings.metric, NoiseMetric::RelativeLstar);
    assert_eq!(settings.target_noise.to_bits(), 0.01_f32.to_bits());
    let absolute = render_args("render a.job.json --noise-metric absolute --target-noise 1.5");
    let settings = absolute.adaptive.settings(8192).unwrap().unwrap();
    assert_eq!(settings.metric, NoiseMetric::AbsoluteDeltaE);
    assert_eq!(settings.target_noise.to_bits(), 1.5_f32.to_bits());
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn bad_adaptive_values_are_refused_at_the_command_line() {
    for word in ["0", "-0.01", "abc", "", "NaN", "inf"] {
        let line = vec![
            "render".to_string(),
            "a.job.json".to_string(),
            "--target-noise".to_string(),
            word.to_string(),
        ];
        let message = parse(&line).expect_err("this target should be refused");
        assert!(message.contains("--target-noise"), "{word:?}: {message}");
    }
    for word in ["0", "-4", "1.5", "many", ""] {
        let line = vec![
            "render".to_string(),
            "a.job.json".to_string(),
            "--min-spp".to_string(),
            word.to_string(),
        ];
        let message = parse(&line).expect_err("this minimum should be refused");
        assert!(message.contains("--min-spp"), "{word:?}: {message}");
    }
    let message = error("render a.job.json --noise-metric fuzzy");
    assert!(message.contains("--noise-metric"), "{message}");
    assert!(message.contains("relative or absolute"), "{message}");
    for flag in ["--target-noise", "--min-spp", "--noise-metric"] {
        let message = error(&format!("render a.job.json {flag}"));
        assert!(message.contains(flag), "{message}");
        assert!(message.contains("needs a value"), "{message}");
    }
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn combinations_that_break_a_rule_are_refused_with_the_rules_message() {
    // A relative target must stay below 1; the absolute one may not.
    let message = render_args("render a.job.json --target-noise 1.5")
        .adaptive
        .settings(512)
        .expect_err("a relative target of 1.5 is out of range");
    assert!(
        message.contains("target noise 1.5 is out of range"),
        "{message}"
    );
    assert!(
        render_args("render a.job.json --target-noise 1.5 --noise-metric absolute")
            .adaptive
            .settings(512)
            .is_ok()
    );
    // Without a target the absolute metric stops at Delta E 1.
    let absolute = render_args("render a.job.json --noise-metric absolute")
        .adaptive
        .settings(512)
        .expect("the absolute metric has a default target")
        .expect("adaptive is on");
    assert_eq!(absolute.metric, NoiseMetric::AbsoluteDeltaE);
    assert_eq!(absolute.target_noise.to_bits(), 1.0_f32.to_bits());
    // A minimum that is not a whole number of passes, or above the cap.
    let message = render_args("render a.job.json --min-spp 100")
        .adaptive
        .settings(512)
        .expect_err("100 is not a multiple of the 16-sample pass");
    assert!(message.contains("multiple of the pass size"), "{message}");
    let message = render_args("render a.job.json --min-spp 1024")
        .adaptive
        .settings(512)
        .expect_err("the minimum is above the cap");
    assert!(message.contains("sample counts"), "{message}");
    // A tiny job still gets a valid provisional pair.
    assert!(
        render_args("render a.job.json --adaptive")
            .adaptive
            .settings(4)
            .is_ok()
    );
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn adaptive_sampling_is_refused_with_the_ray_traced_engine() {
    for line in [
        "render a.job.json --engine ray --adaptive",
        "render a.job.json --adaptive --engine ray",
        "tilt-video v.job.json --engine ray --target-noise 0.02",
    ] {
        let message = error(line);
        assert!(message.contains("--adaptive"), "{line}: {message}");
        assert!(message.contains("Ray Traced"), "{line}: {message}");
    }
    // The path-traced engine, spelled out, is fine.
    assert!(
        render_args("render a.job.json --engine path --adaptive")
            .adaptive
            .enabled
    );
    // And a Ray Traced render without the flags is untouched.
    assert!(
        !render_args("render a.job.json --engine ray")
            .adaptive
            .enabled
    );
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_pages_describe_the_adaptive_flags() {
    for topic in [Topic::Render, Topic::TiltVideo] {
        let page = help::text(topic);
        for flag in [
            "--adaptive",
            "--target-noise",
            "--min-spp",
            "--noise-metric",
        ] {
            assert!(page.contains(flag), "{topic:?} page lacks {flag}");
        }
        assert!(page.contains("relative|absolute"), "{topic:?}");
    }
}

#[test]
fn the_render_and_tilt_video_pages_mention_the_benchmark_outputs() {
    let render = help::text(Topic::Render);
    for flag in ["--raw-out", "--progress-raw", "--sample-offset", "PLRAW1"] {
        assert!(render.contains(flag), "render page lacks {flag}");
    }
    let video = help::text(Topic::TiltVideo);
    for flag in ["--raw-out", "--sample-offset", "frame_NNNN.plraw"] {
        assert!(video.contains(flag), "tilt-video page lacks {flag}");
    }
    assert!(
        video.contains("--progress-raw is for render only"),
        "tilt-video page says --progress-raw is not for videos"
    );
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_gpu_convergence_flag_is_off_by_default() {
    for line in ["render a.job.json", "tilt-video v.job.json"] {
        assert!(!render_args(line).adaptive.gpu_convergence, "{line}");
    }
    // Adaptive alone does not ask for it either.
    assert!(
        !render_args("render a.job.json --adaptive")
            .adaptive
            .gpu_convergence
    );
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_gpu_convergence_flag_goes_with_adaptive_and_implies_nothing_else() {
    for line in [
        "render a.job.json --adaptive --gpu-convergence",
        "render a.job.json --gpu-convergence --adaptive",
        "tilt-video v.job.json --adaptive --gpu-convergence",
        "render a.job.json --target-noise 0.02 --gpu-convergence",
    ] {
        let args = render_args(line);
        assert!(args.adaptive.enabled, "{line}");
        assert!(args.adaptive.gpu_convergence, "{line}");
    }
    let args = render_args("render a.job.json --adaptive --gpu-convergence");
    assert_eq!(args.adaptive.target_noise, None);
    assert_eq!(args.adaptive.min_spp, None);
    assert_eq!(args.adaptive.metric, None);
    // The settings are the plain `--adaptive` ones: the flag changes where the check runs.
    assert_eq!(
        args.adaptive.settings(8192),
        render_args("render a.job.json --adaptive")
            .adaptive
            .settings(8192)
    );
    assert_eq!(args.raw_out, None);
    assert_eq!(args.sample_offset, 0);
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_gpu_convergence_flag_alone_is_refused_with_a_hint() {
    for line in [
        "render a.job.json --gpu-convergence",
        "tilt-video v.job.json --gpu-convergence",
    ] {
        let message = error(line);
        assert!(message.contains("--gpu-convergence"), "{line}: {message}");
        assert!(message.contains("--adaptive"), "{line}: {message}");
    }
    let message = error("render a.job.json --adaptive --gpu-convergence --engine ray");
    assert!(message.contains("--adaptive"), "{message}");
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn the_pages_describe_the_gpu_convergence_flag() {
    for topic in [Topic::Render, Topic::TiltVideo] {
        let page = help::text(topic);
        assert!(
            page.contains("--gpu-convergence"),
            "{topic:?} page lacks it"
        );
        assert!(
            page.contains("[--gpu-convergence]"),
            "{topic:?} usage lacks it"
        );
    }
}
