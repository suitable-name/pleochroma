//! A shader warm-up that compiles on several devices at once.
//!
//! One pipeline compile is a single driver call and cannot be split, and every
//! [`GpuBackend::warm_pipelines_cancellable`] step compiles inside its backend's renderer lock,
//! so the steps of one backend run strictly one after another: a cold start costs about 30 to 60
//! seconds per material class, over some 40 steps. [`warm_in_parallel`] compiles shares of the
//! steps on helper devices instead, each on its own thread, into the persistent disk cache
//! ([`GpuBackend::persists_pipelines`], Vulkan only). The lane device then loads what the helpers
//! built, which is quick. No compile code is new: every device runs the same
//! `warm_pipelines_cancellable` on the same stand-in scene ([`StandInScene`]).
//!
//! # Memory
//!
//! One large compile can take a gigabyte or more of RAM in the driver, and every helper holds a
//! whole device besides. That is why [`compile_threads`] caps the automatic count at 4 (the lane
//! included); `PLEOCHROMA_SHADER_THREADS` raises it to 16 for a machine that has the memory.
//!
//! # Scheduling
//!
//! The scheduling ([`schedule`]) knows nothing of devices: it takes closures that run one step on
//! the lane, on a helper, or load one on the lane, so its rules are tested without a GPU.

