//! The sorted wavefront: the megakernel's bounce in two halves, each run by kernels that see
//! only the rays that need it, over a pool of ray slots that new tuples refill.
//!
//! A bounce of `transport_bounce_step` is cut after its surface phase. The front kernel runs the
//! front half (extend, scatter, and the surface phase, which picks the arm) for new tuples and for
//! rays whose last bounce ended in the medium. One kernel per arm runs that arm, the deferred
//! exit splits, the tail and the next bounce's front half for the rays filed for it. Every kernel
//! files each ray into the queue of its next step through an atomic counter, and every kernel is
//! dispatched indirectly over its own queue only: the rays are sorted by event and by arm, the
//! material class is the pipeline's. A path that ends writes the output slot of its tuple, and
//! its slot takes a new tuple in the next round while the chunk has tuples left.
//!
//! A chunk is one submission without a host round trip. Each round a one-thread kernel opens the
//! round (new tuples for the freed slots, the indirect arguments of every kernel), then the front
//! kernel and the arm kernels run. After the rounds a drain kernel finishes whatever is left, one
//! thread per path like the megakernel, so the number of rounds is a speed setting, never a
//! correctness one. `out_xyz` holds one triple per tuple as after the megakernel, and the
//! per-pixel reduction, the overlapped readback and the chunk timing are the megakernel's: the
//! samples of a pixel are added in the same order.
//!
//! A sample does not depend on the schedule: the state is stored exactly, every draw is keyed by
//! the tuple's seed and its bounce, and the estimator is bound per invocation. Which round starts
//! which tuple follows from path lengths, not from the order of atomics, so two runs agree bit for
//! bit. Against the megakernel a sample differs by compiler rounding between kernels only, which
//! the twin rule of `event_wavefront::compare` allows.
//!
//! Off unless `PLEOCHROMA_SORTED` is set (see [`sorted_requested`]); it then takes every chunk of
//! the chunk loop (final pictures, the bench, the worker, the batch stepper and the viewport's
//! progressive passes), ahead of the staged megakernel. The optics tables, the stone shadow, the
//! legacy wavefront, a device short of buffer bindings or of indirect dispatch, and the adaptive
//! passes keep their engines, with one warning per reason and renderer. The path cache's passes
//! and the browser build never use it.

use std::{collections::HashMap, sync::OnceLock};

use wgpu::BufferUsages;

use crate::{
    geometry::GpuFacetPlane,
    renderer::{
        buffers::{EwHit, EwRayChan, EwRayCore},
        gpu::{
            MEGAKERNEL_STORAGE_BUFFERS, compute,
            timing::{self, Slot},
        },
    },
};

use super::{
    ChunkFrameState, GpuPipelineKind,
    dispatch::count_u32,
    material_class,
    renderer::GpuFrameRenderer,
    scene_buffers::{FrameSceneBuffers, TransportOutputs},
};

/// The process switch, read once (see [`sorted_requested`]).
const SWITCH: &str = "PLEOCHROMA_SORTED";

/// Bytes in a MiB.
pub(super) const MIB: usize = 1024 * 1024;

/// The pool, in MiB, that `PLEOCHROMA_SORTED=1` asks for.
pub(super) const DEFAULT_POOL_MIB: usize = 256;

/// The smallest pool, in MiB, the switch takes.
const MIN_POOL_MIB: usize = 16;

/// The largest pool, in MiB, the switch takes.
const MAX_POOL_MIB: usize = 4096;

/// `shaders/sorted_wavefront.wgsl` behind the megakernel's prelude and bounce text, the event
/// wavefront's ray records and surface phase, and the queue layout, concatenated by `build.rs`.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const SORTED_SHADER_SRC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/sorted_wavefront.generated.wgsl"));

/// The browser build never runs the sorted wavefront (it has no process variable, and the
/// renderer's settings stay off there whatever [`SortedSwitch`] says), so it does not carry the
/// unit.
#[cfg(target_arch = "wasm32")]
pub(super) const SORTED_SHADER_SRC: &str = "";

/// `shaders/sorted_prepare.wgsl` behind the queue layout: the two one-thread kernels.
pub(super) const PREPARE_SHADER_SRC: &str =
    include_str!(concat!(env!("OUT_DIR"), "/sorted_prepare.generated.wgsl"));

/// The front kernel's entry point.
const FRONT_ENTRY: &str = "sw_front_main";

/// The drain kernel's entry point.
const DRAIN_ENTRY: &str = "sw_drain_main";

/// The entry point of the kernel that opens a round.
const PREPARE_ENTRY: &str = "sw_prepare";

/// The entry point of the kernel that lays out the drain.
const PREPARE_DRAIN_ENTRY: &str = "sw_prepare_drain";

/// Binding of the ray cores (`EwRayCore`, `shaders/event_wavefront/00_common.wgsl`).
const RAY_CORE_BINDING: u32 = 41;

/// Binding of the ray channels (`EwRayChan`).
const RAY_CHAN_BINDING: u32 = 42;

/// Binding of the hit records (`EwHit`).
const RAY_HIT_BINDING: u32 = 43;

/// Binding of the chunk's `SwParams` uniform.
const SW_PARAMS_BINDING: u32 = 160;

