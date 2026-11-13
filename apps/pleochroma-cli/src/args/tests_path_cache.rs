//! The path-cache flags of `render` and `tilt-video`: parsing, the settings they stand for, the
//! refusals and the help pages.

use super::{Command, PathCacheArgs, RenderJobArgs, Topic, parse};
use crate::help;
use pleochroma::renderer::accel::{CacheCompression, PathCacheSettings};
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
fn the_path_cache_is_off_by_default() {
    for line in ["render a.job.json", "tilt-video v.job.json"] {
        let args = render_args(line);
        assert_eq!(args.path_cache, PathCacheArgs::default(), "{line}");
        assert_eq!(args.path_cache.settings(), None, "{line}");
    }
}

#[test]
fn path_cache_alone_is_in_render_only() {
    for line in [
        "render a.job.json --path-cache",
        "tilt-video v.job.json --path-cache",
    ] {
        let args = render_args(line);
        assert!(args.path_cache.enabled, "{line}");
        assert!(!args.path_cache.persist, "{line}");
        assert_eq!(args.path_cache.cache_dir, None, "{line}");
        assert_eq!(
            args.path_cache.settings(),
            Some(PathCacheSettings {
                persist: false,
                compression: CacheCompression::Zstd,
            }),
            "{line}"
        );
    }
}

#[test]
fn persist_cache_implies_path_cache() {
    let args = render_args("render a.job.json --persist-cache");
    assert!(args.path_cache.enabled);
    assert!(args.path_cache.persist);
    assert_eq!(
        args.path_cache.settings().map(|settings| settings.persist),
        Some(true)
    );
}

#[test]
fn cache_dir_takes_a_folder_and_turns_nothing_on() {
    let args = render_args("render a.job.json --cache-dir kept/caches");
    assert_eq!(
        args.path_cache.cache_dir,
        Some(PathBuf::from("kept/caches"))
    );
    assert!(!args.path_cache.enabled);
    assert_eq!(args.path_cache.settings(), None);

    let message = error("render a.job.json --cache-dir");
    assert!(message.contains("--cache-dir"), "{message}");
}

#[test]
fn the_flags_may_stand_anywhere_and_combine_with_the_others() {
    let args =
        render_args("render --persist-cache a.job.json --cache-dir c --sample-offset 16 --quiet");
    assert_eq!(args.job, PathBuf::from("a.job.json"));
    assert!(args.path_cache.persist);
    assert_eq!(args.path_cache.cache_dir, Some(PathBuf::from("c")));
    assert_eq!(args.sample_offset, 16);
    assert!(args.quiet);
}

#[test]
fn the_ray_traced_engine_has_no_path_cache() {
    for line in [
        "render a.job.json --path-cache --engine ray",
        "render a.job.json --engine ray --persist-cache",
        "tilt-video v.job.json --engine ray --path-cache",
    ] {
        let message = error(line);
        assert!(message.contains("--path-cache"), "{line}: {message}");
        assert!(message.contains("path-traced"), "{line}: {message}");
    }
    // Without the cache flags the Ray Traced engine is accepted with no path cache.
    assert!(parse(&argv("render a.job.json --engine ray")).is_ok());
}

#[test]
fn the_flags_belong_to_the_job_commands_only() {
    for command in ["solve", "optimize"] {
        let message = error(&format!("{command} --path-cache"));
        assert!(!message.is_empty(), "{command}");
    }
}

#[test]
fn the_pages_describe_the_path_cache_flags() {
    for topic in [Topic::Render, Topic::TiltVideo] {
        let page = help::text(topic);
        for flag in ["--path-cache", "--persist-cache", "--cache-dir"] {
            assert!(page.contains(flag), "{topic:?} page lacks {flag}");
        }
        assert!(
            page.contains("[--path-cache] [--persist-cache] [--cache-dir FOLDER]"),
            "{topic:?} usage lacks them"
        );
    }
}