use super::{GpuAccumulate, GpuBackend, GpuSceneRef, GpuWarmFamilies};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        materials::GemMaterial,
        raytracer::{
            Camera, DEFAULT_FOV_DEG, DEFAULT_MAX_BOUNCES, DEFAULT_POSE, EnvironmentSource,
            LightingPreset, surface::SurfaceScene,
        },
    },
    render_setup::tray_mirror_for,
};
use std::{
    num::NonZeroUsize,
    sync::{
        Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

/// The environment variable that sets the compile thread count ([`compile_threads`]).
const THREADS_ENV: &str = "PLEOCHROMA_SHADER_THREADS";
/// The most compile threads the variable may ask for.
const THREADS_MAX: usize = 16;
/// The most compile threads chosen without the variable (RAM: see the module doc).
const THREADS_AUTO_MAX: usize = 4;
/// Pixels per side of a stand-in scene; the kernels do not depend on the size.
const STAND_IN_SIZE: u32 = 16;
/// Head-on reflectance of the stand-in tray.
const STAND_IN_REFLECTION: f32 = 0.5;
/// The studio light of the stand-in scenes (yaw and pitch, radians); only the tray changes the
/// kernels, the rest keeps the stand-in close to a real render.
const STAND_IN_LIGHT: (f32, f32) = (0.85, 0.95);

/// One warm-up step: a Path-Traced material class, plain or on the tray.
#[derive(Debug, Clone)]
pub struct WarmStep {
    /// The material whose class the step builds.
    pub material: GemMaterial,
    /// The scene sits on a tray (a Tray Reflection or a Stone Shadow), whose kernels are a
    /// variant of their own.
    pub tray: bool,
}

/// [`GpuBackend::warm_up_materials`] times plain and tray: every material class, all the plain
/// variants first, and `first` (when the GPU traces it) leading each variant.
#[must_use]
pub fn catalogue_steps(first: Option<&GemMaterial>) -> Vec<WarmStep> {
    let materials = GpuBackend::warm_up_materials(first);
    [false, true]
        .into_iter()
        .flat_map(|tray| {
            materials.iter().map(move |material| WarmStep {
                material: material.clone(),
                tray,
            })
        })
        .collect()
}

/// The stand-in scene a step warms with: 16 by 16 pixels, a round brilliant at the default pose,
/// the studio light at a fixed pose, and a tray of a fixed reflectance when the step has one.
///
/// It owns everything the scene's references borrow, so it outlives nothing but itself; the
/// borrowing [`GpuSceneRef`] is made inside [`Self::warm`].
pub struct StandInScene {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
    /// `None` when the tray mirror could not be built: the step declines.
    environment: Option<EnvironmentSource<'static>>,
}

impl StandInScene {
    /// The scene of `step`.
    #[must_use]
    pub fn new(step: &WarmStep) -> Self {
        let camera = Camera::new(
            DEFAULT_POSE.yaw,
            DEFAULT_POSE.pitch,
            DEFAULT_POSE.distance,
            DEFAULT_FOV_DEG,
        );
        let planes = StandardGemCuts::standard_round_brilliant();
        let studio = LightingPreset::default()
            .studio(1.0, STAND_IN_LIGHT.0, STAND_IN_LIGHT.1)
            .with_backdrop(if step.tray { 1.0 } else { 0.0 });
        let environment = if step.tray {
            // A Tray Reflection stands in for every tray: with the Stone Shadow the kernels have
            // the same sources and constants (`material_class::pipeline_key` folds the two).
            let stone = StoneGeometry {
                planes: &planes,
                tools: &[],
            };
            match tray_mirror_for(STAND_IN_REFLECTION, studio, stone) {
                Ok(Some(mirror)) => Some(studio.with_tray_mirror(Some(mirror))),
                _ => None,
            }
        } else {
            Some(studio)
        };
        Self {
            camera,
            planes,
            material: step.material.clone(),
            environment,
        }
    }

    /// [`GpuBackend::warm_pipelines_cancellable`] on this scene: no tools, an ideal surface.
    /// Declines when the tray mirror could not be built.
    pub fn warm(
        &self,
        gpu: &GpuBackend,
        families: GpuWarmFamilies,
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        let Some(environment) = self.environment else {
            return GpuAccumulate::Declined;
        };
        let scene = GpuSceneRef {
            camera: &self.camera,
            width: STAND_IN_SIZE,
            height: STAND_IN_SIZE,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces: DEFAULT_MAX_BOUNCES,
            environment,
            estimator: None,
        };
        gpu.warm_pipelines_cancellable(&scene, &[], &SurfaceScene::IDEAL, families, cancel)
    }
}

/// The compile threads, the lane included.
///
/// `PLEOCHROMA_SHADER_THREADS` wins when it is a number
/// (clamped to 1..=16); otherwise half the logical cores, clamped to 1..=4. `1` is the serial
/// behaviour of a warm-up without helpers. See the module doc on memory.
#[must_use]
pub fn compile_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
    threads_for(std::env::var(THREADS_ENV).ok().as_deref(), cores)
}

/// [`compile_threads`] for a given variable value and core count.
fn threads_for(variable: Option<&str>, cores: usize) -> usize {
    if let Some(asked) = variable.and_then(|text| text.trim().parse::<usize>().ok()) {
        return asked.clamp(1, THREADS_MAX);
    }
    (cores / 2).clamp(1, THREADS_AUTO_MAX)
}

/// One finished step.
#[derive(Debug, Clone, Copy)]
pub struct StepDone {
    /// The step's index in the list given to [`warm_in_parallel`].
    pub index: usize,
    /// What the step did. [`GpuAccumulate::Cancelled`] when the cancel flag stopped it.
    pub outcome: GpuAccumulate,
    /// The step compiled a kernel from source instead of loading it from the disk cache, or its
    /// device keeps no disk cache at all. Exact per step, whatever the thread count.
    pub fresh: bool,
    /// The step ran on the lane device, not on a helper.
    pub on_lane: bool,
}

/// What running one step on one device returned.
#[derive(Debug, Clone, Copy)]
struct StepResult {
    outcome: GpuAccumulate,
    fresh: bool,
}

/// One helper's step runner; it owns the helper's device, which drops with it. `None` from a
/// call: the device is unusable now, and the step was not run.
type HelperStep<'a> = Box<dyn FnMut(usize) -> Option<StepResult> + 'a>;

/// How [`schedule`] runs one step on each kind of device.
struct Runners<'a> {
    /// Runs and measures one step on the lane.
    lane_step: &'a (dyn Fn(usize) -> StepResult + Sync),
    /// Loads one step on the lane from the disk cache a helper filled, without a report.
    lane_load: &'a (dyn Fn(usize) + Sync),
    /// Opens a helper device on the calling (helper) thread; `None` when it is unusable.
    open_helper: &'a (dyn Fn() -> Option<HelperStep<'a>> + Sync),
}

/// Boxes a helper's step runner.
fn boxed<'a>(step: impl FnMut(usize) -> Option<StepResult> + 'a) -> HelperStep<'a> {
    Box::new(step)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How many helper threads a warm-up of `count` steps starts: none when `threads` is `1` or the
/// lane keeps no disk cache (a helper's work would be lost), and never more than the steps after
/// the first.
const fn helper_count(threads: usize, persists: bool, count: usize) -> usize {
    if threads <= 1 || !persists {
        return 0;
    }
    let spare = count.saturating_sub(1);
    let asked = threads - 1;
    if asked < spare { asked } else { spare }
}

