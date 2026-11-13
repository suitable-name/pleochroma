use super::{
    display::DisplayImage,
    error_map::false_color,
    metrics::{NoiseFloor, compare},
};
use crate::{
    outcome::{EXIT_DESIGN, EXIT_IO, EXIT_OK, EXIT_USAGE},
    raw_image::RawImage,
    testing,
};
use glam::Vec3;
use pleochroma::{
    color::{ColorSpace, srgb_to_lab},
    renderer::tonemap::tonemap_accumulation,
};
use serde_json::Value;

/// An image of `width` x `height` whose pixel colors and sample counts come from the closures.
fn raw(
    width: u32,
    height: u32,
    pixel: impl Fn(u32, u32) -> [f32; 3],
    count: impl Fn(u32, u32) -> u32,
) -> RawImage {
    let mut xyz = Vec::new();
    let mut counts = Vec::new();
    for y in 0..height {
        for x in 0..width {
            xyz.push(pixel(x, y));
            counts.push(count(x, y));
        }
    }
    RawImage {
        width,
        height,
        xyz,
        counts,
    }
}

fn view(image: &RawImage) -> DisplayImage {
    DisplayImage::from_raw(image, ColorSpace::Srgb)
}

fn gradient(x: u32, y: u32) -> [f32; 3] {
    let value = 0.01_f32.mul_add((x + y) as f32, 0.05);
    [value, value * 1.02, value * 0.95]
}

/// The path of a `PLRAW1` file called `name` in this test process's folder.
fn write_image(name: &str, image: &RawImage) -> String {
    let path = testing::temp_path(name);
    std::fs::write(&path, image.to_bytes().expect("the image encodes")).expect("writable");
    path.to_string_lossy().into_owned()
}

#[test]
fn identical_images_have_no_difference_and_no_glints_to_recall() {
    let image = raw(40, 30, gradient, |_, _| 64);
    let shown = view(&image);
    let result = compare(&shown, &shown, Some(&shown), &image.counts);
    let metrics = result.metrics;
    assert!(metrics.mean_de.abs() < 1e-6, "{}", metrics.mean_de);
    assert!(metrics.p99_de.abs() < 1e-6, "{}", metrics.p99_de);
    assert!(metrics.worst_tile_de.abs() < 1e-6);
    assert_eq!(metrics.changed_area_pct, 0.0);
    // The gradient's maximum is its 99.9th percentile: no pixel is strictly above it.
    assert_eq!(metrics.glint_recall, None);
    let floor = metrics.noise_floor.expect("a second reference was given");
    assert!(floor.mean.abs() < 1e-6 && floor.p99.abs() < 1e-6);
    assert_eq!(metrics.changed_threshold_de, 1.0);
    assert!(result.error_map.iter().all(|de| de.abs() < 1e-6));
}

#[test]
fn a_color_shift_on_a_known_share_of_pixels_is_that_changed_area() {
    let reference = raw(32, 32, |_, _| [0.1, 0.1, 0.1], |_, _| 16);
    let run = raw(
        32,
        32,
        |x, _| {
            if x < 8 {
                [0.4, 0.42, 0.46]
            } else {
                [0.1, 0.1, 0.1]
            }
        },
        |_, _| 16,
    );
    let result = compare(&view(&reference), &view(&run), None, &run.counts);
    let metrics = result.metrics;
    assert_eq!(metrics.noise_floor, None);
    assert_eq!(metrics.changed_threshold_de, 1.0);
    assert_eq!(metrics.changed_area_pct, 25.0);
    assert!(metrics.p99_de > 1.0, "{}", metrics.p99_de);
    // Eight of the 16 columns of the first tile of each tile row are shifted.
    let shifted = f64::from(result.error_map[0]);
    assert!(shifted > 1.0);
    assert_eq!(metrics.worst_tile_de, shifted / 2.0);
}

