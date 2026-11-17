//! The global flags `--headless`, `--minimized` and `--config`, and the launch decision.

use crate::cli::{
    GlobalFlags, HelpTopic, LaunchMode, launch_mode, parse, requests_window, take_global_flags,
};
use std::path::PathBuf;

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(std::string::ToString::to_string).collect()
}

/// Every command line the parser knew before the global flags: good ones, bad ones, help.
fn old_invocations() -> Vec<Vec<String>> {
    let lines: &[&[&str]] = &[
        &[],
        &["--help"],
        &["-h"],
        &["--version"],
        &["frobnicate"],
        &["render", "--help"],
        &[
            "render",
            "--scene",
            "s.json",
            "--out",
            "o.png",
            "--width",
            "64",
            "--height",
            "48",
            "--samples",
            "4",
            "--threads",
            "2",
            "--only-cpu",
        ],
        &["render", "--scene", "s.json"],
        &["serve", "--insecure-no-tls"],
        &[
            "serve",
            "--bind",
            "127.0.0.1:7900",
            "--ca",
            "ca.pem",
            "--cert",
            "server.pem",
            "--key",
            "server.key",
            "--render",
            "--max-connections",
            "8",
        ],
        &["serve", "--help"],
        &["serve", "--no-such-flag"],
        &["join", "127.0.0.1:7880", "--slots", "3"],
        &[
            "join",
            "--coordinator",
            "127.0.0.1:7880",
            "--token",
            "GW1-abc",
        ],
        &["join", "--help"],
        &["cert", "init", "--dir", "pki"],
        &[
            "cert",
            "issue-server",
            "--dir",
            "pki",
            "--host",
            "box",
            "--ip",
            "10.0.0.2",
        ],
        &[
            "cert",
            "issue-client",
            "--dir",
            "pki",
            "--name",
            "laptop",
            "--out",
            "bundle",
            "--role",
            "worker",
        ],
        &[
            "cert",
            "issue-token",
            "--ca",
            "pki/ca.pem",
            "--admin-addr",
            "127.0.0.1:7879",
            "--name",
            "phone",
        ],
        &[
            "cert",
            "claim",
            "--token",
            "GV1-abc",
            "--addr",
            "127.0.0.1:7879",
            "--out",
            "bundle",
        ],
        &["cert", "claim", "--help"],
        &["cert", "bogus"],
    ];
    lines.iter().map(|items| argv(items)).collect()
}

/// `--headless` at the front, in every middle position and at the end leaves `argv`
/// exactly as it was, and the parse result (success, error or help page) is the same.
#[test]
fn headless_anywhere_parses_like_the_plain_line() {
    for plain in old_invocations() {
        let expected = parse(&plain);
        for position in 0..=plain.len() {
            let mut line = plain.clone();
            line.insert(position, "--headless".to_string());
            let flags = take_global_flags(&mut line).unwrap();
            assert!(flags.headless, "line = {line:?}");
            assert!(!flags.minimized);
            assert_eq!(flags.config, None);
            assert_eq!(line, plain, "headless at {position} of {plain:?}");
            assert_eq!(
                parse(&line),
                expected,
                "headless at {position} of {plain:?}"
            );
        }
    }
}

#[test]
fn headless_at_front_middle_and_end_of_one_serve_line() {
    let plain = argv(&["serve", "--bind", "127.0.0.1:7900", "--insecure-no-tls"]);
    let expected = parse(&plain).unwrap();
    for line in [
        argv(&[
            "--headless",
            "serve",
            "--bind",
            "127.0.0.1:7900",
            "--insecure-no-tls",
        ]),
        argv(&[
            "serve",
            "--bind",
            "127.0.0.1:7900",
            "--headless",
            "--insecure-no-tls",
        ]),
        argv(&[
            "serve",
            "--bind",
            "127.0.0.1:7900",
            "--insecure-no-tls",
            "--headless",
        ]),
    ] {
        let mut line = line;
        assert!(take_global_flags(&mut line).unwrap().headless);
        assert_eq!(parse(&line).unwrap(), expected);
    }
}