/// The scheduling of a warm-up, apart from the devices.
///
/// The calling thread runs step `0` on the lane before any helper starts. Then `helpers` threads
/// and the calling thread take the next index from one counter until it runs out or `cancel` is
/// set, and every step is reported once, from the thread that ran it. A helper that cannot open its
/// device takes nothing; one whose device fails at a step hands that step back, and another
/// thread, or the lane after the others, runs it. At the end the lane loads the steps the helpers
/// built, quietly, checking `cancel` between them.
fn schedule(
    count: usize,
    helpers: usize,
    cancel: &AtomicBool,
    runners: &Runners<'_>,
    on_step: &(dyn Fn(StepDone) + Sync),
) {
    if count == 0 || cancel.load(Ordering::Acquire) {
        return;
    }
    let report = |index: usize, result: StepResult, on_lane: bool| {
        on_step(StepDone {
            index,
            outcome: result.outcome,
            fresh: result.fresh,
            on_lane,
        });
    };
    // The first step alone: it also persists the shared base kernel, which helpers would
    // otherwise compile all at once.
    let first = (runners.lane_step)(0);
    report(0, first, true);
    if first.outcome == GpuAccumulate::Cancelled {
        return;
    }
    let next = AtomicUsize::new(1);
    let handed_back = Mutex::new(Vec::<usize>::new());
    let helped = Mutex::new(Vec::<usize>::new());
    let take = || -> Option<usize> {
        if cancel.load(Ordering::Acquire) {
            return None;
        }
        let returned = lock(&handed_back).pop();
        if returned.is_some() {
            return returned;
        }
        let index = next.fetch_add(1, Ordering::Relaxed);
        (index < count).then_some(index)
    };
    let lane_loop = || {
        while let Some(index) = take() {
            let result = (runners.lane_step)(index);
            report(index, result, true);
            if result.outcome == GpuAccumulate::Cancelled {
                break;
            }
        }
    };
    let helper_loop = || {
        let Some(mut step) = (runners.open_helper)() else {
            return;
        };
        while let Some(index) = take() {
            let Some(result) = step(index) else {
                lock(&handed_back).push(index);
                break;
            };
            if result.outcome == GpuAccumulate::Done {
                lock(&helped).push(index);
            }
            report(index, result, false);
            if result.outcome == GpuAccumulate::Cancelled {
                break;
            }
        }
    };
    std::thread::scope(|scope| {
        for number in 0..helpers {
            let spawned = std::thread::Builder::new()
                .name(format!("pleochroma-shader-helper-{number}"))
                .spawn_scoped(scope, helper_loop);
            if let Err(error) = spawned {
                tracing::warn!(%error, "could not start a shader compile helper");
                break;
            }
        }
        lane_loop();
    });
    // Steps a failed helper handed back after the others had finished.
    loop {
        let returned = lock(&handed_back).pop();
        let Some(index) = returned else { break };
        if cancel.load(Ordering::Acquire) {
            break;
        }
        let result = (runners.lane_step)(index);
        report(index, result, true);
    }
    let mut loaded = std::mem::take(&mut *lock(&helped));
    loaded.sort_unstable();
    for index in loaded {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        (runners.lane_load)(index);
    }
}

/// Runs `step` on `device` and measures whether it compiled anything fresh on this thread.
fn run_step(
    device: &GpuBackend,
    step: Option<&WarmStep>,
    families: GpuWarmFamilies,
    cancel: &AtomicBool,
) -> StepResult {
    let Some(step) = step else {
        return StepResult {
            outcome: GpuAccumulate::Declined,
            fresh: false,
        };
    };
    let before = GpuBackend::fresh_pipeline_compiles_on_this_thread();
    let outcome = StandInScene::new(step).warm(device, families, cancel);
    let compiled = GpuBackend::fresh_pipeline_compiles_on_this_thread() > before;
    StepResult {
        outcome,
        // A device that keeps no disk cache compiles everything from source.
        fresh: compiled || !device.persists_pipelines(),
    }
}