#[test]
fn the_noise_floor_raises_the_changed_threshold() {
    let reference = raw(32, 32, |_, _| [0.1, 0.1, 0.1], |_, _| 16);
    let run = raw(
        32,
        32,
        |x, _| {
            if x < 8 {
                [0.4, 0.42, 0.46]
            } else {
                [0.1, 0.1, 0.1]
            }
        },
        |_, _| 16,
    );
    let (shown_reference, shown_run) = (view(&reference), view(&run));
    let without = compare(&shown_reference, &shown_run, None, &run.counts).metrics;
    // The second reference differs from the first exactly as the run does: pure noise.
    let with = compare(&shown_reference, &shown_run, Some(&shown_run), &run.counts).metrics;
    assert_eq!(without.changed_area_pct, 25.0);
    assert_eq!(with.changed_area_pct, 0.0);
    let floor = with.noise_floor.expect("a second reference was given");
    assert_eq!(
        floor,
        NoiseFloor {
            mean: with.mean_de,
            p99: with.p99_de,
        }
    );
    assert_eq!(with.changed_threshold_de, floor.p99);
}

#[test]
fn a_missing_glint_lowers_the_recall_and_marks_its_tile() {
    // 48 x 24 pixels: three full tile columns, one full and one 8-row-high tile row.
    let dark = [0.02, 0.02, 0.02];
    let glint_at = |at: (u32, u32)| {
        raw(
            48,
            24,
            move |x, y| {
                if (x, y) == at {
                    [20.0, 20.0, 20.0]
                } else {
                    dark
                }
            },
            |_, _| 16,
        )
    };
    let reference = glint_at((40, 20));
    let flat = raw(48, 24, |_, _| dark, |_, _| 16);
    let shown_reference = view(&reference);

    let missing = compare(&shown_reference, &view(&flat), None, &flat.counts);
    assert_eq!(missing.metrics.glint_recall, Some(0.0));
    // The glint sits in the 16 x 8 bottom-right tile, so its mean is the pixel's Delta E / 128.
    let glint_de = f64::from(missing.error_map[20 * 48 + 40]);
    assert!(glint_de > 10.0, "{glint_de}");
    assert_eq!(missing.metrics.worst_tile_de, glint_de / 128.0);

    let kept = compare(&shown_reference, &shown_reference, None, &reference.counts);
    assert_eq!(kept.metrics.glint_recall, Some(1.0));

    let moved = glint_at((10, 5));
    let elsewhere = compare(&shown_reference, &view(&moved), None, &moved.counts);
    assert_eq!(elsewhere.metrics.glint_recall, Some(0.0));
}

#[test]
fn a_clipped_reference_has_no_glints_and_a_null_recall() {
    // Every pixel is far above display white: the 99.9th percentile is the maximum.
    let clipped = raw(48, 24, |_, _| [20.0, 20.0, 20.0], |_, _| 16);
    let flat = raw(48, 24, |_, _| [0.02, 0.02, 0.02], |_, _| 16);
    let result = compare(&view(&clipped), &view(&flat), None, &flat.counts);
    assert_eq!(result.metrics.glint_recall, None);

    let path = write_image("image-compare-clipped.plraw", &clipped);
    let outcome = testing::run(&["image-compare", &path, &path, "--json"]);
    assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    let report: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
    assert!(report["glint_recall"].is_null(), "{}", outcome.stdout);
}

#[test]
fn mixed_sample_counts_give_exact_totals_and_savings() {
    let image = raw(4, 4, gradient, |_, y| if y < 2 { 16 } else { 64 });
    let shown = view(&image);
    let metrics = compare(&shown, &shown, None, &image.counts).metrics;
    assert_eq!(metrics.samples_used, 8 * 16 + 8 * 64);
    assert_eq!(metrics.max_spp, 64);
    assert_eq!(metrics.samples_saving, 0.375);
    let uniform = raw(4, 4, gradient, |_, _| 64);
    let metrics = compare(&shown, &shown, None, &uniform.counts).metrics;
    assert_eq!(metrics.samples_used, 16 * 64);
    assert_eq!(metrics.samples_saving, 0.0);
}

