//! Android sample choices, independent of phone/tablet window size.

use pleochroma_mobile_core::{
    app_state::AppState,
    settings::{RenderBackend, RenderQuality, RenderSettings},
};

const LOCAL: &[u32] = &[32, 64];
const REMOTE: &[u32] = &[32, 64, 128, 256];

pub(super) fn remote_ready(app: &AppState) -> bool {
    app.settings.render.backend == RenderBackend::Remote && crate::remote::render_connected(app)
}

pub(super) const fn choices(remote: bool) -> &'static [u32] {
    if remote { REMOTE } else { LOCAL }
}

/// Migrates old quality-based targets and clamps saved tablet settings before rendering.
pub(super) fn normalize(settings: &mut RenderSettings, remote: bool) {
    let cap = if remote { 256 } else { 64 };
    if settings.engine_spec().is_ray() {
        settings.live_spp_compact = settings.live_spp_compact.min(cap);
        settings.live_spp_large = settings.live_spp_large.min(cap);
        return;
    }
    let requested = match settings.quality {
        RenderQuality::Draft => 32,
        RenderQuality::Normal => settings.live_spp_compact,
        RenderQuality::Fine => settings.live_spp_compact.saturating_mul(2),
    };
    let target = choices(remote)
        .iter()
        .copied()
        .rev()
        .find(|&value| value <= requested.min(cap))
        .unwrap_or(32);
    settings.quality = RenderQuality::Normal;
    settings.live_spp_compact = target;
    settings.live_spp_large = target;
}

pub(super) fn apply(app: &mut AppState) {
    let remote = remote_ready(app);
    normalize(&mut app.settings.render, remote);
}

pub(super) fn select(app: &mut AppState, index: i32) -> bool {
    let choices = choices(remote_ready(app));
    let Some(&target) = usize::try_from(index).ok().and_then(|i| choices.get(i)) else {
        return false;
    };
    app.update_settings(|settings| {
        settings.render.quality = RenderQuality::Normal;
        settings.render.live_spp_compact = target;
        settings.render.live_spp_large = target;
    });
    true
}

pub(super) fn export_settings(app: &AppState) -> RenderSettings {
    let mut settings = app.settings.render.clone();
    settings.export_cap_spp = settings
        .export_cap_spp
        .min(if remote_ready(app) { 256 } else { 64 });
    if local_path_gpu(&settings) {
        settings.export_cap_px = settings.export_cap_px.min(512);
    }
    settings
}

pub(super) const fn local_path_gpu(settings: &RenderSettings) -> bool {
    !settings.engine_spec().is_ray()
        && matches!(
            settings.backend,
            RenderBackend::Automatic | RenderBackend::DeviceGpu
        )
}

/// Use the core's compact render budget without changing the tablet's UI layout.
pub(super) fn maintain(
    app: &mut AppState,
    now: std::time::Duration,
) -> pleochroma_mobile_core::viewport::render_view::Maintain {
    let layout = app.window_class;
    if local_path_gpu(&app.settings.render) {
        app.window_class = pleochroma_mobile_core::layout::WindowClass::Compact;
    }
    let outcome = pleochroma_mobile_core::viewport::render_view::maintain(app, now);
    app.window_class = layout;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tablet_and_saved_fine_targets_are_safe_locally() {
        for quality in [
            RenderQuality::Draft,
            RenderQuality::Normal,
            RenderQuality::Fine,
        ] {
            let mut settings = RenderSettings {
                quality,
                ..RenderSettings::default()
            };
            normalize(&mut settings, false);
            assert!(settings.live_spp_compact <= 64);
            assert_eq!(settings.live_spp_large, settings.live_spp_compact);
            assert_eq!(settings.quality, RenderQuality::Normal);
        }
    }

    #[test]
    fn remote_targets_survive_normalization_but_not_a_local_switch() {
        for target in [128, 256] {
            let mut settings = RenderSettings {
                live_spp_compact: target,
                live_spp_large: target,
                ..RenderSettings::default()
            };
            normalize(&mut settings, true);
            assert_eq!(settings.live_spp_compact, target);
            normalize(&mut settings, true);
            assert_eq!(settings.live_spp_large, target);
            normalize(&mut settings, false);
            assert_eq!(settings.live_spp_large, 64);
        }
        assert_eq!(choices(false), &[32, 64]);
        assert_eq!(choices(true), &[32, 64, 128, 256]);
    }

    #[test]
    fn only_local_path_gpu_uses_the_compact_raster_budget() {
        for backend in [
            RenderBackend::Automatic,
            RenderBackend::DeviceGpu,
            RenderBackend::DeviceCpu,
            RenderBackend::Remote,
        ] {
            let mut settings = RenderSettings {
                backend,
                ..RenderSettings::default()
            };
            assert_eq!(
                local_path_gpu(&settings),
                matches!(backend, RenderBackend::Automatic | RenderBackend::DeviceGpu)
            );
            settings.engine = 1;
            assert!(!local_path_gpu(&settings));
        }
        for size in [(1920.0, 1200.0), (1200.0, 1920.0), (1200.0, 1200.0)] {
            let (w, h) =
                pleochroma_mobile_core::viewport::render_view::trace_size(size, true, false);
            assert!(u64::from(w) * u64::from(h) <= 512 * 512);
        }
    }
}
