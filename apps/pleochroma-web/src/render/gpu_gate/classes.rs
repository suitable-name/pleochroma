//! The gate's per-material-class half: which compile units it keeps records for, the lazy
//! compile of the scene's class and of the kernel features it needs, and the texts that name
//! them.
//!
//! A *unit* is what one `GpuCompile` builds (`gpu_admission::feature_unit`): the base kernel set
//! of a class, named by the class value, or one kernel feature of a class (scattering, frosted
//! facets), compiled the first time a scene needs it. The pure parts (units, kernel lists,
//! keys, labels, the admitted set) are `pleochroma_web_core::gpu_admission`'s and tested
//! natively there. The set the owner admits is `gpu_admission::ADMITTED_CLASSES`; until it lists
//! a class, the pool never reports `REASON_CLASS_NOT_TESTED` for it and nothing here compiles
//! it.

use pleochroma_web_core::{
    gpu_admission::{
        self, AdmissionState, ISOTROPIC_CLASS, KernelFeature, class_label, feature_unit,
        unit_class, unit_feature,
    },
    gpu_startup::{self, Stage},
    host::WorkerPool,
};

use super::{GATE, begin_test, start_admit_compile, startup};
use crate::app::Ctx;

/// The classes the gate keeps a record for: isotropic (the start-up compile) and every class
/// the owner admitted.
fn tracked_classes() -> Vec<u32> {
    let mut classes = vec![ISOTROPIC_CLASS];
    classes.extend(
        gpu_admission::ADMITTED_CLASSES
            .iter()
            .copied()
            .filter(|class| *class != ISOTROPIC_CLASS),
    );
    classes
}

/// The compile units the gate keeps a record and a key for: the base set of each tracked
/// class, then each kernel feature of it.
pub(super) fn tracked_units() -> Vec<u32> {
    let mut units = Vec::new();
    for class in tracked_classes() {
        units.push(class);
        units.extend(
            KernelFeature::ALL
                .into_iter()
                .filter_map(|feature| feature_unit(class, feature)),
        );
    }
    units
}

/// The kernels of `unit`, as the names the Worker's `GpuCompile` takes.
pub(super) fn kernel_names(unit: u32) -> Vec<String> {
    gpu_admission::kernels_for_unit(unit)
        .iter()
        .map(|&name| name.to_owned())
        .collect()
}

/// Compiles and tests `class` on first use (the isotropic class: when the start-up compile is
/// the only thing missing).
///
/// Does nothing while another run is going, after the session lost the GPU or a compile
/// failed (the Worker refuses further compiles until the page is reloaded), for a class that
/// failed in this session, or for one whose record says it failed earlier (until "Forget GPU
/// Test Result"). The progress card shows a class other than the isotropic one, since that
/// compile can take seconds.
pub(super) fn ensure_class(ctx: &Ctx, pool: &WorkerPool, class: u32) {
    if !gpu_admission::is_class_admitted(class) || !pool.render().is_gpu_ready() {
        return;
    }
    let (blocked, state) = GATE.with(|gate| {
        let g = gate.borrow();
        (
            g.busy()
                || g.failed_in_session
                || g.compile_blocked
                || g.class_failed.contains_key(&class),
            g.records.get(&class).map(|record| record.state),
        )
    });
    if blocked {
        return;
    }
    // A class that passed before only compiles again in this Worker; a new one gets the full
    // test. The isotropic class was always given the full test when it was not qualified.
    let compile_only = match state {
        Some(AdmissionState::Ok) => true,
        Some(AdmissionState::Unknown) => false,
        Some(_) if class == ISOTROPIC_CLASS => false,
        _ => return,
    };
    if class != ISOTROPIC_CLASS {
        startup::begin_run(ctx, true);
    }
    if compile_only {
        start_admit_compile(ctx, pool, class);
    } else {
        begin_test(ctx, pool, class);
    }
}

/// Compiles the kernels of `feature` for `class` the first time a scene needs them.
///
/// The pool names the feature (`gpu_admission::feature_for_reason` of its refusal) once the
/// class's base set is compiled. The compile runs under the feature's own crash marker and
/// record; a failure, in this session or earlier, sends only the stones that need the feature
/// to the CPU. Does nothing while another run is going, after the session lost the GPU or a
/// compile failed (the Worker refuses further compiles until the page is reloaded), or while the
/// class's base set is not compiled in this Worker. The progress card shows the compile, which
/// can take seconds.
pub(super) fn ensure_feature(ctx: &Ctx, pool: &WorkerPool, class: u32, feature: KernelFeature) {
    let Some(unit) = feature_unit(class, feature) else {
        return;
    };
    if !gpu_admission::is_class_admitted(class) || !pool.render().is_gpu_ready() {
        return;
    }
    let (blocked, state) = GATE.with(|gate| {
        let g = gate.borrow();
        (
            g.busy()
                || g.failed_in_session
                || g.compile_blocked
                || !g.compiled(class)
                || g.class_failed.contains_key(&unit),
            g.records.get(&unit).map(|record| record.state),
        )
    });
    if blocked {
        return;
    }
    // A feature that passed before only compiles again in this Worker; a new one is compiled
    // as a test (its own `Probing` marker, no self-check render).
    let compile_only = match state {
        Some(AdmissionState::Ok) => true,
        Some(AdmissionState::Unknown) => false,
        _ => return,
    };
    startup::begin_run(ctx, true);
    if compile_only {
        start_admit_compile(ctx, pool, unit);
    } else {
        begin_test(ctx, pool, unit);
    }
}

