//! The desktop preview: the mobile UI in a desktop window at a phone or tablet size,
//! so it can be exercised without a device.
//!
//! ```text
//! pleochroma-mobile-preview [--size WxH|phone|phone-land|tablet|tablet-land] [--data DIR]
//!                          [--theme dark|high-contrast] [--cutting]
//!                          [--developer] [--more-page N] [--developer-sample-logs]
//!                          [--hold-preparing DONE/TOTAL]
//! ```
//!
//! `--size` defaults to `390x800` (`phone`). `phone-land` is `800x390`, `tablet` is
//! `800x1280` and `tablet-land` is `1280x800`. `--data` is the app's data directory
//! (default `<temp>/pleochroma-mobile-preview`): settings live in `DIR/files`, shared and
//! printed files in `DIR/cache/share`.
//!
//! `--page library|design|cut|more` opens that destination, `--template N` starts on that
//! template card, `--tab tiers|stone|light|checks` and `--sheet peek|half|full` pick the
//! Design page's tab and sheet height, and `--view solid|diagram|render` sets the viewport
//! mode (a render screenshot is taken once the picture is finished). `--engine path|ray`
//! chooses the renderer. `--theme dark|high-contrast` starts with that theme (`dark` is the
//! default; the choice is not saved). `--cutting` opens Cutting Mode at its first step (it
//! needs a design, so pair it with `--template N`; it also opens the Cut page unless `--page`
//! says otherwise). `--developer` unlocks the Developer page without the About taps, and
//! `--more-page N` opens that More sub-page (6 is Developer, 7 the log viewer; it also opens the
//! More destination unless `--page` says otherwise). `--developer-sample-logs` writes a fixed set
//! of sample log files (one crash snapshot among them) into the logs folder before the first
//! frame, and `--hold-preparing DONE/TOTAL` freezes the render at "Building shaders DONE of
//! TOTAL" (pair it with `--view render` and `--wait`). `--screenshot FILE.png` saves the window shortly after start and
//! quits; `--wait SECONDS` waits that many seconds before saving instead. Press Escape for
//! Back.

use std::{path::PathBuf, process::ExitCode, sync::Arc, time::Duration};

use pleochroma_mobile_android::{
    platform_desktop::DesktopPlatform,
    run::{self, RunOptions},
    tracer_layer::{TracerFileLayer, install_panic_line},
};
use pleochroma_mobile_core::{
    diagnostics::exit::process_start, layout::Destination, platform::AppDirs, settings::ThemeChoice,
};
use tracing_subscriber::{EnvFilter, Layer as _, layer::SubscriberExt as _};

const DEFAULT_SIZE: (f32, f32) = (390.0, 800.0);
const USAGE: &str = "usage: pleochroma-mobile-preview [--size WxH|phone|phone-land|tablet|tablet-land] [--data DIR] [--page library|design|cut|more] [--template N] [--tab tiers|stone|light|checks] [--sheet peek|half|full] [--view solid|diagram|render] [--engine path|ray] [--theme dark|high-contrast] [--cutting] [--developer] [--more-page N] [--developer-sample-logs] [--hold-preparing DONE/TOTAL] [--wait SECONDS] [--screenshot FILE.png]";

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq)]
struct PreviewArgs {
    size: (f32, f32),
    data: PathBuf,
    page: Option<Destination>,
    screenshot: Option<PathBuf>,
    template: Option<usize>,
    tab: Option<i32>,
    sheet: Option<i32>,
    view: Option<i32>,
    engine: Option<u8>,
    theme: Option<ThemeChoice>,
    cutting: bool,
    wait: Option<Duration>,
    developer: bool,
    more_page: Option<i32>,
    developer_sample_logs: bool,
    hold_preparing: Option<(u32, u32)>,
}

/// The More sub-page a number selects (0 to 7: 6 is Developer, 7 the log viewer).
fn parse_more_page(text: &str) -> Result<i32, String> {
    text.parse::<i32>()
        .ok()
        .filter(|page| (0..=7).contains(page))
        .ok_or_else(|| format!("bad more page {text:?}: expected 0 to 7"))
}

/// The frozen preparing counts of `DONE/TOTAL` (`TOTAL` at least 1, `DONE` not above it).
fn parse_hold_preparing(text: &str) -> Result<(u32, u32), String> {
    let bad = || format!("bad --hold-preparing {text:?}: expected DONE/TOTAL, like 3/18");
    let (done, total) = text.split_once('/').ok_or_else(bad)?;
    let done = done.trim().parse::<u32>().map_err(|_| bad())?;
    let total = total.trim().parse::<u32>().map_err(|_| bad())?;
    if total == 0 || done > total {
        return Err(bad());
    }
    Ok((done, total))
}

