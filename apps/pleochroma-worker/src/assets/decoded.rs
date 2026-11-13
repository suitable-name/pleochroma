//! The process-wide registry of DECODED environment maps, keyed by content hash, and the
//! one way the tracer turns a scene into its `EnvironmentSource` ([`environment_source`]).
//!
//! # Lifetimes
//!
//! - A request that resolved its map holds an `Arc` for as long as it streams, so the
//!   tracer thread's [`resolved_hdr_map`] always finds it.
//! - The [`RECENT_KEEP`] most recently resolved maps stay alive between requests: a live
//!   view sends a new request per settle, and re-decoding a large panorama each time
//!   would dominate the latency.
//! - Every map ever registered also leaves a `Weak` behind for the life of the process.
//!   The GPU backend recognises "the same map as last dispatch" by its address; a `Weak`
//!   keeps that allocation reserved (the texels themselves are freed), so a later,
//!   different map can never reuse the address and be mistaken for a stale upload.
//!
//! # Decoding
//!
//! Radiance (`.hdr`) assets are decoded only by
//! `pleochroma::renderer::env_map::environment_from_hdr_bytes` with `HdrLimits::DEFAULT`,
//! the viewer's own builder, so both sides hold the identical map. An `IXENVMAP` float
//! asset (`pleochroma_net::messages::envmap_asset`) is rebuilt with
//! `EnvironmentMap::from_rgb` from its exact texels, so it is bit-identical too. One decode
//! runs at a time per process ([`DECODE_LOCK`]), which bounds peak memory to one decode.
//!
//! # The Stone Shadow
//!
//! A scene with `stone_shadow` set is lit with a spectral shadow map the worker builds from the
//! scene itself (the protocol carries only the setting). The map is resolved ONCE per request
//! ([`resolve_tray`], or a [`TrayPin`] for a coordinator's chunks), at the quality
//! [`shadow_quality`] picks for the request's intent, and the request holds the `Arc` while it
//! traces, like the decoded HDR map above. [`environment_for`] adds it to the lighting. A map
//! that cannot be built refuses the request ([`TrayStop::Refused`]); it never renders
//! without the shadow. The shadow needs the studio rig, so it never meets an HDR map.
//!
//! # The Tray Reflection
//!
//! A scene with `tray_reflection > 0` gets a glossy tray ([`mirror_for_scene`]): a plane at the
//! stone's culet (the shadow's plane) with Schlick reflectance `R0 = tray_reflection`. The plane
//! is built once per request with the shadow: [`resolve_tray`] (hence [`TrayPin`]) returns both
//! as a [`Tray`], refuses a scene whose plane cannot be placed with the mirror's sentence,
//! exactly as a shadow that cannot be built is refused, and [`environment_for`] attaches what the
//! request carried ([`TrayRef`]) to every environment without rebuilding it.

use pleochroma::{
    optics::raytracer::{
        EnvironmentSource,
        environment::{
            TrayMirror,
            tray_shadow::{TrayShadowMap, TrayShadowOff, TrayShadowQuality},
        },
    },
    render_setup::TrayMirrorOff,
    renderer::env_map::{EnvironmentMap, HdrLimits, environment_from_hdr_bytes},
};
use pleochroma_net::{
    SceneState,
    messages::{ContentHash, RequestIntent, envmap_asset, hash_hex},
    scene::HdrEnvironment,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, PoisonError, Weak, atomic::AtomicBool},
};

/// How many recently resolved maps stay decoded between requests.
const RECENT_KEEP: usize = 2;

/// Every map ever registered, by hash (see the module doc comment's "Lifetimes").
static REGISTERED: Mutex<Vec<(ContentHash, Weak<EnvironmentMap>)>> = Mutex::new(Vec::new());

/// The maps kept decoded between requests, most recent last.
static RECENT: Mutex<VecDeque<(ContentHash, Arc<EnvironmentMap>)>> = Mutex::new(VecDeque::new());

/// Serialises decodes process-wide (see the module doc comment's "Decoding").
static DECODE_LOCK: Mutex<()> = Mutex::new(());

/// The decoded map for `hash`, if one is alive (pinned by a request or kept recent).
#[must_use]
pub fn lookup(hash: &ContentHash) -> Option<Arc<EnvironmentMap>> {
    let found = REGISTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .filter(|(h, _)| h == hash)
        .find_map(|(_, weak)| weak.upgrade());
    if let Some(map) = &found {
        remember_recent(hash, map);
    }
    found
}