/// Binding of the queue regions.
const SW_QUEUES_BINDING: u32 = 161;

/// Binding of the queue lengths and control words.
const SW_COUNTS_BINDING: u32 = 162;

/// Binding of the indirect arguments (the one-thread kernels only).
const SW_INDIRECT_BINDING: u32 = 163;

/// The bindings every transport kernel reaches, in this order (`sw_reach` makes them the same
/// for all): the megakernel's fifteen, the three ray records, the parameters, the queues and the
/// counters.
pub(super) const TRANSPORT_SLOTS: [u32; 21] = [
    0,
    1,
    2,
    3,
    4,
    5,
    6,
    7,
    8,
    9,
    10,
    11,
    12,
    13,
    14,
    RAY_CORE_BINDING,
    RAY_CHAN_BINDING,
    RAY_HIT_BINDING,
    SW_PARAMS_BINDING,
    SW_QUEUES_BINDING,
    SW_COUNTS_BINDING,
];

/// The bindings of the two one-thread kernels, in this order.
pub(super) const PREPARE_SLOTS: [u32; 3] =
    [SW_PARAMS_BINDING, SW_COUNTS_BINDING, SW_INDIRECT_BINDING];

/// Storage buffers a transport kernel binds: the megakernel's, the three ray records, the queues
/// and the counters.
const SORTED_STORAGE_BUFFERS: u32 = MEGAKERNEL_STORAGE_BUFFERS + 5;

/// Buffers a transport kernel binds in all ([`TRANSPORT_SLOTS`]).
const SORTED_TOTAL_BUFFERS: u32 = 21;

/// Queue regions per slot (`SW_QUEUES` of `shaders/sorted_queues.wgsl`).
pub(super) const SW_QUEUES: usize = 14;

/// Words of the counter buffer (`SW_COUNTERS`).
pub(super) const SW_COUNTERS: usize = 34;

/// Indirect argument triples (`SW_INDIRECTS`).
pub(super) const SW_INDIRECTS: usize = 7;

/// The front kernel's triple (`SW_I_FRONT`).
pub(super) const SW_I_FRONT: usize = 0;

/// The first arm kernel's triple (`SW_I_ARM`); the arms follow in [`SortedArm::ALL`] order.
pub(super) const SW_I_ARM: usize = 1;

/// The drain's triple (`SW_I_DRAIN`).
pub(super) const SW_I_DRAIN: usize = 6;

/// The `SwParams` flag that counts and checksums every finished path (`SW_FLAG_CHECK`).
pub(super) const SW_FLAG_CHECK: u32 = 1;

/// Bytes of one ray slot: its three records and its place in every queue (712).
pub(super) const SLOT_BYTES: usize = size_of::<EwRayCore>()
    + size_of::<EwRayChan>()
    + size_of::<EwHit>()
    + SW_QUEUES * size_of::<u32>();

/// The pool grows in steps of this many slots (44.5 MiB) and never shrinks.
const POOL_GROWTH_SLOTS: usize = 65_536;

/// The most slots: a 64-wide indirect dispatch addresses at most 65,535 workgroups.
pub(super) const MAX_POOL_SLOTS: usize = 65_535 * 64;

/// Rounds per tuple and slot that [`rounds_for`] plans while the pool is refilled. A path holds
/// its slot for one round per bounce: about 3 rounds on average (2.9 to 3.8 at 32 bounces), and 5
/// to 7 for the stone's own rays, which dominate a frame the stone fills. A round planned beyond
/// the need costs a few empty dispatches; a tuple a short plan leaves unstarted goes to the drain.
const ROUNDS_PER_TUPLE: usize = 8;

/// The most bounces that the rounds [`rounds_for`] plans after the last refill cover (one round
/// per bounce, plus the round of the first front half); a longer bounce cap leaves its longest
/// paths to the drain.
const TAIL_ROUNDS_MAX: u32 = 32;

/// The most rounds one chunk encodes; the drain finishes the rest.
pub(super) const MAX_ROUNDS: u32 = 1024;

/// What a missing resource means in [`GpuFrameRenderer::dispatch_sorted`].
const PREPARED: &str = "sorted_plan_for checked the sorted wavefront's resources";

/// One `(binding, buffer, size)` entry of a bind group.
type Entry<'a> = (u32, &'a wgpu::Buffer, Option<wgpu::BufferAddress>);

/// `SwParams` of the sorted wavefront's two units, written once per chunk.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct GpuSortedParams {
    /// The tuples of the chunk.
    tuples: u32,
    /// The ray slots the chunk uses, at most `tuples`.
    pool: u32,
    /// [`SW_FLAG_CHECK`] or 0.
    flags: u32,
    /// Padding to 16 bytes.
    _pad: u32,
}

/// The sorted wavefront's settings while it is on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SortedSettings {
    /// The pool budget in bytes: the ray records and the queue entries of every slot.
    pub pool_bytes: usize,
    /// At most this many slots (the tests); `None` plans them from the budget and the device.
    pub pool_slots: Option<usize>,
    /// Exactly this many rounds before the drain (the tests); `None` plans them from the
    /// chunk, the pool and the bounce cap.
    pub rounds: Option<u32>,
    /// Counts and checksums every finished path, read back by the tests.
    pub check: bool,
}