#[test]
fn a_repeated_headless_is_stripped_every_time() {
    let mut line = argv(&[
        "--headless",
        "serve",
        "--headless",
        "--insecure-no-tls",
        "--headless",
    ]);
    assert!(take_global_flags(&mut line).unwrap().headless);
    assert_eq!(line, argv(&["serve", "--insecure-no-tls"]));
}

#[test]
fn a_line_without_global_flags_is_untouched() {
    for plain in old_invocations() {
        let mut line = plain.clone();
        assert_eq!(
            take_global_flags(&mut line).unwrap(),
            GlobalFlags::default()
        );
        assert_eq!(line, plain);
    }
}

#[test]
fn minimized_and_config_are_stripped_and_stored() {
    let mut line = argv(&["--minimized", "--config", "pleochroma.toml"]);
    let flags = take_global_flags(&mut line).unwrap();
    assert!(flags.minimized);
    assert!(!flags.headless);
    assert_eq!(flags.config, Some(PathBuf::from("pleochroma.toml")));
    assert!(line.is_empty());

    let mut line = argv(&[
        "serve",
        "--config=/etc/pleochroma/coordinator.toml",
        "--insecure-no-tls",
        "--minimized",
    ]);
    let flags = take_global_flags(&mut line).unwrap();
    assert!(flags.minimized);
    assert_eq!(
        flags.config,
        Some(PathBuf::from("/etc/pleochroma/coordinator.toml"))
    );
    assert_eq!(line, argv(&["serve", "--insecure-no-tls"]));
    assert_eq!(parse(&line), parse(&argv(&["serve", "--insecure-no-tls"])));
}

#[test]
fn the_last_config_wins_and_the_flags_combine() {
    let mut line = argv(&[
        "--config",
        "a.toml",
        "--headless",
        "serve",
        "--config",
        "b.toml",
        "--insecure-no-tls",
    ]);
    let flags = take_global_flags(&mut line).unwrap();
    assert!(flags.headless);
    assert_eq!(flags.config, Some(PathBuf::from("b.toml")));
    assert_eq!(line, argv(&["serve", "--insecure-no-tls"]));
}

#[test]
fn config_without_a_value_is_an_error() {
    for line in [
        argv(&["serve", "--config"]),
        argv(&["--config", "--headless"]),
        argv(&["--config="]),
    ] {
        let mut line = line;
        let error = take_global_flags(&mut line).unwrap_err();
        assert!(error.contains("--config"), "{error}");
    }
}

/// `--help` and `--version` come out of the parser as they did before, with the new flags
/// anywhere on the line.
#[test]
fn help_and_version_are_unchanged_by_the_global_flags() {
    for plain in [
        argv(&["--help"]),
        argv(&["-h"]),
        argv(&["--version"]),
        argv(&["serve", "--help"]),
        argv(&["cert", "claim", "--help"]),
    ] {
        let expected = parse(&plain);
        for extra in [
            argv(&["--headless"]),
            argv(&["--minimized"]),
            argv(&["--config", "x.toml"]),
            argv(&["--headless", "--minimized", "--config", "x.toml"]),
        ] {
            for at_front in [true, false] {
                let mut line = Vec::new();
                if at_front {
                    line.extend(extra.clone());
                    line.extend(plain.clone());
                } else {
                    line.extend(plain.clone());
                    line.extend(extra.clone());
                }
                take_global_flags(&mut line).unwrap();
                assert_eq!(line, plain);
                assert_eq!(parse(&line), expected, "{line:?}");
            }
        }
    }
    let mut line = argv(&["--headless", "serve", "--help"]);
    take_global_flags(&mut line).unwrap();
    assert_eq!(
        parse(&line).unwrap(),
        crate::cli::Command::Help(HelpTopic::Serve)
    );
}

/// `--version` and `-V` give the version command, also with the global flags around them;
/// `--help` still wins over them.
#[test]
fn version_is_a_command_of_its_own() {
    use crate::cli::Command;
    assert_eq!(parse(&argv(&["--version"])), Ok(Command::Version));
    assert_eq!(parse(&argv(&["-V"])), Ok(Command::Version));
    let mut line = argv(&["--headless", "--version", "--config", "x.toml"]);
    take_global_flags(&mut line).unwrap();
    assert_eq!(parse(&line), Ok(Command::Version));
    assert_eq!(
        parse(&argv(&["--version", "--help"])),
        Ok(Command::Help(HelpTopic::Root))
    );
    assert!(HelpTopic::Root.usage().contains("--version"));
}