#[test]
fn the_display_colors_agree_with_the_srgb_lab_conversion() {
    let image = raw(1, 1, |_, _| [0.3, 0.25, 0.2], |_, _| 1);
    let shown = view(&image);
    let rgba = tonemap_accumulation(1, 1, 1, &[Vec3::new(0.3, 0.25, 0.2)], ColorSpace::Srgb);
    let expected = srgb_to_lab([
        f64::from(rgba[0]) / 255.0,
        f64::from(rgba[1]) / 255.0,
        f64::from(rgba[2]) / 255.0,
    ]);
    for (found, wanted) in shown.lab[0].iter().zip(expected) {
        // The two conversions use differently rounded matrices: a fifth of a unit is plenty.
        assert!(
            (f64::from(*found) - wanted).abs() < 0.2,
            "{found} vs {wanted}"
        );
    }
}

#[test]
fn the_error_map_colors_follow_the_stops() {
    assert_eq!(false_color(0.0), [0, 0, 0]);
    assert_eq!(false_color(1.0), [0, 0, 255]);
    assert_eq!(false_color(1.5), [0, 128, 128]);
    assert_eq!(false_color(2.0), [0, 255, 0]);
    assert_eq!(false_color(5.0), [255, 255, 0]);
    assert_eq!(false_color(7.5), [255, 128, 0]);
    assert_eq!(false_color(10.0), [255, 0, 0]);
    assert_eq!(false_color(250.0), [255, 0, 0]);
}

#[test]
fn files_are_read_compared_and_reported_in_sorted_json() {
    let reference = raw(32, 32, gradient, |_, _| 32);
    let run = raw(32, 32, gradient, |_, y| if y < 16 { 16 } else { 32 });
    let reference_path = write_image("image-compare-ref.plraw", &reference);
    let run_path = write_image("image-compare-run.plraw", &run);
    let png = testing::temp_path("image-compare-error.png");
    let png_path = png.to_string_lossy().into_owned();

    let outcome = testing::run(&[
        "image-compare",
        &reference_path,
        &run_path,
        "--ref2",
        &reference_path,
        "--error-png",
        &png_path,
        "--json",
    ]);
    assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    let report: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
    let object = report.as_object().expect("a JSON object");
    let keys: Vec<&str> = object.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "changed_area_pct",
            "changed_threshold_de",
            "color_space",
            "glint_recall",
            "height",
            "max_spp",
            "mean_de",
            "noise_floor_de",
            "p99_de",
            "samples_saving",
            "samples_used",
            "width",
            "worst_tile_de",
        ]
    );
    assert_eq!(report["samples_used"], 32 * 16 * 16 + 32 * 16 * 32);
    assert_eq!(report["max_spp"], 32);
    assert_eq!(report["samples_saving"], 0.25);
    assert_eq!(report["color_space"], "srgb");
    assert_eq!(report["changed_area_pct"], 0.0);
    let again = testing::run(&[
        "image-compare",
        &reference_path,
        &run_path,
        "--ref2",
        &reference_path,
        "--error-png",
        &png_path,
        "--json",
    ]);
    assert_eq!(
        again.stdout, outcome.stdout,
        "the same files, the same bytes"
    );

    let map = outcome
        .files
        .iter()
        .find(|file| file.path == png)
        .expect("the error map is a file of the outcome");
    let bytes = map.bytes.as_deref().expect("a PNG is binary");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn the_text_report_has_one_line_per_number_and_can_go_to_a_file() {
    let image = raw(32, 32, gradient, |_, _| 8);
    let path = write_image("image-compare-text.plraw", &image);
    let outcome = testing::run(&["image-compare", &path, &path]);
    assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    let keys: Vec<&str> = outcome
        .stdout
        .lines()
        .map(|line| line.split(": ").next().expect("a key"))
        .collect();
    assert_eq!(
        keys,
        [
            "changed_area_pct",
            "changed_threshold_de",
            "worst_tile_de",
            "glint_recall",
            "samples_used",
            "max_spp",
            "samples_saving",
            "mean_de",
            "p99_de",
        ]
    );
    assert!(outcome.stdout.contains("changed_area_pct: 0.0000\n"));
    assert!(outcome.stdout.contains("glint_recall: no glints\n"));
    assert!(outcome.stdout.contains("samples_used: 8192\n"));

    let target = testing::temp_path("image-compare-report.txt");
    let target_text = target.to_string_lossy().into_owned();
    let written = testing::run(&["image-compare", &path, &path, "--out", &target_text]);
    assert_eq!(written.exit, EXIT_OK, "{}", written.stderr);
    assert_eq!(written.stdout, "");
    assert_eq!(
        testing::file_text(&written, &target_text),
        Some(outcome.stdout.as_str())
    );
}