/// Builds the map one asset holds: an `IXENVMAP` float asset (told apart by its
/// magic) is rebuilt from its texels with `EnvironmentMap::from_rgb`; anything else goes
/// through the shared Radiance builder. Both routes are bounded before the pixel buffer
/// exists (the asset's exact length, `HdrLimits::DEFAULT`'s header check).
fn build_map(bytes: &[u8]) -> Result<EnvironmentMap, String> {
    if envmap_asset::is_envmap_asset(bytes) {
        let (width, height, texels) =
            envmap_asset::decode_envmap_asset(bytes).map_err(|e| e.to_string())?;
        return EnvironmentMap::from_rgb(width as usize, height as usize, texels)
            .map_err(|e| e.to_string());
    }
    environment_from_hdr_bytes(bytes, HdrLimits::DEFAULT).map_err(|e| e.to_string())
}

/// Decodes `bytes` (the asset `expected` names) with the shared builder and registers
/// the result. The decoded dimensions must equal `expected`'s.
///
/// # Errors
///
/// A human-readable reason when the bytes do not decode (malformed, over
/// `HdrLimits::DEFAULT`) or decode to other dimensions than the scene declared.
pub fn decode_and_register(
    expected: &HdrEnvironment,
    bytes: &[u8],
) -> Result<Arc<EnvironmentMap>, String> {
    let map = {
        let _one_at_a_time = DECODE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        build_map(bytes)?
    };
    if (map.width(), map.height()) != (expected.width as usize, expected.height as usize) {
        return Err(format!(
            "the HDR map decodes to {}x{} texels but the scene declared {}x{}",
            map.width(),
            map.height(),
            expected.width,
            expected.height
        ));
    }
    let map = Arc::new(map);
    let mut registered = REGISTERED.lock().unwrap_or_else(PoisonError::into_inner);
    // Drops this hash's dead entries only: a fresh decode of the same hash is identical,
    // so its old address may be reused, unlike another hash's (see the module doc).
    registered.retain(|(h, weak)| *h != expected.content_hash || weak.strong_count() > 0);
    registered.push((expected.content_hash, Arc::downgrade(&map)));
    drop(registered);
    remember_recent(&expected.content_hash, &map);
    Ok(map)
}

/// Moves `hash` to the most-recent end of [`RECENT`], dropping the oldest past
/// [`RECENT_KEEP`].
fn remember_recent(hash: &ContentHash, map: &Arc<EnvironmentMap>) {
    push_recent(
        &mut RECENT.lock().unwrap_or_else(PoisonError::into_inner),
        hash,
        map,
    );
}

/// [`remember_recent`] on the locked list.
fn push_recent(
    recent: &mut VecDeque<(ContentHash, Arc<EnvironmentMap>)>,
    hash: &ContentHash,
    map: &Arc<EnvironmentMap>,
) {
    recent.retain(|(h, _)| h != hash);
    recent.push_back((*hash, Arc::clone(map)));
    while recent.len() > RECENT_KEEP {
        recent.pop_front();
    }
}

/// The decoded HDR map `scene` is lit by, or `None` for the studio rig.
///
/// # Panics
///
/// When `scene` names an HDR map that no request resolved first. The request path
/// resolves and pins the map before tracing; the panic (reported to the viewer as
/// `TRACE_PANIC`) keeps an HDR scene from ever being lit by the studio rig.
#[must_use]
pub fn resolved_hdr_map(scene: &SceneState) -> Option<Arc<EnvironmentMap>> {
    let hdr = scene.hdr()?;
    Some(lookup(&hdr.content_hash).unwrap_or_else(|| {
        panic!(
            "HDR environment {} was not resolved before tracing (the request path must fetch it first)",
            hash_hex(&hdr.content_hash)
        )
    }))
}

/// The `EnvironmentSource` the tracer lights `scene` with.
///
/// The studio rig from the scene's lighting fields, or `hdr_map` (from
/// [`resolved_hdr_map`]) lit as the viewer lights an HDR scene: `EnvironmentSource::hdr_at`
/// at `scene.exposure`, turned by `scene.hdr_orientation` for the scene's camera yaw, with
/// no backdrop and no surface glare. The studio rig carries the scene's backdrop and
/// `surface_glare`. `scene.backdrop` is the sender's resolved tray radiance at exposure 1
/// (`Backdrop::resolve`), passed through unchanged, so every peer lights the same tray.
/// Either way the exposure is applied once, in the lighting, so the raw sums a worker
/// streams and the pictures it tone-maps carry it alike.
#[must_use]
pub fn environment_source<'a>(
    scene: &SceneState,
    hdr_map: Option<&'a EnvironmentMap>,
) -> EnvironmentSource<'a> {
    hdr_map.map_or_else(
        || {
            scene
                .lighting_preset
                .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
                .with_backdrop(scene.backdrop)
                .with_surface_glare(scene.surface_glare)
                .with_head_shadow(scene.head_shadow_deg)
        },
        |map| {
            EnvironmentSource::hdr_at(map, scene.exposure)
                .with_hdr_rotation(scene.hdr_orientation.rotation_for(scene.yaw))
        },
    )
}