/// Warms `steps` on `lane` and, when the lane keeps a disk cache, on `threads - 1` helper devices
/// in parallel.
///
/// With `threads <= 1`, or a lane without a disk cache ([`GpuBackend::persists_pipelines`]; a
/// helper's work would be lost), every step runs serially on `lane`. Otherwise the calling thread
/// first runs `steps[0]` on `lane` (the most needed class; it also persists the shared base
/// kernel), then `threads - 1` helpers, each on its own [`GpuBackend::acquire`] device, and the
/// calling thread take the next step from one counter. A helper whose device fails, or keeps no
/// disk cache, exits without taking steps. `on_step` hears every step once, when it finishes, from
/// the thread that ran it. After the last step the lane loads, quietly, the steps the helpers
/// built. The helper devices drop at the end, which frees their memory.
///
/// `cancel` is checked before each step and inside it ([`StandInScene::warm`]); the load pass
/// checks it too.
pub fn warm_in_parallel(
    lane: &GpuBackend,
    steps: &[WarmStep],
    families: GpuWarmFamilies,
    threads: usize,
    cancel: &AtomicBool,
    on_step: impl Fn(StepDone) + Sync,
) {
    warm_steps(lane, steps, families, threads, cancel, &on_step, true);
}

/// [`warm_in_parallel`] for a `lane` that is a device of its own, dropped afterwards: the steps
/// only fill the disk cache, so the helpers' steps are not loaded onto `lane` at the end.
pub fn fill_disk_cache_in_parallel(
    lane: &GpuBackend,
    steps: &[WarmStep],
    families: GpuWarmFamilies,
    threads: usize,
    cancel: &AtomicBool,
    on_step: impl Fn(StepDone) + Sync,
) {
    warm_steps(lane, steps, families, threads, cancel, &on_step, false);
}

