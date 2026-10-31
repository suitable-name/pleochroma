//! Selecting the correct, independently qualified renderer for the current scene.

use pleochroma_web_core::{
    gpu_admission::{AdmissionState, ISOTROPIC_CLASS, REASON_CLASS_NOT_TESTED, feature_for_reason},
    gpu_startup::GpuState,
    host::WorkerPool,
};
use slint::ComponentHandle;

use super::{
    COMPILE_BLOCKED_TEXT, GATE, begin_test, classes, ray, show, start_admit_compile,
    update_backend_text,
};
use crate::{RenderModel, app::Ctx};

/// Admits the GPU when the first CPU frame is displayed, the adapter passed, the scene is
/// eligible and the kernels of the scene's material class are compiled in this page's
/// worker; starts that compile (`admit`) when it is the only thing missing.
///
/// The isotropic kernels are compiled at start-up. Another class is compiled and tested on
/// first use, once the pool says the scene waits for it ([`classes::ensure_class`]); the
/// scattering and frosted-facet kernels of any class are compiled when a scene first needs them
/// ([`classes::ensure_feature`]).
pub fn admit_if_ready(ctx: &Ctx, pool: &WorkerPool) {
    if pool.render().engine().engine != ctx.state.borrow().settings.engine_spec().engine {
        pool.render().set_gpu_admitted(false);
        return;
    }
    if ray::selected(ctx) {
        ray::admit(ctx, pool);
        return;
    }
    if ray::busy() {
        pool.render().set_gpu_admitted(false);
        return;
    }
    if GATE.with(|gate| gate.borrow().ray_only_device) {
        pool.render().set_gpu_admitted(false);
        return;
    }
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RenderModel>();
    let has_frame = model.get_has_frame();
    let (compiled, running, failed, blocked, record_state) = GATE.with(|gate| {
        let gate = gate.borrow();
        (
            gate.compiled(ISOTROPIC_CLASS),
            gate.busy(),
            gate.failed_in_session,
            gate.compile_blocked,
            gate.records
                .get(&ISOTROPIC_CLASS)
                .map(|record| record.state),
        )
    });
    if compiled && !running && !failed {
        // After a failed compile the worker refuses every later one: only a reload tests again,
        // so "Forget GPU Test Result" is not offered.
        show(
            ctx,
            GpuState::Ok,
            if blocked {
                COMPILE_BLOCKED_TEXT
            } else {
                "Path-Traced GPU kernels are qualified"
            },
            !blocked,
        );
    } else if !running && !failed && !blocked && pool.render().is_gpu_ready() {
        // Never by itself on a blocked worker: the compile would be refused and the refusal
        // stored as a Failed verdict of the unit.
        match record_state {
            Some(AdmissionState::Ok) => start_admit_compile(ctx, pool, ISOTROPIC_CLASS),
            Some(AdmissionState::Unknown) => begin_test(ctx, pool, ISOTROPIC_CLASS),
            _ => {}
        }
    }
    let is_ok = GpuState::from_code(model.get_gpu_state()) == GpuState::Ok;

    let mut admitted = false;
    if has_frame && is_ok {
        let class = pool.render().gpu_scene_class().unwrap_or(ISOTROPIC_CLASS);
        match pool.render().gpu_ineligibility_reason() {
            None => {
                if GATE.with(|gate| gate.borrow().compiled(class)) {
                    admitted = true;
                } else if class == ISOTROPIC_CLASS {
                    classes::ensure_class(ctx, pool, ISOTROPIC_CLASS);
                }
            }
            // The class is admitted but not compiled and tested here yet: first use.
            Some(REASON_CLASS_NOT_TESTED) => classes::ensure_class(ctx, pool, class),
            // The class is compiled but the stone reaches kernels that are not (scattering,
            // frosted facets): they are compiled the first time a scene needs them.
            Some(reason) => {
                if let Some(feature) = feature_for_reason(reason) {
                    classes::ensure_feature(ctx, pool, class, feature);
                }
            }
        }
    }
    pool.render().set_gpu_admitted(admitted);
    update_backend_text(ctx, pool);
}