/// The theme a name selects (`dark` or `high-contrast`).
fn parse_theme(text: &str) -> Result<ThemeChoice, String> {
    [
        ("dark", ThemeChoice::Dark),
        ("high-contrast", ThemeChoice::HighContrast),
    ]
    .into_iter()
    .find(|(name, _)| name.eq_ignore_ascii_case(text))
    .map(|(_, theme)| theme)
    .ok_or_else(|| format!("bad theme {text:?}: expected dark or high-contrast"))
}

/// The viewport mode a name selects (0 Solid, 1 Diagram, 2 Render).
fn parse_view(text: &str) -> Result<i32, String> {
    ["solid", "diagram", "render"]
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|index| i32::try_from(index).ok())
        .ok_or_else(|| format!("bad view {text:?}: expected solid, diagram or render"))
}

/// The Design page tab a name selects (0 Tiers, 1 Stone, 2 Light, 3 Checks).
fn parse_tab(text: &str) -> Result<i32, String> {
    ["tiers", "stone", "light", "checks"]
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|index| i32::try_from(index).ok())
        .ok_or_else(|| format!("bad tab {text:?}: expected tiers, stone, light or checks"))
}

/// The sheet detent a name selects (0 peek, 1 half, 2 full).
fn parse_sheet(text: &str) -> Result<i32, String> {
    ["peek", "half", "full"]
        .iter()
        .position(|name| name.eq_ignore_ascii_case(text))
        .and_then(|index| i32::try_from(index).ok())
        .ok_or_else(|| format!("bad sheet {text:?}: expected peek, half or full"))
}

/// A destination from its lower-case label.
fn parse_page(text: &str) -> Result<Destination, String> {
    Destination::ALL
        .into_iter()
        .find(|destination| destination.label().eq_ignore_ascii_case(text))
        .ok_or_else(|| format!("bad page {text:?}: expected library, design, cut or more"))
}

/// A window size from a preset name or `WxH` (logical px, 200 to 4000 per side).
fn parse_size(text: &str) -> Result<(f32, f32), String> {
    match text {
        "phone" => return Ok(DEFAULT_SIZE),
        "phone-land" => return Ok((800.0, 390.0)),
        "tablet" => return Ok((800.0, 1280.0)),
        "tablet-land" => return Ok((1280.0, 800.0)),
        _ => {}
    }
    let (width, height) = text
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("bad size {text:?}: expected WxH or a preset name"))?;
    let parse = |side: &str| {
        side.trim()
            .parse::<u16>()
            .ok()
            .filter(|value| (200..=4000).contains(value))
            .map(f32::from)
            .ok_or_else(|| format!("bad size {text:?}: each side must be 200 to 4000"))
    };
    Ok((parse(width)?, parse(height)?))
}

