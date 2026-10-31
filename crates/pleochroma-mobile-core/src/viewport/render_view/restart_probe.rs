//! A cheap stand-in for [`render::needs_restart`]'s scene comparison.
//!
//! `needs_restart` describes the whole scene (`render::scene::prepare`: concave tool
//! placement, frosted planes, the material snapshot, the effects spec) to compare it with
//! what the last start asked for, and the driver asked it on every poll (100 ms) while the
//! render is shown. [`RestartProbe`] holds exactly the inputs `prepare` reads, as cheap
//! copies, so the driver can ask "did any of them change since the last attempt?" with
//! plain comparisons, and run the full comparison only when one did.
//!
//! # The audit: everything `prepare` and `needs_restart` read
//!
//! `scene::prepare(state, view)` reads, and the probe holds:
//!
//! - `state.render.view()` -- the raster `needs_restart` prepares at (`view`);
//! - `state.session.current_generation()` -- the design (planes, tiers, tools, frosting,
//!   effects, the girdle width: every design edit goes through `session.apply`, which bumps
//!   it) and which solve result is current (`generation`);
//! - `state.solve` -- the solve result for that generation: planes, tiers, failure
//!   (`solve_revision`, bumped by every solve start, result and invalidation);
//! - `state.operations.document_id()` (`document`);
//! - `state.window_class` -- only whether it is compact (`compact`), for the sample target
//!   (`render_settings`, `live_target_spp`);
//! - `state.settings.battery_saver` (`live_target_spp`);
//! - `state.settings.render` -- engine, backend, quality, bounces, denoise, spp (`render`);
//! - `state.settings.lighting` (`lighting`) and `state.settings.material_name`
//!   (`material_name`) -- through `render_settings`;
//! - `state.settings.remote`, only while the backend is Remote (`remote`);
//! - `state.props.link_material`, `custom_materials`, `custom_snapshots` -- the material
//!   the render resolves, its recipe, and `snapshot_for` (`link_material`,
//!   `custom_materials`, `custom_snapshots`);
//! - `state.hdr` -- which map lights the scene, by [`HdrState::identity`] (`hdr`);
//! - `state.viewport.camera` -- yaw, pitch and distance, as bits (`camera`).
//!
//! `state.platform()` (the power-save flag `start` reads) is not part of the scene key, so
//! it is not part of the probe either.
//!
//! The probe owns copies, made once per CHANGE (`of`); a poll only compares them with the
//! state ([`RestartProbe::differs_from_state`]) and allocates nothing.
//!
//! [`render::needs_restart`]: crate::render::needs_restart
//! [`HdrState::identity`]: crate::hdr::HdrState::identity

use pleochroma::optics::materials::GemMaterial;
use pleochroma_formats::native::CustomMaterialSnapshot;

use crate::{
    app_state::AppState,
    layout::WindowClass,
    render::RenderView,
    settings::{LightingSettings, RemoteSettings, RenderBackend, RenderSettings},
};

/// The inputs of one scene description, as the last start attempt (or the last
/// "nothing changed" answer) saw them.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RestartProbe {
    view: Option<RenderView>,
    generation: u64,
    solve_revision: u64,
    document: u64,
    compact: bool,
    battery_saver: bool,
    render: RenderSettings,
    lighting: LightingSettings,
    material_name: String,
    link_material: bool,
    remote: Option<RemoteSettings>,
    custom_materials: Vec<GemMaterial>,
    custom_snapshots: Vec<(String, CustomMaterialSnapshot)>,
    hdr: usize,
    camera: [u32; 3],
}

impl RestartProbe {
    /// The probe of `state` as it stands (clones the settings and the custom materials:
    /// call it when something changed, not on every poll).
    pub(super) fn of(state: &AppState) -> Self {
        Self {
            view: state.render.view(),
            generation: state.session.current_generation(),
            solve_revision: state.solve.revision(),
            document: state.operations.document_id(),
            compact: state.window_class == WindowClass::Compact,
            battery_saver: state.settings.battery_saver,
            render: state.settings.render.clone(),
            lighting: state.settings.lighting.clone(),
            material_name: state.settings.material_name.clone(),
            link_material: state.props.link_material,
            remote: remote_of(state).cloned(),
            custom_materials: state.props.custom_materials.clone(),
            custom_snapshots: state.props.custom_snapshots.clone(),
            hdr: state.hdr.identity(),
            camera: camera_bits(state),
        }
    }

