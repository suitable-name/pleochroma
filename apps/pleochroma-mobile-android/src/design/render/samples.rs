//! Android sample choices, independent of phone/tablet window size.

use pleochroma_mobile_core::{
    app_state::AppState,
    settings::{RenderBackend, RenderQuality, RenderSettings},
};

const LOCAL: &[u32] = &[16, 32, 64];
const REMOTE: &[u32] = &[32, 64, 128, 256];

pub(super) fn remote_ready(app: &AppState) -> bool {
    app.settings.render.backend == RenderBackend::Remote && crate::remote::render_connected(app)
}

pub(super) const fn choices(remote: bool) -> &'static [u32] {
    if remote { REMOTE } else { LOCAL }
}

/// Clamps the sample targets to the backend's cap. The quality setting is mapped onto a
/// sample target for the sample-based engines. The local Path-Traced preview stays at 32
/// samples until the user actively picks 64.
pub(super) fn normalize(settings: &mut RenderSettings, remote: bool) {
    let cap = if remote { 256 } else { 64 };
    if settings.engine_spec().is_ray() {
        settings.live_spp_compact = settings.live_spp_compact.min(cap);
        settings.live_spp_large = settings.live_spp_large.min(cap);
        return;
    }
    let local_cap = if settings.local_spp_chosen { 64 } else { 32 };
    let cap = if remote { cap } else { local_cap };
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
    let remote = remote_ready(app);
    let choices = choices(remote);
    let Some(&target) = usize::try_from(index).ok().and_then(|i| choices.get(i)) else {
        return false;
    };
    app.update_settings(|settings| {
        settings.render.quality = RenderQuality::Normal;
        settings.render.live_spp_compact = target;
        settings.render.live_spp_large = target;
        // Only a pick on this device is the user's choice of 64 local samples; a remote pick
        // says nothing about what the phone itself may do.
        if !remote {
            settings.render.local_spp_chosen = target >= 64;
        }
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

/// Runs the core's maintenance with the compact window class on local GPU paths, then
/// restores the real window class, so the budget does not change the tablet's layout.
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

    /// The defaults on Path-Traced, whose sample targets these tests cover (a fresh install
    /// starts on Ray-Traced, which finishes at its tap count instead).
    fn path_traced() -> RenderSettings {
        RenderSettings {
            engine: 0,
            ..RenderSettings::default()
        }
    }

    #[test]
    fn tablet_and_saved_fine_targets_are_safe_locally() {
        for quality in [
            RenderQuality::Draft,
            RenderQuality::Normal,
            RenderQuality::Fine,
        ] {
            let mut settings = RenderSettings {
                quality,
                ..path_traced()
            };
            normalize(&mut settings, false);
            assert_eq!(settings.live_spp_compact, 32);
            assert_eq!(settings.live_spp_large, settings.live_spp_compact);
            assert_eq!(settings.quality, RenderQuality::Normal);
        }
    }

    #[test]
    fn a_fresh_install_uses_32_samples_locally() {
        let settings = RenderSettings::default();
        assert_eq!(
            (settings.live_spp_compact, settings.live_spp_large),
            (32, 32)
        );
        assert!(!settings.local_spp_chosen);
    }

    #[test]
    fn an_old_saved_64_falls_back_to_32_until_picked() {
        // A 0.11.1 file has 64 / 128 and no flag; the flag loads as off (see the settings test).
        let mut settings = RenderSettings {
            live_spp_compact: 64,
            live_spp_large: 128,
            ..path_traced()
        };
        assert!(!settings.local_spp_chosen);
        normalize(&mut settings, false);
        assert_eq!(
            (settings.live_spp_compact, settings.live_spp_large),
            (32, 32)
        );
    }

    #[test]
    fn a_chosen_64_persists_and_normalizes_to_64() {
        let mut settings = RenderSettings {
            live_spp_compact: 64,
            live_spp_large: 64,
            local_spp_chosen: true,
            ..path_traced()
        };
        let mut back = settings.clone();
        normalize(&mut back, false);
        assert_eq!((back.live_spp_compact, back.live_spp_large), (64, 64));
        // The chosen target is also what the list shows (index 2 of the local choices).
        assert_eq!(
            choices(false)
                .iter()
                .position(|&s| s == back.live_spp_compact),
            Some(2)
        );
        // Draft still drops to 32.
        settings.quality = RenderQuality::Draft;
        normalize(&mut settings, false);
        assert_eq!(settings.live_spp_compact, 32);
    }

    #[test]
    fn a_chosen_16_stays_16_locally() {
        let mut settings = RenderSettings {
            live_spp_compact: 16,
            live_spp_large: 16,
            ..path_traced()
        };
        normalize(&mut settings, false);
        assert_eq!(
            (settings.live_spp_compact, settings.live_spp_large),
            (16, 16)
        );
        assert!(!settings.local_spp_chosen);
        assert_eq!(choices(false).iter().position(|&s| s == 16), Some(0));
    }

    #[test]
    fn remote_targets_survive_normalization_but_not_a_local_switch() {
        for target in [128, 256] {
            let mut settings = RenderSettings {
                live_spp_compact: target,
                live_spp_large: target,
                ..path_traced()
            };
            normalize(&mut settings, true);
            assert_eq!(settings.live_spp_compact, target);
            normalize(&mut settings, true);
            assert_eq!(settings.live_spp_large, target);
            normalize(&mut settings, false);
            assert_eq!(settings.live_spp_large, 32);
            settings.live_spp_compact = target;
            settings.local_spp_chosen = true;
            normalize(&mut settings, false);
            assert_eq!(settings.live_spp_large, 64);
        }
        assert_eq!(choices(false), &[16, 32, 64]);
        assert_eq!(choices(true), &[32, 64, 128, 256]);
    }

    #[test]
    fn a_remote_pick_does_not_choose_64_local_samples() {
        use pleochroma_mobile_core::platform_mock::MockPlatform;
        use std::sync::Arc;

        let mut app = AppState::new(Arc::new(MockPlatform::default()));
        app.update_settings(|settings| {
            settings.remote.enrolled = true;
            "host:1".clone_into(&mut settings.remote.address);
            settings.render.backend = RenderBackend::Remote;
        });
        crate::remote::record_render_connection(&app, true);
        assert!(remote_ready(&app));
        // Index 2 of the remote list is 128 samples.
        assert!(select(&mut app, 2));
        assert_eq!(app.settings.render.live_spp_compact, 128);
        assert!(!app.settings.render.local_spp_chosen);
        // On this device a pick of 64 is the user's choice.
        crate::remote::record_render_connection(&app, false);
        assert!(select(&mut app, 2));
        assert!(app.settings.render.local_spp_chosen);
        crate::remote::record_render_connection(&app, false);
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
                ..path_traced()
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