/// Parses the arguments after the program name.
fn parse_args(
    args: impl IntoIterator<Item = String>,
    default_data: PathBuf,
) -> Result<PreviewArgs, String> {
    let mut parsed = PreviewArgs {
        size: DEFAULT_SIZE,
        data: default_data,
        page: None,
        screenshot: None,
        template: None,
        tab: None,
        sheet: None,
        view: None,
        engine: None,
        theme: None,
        cutting: false,
        wait: None,
        developer: false,
        more_page: None,
        developer_sample_logs: false,
        hold_preparing: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size" => {
                let value = args.next().ok_or("--size needs a value")?;
                parsed.size = parse_size(&value)?;
            }
            "--data" => {
                let value = args.next().ok_or("--data needs a directory")?;
                parsed.data = PathBuf::from(value);
            }
            "--page" => {
                let value = args.next().ok_or("--page needs a destination")?;
                parsed.page = Some(parse_page(&value)?);
            }
            "--template" => {
                let value = args.next().ok_or("--template needs a number")?;
                let index = value
                    .parse::<usize>()
                    .map_err(|_| format!("bad template {value:?}: expected a card number"))?;
                parsed.template = Some(index);
            }
            "--tab" => {
                let value = args.next().ok_or("--tab needs a tab name")?;
                parsed.tab = Some(parse_tab(&value)?);
            }
            "--sheet" => {
                let value = args.next().ok_or("--sheet needs peek, half or full")?;
                parsed.sheet = Some(parse_sheet(&value)?);
            }
            "--engine" => {
                let value = args.next().ok_or("--engine needs path or ray")?;
                parsed.engine = Some(match value.as_str() {
                    "path" => 0,
                    "ray" => 1,
                    _ => return Err("--engine needs path or ray".to_owned()),
                });
            }
            "--theme" => {
                let value = args.next().ok_or("--theme needs dark or high-contrast")?;
                parsed.theme = Some(parse_theme(&value)?);
            }
            "--cutting" => parsed.cutting = true,
            "--developer" => parsed.developer = true,
            "--developer-sample-logs" => parsed.developer_sample_logs = true,
            "--more-page" => {
                let value = args.next().ok_or("--more-page needs a page number")?;
                parsed.more_page = Some(parse_more_page(&value)?);
            }
            "--hold-preparing" => {
                let value = args.next().ok_or("--hold-preparing needs DONE/TOTAL")?;
                parsed.hold_preparing = Some(parse_hold_preparing(&value)?);
            }
            "--view" => {
                let value = args.next().ok_or("--view needs solid, diagram or render")?;
                parsed.view = Some(parse_view(&value)?);
            }
            "--wait" => {
                let value = args.next().ok_or("--wait needs a number of seconds")?;
                let seconds = value
                    .parse::<u64>()
                    .ok()
                    .filter(|seconds| (1..=600).contains(seconds))
                    .ok_or_else(|| format!("bad wait {value:?}: expected 1 to 600 seconds"))?;
                parsed.wait = Some(Duration::from_secs(seconds));
            }
            "--screenshot" => {
                let value = args.next().ok_or("--screenshot needs a file")?;
                parsed.screenshot = Some(PathBuf::from(value));
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(parsed)
}

/// Installs the console log and the tracer file layer as one registry, then the panic line.
///
/// The registry goes in with `set_global_default`, never `.init()`: the file layer decides in
/// `on_event`, not in a filter, and the console filter is that layer's own.
fn install_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .with(TracerFileLayer);
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        eprintln!("a log subscriber was already installed");
    }
    install_panic_line();
}

