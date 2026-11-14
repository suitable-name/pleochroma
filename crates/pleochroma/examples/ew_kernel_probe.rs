//! Which event-wavefront kernels this machine's GPU driver builds (`ew_kernel_probe`).
//!
//! Builds every kernel of the three pass classes the phone and the browser trace with, then
//! variants of single kernels with a part edited (`VARIANTS`), then synthetic shaders that grow
//! one resource at a time (a private array, straight-line code), and prints one line per build:
//! its time and whether the driver built it. The pass classes are the kind with the oriented and
//! zoned oriented estimators compiled out, and the extraordinary curve compiled out except for
//! uniaxial (`pleochroma_web_core::gpu_admission::browser_pipeline_class`).
//!
//! A pipeline the driver rejects makes wgpu lose the whole device, so the probe takes a fresh
//! device after every rejection; the variants start on a fresh device too. Nothing is
//! dispatched: the variants only locate what a driver rejects and are never traced.
//!
//! On an Android phone, from a desktop shell (the app is not involved):
//!
//! ```text
//! cargo ndk -t arm64-v8a build --profile probe -p pleochroma --features gpu --example ew_kernel_probe
//! adb push target/aarch64-linux-android/probe/examples/ew_kernel_probe /data/local/tmp/
//! adb shell chmod 755 /data/local/tmp/ew_kernel_probe
//! adb shell /data/local/tmp/ew_kernel_probe
//! ```
//!
//! On a desktop: `cargo run --profile probe -p pleochroma --features gpu --example ew_kernel_probe`.

use std::{
    fmt::Write as _,
    io::Write as _,
    sync::{PoisonError, atomic::Ordering},
    time::Instant,
};

use pleochroma::renderer::gpu::{
    GpuContext,
    event_wavefront::{EwKernel, kernels::arms_for},
    frame::material_class,
};

/// The uniaxial pass class.
const UNIAXIAL: u32 =
    material_class::UNIAXIAL | material_class::NO_ORIENTED | material_class::NO_ZONED_ORIENTED;

/// The isotropic pass class.
const ISOTROPIC: u32 = material_class::ISOTROPIC
    | material_class::NO_ORIENTED
    | material_class::NO_ZONED_ORIENTED
    | material_class::NO_EXTRAORDINARY_DISPERSION;

/// The biaxial pass class.
const BIAXIAL: u32 = material_class::BIAXIAL
    | material_class::NO_ORIENTED
    | material_class::NO_ZONED_ORIENTED
    | material_class::NO_EXTRAORDINARY_DISPERSION;

/// The pass class of each kind, as the phone and the browser compile it.
const PASS_CLASSES: [(&str, u32); 3] = [
    ("uniaxial", UNIAXIAL),
    ("isotropic", ISOTROPIC),
    ("biaxial", BIAXIAL),
];

/// An edit of a kernel's source: the text to find, exactly once, and what replaces it. The
/// source is searched with `\n` line ends.
type Cut = (&'static str, &'static str);

/// `ew_split_resolve` as it was before the Adreno 740 fix: a shared copy of the planes, which the
/// exit splits' re-entry probes read. The control shows whether a driver still rejects it.
const SPLIT_SHARED_PLANES: [Cut; 2] = [
    (
        "fn ew_split_resolve(@builtin(global_invocation_id) gid: vec3<u32>) {\n    let i = gid.x;",
        "fn ew_split_resolve(\n    @builtin(global_invocation_id) gid: vec3<u32>,\n    @builtin(local_invocation_index) local_index: u32,\n) {\n    ew_fill_planes_shared(local_index);\n    let i = gid.x;",
    ),
    (
        "intersect_ray_via(hit_point + dir_k * 1e-4, dir_k, false)",
        "intersect_ray(hit_point + dir_k * 1e-4, dir_k)",
    ),
];

/// One variant: a kernel, the class it is built for and the edits applied to its source.
struct Variant {
    name: &'static str,
    kernel: EwKernel,
    class: u32,
    cuts: &'static [Cut],
}

/// Kernels with a part edited, built after the shipped ones. Add one here to locate what a
/// driver rejects.
const VARIANTS: [Variant; 1] = [Variant {
    name: "split_resolve shared planes",
    kernel: EwKernel::SplitResolve,
    class: ISOTROPIC,
    cuts: &SPLIT_SHARED_PLANES,
}];

/// A compute shader whose one dynamically indexed private array holds `vec4s` `vec4<f32>`s.
fn private_array_shader(vec4s: u32) -> String {
    format!(
        "@group(0) @binding(0) var<storage, read_write> data: array<vec4<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
    var a: array<vec4<f32>, {vec4s}>;
    let n = arrayLength(&data);
    for (var i: u32 = 0u; i < {vec4s}u; i = i + 1u) {{
        a[i] = data[(gid.x + i) % n];
    }}
    var acc = vec4<f32>(0.0);
    for (var i: u32 = 0u; i < {vec4s}u; i = i + 1u) {{
        acc = acc + a[(i * 7u + gid.x) % {vec4s}u];
    }}
    data[gid.x % n] = acc;
}}
"
    )
}

