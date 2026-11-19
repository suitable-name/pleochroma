//! The Android entry point: `android_main`, called by the NativeActivity glue on the
//! thread it creates for the app.
//!
//! It installs logging, starts Slint's Android backend with a listener that forwards the
//! activity lifecycle to the UI, builds the JNI [`AndroidPlatform`], runs the start-up check
//! of the diagnostics and sets the shader cache folder it allows, and hands the platform to
//! [`crate::run::run`]. Nothing here may panic out of `android_main`: a failure is
//! logged and the function returns, which ends the activity's native thread.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Once},
};

use pleochroma_mobile_core::app_state::Lifecycle;
use slint::android::{
    AndroidApp,
    android_activity::{MainEvent, PollEvent},
};

use crate::platform_android::{AndroidPlatform, init_logging};

/// Called by the `android-activity` glue once the native activity exists.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    init_logging();
    tracing::info!("Pleochroma starting (android_main)");
    if catch_unwind(AssertUnwindSafe(|| start(app))).is_err() {
        tracing::error!("android_main ended by a panic; the backtrace is in the log above");
    }
}

/// Starts the backend, the platform and the UI, and runs until the activity is destroyed.
fn start(app: AndroidApp) {
    if let Err(error) = slint::android::init_with_event_listener(app.clone(), forward_lifecycle) {
        tracing::error!(%error, "could not start Slint's Android backend");
        return;
    }
    let platform = match AndroidPlatform::new(&app) {
        Ok(platform) => platform,
        Err(error) => {
            tracing::error!(%error, "could not set up the Android platform services");
            return;
        }
    };
    let platform: Arc<dyn pleochroma_mobile_core::platform::Platform> = Arc::new(platform);
    prepare_gpu_cache(&platform);
    if let Err(error) = crate::run::run(&platform) {
        tracing::error!(%error, "the UI ended with an error");
    }
    tracing::info!("Pleochroma finished (android_main returns)");
}

/// The start-up check of the diagnostics, then the shader cache folder it allows.
///
/// Runs once per process and before any GPU work, in this order: logging (already up), the
/// check, then the folder. A later `android_main` in the same process does not repeat it.
fn prepare_gpu_cache(platform: &Arc<dyn pleochroma_mobile_core::platform::Platform>) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let folder = pleochroma_mobile_core::diagnostics::exit::process_start(platform);
        set_shader_cache(folder);
    });
}

/// Turns the shader cache on in `folder`, or leaves it off for `None`.
#[cfg(feature = "device-gpu")]
fn set_shader_cache(folder: Option<PathBuf>) {
    if let Some(folder) = folder
        && !pleochroma::renderer::gpu::pipeline_cache::set_pipeline_cache_dir(folder)
    {
        tracing::info!("the shader cache folder was already set");
    }
}

/// Without the device GPU there is no shader cache to turn on.
#[cfg(not(feature = "device-gpu"))]
fn set_shader_cache(_folder: Option<PathBuf>) {}

/// Maps the activity lifecycle events Slint's loop sees onto [`Lifecycle`].
fn forward_lifecycle(event: &PollEvent<'_>) {
    let PollEvent::Main(main_event) = event else {
        return;
    };
    let lifecycle = match main_event {
        MainEvent::Pause => Lifecycle::Paused,
        MainEvent::Resume { .. } => Lifecycle::Resumed,
        MainEvent::Stop | MainEvent::Destroy => Lifecycle::Stopping,
        _ => return,
    };
    crate::run::notify_lifecycle(lifecycle);
}