#[test]
fn images_of_different_sizes_are_a_usage_error_naming_both() {
    let wide = write_image("image-compare-wide.plraw", &raw(40, 30, gradient, |_, _| 4));
    let tall = write_image("image-compare-tall.plraw", &raw(30, 40, gradient, |_, _| 4));
    let outcome = testing::run(&["image-compare", &wide, &tall]);
    assert_eq!(outcome.exit, EXIT_USAGE, "{}", outcome.stderr);
    assert!(outcome.stderr.contains("40 x 30"), "{}", outcome.stderr);
    assert!(outcome.stderr.contains("30 x 40"), "{}", outcome.stderr);
    assert!(outcome.stderr.contains(&wide), "{}", outcome.stderr);
    assert!(outcome.stderr.contains(&tall), "{}", outcome.stderr);

    let third = testing::run(&["image-compare", &wide, &wide, "--ref2", &tall]);
    assert_eq!(third.exit, EXIT_USAGE, "{}", third.stderr);
}

#[test]
fn an_unreadable_or_invalid_file_is_exit_3() {
    let good = write_image("image-compare-good.plraw", &raw(16, 16, gradient, |_, _| 1));
    let missing = testing::temp_path("image-compare-none.plraw");
    let missing_text = missing.to_string_lossy().into_owned();
    let outcome = testing::run(&["image-compare", &good, &missing_text]);
    assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
    assert!(
        outcome.stderr.starts_with("error: cannot read"),
        "{}",
        outcome.stderr
    );

    let bad = testing::temp_path("image-compare-bad.plraw");
    std::fs::write(&bad, b"not an image").expect("writable");
    let bad_text = bad.to_string_lossy().into_owned();
    let outcome = testing::run(&["image-compare", &good, &bad_text]);
    assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
    assert!(outcome.stderr.contains("PLRAW1"), "{}", outcome.stderr);
}

#[test]
fn the_job_names_the_color_space_and_a_bad_job_is_exit_2() {
    let image = raw(16, 16, gradient, |_, _| 1);
    let path = write_image("image-compare-job-image.plraw", &image);
    let job = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../benchmarks/render_accel/jobs/s2_sapphire.job.json"
    );
    let outcome = testing::run(&[
        "image-compare",
        &path,
        &path,
        "--json",
        "--tonemap-from",
        job,
    ]);
    assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    let report: Value = serde_json::from_str(&outcome.stdout).expect("valid JSON");
    assert_eq!(report["color_space"], "srgb");

    let not_a_job = testing::temp_path("image-compare-not-a-job.job.json");
    std::fs::write(&not_a_job, b"{}").expect("writable");
    let not_a_job_text = not_a_job.to_string_lossy().into_owned();
    let outcome = testing::run(&[
        "image-compare",
        &path,
        &path,
        "--tonemap-from",
        &not_a_job_text,
    ]);
    assert_eq!(outcome.exit, EXIT_DESIGN, "{}", outcome.stderr);
}