/// A compute shader of `steps` dependent transcendental steps of straight-line code.
fn code_length_shader(steps: u32) -> String {
    let mut body = String::new();
    for step in 0..steps {
        let scale = f64::from(step).mul_add(1e-6, 1.0);
        // Writing to a `String` cannot fail.
        let _ = writeln!(body, "    x = sin(x) * {scale:.6} + cos(x.yzwx);");
    }
    format!(
        "@group(0) @binding(0) var<storage, read_write> data: array<vec4<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {{
    let n = arrayLength(&data);
    var x = data[gid.x % n];
{body}    data[gid.x % n] = x;
}}
"
    )
}

/// The device every build runs on, replaced after a rejection.
struct Probe {
    ctx: GpuContext,
}

impl Probe {
    fn acquire() -> Self {
        let ctx = GpuContext::acquire().unwrap_or_else(|error| {
            eprintln!("no GPU device: {error}");
            std::process::exit(1)
        });
        Self { ctx }
    }

    /// The first failure the device recorded, `None` while it has none.
    fn failure(&self) -> Option<String> {
        if !self.ctx.validation_error_seen.load(Ordering::Acquire) {
            return None;
        }
        let detail = self
            .ctx
            .last_uncaptured_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Some(detail.unwrap_or_else(|| "no detail recorded".to_owned()))
    }

    /// Builds `entry` of `source` with the pipeline-overridable `constants`, prints the outcome
    /// and takes a fresh device after a rejection. Returns whether the driver built it.
    fn build(&mut self, label: &str, source: &str, entry: &str, constants: &[(&str, f64)]) -> bool {
        // The label goes out first: if the driver crashes the process, it names the build.
        print!("  {label:<34} ");
        // A failed flush only delays the label; the outcome below still prints.
        let _ = std::io::stdout().flush();
        let started = Instant::now();
        let module = self
            .ctx
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let pipeline = self
            .ctx
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    zero_initialize_workgroup_memory: true,
                },
                cache: None,
            });
        let ms = started.elapsed().as_millis();
        drop(pipeline);
        drop(module);
        let Some(why) = self.failure() else {
            println!("{ms:>6} ms  built");
            return true;
        };
        let why = why.lines().next().unwrap_or_default();
        println!("{ms:>6} ms  REJECTED: {why}");
        *self = Self::acquire();
        false
    }
}

/// Whether `kernel` is one of the surface interaction arm kernels, which `arms_for` selects per
/// class.
const fn is_arm(kernel: EwKernel) -> bool {
    matches!(
        kernel,
        EwKernel::ArmFrosted
            | EwKernel::ArmTir
            | EwKernel::ArmUniEntry
            | EwKernel::ArmUniEntryTransmit
            | EwKernel::ArmUniEntryFinish
            | EwKernel::ArmUniInternal
            | EwKernel::ArmUniInternalTransmit
            | EwKernel::ArmGeneric
    )
}

/// `source` with `\n` line ends and `cuts` applied, `None` when an anchor does not occur exactly
/// once (the kernel text moved; the cut needs updating).
fn apply_cuts(source: &str, cuts: &[Cut]) -> Option<String> {
    let mut text = source.replace("\r\n", "\n");
    for (anchor, replacement) in cuts {
        if text.matches(anchor).count() != 1 {
            return None;
        }
        text = text.replacen(anchor, replacement, 1);
    }
    Some(text)
}

fn main() {
    let started = Instant::now();
    let mut probe = Probe::acquire();
    let info = probe.ctx.adapter.get_info();
    println!(
        "adapter: {} ({:?}; driver {}, {})",
        info.name, info.backend, info.driver, info.driver_info
    );

    let mut rejected = Vec::new();
    for (kind, class) in PASS_CLASSES {
        println!("{kind} pass class {class}:");
        let arms = arms_for(class, true);
        let constants = material_class::pipeline_constants(class);
        for kernel in EwKernel::ALL {
            if is_arm(kernel) && !arms.contains(&kernel) {
                continue;
            }
            let entry = kernel.entry_point();
            if !probe.build(entry, kernel.source(), entry, &constants) {
                rejected.push(format!("{entry} ({kind})"));
            }
        }
    }

    println!("variants, from a fresh device:");
    probe = Probe::acquire();
    let mut built = Vec::new();
    for variant in &VARIANTS {
        let Some(source) = apply_cuts(variant.kernel.source(), variant.cuts) else {
            println!("  {:<34} skipped: an anchor moved", variant.name);
            continue;
        };
        let entry = variant.kernel.entry_point();
        let constants = material_class::pipeline_constants(variant.class);
        if probe.build(variant.name, &source, entry, &constants) {
            built.push(variant.name.to_owned());
        }
    }

    println!("synthetic limits:");
    for vec4s in [256, 2048] {
        let label = format!("private array {} KiB", vec4s * 16 / 1024);
        if probe.build(&label, &private_array_shader(vec4s), "main", &[]) {
            built.push(label);
        }
    }
    for steps in [1000, 4000] {
        let label = format!("straight-line code {steps} steps");
        if probe.build(&label, &code_length_shader(steps), "main", &[]) {
            built.push(label);
        }
    }

    let list = |names: &[String]| {
        if names.is_empty() {
            "none".to_owned()
        } else {
            names.join(", ")
        }
    };
    println!("kernels rejected: {}", list(&rejected));
    println!("variants built: {}", list(&built));
    println!("done in {:.1} s", started.elapsed().as_secs_f64());
}