impl SortedSettings {
    /// On with a pool budget of `pool_bytes`, the slots and rounds planned, no checks.
    #[must_use]
    pub const fn with_pool(pool_bytes: usize) -> Self {
        Self {
            pool_bytes,
            pool_slots: None,
            rounds: None,
            check: false,
        }
    }
}

/// A renderer's switch of the sorted wavefront.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortedSwitch {
    /// Follows `PLEOCHROMA_SORTED` ([`sorted_requested`]).
    #[default]
    Process,
    /// Off, whatever the variable says.
    Off,
    /// On with these settings, whatever the variable says.
    On(SortedSettings),
}

/// The pool budget in bytes that `value`, the content of `PLEOCHROMA_SORTED`, asks for; `None`
/// keeps the sorted wavefront off.
///
/// Unset, empty or `0`: off. `1`: on with a pool of 256 MiB. A whole number from 16 to 4096: on
/// with a pool of that many MiB. Anything else: off.
#[must_use]
pub fn sorted_requested(value: Option<&str>) -> Option<usize> {
    let text = value.map(str::trim)?;
    match text {
        "" | "0" => None,
        "1" => Some(DEFAULT_POOL_MIB * MIB),
        _ => text
            .parse::<usize>()
            .ok()
            .filter(|mib| (MIN_POOL_MIB..=MAX_POOL_MIB).contains(mib))
            .map(|mib| mib * MIB),
    }
}

/// The process's pool budget (`PLEOCHROMA_SORTED`), read once; the first read logs it or warns
/// about a value the switch does not take.
fn process_sorted() -> Option<usize> {
    static POOL: OnceLock<Option<usize>> = OnceLock::new();
    *POOL.get_or_init(|| {
        let value = std::env::var(SWITCH).ok();
        let pool = sorted_requested(value.as_deref());
        log_switch(value.as_deref(), pool);
        pool
    })
}

/// The one line [`process_sorted`] logs.
fn log_switch(value: Option<&str>, pool: Option<usize>) {
    if let Some(bytes) = pool {
        tracing::info!(
            target: "pleochroma_cut::render_bench",
            "sorted wavefront on ({SWITCH}): a ray pool of {} MiB; adaptive passes, the path \
             cache's passes, the optics tables, the stone shadow and the legacy wavefront keep \
             their engines",
            bytes / MIB
        );
        return;
    }
    if let Some(text) = value
        .map(str::trim)
        .filter(|text| !text.is_empty() && *text != "0")
    {
        tracing::warn!(
            target: "pleochroma_cut::render_bench",
            "{SWITCH}={text} is not 0, 1 or a pool of {MIN_POOL_MIB} to {MAX_POOL_MIB} MiB: the \
             sorted wavefront stays off"
        );
    }
}

/// The arm kernels, one per arm of the bounce text (`ARM_FROSTED` 1 to `ARM_GENERIC` 5).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SortedArm {
    /// The frosted (ground) finish.
    Frosted,
    /// Total internal reflection.
    Tir,
    /// The entry into a uniaxial stone.
    UniEntry,
    /// A uniaxial stone from inside: partial reflection and exit.
    UniInternal,
    /// Isotropic and biaxial reflection and refraction.
    Generic,
}

impl SortedArm {
    /// Every arm, in kernel order.
    pub(super) const ALL: [Self; 5] = [
        Self::Frosted,
        Self::Tir,
        Self::UniEntry,
        Self::UniInternal,
        Self::Generic,
    ];

    /// The kernel's entry point.
    pub(super) const fn entry_point(self) -> &'static str {
        match self {
            Self::Frosted => "sw_arm_frosted",
            Self::Tir => "sw_arm_tir",
            Self::UniEntry => "sw_arm_uni_entry",
            Self::UniInternal => "sw_arm_uni_internal",
            Self::Generic => "sw_arm_generic",
        }
    }

    /// The arm's place in the queue and indirect tables: its number in the bounce text minus 1.
    pub(super) const fn index(self) -> usize {
        match self {
            Self::Frosted => 0,
            Self::Tir => 1,
            Self::UniEntry => 2,
            Self::UniInternal => 3,
            Self::Generic => 4,
        }
    }
}

/// The arm kernels a pipeline of `class` dispatches: every arm its rays can be filed for. An
/// isotropic class never has an anisotropic ray and a biaxial class never takes a uniaxial arm
/// (`MATERIAL_CLASS` folds both in `transport_hoist_ray_constants`), the rule the event
/// wavefront's `kernels::arms_for` relies on too; the frosted arm is in every list, because a
/// ground facet or the abraded edge can choose it in any scene.
#[must_use]
pub(super) fn sorted_arms(class: u32) -> Vec<SortedArm> {
    let kind = class & material_class::KIND_MASK;
    if kind == material_class::UNIAXIAL || kind == material_class::GENERIC {
        SortedArm::ALL.to_vec()
    } else {
        vec![SortedArm::Frosted, SortedArm::Tir, SortedArm::Generic]
    }
}