/// The Stone Shadow map of one scene: built (or reused from `pleochroma`'s three-entry cache)
/// by [`shadow_for_scene`] and held by the request for as long as it traces.
pub type Shadow = Option<Arc<TrayShadowMap>>;

/// The Stone Shadow map and the Tray Reflection mirror of one request, resolved once by
/// [`resolve_tray`] (or a [`TrayPin`]) and held by the request while it traces.
#[derive(Debug, Clone, PartialEq)]
pub struct Tray {
    /// The scene's Stone Shadow map, `Some` exactly when `scene.stone_shadow` is set.
    pub shadow: Shadow,
    /// The scene's glossy tray, `Some` exactly when `scene.tray_reflection > 0.0`.
    pub mirror: Option<TrayMirror>,
}

impl Tray {
    /// The borrow every tracer entry point takes.
    #[must_use]
    pub fn view(&self) -> TrayRef<'_> {
        TrayRef {
            shadow: self.shadow.as_deref(),
            mirror: self.mirror,
        }
    }
}

/// A borrow of a request's [`Tray`] ([`Tray::view`]), what every tracer entry point takes. The
/// default is a scene with neither the Stone Shadow nor the Tray Reflection.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrayRef<'a> {
    /// The request's Stone Shadow map.
    pub shadow: Option<&'a TrayShadowMap>,
    /// The request's glossy tray.
    pub mirror: Option<TrayMirror>,
}

/// The map quality a request is traced with.
///
/// A `RenderRequest` carries its [`RequestIntent`]: the desktop's live viewport sends
/// `Interactive` and sums this worker's samples with its own local half, which builds the
/// `Preview` map, so the two halves must agree. Every other request (`RenderRequest` with the
/// `Batch` intent, `FinalImageRequest`, `BatchItem`, `JobSubmit` and `LivePassRequest`, whose
/// only client is the desktop's adaptive export) is a finished picture and takes the `Final`
/// map; those pass `None`.
#[must_use]
pub const fn shadow_quality(intent: Option<RequestIntent>) -> TrayShadowQuality {
    match intent {
        Some(RequestIntent::Interactive) => TrayShadowQuality::Preview,
        Some(RequestIntent::Batch) | None => TrayShadowQuality::Final,
    }
}

/// Why a request's Stone Shadow or Tray Reflection could not be resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayStop {
    /// `cancel` was raised while the map was being built: the request is cancelled, not
    /// refused.
    Cancelled,
    /// The scene cannot have its shadow or its mirror (`TrayShadowOff::message`,
    /// `TrayMirrorOff::message`): the request is refused with this sentence, never traced
    /// without what it asked for.
    Refused(&'static str),
}

impl TrayStop {
    /// The sentence a refused request answers with (`TrayShadowOff::message` or
    /// `TrayMirrorOff::message`).
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Cancelled => TrayShadowOff::Cancelled.message(),
            Self::Refused(message) => message,
        }
    }
}

/// Resolves the shadow map and the mirror of `scene` once, for the request that owns it.
///
/// The quality is the one [`shadow_quality`] picks for `intent`; a scene without the Stone
/// Shadow resolves to no map and one without the Tray Reflection to no mirror.
///
/// # Errors
///
/// [`TrayStop::Cancelled`] when `cancel` fired, [`TrayStop::Refused`] for any other reason
/// the shadow or the mirror is unavailable (the sentence of `TrayShadowOff::message` or
/// `TrayMirrorOff::message`).
pub fn resolve_tray(
    scene: &SceneState,
    intent: Option<RequestIntent>,
    cancel: &AtomicBool,
) -> Result<Tray, TrayStop> {
    // A scene whose Tray Reflection cannot be placed is refused with the mirror's sentence, never
    // traced with a matte tray; the plane is cheap, so it is built before the shadow map.
    let mirror = mirror_for_scene(scene).map_err(|off| TrayStop::Refused(off.message()))?;
    let shadow =
        shadow_for_scene(scene, shadow_quality(intent), cancel).map_err(|off| match off {
            TrayShadowOff::Cancelled => TrayStop::Cancelled,
            refused => TrayStop::Refused(refused.message()),
        })?;
    Ok(Tray { shadow, mirror })
}