/// The progress label of `stage` for compile `unit`: the feature that compiles, or the class
/// when it is not the isotropic one.
pub(super) fn stage_label(stage: Stage, adapter: Option<&str>, unit: u32) -> String {
    let class = unit_class(unit);
    if let (Stage::Compile { done, total }, Some(feature)) = (stage, unit_feature(unit)) {
        return gpu_admission::feature_compile_label(feature, class, done, total);
    }
    let label = gpu_startup::stage_label(stage, adapter);
    match stage {
        Stage::Compile { .. } | Stage::SelfCheck if class != ISOTROPIC_CLASS => {
            format!("{label} for {} stones", class_label(class))
        }
        _ => label,
    }
}

/// The sentence for a scene of `class` that has no GPU kernels yet (testing, failed, not
/// tested), `None` for the isotropic class, a class the owner has not admitted (the pool's
/// reason says so) and a class that is compiled.
pub(super) fn class_note(class: u32) -> Option<String> {
    if class == ISOTROPIC_CLASS || !gpu_admission::is_class_admitted(class) {
        return None;
    }
    let label = class_label(class);
    GATE.with(|gate| {
        let g = gate.borrow();
        if g.compiled(class) {
            return None;
        }
        if (g.testing || g.admitting) && g.run() == class {
            return Some(format!("Testing the GPU for {label} stones."));
        }
        if let Some(reason) = g.class_failed.get(&class) {
            return Some(format!(
                "The GPU test for {label} stones failed ({reason}); they render on the CPU. \
                 Reload the page to test again."
            ));
        }
        let record = g.records.get(&class)?;
        match record.state {
            AdmissionState::Failed => Some(format!(
                "The GPU test for {label} stones failed earlier ({}); they render on the CPU. \
                 {}",
                record.reason.as_deref().unwrap_or("unknown reason"),
                if g.compile_blocked {
                    "Reload the page, then forget the GPU test result to test again."
                } else {
                    "Forget the GPU test result to test again."
                }
            )),
            AdmissionState::Unknown if g.compile_blocked => Some(format!(
                "The GPU is not tested for {label} stones: a kernel failed to compile in this \
                 session, so no further test runs until the page is reloaded."
            )),
            AdmissionState::Unknown => {
                Some(format!("The GPU is not tested for {label} stones yet."))
            }
            AdmissionState::Probing | AdmissionState::Ok => None,
        }
    })
}

/// Why a stone of `class` that needs `feature` stays on the CPU although the kernels were
/// refused for good: the feature's compile failed in this session or earlier. `None` while the
/// feature is compiled, compiling, or merely not compiled yet (the pool's own reason says so).
///
/// A sentence fragment for the Render panel, in the style of the pool's reasons.
pub(super) fn feature_failure(class: u32, feature: KernelFeature) -> Option<String> {
    let unit = feature_unit(class, feature)?;
    let noun = feature.noun();
    GATE.with(|gate| {
        let g = gate.borrow();
        if g.compiled(unit) {
            return None;
        }
        if let Some(reason) = g.class_failed.get(&unit) {
            return Some(format!(
                "the GPU {noun} kernels failed to compile ({reason}); this stone renders on the \
                 CPU until the page is reloaded"
            ));
        }
        let record = g.records.get(&unit)?;
        (record.state == AdmissionState::Failed).then(|| {
            format!(
                "the GPU {noun} kernels failed to compile earlier ({}); this stone renders on \
                 the CPU until {}",
                record.reason.as_deref().unwrap_or("unknown reason"),
                if g.compile_blocked {
                    "the page is reloaded and the GPU test result is forgotten"
                } else {
                    "the GPU test result is forgotten"
                }
            )
        })
    })
}

/// The passed sentence of compile `unit` for the Render panel.
pub(super) fn passed_text(unit: u32, total_ms: f64) -> String {
    if let Some(feature) = unit_feature(unit) {
        return gpu_admission::feature_passed_text(feature, unit_class(unit), total_ms);
    }
    if unit == ISOTROPIC_CLASS {
        format!("GPU test passed (total {:.1} s)", total_ms / 1000.0)
    } else {
        format!(
            "GPU test passed for {} stones (total {:.1} s)",
            class_label(unit),
            total_ms / 1000.0
        )
    }
}