/// Why a chunk the switch gives to the sorted wavefront runs on another engine.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum SortedDecline {
    /// The legacy compacting wavefront is selected.
    LegacyWavefront,
    /// The class uses the optics lookup tables.
    OpticsTables,
    /// The scene has a stone shadow or a tray reflection on the tray.
    TrayVariant,
    /// The device binds too few buffers per shader stage.
    DeviceLimits,
    /// The device cannot dispatch indirectly.
    NoIndirectDispatch,
    /// An adaptive (live-tile) pass.
    AdaptivePass,
}

impl SortedDecline {
    /// The decline's bit in the renderer's record of logged declines.
    pub(super) const fn bit(self) -> u32 {
        match self {
            Self::LegacyWavefront => 1,
            Self::OpticsTables => 2,
            Self::TrayVariant => 4,
            Self::DeviceLimits => 8,
            Self::NoIndirectDispatch => 16,
            Self::AdaptivePass => 32,
        }
    }

    /// The reason, for the log line.
    const fn reason(self) -> &'static str {
        match self {
            Self::LegacyWavefront => "the legacy wavefront is selected",
            Self::OpticsTables => {
                "the optics lookup tables are on, and the sorted kernels have no table variant yet"
            }
            Self::TrayVariant => {
                "the scene has a stone shadow or tray reflection, and the sorted kernels have no \
                 tray variant yet"
            }
            Self::DeviceLimits => {
                "the device binds fewer than 16 storage buffers (21 buffers in all) per shader \
                 stage"
            }
            Self::NoIndirectDispatch => "the device cannot dispatch indirectly",
            Self::AdaptivePass => "adaptive passes run on the megakernel's live kernels",
        }
    }
}

/// The first reason a chunk of `class` cannot run sorted with the pipeline kind `kind` on a
/// device with `limits` that can (`indirect`) or cannot dispatch indirectly; `None` when it can.
#[must_use]
pub(super) const fn sorted_decline(
    kind: GpuPipelineKind,
    class: u32,
    limits: &wgpu::Limits,
    indirect: bool,
) -> Option<SortedDecline> {
    if matches!(kind, GpuPipelineKind::Wavefront) {
        Some(SortedDecline::LegacyWavefront)
    } else if material_class::uses_optics_tables(class) {
        Some(SortedDecline::OpticsTables)
    } else if material_class::uses_tray_variant(class) {
        Some(SortedDecline::TrayVariant)
    } else if limits.max_storage_buffers_per_shader_stage < SORTED_STORAGE_BUFFERS
        || limits.max_buffers_and_acceleration_structures_per_shader_stage < SORTED_TOTAL_BUFFERS
    {
        Some(SortedDecline::DeviceLimits)
    } else if indirect {
        None
    } else {
        Some(SortedDecline::NoIndirectDispatch)
    }
}

/// One chunk's schedule.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) struct SortedPlan {
    /// The chunk's tuples.
    tuples: usize,
    /// The slots of the pool it uses.
    slots: usize,
    /// The rounds before the drain.
    rounds: u32,
    /// `SwParams::flags`.
    flags: u32,
}

/// The slots a chunk of `tuples` tuples gets under a pool budget of `pool_bytes` on a device whose
/// largest storage binding holds `binding_bytes`: the widest record (`EwRayChan`) of every slot
/// must fit one binding, and one dispatch must address every slot.
#[must_use]
pub(super) fn pool_slots_for(tuples: usize, pool_bytes: usize, binding_bytes: usize) -> usize {
    tuples
        .min(pool_bytes / SLOT_BYTES)
        .min(binding_bytes / size_of::<EwRayChan>())
        .min(MAX_POOL_SLOTS)
}

/// The rounds a chunk of `tuples` tuples runs with `slots` slots before the drain. A pool as large
/// as the chunk is never refilled: one round per bounce plus the first front ends every path of
/// up to 32 bounces in the rounds. A smaller pool is refilled while tuples are left
/// ([`ROUNDS_PER_TUPLE`] rounds per tuple and slot), then gets the same tail. At most
/// [`MAX_ROUNDS`].
#[must_use]
pub(super) fn rounds_for(tuples: usize, slots: usize, max_bounces: u32) -> u32 {
    if tuples == 0 || slots == 0 {
        return 0;
    }
    let tail = max_bounces.min(TAIL_ROUNDS_MAX) + 1;
    let refill = if slots >= tuples {
        0
    } else {
        u32::try_from((tuples * ROUNDS_PER_TUPLE).div_ceil(slots)).unwrap_or(MAX_ROUNDS)
    };
    refill.saturating_add(tail).min(MAX_ROUNDS)
}

/// The sorted kernels of one class.
struct ClassPipelines {
    /// `sw_front_main`.
    front: wgpu::ComputePipeline,
    /// The class's arm kernels ([`sorted_arms`]).
    arms: Vec<(SortedArm, wgpu::ComputePipeline)>,
    /// `sw_drain_main`.
    drain: wgpu::ComputePipeline,
}

/// The two one-thread kernels (one unit for every class).
struct PreparePipelines {
    /// `sw_prepare`.
    round: wgpu::ComputePipeline,
    /// `sw_prepare_drain`.
    drain: wgpu::ComputePipeline,
}