/// The body of [`warm_in_parallel`] and [`fill_disk_cache_in_parallel`]; `load_on_lane` says
/// whether the lane loads the helpers' steps at the end.
fn warm_steps(
    lane: &GpuBackend,
    steps: &[WarmStep],
    families: GpuWarmFamilies,
    threads: usize,
    cancel: &AtomicBool,
    on_step: &(dyn Fn(StepDone) + Sync),
    load_on_lane: bool,
) {
    let helpers = helper_count(threads, lane.persists_pipelines(), steps.len());
    let lane_step = |index: usize| run_step(lane, steps.get(index), families, cancel);
    let lane_load = |index: usize| {
        if !load_on_lane {
            return;
        }
        if let Some(step) = steps.get(index) {
            // The disk cache has it: this only loads it into the lane's device.
            StandInScene::new(step).warm(lane, families, cancel);
        }
    };
    let open_helper = || {
        let gpu = GpuBackend::acquire();
        if gpu.adapter_label().is_none() || !gpu.persists_pipelines() {
            return None;
        }
        Some(boxed(move |index: usize| {
            if gpu.is_lost() {
                return None;
            }
            Some(run_step(&gpu, steps.get(index), families, cancel))
        }))
    };
    schedule(
        steps.len(),
        helpers,
        cancel,
        &Runners {
            lane_step: &lane_step,
            lane_load: &lane_load,
            open_helper: &open_helper,
        },
        on_step,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    fn done(fresh: bool) -> StepResult {
        StepResult {
            outcome: GpuAccumulate::Done,
            fresh,
        }
    }

    /// What a scheduling test recorded.
    #[derive(Default)]
    struct Log {
        reports: Mutex<Vec<StepDone>>,
        loaded: Mutex<Vec<usize>>,
    }

    impl Log {
        fn report(&self, step: StepDone) {
            lock(&self.reports).push(step);
        }

        fn indices(&self) -> Vec<usize> {
            let mut all: Vec<usize> = lock(&self.reports).iter().map(|s| s.index).collect();
            all.sort_unstable();
            all
        }
    }

    /// Runs the scheduler with devices that finish every step; `helper_opens` says whether the
    /// helpers get a device.
    fn run_all(count: usize, helpers: usize, helper_opens: bool, cancel: &AtomicBool) -> Log {
        let log = Log::default();
        let lane_step = |_: usize| done(false);
        let lane_load = |index: usize| lock(&log.loaded).push(index);
        let open = || {
            if helper_opens {
                Some(boxed(|_: usize| Some(done(true))))
            } else {
                None
            }
        };
        schedule(
            count,
            helpers,
            cancel,
            &Runners {
                lane_step: &lane_step,
                lane_load: &lane_load,
                open_helper: &open,
            },
            &|step| log.report(step),
        );
        log
    }

    #[test]
    fn every_step_is_reported_exactly_once() {
        for helpers in [0, 1, 3, 8] {
            let log = run_all(40, helpers, true, &AtomicBool::new(false));
            assert_eq!(log.indices(), (0..40).collect::<Vec<_>>(), "{helpers}");
        }
    }

    #[test]
    fn the_first_step_runs_on_the_lane_before_any_helper_starts() {
        let started = AtomicBool::new(false);
        let early = AtomicBool::new(false);
        let log = Log::default();
        let lane_step = |index: usize| {
            if index == 0 {
                started.store(true, Ordering::SeqCst);
            }
            done(false)
        };
        let open = || {
            if !started.load(Ordering::SeqCst) {
                early.store(true, Ordering::SeqCst);
            }
            Some(boxed(|_: usize| Some(done(true))))
        };
        schedule(
            12,
            3,
            &AtomicBool::new(false),
            &Runners {
                lane_step: &lane_step,
                lane_load: &|_| {},
                open_helper: &open,
            },
            &|step| log.report(step),
        );
        assert!(!early.load(Ordering::SeqCst), "a helper started too early");
        let reports = lock(&log.reports);
        assert_eq!(
            reports.first().map(|s| (s.index, s.on_lane)),
            Some((0, true))
        );
    }

    #[test]
    fn a_helper_without_a_device_leaves_every_step_to_the_lane() {
        let log = run_all(10, 3, false, &AtomicBool::new(false));
        assert_eq!(log.indices(), (0..10).collect::<Vec<_>>());
        assert!(lock(&log.reports).iter().all(|s| s.on_lane));
        assert!(lock(&log.loaded).is_empty(), "nothing to load");
    }

    #[test]
    fn the_steps_of_a_helper_that_fails_go_to_the_others() {
        // Every helper opens, then fails at its first step: each hands the step back.
        let log = Log::default();
        let failed = AtomicU32::new(0);
        let lane_step = |_: usize| done(false);
        let open = || {
            Some(boxed(|_: usize| {
                failed.fetch_add(1, Ordering::SeqCst);
                None
            }))
        };
        schedule(
            9,
            2,
            &AtomicBool::new(false),
            &Runners {
                lane_step: &lane_step,
                lane_load: &|_| {},
                open_helper: &open,
            },
            &|step| log.report(step),
        );
        assert_eq!(log.indices(), (0..9).collect::<Vec<_>>());
        assert!(lock(&log.reports).iter().all(|s| s.on_lane));
    }

    #[test]
    fn without_helpers_the_lane_runs_every_step_in_order() {
        let log = run_all(6, 0, true, &AtomicBool::new(false));
        let order: Vec<usize> = lock(&log.reports).iter().map(|s| s.index).collect();
        assert_eq!(order, vec![0, 1, 2, 3, 4, 5]);
        assert!(lock(&log.reports).iter().all(|s| s.on_lane));
    }

    #[test]
    fn threads_and_the_disk_cache_decide_the_helpers() {
        assert_eq!(helper_count(1, true, 40), 0, "one thread is serial");
        assert_eq!(helper_count(0, true, 40), 0);
        assert_eq!(helper_count(4, false, 40), 0, "no disk cache, no helpers");
        assert_eq!(helper_count(4, true, 40), 3);
        assert_eq!(
            helper_count(4, true, 2),
            1,
            "never more than the other steps"
        );
        assert_eq!(helper_count(4, true, 1), 0);
        assert_eq!(helper_count(4, true, 0), 0);
    }

    #[test]
    fn a_cancel_stops_new_steps() {
        let cancel = AtomicBool::new(true);
        assert!(run_all(10, 2, true, &cancel).indices().is_empty());

        // Cancelled after the third step, serially: no fourth step starts.
        let cancel = AtomicBool::new(false);
        let log = Log::default();
        let ran = AtomicU32::new(0);
        let lane_step = |_: usize| {
            if ran.fetch_add(1, Ordering::SeqCst) == 2 {
                cancel.store(true, Ordering::SeqCst);
            }
            done(false)
        };
        schedule(
            10,
            0,
            &cancel,
            &Runners {
                lane_step: &lane_step,
                lane_load: &|index| lock(&log.loaded).push(index),
                open_helper: &|| None,
            },
            &|step| log.report(step),
        );
        assert_eq!(log.indices(), vec![0, 1, 2]);
    }

    #[test]
    fn a_cancelled_step_is_reported_and_ends_the_run() {
        let log = Log::default();
        let lane_step = |index: usize| StepResult {
            outcome: if index == 2 {
                GpuAccumulate::Cancelled
            } else {
                GpuAccumulate::Done
            },
            fresh: false,
        };
        schedule(
            10,
            0,
            &AtomicBool::new(false),
            &Runners {
                lane_step: &lane_step,
                lane_load: &|_| {},
                open_helper: &|| None,
            },
            &|step| log.report(step),
        );
        assert_eq!(log.indices(), vec![0, 1, 2]);
        assert_eq!(
            lock(&log.reports).last().map(|s| s.outcome),
            Some(GpuAccumulate::Cancelled)
        );
    }

    #[test]
    fn the_lane_loads_exactly_the_steps_the_helpers_built_without_reporting_them_again() {
        let log = run_all(30, 3, true, &AtomicBool::new(false));
        let mut helped: Vec<usize> = lock(&log.reports)
            .iter()
            .filter(|s| !s.on_lane)
            .map(|s| s.index)
            .collect();
        helped.sort_unstable();
        let mut loaded = lock(&log.loaded).clone();
        loaded.sort_unstable();
        assert_eq!(loaded, helped);
        assert_eq!(log.indices(), (0..30).collect::<Vec<_>>(), "reported once");
    }

    #[test]
    fn a_cancel_during_the_load_pass_stops_it() {
        let cancel = AtomicBool::new(false);
        let log = Log::default();
        let lane_step = |_: usize| done(false);
        let open = || Some(boxed(|_: usize| Some(done(true))));
        schedule(
            30,
            3,
            &cancel,
            &Runners {
                lane_step: &lane_step,
                lane_load: &|index| {
                    lock(&log.loaded).push(index);
                    cancel.store(true, Ordering::SeqCst);
                },
                open_helper: &open,
            },
            &|step| log.report(step),
        );
        assert!(lock(&log.loaded).len() <= 1);
    }

    #[test]
    fn the_thread_count_follows_the_variable_and_the_cores() {
        assert_eq!(threads_for(None, 16), 4, "half the cores, at most 4");
        assert_eq!(threads_for(None, 8), 4);
        assert_eq!(threads_for(None, 6), 3);
        assert_eq!(threads_for(None, 2), 1);
        assert_eq!(threads_for(None, 1), 1, "never below one");
        assert_eq!(threads_for(Some("6"), 2), 6, "the variable wins");
        assert_eq!(threads_for(Some(" 16 "), 2), 16);
        assert_eq!(threads_for(Some("99"), 2), 16, "at most 16");
        assert_eq!(threads_for(Some("0"), 8), 1, "at least 1");
        assert_eq!(threads_for(Some("many"), 8), 4, "not a number: ignored");
        assert_eq!(threads_for(Some(""), 8), 4);
    }

    #[test]
    fn the_catalogue_has_every_class_plain_first_then_on_the_tray() {
        let steps = catalogue_steps(None);
        let classes = GpuBackend::warm_up_materials(None).len();
        assert_eq!(steps.len(), 2 * classes);
        assert!(steps[..classes].iter().all(|s| !s.tray));
        assert!(steps[classes..].iter().all(|s| s.tray));
        for (plain, tray) in steps[..classes].iter().zip(&steps[classes..]) {
            assert_eq!(plain.material.name, tray.material.name);
        }
    }

    #[test]
    fn a_stand_in_scene_without_a_gpu_declines_cleanly() {
        let backend = GpuBackend::disabled();
        let cancel = AtomicBool::new(false);
        for step in catalogue_steps(None).iter().take(2) {
            let scene = StandInScene::new(step);
            assert_eq!(
                scene.warm(&backend, GpuWarmFamilies::default(), &cancel),
                GpuAccumulate::Declined
            );
        }
    }

    #[test]
    fn a_disabled_backend_warms_nothing_in_parallel() {
        let backend = GpuBackend::disabled();
        let steps = catalogue_steps(None);
        let reports = Mutex::new(Vec::new());
        warm_in_parallel(
            &backend,
            &steps,
            GpuWarmFamilies::default(),
            4,
            &AtomicBool::new(false),
            |step| lock(&reports).push(step),
        );
        let reports = lock(&reports);
        assert_eq!(reports.len(), steps.len(), "serial, each reported once");
        assert!(reports.iter().all(|s| s.on_lane && s.fresh));
        drop(reports);
    }
}
