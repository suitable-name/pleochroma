//! Resolves the video export's folder-name template into a sanitised folder name.
//!
//! The template uses `{design}`, `{material}`, `{axis}`, `{fps}` and so on. The video-only
//! variables are handled by `substitute_video_only`.
//!
//! This is a separate resolver rather than `filename_template::resolve_export_path`. That
//! one appends `.png` and resolves a file path, while a video needs a folder name (the
//! parent of the numbered frames and the muxed video). Its sanitiser and collision check
//! are private. This module reuses the public `filename_template::render` and
//! `TemplateContext` for the variables both share, and substitutes the video-only
//! variables first.

use crate::bridge::export_thread::filename_template::{self, TemplateContext};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

/// The default folder-name template for a tilt video export.
///
/// The Slint default (`TiltVideoExportModel.filename_template`) and the settings default
/// (`DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE`) repeat this string, because a Slint property
/// default cannot reference a Rust constant. Keep all three equal.
pub const DEFAULT_VIDEO_TEMPLATE: &str =
    "tilt_video_{design}_{material}_axis{axis}_{width}x{height}_{timestamp}";

/// Video-only template variables, which `filename_template::render` does not know. They
/// are substituted first, and the result goes to `render` for the shared variables.
#[derive(Debug, Clone)]
pub struct VideoTemplateExtras {
    /// `{axis}`: the axes' label, as `axes_label` builds it (e.g. `"45"` or `"0+90"`).
    pub axis_label: String,
    pub fps: u32,
    pub step_deg: f64,
    pub start_deg: f64,
    pub end_deg: f64,
    pub total_frames: usize,
}

fn substitute_video_only(template: &str, extras: &VideoTemplateExtras) -> String {
    template
        .replace("{axis}", &extras.axis_label)
        .replace("{fps}", &extras.fps.to_string())
        .replace("{step}", &format!("{:.2}", extras.step_deg))
        .replace("{start}", &format!("{:.1}", extras.start_deg))
        .replace("{end}", &format!("{:.1}", extras.end_deg))
        .replace("{frames}", &extras.total_frames.to_string())
}

const fn is_forbidden_char(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 0x20
}

/// Sanitises a rendered folder name with `filename_template`'s forbidden-character and
/// trailing-dot rules.
///
/// That sanitiser is private, so the rules are copied here. Unlike it, there is no reserved
/// device-name check (`CON`, `LPT1`): a folder name built from a template is not
/// realistically one of those, and copying the table is not worth it.
fn sanitize_folder_name(raw: &str) -> String {
    let mut cleaned: String = raw
        .chars()
        .map(|c| if is_forbidden_char(c) { '_' } else { c })
        .collect();
    while matches!(cleaned.chars().last(), Some('.' | ' ')) {
        cleaned.pop();
    }
    if cleaned.is_empty() {
        cleaned = "tilt_video".to_string();
    }
    cleaned
}

/// Renders and sanitises `template` against `ctx` and `extras`, without checking for
/// existing folders.
///
/// The export adds that check with [`unique_folder_path`]. The dialog's live "Resolves to"
/// preview calls this directly, as `ExportModel.preview_filename_template` does: a preview
/// has no collision check.
#[must_use]
pub fn resolve_folder_name(
    template: &str,
    ctx: &TemplateContext,
    extras: &VideoTemplateExtras,
) -> String {
    let pre_substituted = substitute_video_only(template, extras);
    let rendered = filename_template::render(&pre_substituted, ctx, SystemTime::now());
    sanitize_folder_name(&rendered)
}

/// Finds a folder path under `export_dir` that does not exist yet, appending ` (2)`, ` (3)`,
/// ... on collision. The folder version of `filename_template`'s `unique_path`.
#[must_use]
pub fn unique_folder_path(export_dir: &Path, name: &str) -> PathBuf {
    let mut candidate = export_dir.join(name);
    let mut suffix = 2u32;
    while candidate.exists() {
        candidate = export_dir.join(format!("{name} ({suffix})"));
        suffix += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ctx() -> TemplateContext {
        TemplateContext {
            design: "Round Brilliant".to_string(),
            designer: String::new(),
            shape: "Round".to_string(),
            material: "Diamond".to_string(),
            ri: "2.42".to_string(),
            width: 512,
            height: 512,
            spp: 64,
            bounces: 12,
            colorspace: "sRGB".to_string(),
            preset: String::new(),
            lighting: "Gem Studio Ring Lights".to_string(),
            yaw_deg: 0.0,
            pitch_deg: 90.0,
            distance: 2.4,
            exposure: 1.0,
        }
    }

    fn sample_extras() -> VideoTemplateExtras {
        VideoTemplateExtras {
            axis_label: "45".to_string(),
            fps: 30,
            step_deg: 1.0,
            start_deg: -90.0,
            end_deg: 90.0,
            total_frames: 181,
        }
    }

    #[test]
    fn resolve_folder_name_substitutes_both_shared_and_video_only_variables() {
        let name = resolve_folder_name(
            "{design}_{material}_axis{axis}_{width}x{height}_{fps}fps",
            &sample_ctx(),
            &sample_extras(),
        );
        assert_eq!(name, "Round Brilliant_Diamond_axis45_512x512_30fps");
    }

    #[test]
    fn resolve_folder_name_strips_forbidden_characters() {
        let mut ctx = sample_ctx();
        ctx.design = "A/B:C".to_string();
        let name = resolve_folder_name("{design}", &ctx, &sample_extras());
        assert_eq!(name, "A_B_C");
    }

    #[test]
    fn resolve_folder_name_never_returns_empty() {
        let mut ctx = sample_ctx();
        ctx.design.clear();
        let name = resolve_folder_name("", &ctx, &sample_extras());
        assert_eq!(name, "tilt_video");
    }

    #[test]
    fn resolve_folder_name_leaves_an_unrecognised_placeholder_literal() {
        let name = resolve_folder_name("{nonsense}", &sample_ctx(), &sample_extras());
        assert_eq!(name, "{nonsense}");
    }

    #[test]
    fn unique_folder_path_appends_a_counter_on_collision() {
        let dir =
            std::env::temp_dir().join(format!("tilt_video_template_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir(dir.join("clip")).unwrap();

        let resolved = unique_folder_path(&dir, "clip");
        assert_eq!(resolved, dir.join("clip (2)"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