/// The ray pool, its queues and the chunk's small buffers. All are replaced together when the
/// pool grows; a chunk in flight keeps the old ones alive through its bind groups.
struct SortedBuffers {
    /// `EwRayCore` per slot.
    ray_core: wgpu::Buffer,
    /// `EwRayChan` per slot.
    ray_chan: wgpu::Buffer,
    /// `EwHit` per slot.
    ray_hit: wgpu::Buffer,
    /// [`SW_QUEUES`] regions of one entry per slot.
    queues: wgpu::Buffer,
    /// [`SW_COUNTERS`] words, cleared per chunk (`COPY_SRC` for the tests).
    counts: wgpu::Buffer,
    /// [`SW_INDIRECTS`] argument triples.
    indirect: wgpu::Buffer,
    /// The `SwParams` uniform, shared by both chunk slots (queue order keeps them apart).
    params: wgpu::Buffer,
    /// Slots the pool holds.
    slots: usize,
}

impl SortedBuffers {
    /// A pool of `slots` slots and the small buffers.
    fn new(device: &wgpu::Device, slots: usize) -> Self {
        let buffer = |label: &str, bytes: usize, usage: BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes as wgpu::BufferAddress,
                usage,
                mapped_at_creation: false,
            })
        };
        Self {
            ray_core: buffer(
                "sorted wavefront ray cores",
                slots * size_of::<EwRayCore>(),
                BufferUsages::STORAGE,
            ),
            ray_chan: buffer(
                "sorted wavefront ray channels",
                slots * size_of::<EwRayChan>(),
                BufferUsages::STORAGE,
            ),
            ray_hit: buffer(
                "sorted wavefront ray hits",
                slots * size_of::<EwHit>(),
                BufferUsages::STORAGE,
            ),
            queues: buffer(
                "sorted wavefront queues",
                slots * SW_QUEUES * size_of::<u32>(),
                BufferUsages::STORAGE,
            ),
            counts: buffer(
                "sorted wavefront counters",
                SW_COUNTERS * size_of::<u32>(),
                BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
            ),
            indirect: buffer(
                "sorted wavefront indirect arguments",
                SW_INDIRECTS * 3 * size_of::<u32>(),
                BufferUsages::STORAGE | BufferUsages::INDIRECT,
            ),
            params: buffer(
                "sorted wavefront params",
                size_of::<GpuSortedParams>(),
                BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            ),
            slots,
        }
    }
}

/// One chunk's bind groups, one per pipeline (a pipeline with an implicit layout takes only
/// bind groups made from its own layout).
struct SortedGroups {
    /// `sw_prepare`'s.
    prepare_round: wgpu::BindGroup,
    /// `sw_prepare_drain`'s.
    prepare_drain: wgpu::BindGroup,
    /// The front kernel's.
    front: wgpu::BindGroup,
    /// The arm kernels', in [`ClassPipelines::arms`] order.
    arms: Vec<wgpu::BindGroup>,
    /// The drain kernel's.
    drain: wgpu::BindGroup,
}

/// The sorted wavefront's switch, pipelines and buffers. Empty (nothing compiled, nothing
/// allocated) until a chunk runs sorted.
#[derive(Default)]
pub(super) struct SortedState {
    /// This renderer's switch.
    switch: SortedSwitch,
    /// The front, arm and drain kernels per class value.
    pipelines: HashMap<u32, ClassPipelines>,
    /// The two one-thread kernels.
    prepare: Option<PreparePipelines>,
    /// The pool and the chunk's small buffers.
    buffers: Option<SortedBuffers>,
    /// The declines already logged ([`SortedDecline::bit`]).
    declined: u32,
}

impl GpuFrameRenderer {
    /// Switches the sorted wavefront for every LATER frame of this renderer: off, on with
    /// settings of its own, or following `PLEOCHROMA_SORTED` (the default). The chunk timing
    /// starts over, because the cost per tuple changes. The browser build stays off whatever
    /// the switch says: it carries no sorted unit.
    pub const fn set_sorted(&mut self, switch: SortedSwitch) {
        self.sorted.switch = switch;
        self.last_scene_key = None;
    }

    /// The settings in force; `None` while the sorted wavefront is off, and always in the browser
    /// build.
    fn sorted_settings(&self) -> Option<SortedSettings> {
        let requested = match self.sorted.switch {
            SortedSwitch::Process => process_sorted().map(SortedSettings::with_pool),
            SortedSwitch::Off => None,
            SortedSwitch::On(settings) => Some(settings),
        };
        // The browser build carries no sorted unit (`SORTED_SHADER_SRC` is empty there).
        if cfg!(target_arch = "wasm32") {
            None
        } else {
            requested
        }
    }