/// A request's Stone Shadow and Tray Reflection, resolved on first use and held until the pin is
/// dropped.
///
/// The way `crate::serve`'s request loop holds the decoded HDR map: every sub-batch, GPU
/// decline fallback and chunk of the request reads the same `Arc`, so a map evicted from
/// `pleochroma`'s three-entry cache mid-render is never rebuilt, and the first (slow) build
/// happens before the timed sub-batch loop starts. One pin serves one scene.
pub struct TrayPin {
    intent: Option<RequestIntent>,
    held: Mutex<Option<Tray>>,
}

impl TrayPin {
    /// A pin for a request with `intent` (`None` for a finished-picture request, see
    /// [`shadow_quality`]).
    #[must_use]
    pub const fn new(intent: Option<RequestIntent>) -> Self {
        Self {
            intent,
            held: Mutex::new(None),
        }
    }

    /// The pinned shadow and mirror of `scene`: built by the first call and returned again by
    /// every later one. A failed or cancelled build is not remembered.
    ///
    /// # Errors
    ///
    /// As [`resolve_tray`].
    pub fn resolve(&self, scene: &SceneState, cancel: &AtomicBool) -> Result<Tray, TrayStop> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(tray) = held.as_ref() {
            return Ok(tray.clone());
        }
        let tray = resolve_tray(scene, self.intent, cancel)?;
        *held = Some(tray.clone());
        drop(held);
        Ok(tray)
    }
}

/// [`environment_source`] with the request's resolved Stone Shadow and Tray Reflection.
///
/// `tray.shadow` must be `Some` exactly when `scene.stone_shadow` is, and `tray.mirror` exactly
/// when `scene.tray_reflection > 0.0`: a scene is never lit without the shadow or the glossy tray
/// it asked for (debug builds assert both). Both come from the request's one [`resolve_tray`], so
/// nothing is rebuilt here and no tracer entry point needs a further argument.
#[must_use]
pub fn environment_for<'a>(
    scene: &SceneState,
    hdr_map: Option<&'a EnvironmentMap>,
    tray: TrayRef<'a>,
) -> EnvironmentSource<'a> {
    debug_assert_eq!(
        tray.shadow.is_some(),
        scene.stone_shadow,
        "a shadowed scene is traced with its resolved map, an unshadowed one without"
    );
    debug_assert_eq!(
        tray.mirror.is_some(),
        scene.tray_reflection > 0.0,
        "a reflective scene is traced with its resolved mirror, a matte one without"
    );
    environment_source(scene, hdr_map)
        .with_tray_shadow(tray.shadow)
        .with_tray_mirror(tray.mirror)
}

/// The glossy tray of one scene: `pleochroma::render_setup::tray_mirror_for` over the scene's own
/// planes and tools, `Ok(None)` for a scene with `tray_reflection == 0` (the matte tray).
///
/// The plane sits where the Stone Shadow's does, and the reflectance is `scene.tray_reflection`
/// (the head-on `R0` of a Schlick floor). The mirror is plain data (`TrayMirror` is `Copy`), but
/// building the plane costs a solid build of the stone, so a request resolves it once
/// ([`resolve_tray`]) and carries it.
///
/// # Errors
///
/// [`TrayMirrorOff::NeedsStudioBackdrop`] for an HDR map or a scene without a lit studio
/// backdrop; [`TrayMirrorOff::DegenerateStone`] for a stone that is not closed.
pub fn mirror_for_scene(scene: &SceneState) -> Result<Option<TrayMirror>, TrayMirrorOff> {
    if scene.tray_reflection <= 0.0 {
        return Ok(None);
    }
    if scene.hdr().is_some() {
        return Err(TrayMirrorOff::NeedsStudioBackdrop);
    }
    pleochroma::render_setup::tray_mirror_for(
        scene.tray_reflection,
        environment_source(scene, None),
        crate::render_core::stone_geometry(scene),
    )
}

/// Builds and retains the shadow allocation for one request; callers keep it while tracing.
///
/// # Errors
/// Returns cancellation or the unsupported scene reason, never a shadow-free fallback.
pub fn shadow_for_scene(
    scene: &SceneState,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
) -> Result<Shadow, TrayShadowOff> {
    if !scene.stone_shadow {
        return Ok(None);
    }
    if scene.hdr().is_some() {
        return Err(TrayShadowOff::Hdr);
    }
    let material = scene.traced_material();
    pleochroma::render_setup::stone_shadow_for(
        true,
        environment_source(scene, None),
        pleochroma::geometry::StoneGeometry {
            planes: &scene.planes,
            tools: &scene.tools,
        },
        &material,
        scene.max_bounces,
        quality,
        cancel,
        &mut |_| {},
    )
}