/// `--version` is answered before the GPU setup, the update check and the log are started, so
/// it needs no more than the line itself: this is the rule `main` applies first.
#[test]
fn a_version_request_is_known_before_anything_starts() {
    use crate::cli::{version_line, wants_version};
    assert!(wants_version(&argv(&["--version"])));
    assert!(wants_version(&argv(&["-V"])));
    // After the global flags are taken out, and with the start-up switches before it.
    let mut line = argv(&["--headless", "--version", "--config", "x.toml"]);
    take_global_flags(&mut line).unwrap();
    assert!(wants_version(&line));
    assert!(wants_version(&argv(&["--no-update-check", "--version"])));
    assert!(wants_version(&argv(&["--radv-workaround", "off", "-V"])));
    assert!(wants_version(&argv(&["--radv-workaround=on", "--version"])));
    // Help wins; a command is not a version request; nor is an empty line.
    assert!(!wants_version(&argv(&["--version", "--help"])));
    assert!(!wants_version(&argv(&["serve", "--version"])));
    assert!(!wants_version(&argv(&[])));
    // The line is the program name and the version, and nothing else.
    let text = version_line();
    assert_eq!(
        text,
        format!("pleochroma-worker {}", env!("CARGO_PKG_VERSION"))
    );
    assert!(!text.contains('\n'));
}

#[test]
fn the_root_help_documents_the_global_flags() {
    let root = HelpTopic::Root.usage();
    for flag in ["--headless", "--minimized", "--config"] {
        assert!(root.contains(flag), "{root}");
    }
}

#[test]
fn only_a_bare_line_asks_for_the_window() {
    assert!(requests_window(&[]));
    assert!(requests_window(&argv(&["--no-update-check"])));
    assert!(requests_window(&argv(&["--minimized"])));
    assert!(requests_window(&argv(&["--config", "x.toml"])));
    assert!(requests_window(&argv(&["--config=x.toml", "--minimized"])));
    assert!(requests_window(&argv(&["--radv-workaround", "off"])));
    assert!(!requests_window(&argv(&["--help"])));
    assert!(!requests_window(&argv(&["-h"])));
    assert!(!requests_window(&argv(&["--version"])));
    assert!(!requests_window(&argv(&["--headless"])));
    assert!(!requests_window(&argv(&["serve"])));
    assert!(!requests_window(&argv(&[
        "--minimized",
        "serve",
        "--render"
    ])));
    assert!(!requests_window(&argv(&[
        "--config",
        "x.toml",
        "--version"
    ])));
}

/// A build without the `gui` feature never has a window, whatever the line says.
#[cfg(not(feature = "gui"))]
#[test]
fn a_build_without_gui_is_always_headless() {
    for flags in [
        GlobalFlags::default(),
        GlobalFlags {
            headless: true,
            ..GlobalFlags::default()
        },
        GlobalFlags {
            minimized: true,
            ..GlobalFlags::default()
        },
    ] {
        for line in [argv(&[]), argv(&["--no-update-check"]), argv(&["serve"])] {
            assert_eq!(launch_mode(&flags, &line), LaunchMode::Headless);
        }
    }
}

#[cfg(feature = "gui")]
#[test]
fn a_gui_build_opens_the_window_only_for_a_bare_line() {
    let plain = GlobalFlags::default();
    assert_eq!(launch_mode(&plain, &[]), LaunchMode::Gui);
    assert_eq!(
        launch_mode(&plain, &argv(&["--no-update-check"])),
        LaunchMode::Gui
    );
    assert_eq!(launch_mode(&plain, &argv(&["serve"])), LaunchMode::Headless);
    assert_eq!(
        launch_mode(&plain, &argv(&["--help"])),
        LaunchMode::Headless
    );
    assert_eq!(
        launch_mode(&plain, &argv(&["--version"])),
        LaunchMode::Headless
    );
    let headless = GlobalFlags {
        headless: true,
        ..GlobalFlags::default()
    };
    assert_eq!(launch_mode(&headless, &[]), LaunchMode::Headless);
}