    /// The first reason chunks of `class` cannot run sorted on this renderer and device.
    fn sorted_decline_for(&self, class: u32) -> Option<SortedDecline> {
        let indirect = self
            .ctx
            .adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION);
        sorted_decline(
            self.pipeline_kind,
            class,
            &self.ctx.device.limits(),
            indirect,
        )
    }

    /// Logs `decline` the first time this renderer meets it.
    fn note_sorted_decline(&mut self, decline: SortedDecline) {
        if self.sorted.declined & decline.bit() != 0 {
            return;
        }
        self.sorted.declined |= decline.bit();
        tracing::warn!(
            target: "pleochroma_cut::render_bench",
            "sorted wavefront ({SWITCH}) declined: {}; these chunks keep their engine",
            decline.reason()
        );
    }

    /// Called as an adaptive pass starts: while the sorted wavefront is on, says once that such
    /// passes keep the megakernel's live kernels.
    pub(super) fn note_sorted_live_pass(&mut self) {
        if self.sorted_settings().is_some() {
            self.note_sorted_decline(SortedDecline::AdaptivePass);
        }
    }

    /// The largest storage binding (and buffer) of this device, in bytes.
    fn storage_binding_bytes(&self) -> usize {
        let limits = self.ctx.device.limits();
        usize::try_from(
            limits
                .max_storage_buffer_binding_size
                .min(limits.max_buffer_size),
        )
        .unwrap_or(usize::MAX)
    }

    /// The slots a chunk of `tuples` tuples gets under `settings` on this device.
    fn sorted_slots(&self, settings: SortedSettings, tuples: usize) -> usize {
        let planned = pool_slots_for(tuples, settings.pool_bytes, self.storage_binding_bytes());
        settings
            .pool_slots
            .map_or(planned, |slots| slots.min(planned))
    }

    /// Prepares the chunk of `pixels` pixels of the frame `state` for the sorted wavefront:
    /// compiles the class's kernels on first use and grows the pool to the chunk. False when the
    /// chunk does not run sorted: the switch is off, a decline (logged once per reason), or no
    /// slot (an empty chunk, or a budget or device binding smaller than one slot).
    pub(super) fn ensure_sorted(&mut self, state: &ChunkFrameState<'_>, pixels: usize) -> bool {
        let Some(settings) = self.sorted_settings() else {
            return false;
        };
        if let Some(decline) = self.sorted_decline_for(state.pipeline_class) {
            self.note_sorted_decline(decline);
            return false;
        }
        let slots = self.sorted_slots(settings, pixels * state.spp as usize);
        if slots == 0 {
            return false;
        }
        self.ensure_sorted_pipelines(state.pipeline_class);
        let cap = pool_slots_for(
            MAX_POOL_SLOTS,
            settings.pool_bytes,
            self.storage_binding_bytes(),
        );
        self.ensure_sorted_buffers(slots, cap);
        true
    }

    /// Prepares the engine of the next chunk: the sorted wavefront when it takes the chunk, else
    /// the staged megakernel (a no-op while its switch is off).
    pub(super) fn ensure_chunk_engine(&mut self, state: &ChunkFrameState<'_>, pixels: usize) {
        if !self.ensure_sorted(state, pixels) {
            self.ensure_staged(state, pixels);
        }
    }

    /// Compiles the one-thread kernels and the kernels of `class` on first use.
    fn ensure_sorted_pipelines(&mut self, class: u32) {
        if self.sorted.prepare.is_none() {
            let info = self.ctx.adapter.get_info();
            let device = &self.ctx.device;
            let round = compute::create_persisted_compute_pipeline(
                device,
                &info,
                PREPARE_ENTRY,
                PREPARE_SHADER_SRC,
                PREPARE_ENTRY,
                &[],
            );
            let drain = compute::create_persisted_compute_pipeline(
                device,
                &info,
                PREPARE_DRAIN_ENTRY,
                PREPARE_SHADER_SRC,
                PREPARE_DRAIN_ENTRY,
                &[],
            );
            self.sorted.prepare = Some(PreparePipelines { round, drain });
        }
        if self.sorted.pipelines.contains_key(&class) {
            return;
        }
        let pipelines = self.build_sorted_pipelines(class);
        self.sorted.pipelines.insert(class, pipelines);
    }

    /// Compiles the front, arm and drain kernels of `class`: megakernel-sized each, persisted
    /// where the device keeps a pipeline cache.
    fn build_sorted_pipelines(&self, class: u32) -> ClassPipelines {
        let info = self.ctx.adapter.get_info();
        let constants = material_class::pipeline_constants(class);
        let kind = class & material_class::KIND_MASK;
        let features = class & material_class::FEATURE_MASK;
        let build = |entry: &str| {
            compute::create_persisted_compute_pipeline(
                &self.ctx.device,
                &info,
                &format!("{entry} (MATERIAL_CLASS={kind}, features {features:#x})"),
                SORTED_SHADER_SRC,
                entry,
                &constants,
            )
        };
        let pipelines = ClassPipelines {
            front: build(FRONT_ENTRY),
            arms: sorted_arms(class)
                .into_iter()
                .map(|arm| (arm, build(arm.entry_point())))
                .collect(),
            drain: build(DRAIN_ENTRY),
        };
        tracing::info!(
            target: "pleochroma_cut::render_bench",
            "gpu pipeline: sorted wavefront, class {kind} features {features:#x}, {} arm kernels",
            pipelines.arms.len()
        );
        pipelines
    }

    /// Grows the pool to at least `slots` slots, in steps of [`POOL_GROWTH_SLOTS`] up to `cap`,
    /// and makes the small buffers with it.
    fn ensure_sorted_buffers(&mut self, slots: usize, cap: usize) {
        if self
            .sorted
            .buffers
            .as_ref()
            .is_some_and(|buffers| buffers.slots >= slots)
        {
            return;
        }
        let grown = slots
            .next_multiple_of(POOL_GROWTH_SLOTS)
            .min(cap)
            .max(slots);
        self.sorted.buffers = Some(SortedBuffers::new(&self.ctx.device, grown));
    }

    /// The schedule of a chunk of `tuples` tuples, or `None` when it does not run sorted: the
    /// switch is off, the chunk declines, or [`Self::ensure_sorted`] has not prepared it. The
    /// slots are planned for the pool that exists, so the plan always fits it.
    pub(super) fn sorted_plan_for(
        &self,
        state: &ChunkFrameState<'_>,
        tuples: usize,
    ) -> Option<SortedPlan> {
        let settings = self.sorted_settings()?;
        let pool = self.sorted.buffers.as_ref()?.slots;
        if self.sorted_decline_for(state.pipeline_class).is_some()
            || self.sorted.prepare.is_none()
            || !self.sorted.pipelines.contains_key(&state.pipeline_class)
        {
            return None;
        }
        let slots = self.sorted_slots(settings, tuples).min(pool);
        if slots == 0 {
            return None;
        }
        let rounds = settings
            .rounds
            .unwrap_or_else(|| rounds_for(tuples, slots, state.scene.max_bounces))
            .min(MAX_ROUNDS);
        Some(SortedPlan {
            tuples,
            slots,
            rounds,
            flags: if settings.check { SW_FLAG_CHECK } else { 0 },
        })
    }

    /// Queues one chunk through the sorted wavefront, in one submission and without waiting:
    /// the counters are cleared, the rounds of `plan` and the drain are recorded into one compute
    /// pass, and `outputs.xyz()` ends up holding every tuple's XYZ as the megakernel's dispatch
    /// leaves it. The pool, the queues and the counters are shared by both chunk slots: a chunk's
    /// commands run to their end before the next chunk's begin (queue order), and the clear and
    /// the uniform write are ordered with them.
    ///
    /// # Panics
    ///
    /// Panics if `plan` did not come from [`Self::sorted_plan_for`] of this renderer.
    pub(super) fn dispatch_sorted(
        &self,
        state: &ChunkFrameState<'_>,
        frame_buffers: &FrameSceneBuffers,
        params_buf: &wgpu::Buffer,
        outputs: &TransportOutputs,
        plan: &SortedPlan,
    ) {
        let pipelines = self
            .sorted
            .pipelines
            .get(&state.pipeline_class)
            .expect(PREPARED);
        let prepare = self.sorted.prepare.as_ref().expect(PREPARED);
        let buffers = self.sorted.buffers.as_ref().expect(PREPARED);
        let mut mark = timing::Mark::now();
        let params = GpuSortedParams {
            tuples: count_u32(plan.tuples),
            pool: count_u32(plan.slots),
            flags: plan.flags,
            _pad: 0,
        };
        self.ctx
            .queue
            .write_buffer(&buffers.params, 0, bytemuck::bytes_of(&params));
        let transport = transport_entries(state, frame_buffers, params_buf, outputs, buffers);
        let groups = sorted_groups(&self.ctx.device, pipelines, prepare, &transport, buffers);
        mark.lap(Slot::TransportBind);
        let mut encoder = self
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("pleochroma gpu sorted wavefront encoder"),
            });
        encoder.clear_buffer(&buffers.counts, 0, None);
        encode_sorted(
            &mut encoder,
            pipelines,
            prepare,
            &groups,
            &buffers.indirect,
            plan.rounds,
        );
        let commands = encoder.finish();
        mark.lap(Slot::TransportEncode);
        self.ctx.queue.submit(Some(commands));
        mark.lap(Slot::TransportSubmit);
        if timing::enabled() {
            tracing::info!(
                target: "pleochroma_cut::render_bench",
                "gpu_timing sorted_chunk tuples={} pool={} rounds={} arms={} check={}",
                params.tuples,
                params.pool,
                plan.rounds,
                pipelines.arms.len(),
                params.flags & SW_FLAG_CHECK != 0
            );
        }
    }

    /// The counters of the last sorted chunk ([`SW_COUNTERS`] words), read back once every
    /// submission has finished; `None` before the first sorted chunk.
    #[cfg(test)]
    pub(super) fn sorted_counters(&self) -> Option<Vec<u32>> {
        let buffers = self.sorted.buffers.as_ref()?;
        Some(compute::readback(
            &self.ctx.device,
            &self.ctx.queue,
            &buffers.counts,
            SW_COUNTERS,
        ))
    }

    /// The declines this renderer has logged ([`SortedDecline::bit`]).
    #[cfg(test)]
    pub(super) const fn sorted_declined(&self) -> u32 {
        self.sorted.declined
    }
}

