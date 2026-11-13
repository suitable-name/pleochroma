//! `image-compare`: two `PLRAW1` renders of one scene, compared as a viewer would see them.
//!
//! The command exists for the render-acceleration benchmark (`benchmarks/render_accel`): it says
//! how far an accelerated render is from a reference, and how many samples it saved. The files
//! are the linear images `render` and `tilt-video` write with `--raw-out` and `--progress-raw`
//! (see [`crate::raw_image`]).
//!
//! # Pipeline
//!
//! 1. Each image's mean XYZ goes through the renderer's own tone map (`display`) into the color
//!    space of the written picture: sRGB, or the job's with `--tonemap-from`. The exposure is
//!    already part of the XYZ values, because the lighting applies it while rendering, so the
//!    tone map itself runs at exposure 1, as it does for every picture.
//! 2. The difference of two pixels is CIEDE2000 between their L*a*b* display colors.
//! 3. `metrics` turns the per-pixel differences into the reported numbers.
//!
//! # Output
//!
//! One `key: value` line per number, or `--json` (keys sorted); `--out FILE` writes the report to
//! a file instead. `--error-png FILE` adds a false-color Delta E map (`error_map`). Exit codes as
//! the other commands: 1 for two images of different sizes, 2 for a job file that cannot be used,
//! 3 for a file that cannot be read or written.

mod display;
mod error_map;
mod metrics;
#[cfg(test)]
mod tests;

use self::{
    display::DisplayImage,
    metrics::{Comparison, Metrics},
};
use super::deliver;
use crate::{
    args::ImageCompareArgs,
    format::{fixed, json_number, json_text},
    outcome::{CliError, CommandResult},
    raw_image::RawImage,
};
use pleochroma::color::ColorSpace;
use pleochroma_render_jobs::{JobColorSpace, JobKind, codec};
use serde_json::{Value, json};
use std::path::Path;

/// Decimals of every real number of the report.
const DECIMALS: usize = 4;

/// Runs `image-compare`.
///
/// # Errors
///
/// A usage error when the images differ in size, an I/O error when an image is not a readable
/// `PLRAW1` file or the map cannot be encoded, a design error when the job file cannot be used.
pub fn run(args: &ImageCompareArgs) -> CommandResult {
    let reference = read_image(&args.reference)?;
    let run_image = read_image(&args.run)?;
    require_same_size(&args.reference, &reference, &args.run, &run_image)?;
    let second = match &args.ref2 {
        Some(path) => {
            let image = read_image(path)?;
            require_same_size(&args.reference, &reference, path, &image)?;
            Some(image)
        }
        None => None,
    };
    let space = match &args.tonemap_from {
        Some(path) => job_color_space(path)?,
        None => ColorSpace::Srgb,
    };
    let reference_view = DisplayImage::from_raw(&reference, space);
    let run_view = DisplayImage::from_raw(&run_image, space);
    let second_view = second
        .as_ref()
        .map(|image| DisplayImage::from_raw(image, space));
    let Comparison { metrics, error_map } = metrics::compare(
        &reference_view,
        &run_view,
        second_view.as_ref(),
        &run_image.counts,
    );
    let report = if args.json {
        json_text(&json_report(&metrics, &reference, space))
    } else {
        text_report(&metrics)
    };
    let mut outcome = deliver(report, args.out.as_deref());
    if let Some(path) = &args.error_png {
        let rgba = error_map::to_rgba(&error_map);
        let png = pleochroma::render_setup::encode_png_with_icc(
            &rgba,
            reference.width,
            reference.height,
            ColorSpace::Srgb,
        )
        .map_err(|error| CliError::io(format!("cannot encode {}: {error}", path.display())))?;
        outcome = outcome.with_binary_file(path.clone(), png);
    }
    Ok(outcome)
}

/// Reads and checks a `PLRAW1` file.
fn read_image(path: &Path) -> Result<RawImage, CliError> {
    let bytes = std::fs::read(path)
        .map_err(|error| CliError::io(format!("cannot read {}: {error}", path.display())))?;
    RawImage::from_bytes(&bytes)
        .map_err(|error| CliError::io(format!("{}: {error}", path.display())))
}