    /// Whether any input differs from `state`'s now. Compares field by field, allocates
    /// nothing.
    pub(super) fn differs_from_state(&self, state: &AppState) -> bool {
        self.view != state.render.view()
            || self.generation != state.session.current_generation()
            || self.solve_revision != state.solve.revision()
            || self.document != state.operations.document_id()
            || self.compact != (state.window_class == WindowClass::Compact)
            || self.battery_saver != state.settings.battery_saver
            || self.render != state.settings.render
            || self.lighting != state.settings.lighting
            || self.material_name != state.settings.material_name
            || self.link_material != state.props.link_material
            || self.remote.as_ref() != remote_of(state)
            || self.custom_materials != state.props.custom_materials
            || self.custom_snapshots != state.props.custom_snapshots
            || self.hdr != state.hdr.identity()
            || self.camera != camera_bits(state)
    }
}

/// The remote settings the scene key carries: only while the backend is Remote.
fn remote_of(state: &AppState) -> Option<&RemoteSettings> {
    (state.settings.render.backend == RenderBackend::Remote).then_some(&state.settings.remote)
}

/// The orbit pose the scene reads, as bits (exact, and equal to itself for a NaN).
const fn camera_bits(state: &AppState) -> [u32; 3] {
    let pose = state.viewport.camera;
    [
        pose.yaw.to_bits(),
        pose.pitch.to_bits(),
        pose.distance.to_bits(),
    ]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{platform_mock::MockPlatform, settings::RenderQuality};

    fn state() -> AppState {
        AppState::new(Arc::new(MockPlatform::default()))
    }

    /// Whether the probe taken before `change` differs from the state after it.
    fn differs_after(change: impl FnOnce(&mut AppState)) -> bool {
        let mut app = state();
        let probe = RestartProbe::of(&app);
        change(&mut app);
        probe.differs_from_state(&app)
    }

    #[test]
    fn probes_of_the_same_state_are_equal() {
        let app = state();
        let (first, second) = (RestartProbe::of(&app), RestartProbe::of(&app));
        assert_eq!(first, second);
        assert!(!first.differs_from_state(&app));
        assert!(!differs_after(|_| {}));
    }

    /// One assertion per input `prepare` reads (the audit list in the module docs).
    #[test]
    fn each_input_the_scene_reads_makes_the_probe_differ() {
        assert!(differs_after(|app| app.viewport.camera.yaw += 0.1), "yaw");
        assert!(
            differs_after(|app| app.viewport.camera.pitch += 0.1),
            "pitch"
        );
        assert!(
            differs_after(|app| app.viewport.camera.distance += 0.1),
            "distance"
        );
        assert!(
            differs_after(|app| app.settings.material_name.push('x')),
            "material name"
        );
        assert!(
            differs_after(|app| app.props.custom_materials.push(GemMaterial::diamond())),
            "custom materials"
        );
        assert!(
            differs_after(|app| app.window_class = WindowClass::Expanded),
            "window class"
        );
        assert!(
            differs_after(|app| app.settings.battery_saver = true),
            "battery saver"
        );
        assert!(
            differs_after(|app| app.settings.render.quality = RenderQuality::Fine),
            "render settings"
        );
        assert!(
            differs_after(|app| app.settings.lighting.exposure += 1.0),
            "lighting"
        );
        assert!(
            differs_after(|app| app.props.link_material = !app.props.link_material),
            "link material"
        );
        assert!(
            differs_after(|app| app.settings.render.backend = RenderBackend::Remote),
            "remote backend"
        );
        assert!(
            differs_after(|app| app.session.mark_unsaved()),
            "generation"
        );
        assert!(
            differs_after(|app| app.solve.invalidate()),
            "solve revision"
        );
    }

    /// The inputs a test cannot move through the public state are compared too: moving the
    /// probe's own copy of each makes it differ from the untouched state.
    #[test]
    fn the_probe_compares_the_view_the_hdr_and_the_document() {
        let app = state();
        let base = RestartProbe::of(&app);
        let mut moved = base.clone();
        moved.view = Some(RenderView {
            width: 32,
            height: 24,
        });
        assert!(moved.differs_from_state(&app), "view");
        let mut moved = base.clone();
        moved.hdr += 1;
        assert!(moved.differs_from_state(&app), "hdr");
        let mut moved = base.clone();
        moved.document += 1;
        assert!(moved.differs_from_state(&app), "document");
        let mut moved = base;
        moved.remote = Some(RemoteSettings::default());
        assert!(moved.differs_from_state(&app), "remote");
    }
}