/// The transport kernels' bindings ([`TRANSPORT_SLOTS`], in that order): the megakernel's
/// fifteen as `build_chunk_bind_group` binds them, the ray records, the parameters and the
/// queues.
fn transport_entries<'a>(
    state: &ChunkFrameState<'_>,
    frame_buffers: &'a FrameSceneBuffers,
    params_buf: &'a wgpu::Buffer,
    outputs: &'a TransportOutputs,
    buffers: &'a SortedBuffers,
) -> [Entry<'a>; 21] {
    let planes_bytes = (state.packed_len * size_of::<GpuFacetPlane>()) as wgpu::BufferAddress;
    let finishes_bytes = (state.packed_len * size_of::<u32>()) as wgpu::BufferAddress;
    let entries: [Entry<'a>; 21] = [
        (0, &frame_buffers.camera, None),
        (1, params_buf, None),
        (2, &frame_buffers.material, None),
        (3, &frame_buffers.planes, Some(planes_bytes)),
        (4, outputs.xyz(), None),
        (5, outputs.radiance(), None),
        (6, outputs.lambdas(), None),
        (7, outputs.path_pdf(), None),
        (8, &frame_buffers.facet_finishes, Some(finishes_bytes)),
        (9, outputs.compat(), None),
        (10, &frame_buffers.hdr_env.texels, None),
        (11, &frame_buffers.hdr_env.dims, None),
        (12, &frame_buffers.hdr_env.dist_func, None),
        (13, &frame_buffers.hdr_env.dist_cdf, None),
        (14, &frame_buffers.hdr_env.dist_dims, None),
        (RAY_CORE_BINDING, &buffers.ray_core, None),
        (RAY_CHAN_BINDING, &buffers.ray_chan, None),
        (RAY_HIT_BINDING, &buffers.ray_hit, None),
        (SW_PARAMS_BINDING, &buffers.params, None),
        (SW_QUEUES_BINDING, &buffers.queues, None),
        (SW_COUNTS_BINDING, &buffers.counts, None),
    ];
    debug_assert!(entries.iter().map(|entry| entry.0).eq(TRANSPORT_SLOTS));
    entries
}