fn main() -> ExitCode {
    install_logging();

    let default_data = std::env::temp_dir().join("pleochroma-mobile-preview");
    let args = match parse_args(std::env::args().skip(1), default_data) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let dirs = AppDirs {
        files: args.data.join("files"),
        cache: args.data.join("cache"),
    };
    tracing::info!(size = ?args.size, data = %args.data.display(), "starting the mobile preview");
    let platform: Arc<dyn pleochroma_mobile_core::platform::Platform> =
        Arc::new(DesktopPlatform::new(dirs));
    // Before any GPU work: the start-up check decides whether the shader cache is on.
    let cache_dir = process_start(&platform);
    #[cfg(feature = "device-gpu")]
    if let Some(dir) = cache_dir
        && !pleochroma::renderer::gpu::pipeline_cache::set_pipeline_cache_dir(dir)
    {
        tracing::warn!("the shader cache folder was already set");
    }
    #[cfg(not(feature = "device-gpu"))]
    drop(cache_dir);
    let options = RunOptions {
        initial_size: Some(args.size),
        // Cutting Mode belongs to the Cut page: open it there unless `--page` says otherwise.
        destination: args
            .page
            .or_else(|| args.cutting.then_some(Destination::Cut))
            .or_else(|| args.more_page.map(|_| Destination::More)),
        screenshot: args.screenshot,
        template: args.template,
        tab: args.tab,
        sheet: args.sheet,
        view: args.view,
        engine: args.engine,
        screenshot_delay: args.wait,
        theme: args.theme,
        cutting: args.cutting,
        developer: args.developer,
        more_page: args.more_page,
        developer_sample_logs: args.developer_sample_logs,
        hold_preparing: args.hold_preparing,
    };
    match run::run_with(&platform, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("the preview failed: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn defaults_to_a_phone() {
        let parsed = parse_args(args(&[]), PathBuf::from("d")).unwrap();
        assert_eq!(parsed.size, (390.0, 800.0));
        assert_eq!(parsed.data, PathBuf::from("d"));
    }

    #[test]
    fn presets_and_explicit_sizes() {
        assert_eq!(parse_size("phone-land").unwrap(), (800.0, 390.0));
        assert_eq!(parse_size("tablet").unwrap(), (800.0, 1280.0));
        assert_eq!(parse_size("tablet-land").unwrap(), (1280.0, 800.0));
        assert_eq!(parse_size("412x915").unwrap(), (412.0, 915.0));
        assert_eq!(parse_size("600X960").unwrap(), (600.0, 960.0));
    }

    #[test]
    fn bad_sizes_are_rejected() {
        for bad in ["", "390", "axb", "10x10", "390x", "x800", "99999x800"] {
            assert!(parse_size(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn flags_are_parsed() {
        let parsed = parse_args(
            args(&["--size", "tablet", "--data", "some/dir"]),
            PathBuf::from("d"),
        )
        .unwrap();
        assert_eq!(parsed.size, (800.0, 1280.0));
        assert_eq!(parsed.data, PathBuf::from("some/dir"));
    }

    #[test]
    fn page_and_screenshot_flags_are_parsed() {
        let parsed = parse_args(
            args(&["--page", "cut", "--screenshot", "shot.png"]),
            PathBuf::from("d"),
        )
        .unwrap();
        assert_eq!(parsed.page, Some(Destination::Cut));
        assert_eq!(parsed.screenshot, Some(PathBuf::from("shot.png")));
        assert!(parse_page("Library").is_ok());
        assert!(parse_page("nowhere").is_err());
    }

    #[test]
    fn tab_and_sheet_flags_are_parsed() {
        let parsed = parse_args(
            args(&["--tab", "Light", "--sheet", "full", "--template", "1"]),
            PathBuf::from("d"),
        )
        .unwrap();
        assert_eq!(parsed.tab, Some(2));
        assert_eq!(parsed.sheet, Some(2));
        assert_eq!(parsed.template, Some(1));
        assert_eq!(parse_tab("tiers").unwrap(), 0);
        assert_eq!(parse_tab("checks").unwrap(), 3);
        assert_eq!(parse_sheet("peek").unwrap(), 0);
        assert_eq!(parse_sheet("half").unwrap(), 1);
        assert!(parse_tab("nowhere").is_err());
        assert!(parse_sheet("huge").is_err());
        assert!(parse_args(args(&["--tab"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--sheet", "x"]), PathBuf::new()).is_err());
    }

    #[test]
    fn theme_and_cutting_flags_are_parsed() {
        let plain = parse_args(args(&[]), PathBuf::new()).unwrap();
        assert_eq!(
            plain.theme, None,
            "the stored theme (dark by default) applies"
        );
        assert!(!plain.cutting);
        let parsed = parse_args(
            args(&["--theme", "High-Contrast", "--cutting"]),
            PathBuf::new(),
        )
        .unwrap();
        assert_eq!(parsed.theme, Some(ThemeChoice::HighContrast));
        assert!(parsed.cutting);
        assert_eq!(parse_theme("dark").unwrap(), ThemeChoice::Dark);
        assert!(parse_theme("light").is_err());
        assert!(parse_args(args(&["--theme"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--theme", "sepia"]), PathBuf::new()).is_err());
    }

    #[test]
    fn developer_flags_are_parsed() {
        let plain = parse_args(args(&[]), PathBuf::new()).unwrap();
        assert!(!plain.developer && !plain.developer_sample_logs);
        assert_eq!(plain.more_page, None);
        assert_eq!(plain.hold_preparing, None);
        let parsed = parse_args(
            args(&[
                "--developer",
                "--developer-sample-logs",
                "--more-page",
                "7",
                "--hold-preparing",
                "3/18",
            ]),
            PathBuf::new(),
        )
        .unwrap();
        assert!(parsed.developer && parsed.developer_sample_logs);
        assert_eq!(parsed.more_page, Some(7));
        assert_eq!(parsed.hold_preparing, Some((3, 18)));
    }

    #[test]
    fn bad_developer_values_are_rejected() {
        for bad in ["", "x", "-1", "8", "6.5"] {
            assert!(parse_more_page(bad).is_err(), "{bad:?} should be rejected");
        }
        for bad in ["", "3", "3/", "/18", "a/b", "19/18", "0/0", "3-18"] {
            assert!(
                parse_hold_preparing(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert_eq!(parse_hold_preparing("0/18"), Ok((0, 18)));
        assert!(parse_args(args(&["--more-page"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--hold-preparing"]), PathBuf::new()).is_err());
    }

    #[test]
    fn unknown_or_incomplete_flags_are_rejected() {
        assert!(parse_args(args(&["--bogus"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--size"]), PathBuf::new()).is_err());
        assert!(parse_args(args(&["--data"]), PathBuf::new()).is_err());
    }
}