/// Refuses two images of different sizes, naming both.
fn require_same_size(
    first_path: &Path,
    first: &RawImage,
    second_path: &Path,
    second: &RawImage,
) -> Result<(), CliError> {
    if (first.width, first.height) == (second.width, second.height) {
        return Ok(());
    }
    Err(CliError::usage(format!(
        "image-compare needs two images of the same size: {} is {} x {} pixels, {} is {} x {}",
        first_path.display(),
        first.width,
        first.height,
        second_path.display(),
        second.width,
        second.height,
    )))
}

/// The color space of the picture a job writes.
fn job_color_space(path: &Path) -> Result<ColorSpace, CliError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| CliError::io(format!("cannot read {}: {error}", path.display())))?;
    let job = codec::from_text(&text)
        .map_err(|error| CliError::design(format!("{}: {error}", path.display())))?;
    let space = match &job.kind {
        JobKind::Still(still) => still.color_space,
        JobKind::TiltVideo(video) => video.color_space,
    };
    Ok(match space {
        JobColorSpace::Srgb => ColorSpace::Srgb,
        JobColorSpace::DisplayP3 => ColorSpace::DisplayP3,
        JobColorSpace::Rec2020 => ColorSpace::Rec2020,
    })
}

/// The JSON name of a color space.
const fn space_word(space: ColorSpace) -> &'static str {
    match space {
        ColorSpace::Srgb => "srgb",
        ColorSpace::DisplayP3 => "display_p3",
        ColorSpace::Rec2020 => "rec2020",
        ColorSpace::AcesCg => "aces_cg",
    }
}

/// The report as `key: value` lines.
fn text_report(metrics: &Metrics) -> String {
    let mut lines = Vec::new();
    if let Some(floor) = metrics.noise_floor {
        lines.push(("noise_floor_de.mean", fixed(floor.mean, DECIMALS)));
        lines.push(("noise_floor_de.p99", fixed(floor.p99, DECIMALS)));
    }
    lines.push((
        "changed_area_pct",
        fixed(metrics.changed_area_pct, DECIMALS),
    ));
    lines.push((
        "changed_threshold_de",
        fixed(metrics.changed_threshold_de, DECIMALS),
    ));
    lines.push(("worst_tile_de", fixed(metrics.worst_tile_de, DECIMALS)));
    lines.push((
        "glint_recall",
        metrics
            .glint_recall
            .map_or_else(|| "no glints".to_owned(), |recall| fixed(recall, DECIMALS)),
    ));
    lines.push(("samples_used", metrics.samples_used.to_string()));
    lines.push(("max_spp", metrics.max_spp.to_string()));
    lines.push(("samples_saving", fixed(metrics.samples_saving, DECIMALS)));
    lines.push(("mean_de", fixed(metrics.mean_de, DECIMALS)));
    lines.push(("p99_de", fixed(metrics.p99_de, DECIMALS)));
    let mut text = String::new();
    for (key, value) in lines {
        text.push_str(key);
        text.push_str(": ");
        text.push_str(&value);
        text.push('\n');
    }
    text
}

/// The report as a JSON object (the map sorts its keys).
fn json_report(metrics: &Metrics, image: &RawImage, space: ColorSpace) -> Value {
    let decimals = i32::try_from(DECIMALS).unwrap_or(4);
    let mut report = json!({
        "changed_area_pct": json_number(metrics.changed_area_pct, decimals),
        "changed_threshold_de": json_number(metrics.changed_threshold_de, decimals),
        "color_space": space_word(space),
        "glint_recall": metrics
            .glint_recall
            .map_or(Value::Null, |recall| json_number(recall, decimals)),
        "height": image.height,
        "max_spp": metrics.max_spp,
        "mean_de": json_number(metrics.mean_de, decimals),
        "p99_de": json_number(metrics.p99_de, decimals),
        "samples_saving": json_number(metrics.samples_saving, decimals),
        "samples_used": metrics.samples_used,
        "width": image.width,
        "worst_tile_de": json_number(metrics.worst_tile_de, decimals),
    });
    if let Some(floor) = metrics.noise_floor {
        report["noise_floor_de"] = json!({
            "mean": json_number(floor.mean, decimals),
            "p99": json_number(floor.p99, decimals),
        });
    }
    report
}