/// One chunk's bind groups: the transport table for the front, every arm and the drain kernel,
/// the prepare table for the two one-thread kernels.
fn sorted_groups(
    device: &wgpu::Device,
    pipelines: &ClassPipelines,
    prepare: &PreparePipelines,
    transport: &[Entry<'_>],
    buffers: &SortedBuffers,
) -> SortedGroups {
    let prepare_entries: [Entry<'_>; 3] = [
        (SW_PARAMS_BINDING, &buffers.params, None),
        (SW_COUNTS_BINDING, &buffers.counts, None),
        (SW_INDIRECT_BINDING, &buffers.indirect, None),
    ];
    debug_assert!(
        prepare_entries
            .iter()
            .map(|entry| entry.0)
            .eq(PREPARE_SLOTS)
    );
    SortedGroups {
        prepare_round: compute::bind_buffers_sized(
            device,
            PREPARE_ENTRY,
            &prepare.round,
            &prepare_entries,
        ),
        prepare_drain: compute::bind_buffers_sized(
            device,
            PREPARE_DRAIN_ENTRY,
            &prepare.drain,
            &prepare_entries,
        ),
        front: compute::bind_buffers_sized(device, FRONT_ENTRY, &pipelines.front, transport),
        arms: pipelines
            .arms
            .iter()
            .map(|(arm, pipeline)| {
                compute::bind_buffers_sized(device, arm.entry_point(), pipeline, transport)
            })
            .collect(),
        drain: compute::bind_buffers_sized(device, DRAIN_ENTRY, &pipelines.drain, transport),
    }
}

/// Records the rounds and the drain of one chunk into one compute pass: per round the kernel
/// that opens it, the front kernel and every arm kernel, each over the indirect arguments the
/// opening kernel just wrote; then the drain's layout and the drain. wgpu makes each dispatch of
/// a compute pass its own usage scope, so every dispatch sees the writes of the ones before.
fn encode_sorted(
    encoder: &mut wgpu::CommandEncoder,
    pipelines: &ClassPipelines,
    prepare: &PreparePipelines,
    groups: &SortedGroups,
    indirect: &wgpu::Buffer,
    rounds: u32,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("pleochroma gpu sorted wavefront pass"),
        timestamp_writes: None,
    });
    for _ in 0..rounds {
        pass.set_pipeline(&prepare.round);
        pass.set_bind_group(0, &groups.prepare_round, &[]);
        pass.dispatch_workgroups(1, 1, 1);
        pass.set_pipeline(&pipelines.front);
        pass.set_bind_group(0, &groups.front, &[]);
        pass.dispatch_workgroups_indirect(indirect, indirect_offset(SW_I_FRONT));
        for ((arm, pipeline), group) in pipelines.arms.iter().zip(&groups.arms) {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups_indirect(indirect, indirect_offset(SW_I_ARM + arm.index()));
        }
    }
    pass.set_pipeline(&prepare.drain);
    pass.set_bind_group(0, &groups.prepare_drain, &[]);
    pass.dispatch_workgroups(1, 1, 1);
    pass.set_pipeline(&pipelines.drain);
    pass.set_bind_group(0, &groups.drain, &[]);
    pass.dispatch_workgroups_indirect(indirect, indirect_offset(SW_I_DRAIN));
}

/// The byte offset of the indirect arguments at `index` (three words each).
const fn indirect_offset(index: usize) -> wgpu::BufferAddress {
    (index * 3 * size_of::<u32>()) as wgpu::BufferAddress
}
