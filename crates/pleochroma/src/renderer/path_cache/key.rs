//! The cache key: a 128-bit identity of everything the cached geometry depends on.
//!
//! Two renders may share a cache only if they agree on the solved stone, its material (including
//! the crystal axis and zoning), the camera and the stone's transform, the frame size, and every
//! setting that changes what phase A writes. Lighting, exposure, tone mapping, the sample count,
//! adaptive-sampling settings and the background are not inputs of phase A and are not part of the
//! key.
//!
//! # Canonical encoding
//!
//! [`canonical_bytes`] lists the inputs in a fixed order as little-endian words, every variable
//! length part prefixed with its length, every float as its IEEE bits. The key is a stable 128-bit
//! hash of those bytes ([`StableHasher`]): the hash is written out here, not taken from the
//! standard library, because a key is a folder name on disk and `DefaultHasher` may change between
//! Rust releases. The layout:
//!
//! | Part | Content |
//! |---|---|
//! | magic | `PCKEY1` (6 bytes) |
//! | format | [`CACHE_FORMAT_VERSION`] (`u32`) |
//! | planes | count (`u32`), then per plane `normal.x`, `normal.y`, `normal.z`, `d` (`f32` bits) |
//! | finishes | count of non-default finishes (`u32`), then per entry plane index (`u32`) and the finish name (length-prefixed) |
//! | material | the crystal axis `c_axis` (3 `f32` bits), the `alpha_axis` (flag byte, then 3 `f32` bits), then the full [`GemMaterial`] (length-prefixed `Debug` text, which holds every parameter, the zoning and the inclusions) |
//! | camera | `origin`, `forward`, `right`, `up` (3 `f32` bits each), `fov_tan` |
//! | transform | the stone transform, 16 `f32` bits, column major |
//! | frame | width, height, `max_bounces` (`u32`) |
//! | build | tile, `k0`, `k_max` (`u32`), cutoff, reference wavelength, dispersion gate, target noise (`f32` bits), metric tag (`u8`) |
//! | tables | only with the optics tables on: `OT` (2 bytes), [`OPTICS_TABLES_REVISION`] (`u32`); nothing when off, so an off key is unchanged |

use std::fmt;

use glam::Mat4;

use super::{positions::BuildSettings, record::REFERENCE_NM};
use crate::{
    geometry::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, FacetFinish},
        tables::{OpticsTables, default_optics_tables},
    },
    renderer::{adaptive::NoiseMetric, frame_scene::FrameScene},
};

/// Version of the cache contents; bump it whenever phase A would write different bytes.
pub const CACHE_FORMAT_VERSION: u32 = 3;

/// First bytes of the canonical encoding.
const MAGIC: &[u8; 6] = b"PCKEY1";

/// FNV-1a 64-bit offset basis, lane A.
const FNV_OFFSET_A: u64 = 0xcbf2_9ce4_8422_2325;
/// A second, unrelated offset basis for lane B.
const FNV_OFFSET_B: u64 = 0x9e37_79b9_7f4a_7c15;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
/// An odd multiplier (the 64-bit golden ratio) for lane B.
const LANE_B_PRIME: u64 = 0xff51_afd7_ed55_8ccd;

/// A 128-bit stable hash over a byte stream, with no dependency on the platform or the Rust release.
///
/// Lane A is FNV-1a 64. Lane B xors each byte into a different state and multiplies by another odd
/// constant, with a rotation so its high bits diffuse. A final avalanche mixes both lanes and the
/// length. This is an identity for a cache folder, not a cryptographic hash.
#[derive(Clone, Debug)]
pub struct StableHasher {
    a: u64,
    b: u64,
    len: u64,
}

impl Default for StableHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// The `SplitMix64` finalizer.
const fn avalanche(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

impl StableHasher {
    /// A hasher that has seen no bytes.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            a: FNV_OFFSET_A,
            b: FNV_OFFSET_B,
            len: 0,
        }
    }

    /// Feeds bytes.
    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.a = (self.a ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
            self.b = (self.b ^ u64::from(byte))
                .wrapping_mul(LANE_B_PRIME)
                .rotate_left(29);
        }
        self.len = self.len.wrapping_add(bytes.len() as u64);
    }

    /// The 64 bits of lane A alone, which is plain FNV-1a 64.
    #[must_use]
    pub const fn lane_a(&self) -> u64 {
        self.a
    }

    /// The 128-bit digest, big-endian lane order, of the bytes written so far.
    #[must_use]
    pub const fn finish(&self) -> [u8; 16] {
        let hi = avalanche(self.a ^ self.len.rotate_left(17) ^ avalanche(self.b));
        let lo = avalanche(self.b ^ hi.rotate_left(32) ^ self.len);
        let (hi, lo) = (hi.to_be_bytes(), lo.to_be_bytes());
        let mut out = [0_u8; 16];
        let mut i = 0;
        while i < 8 {
            out[i] = hi[i];
            out[i + 8] = lo[i];
            i += 1;
        }
        out
    }
}

/// Everything phase A depends on, borrowed.
#[derive(Clone, Copy)]
pub struct CacheKeyInputs<'s> {
    /// The solved stone, in the order the renderer uses.
    pub planes: &'s [GpuFacetPlane],
    /// Per-plane finish; a shorter slice is padded with the default finish, as the renderer does.
    pub facet_finishes: &'s [FacetFinish],
    /// The material, with its crystal axis, zoning and inclusions.
    pub material: &'s GemMaterial,
    /// The camera pose and projection.
    pub camera: &'s Camera,
    /// The stone's transform; [`Mat4::IDENTITY`] when the planes are already in world space.
    pub stone_transform: Mat4,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Internal bounce limit of the frame.
    pub max_bounces: u32,
    /// The build settings (tile size, `K0`, `K_MAX`, cutoff, dispersion gate, noise target).
    pub settings: &'s BuildSettings,
    /// The optics lookup-table mode the cached weights are traced under; Fresnel and
    /// attenuation weights differ between the modes.
    pub optics_tables: OpticsTables,
}

impl<'s> CacheKeyInputs<'s> {
    /// The inputs of a frame scene; lighting is deliberately not read.
    ///
    /// The optics-table mode is the process default, which is what `PreparedMaterial::new` reads
    /// for the render; [`Self::with_optics_tables`] overrides it.
    #[must_use]
    pub fn from_scene(
        scene: &FrameScene<'s>,
        stone_transform: Mat4,
        settings: &'s BuildSettings,
    ) -> Self {
        Self {
            planes: scene.planes,
            facet_finishes: scene.facet_finishes,
            material: scene.material,
            camera: scene.camera,
            stone_transform,
            width: scene.width,
            height: scene.height,
            max_bounces: scene.max_bounces,
            settings,
            optics_tables: default_optics_tables(),
        }
    }

    /// These inputs with an explicit optics-table mode.
    #[must_use]
    pub const fn with_optics_tables(mut self, mode: OpticsTables) -> Self {
        self.optics_tables = mode;
        self
    }
}

/// Revision of the optics-table algorithm (the table layout, node counts and interpolation); bump
/// it whenever a table-on render would produce different weights, so older table-on caches miss.
pub const OPTICS_TABLES_REVISION: u32 = 1;

/// Marks the optics-table part of an encoding; written only for [`OpticsTables::On`], so
/// off-mode keys keep their original bytes.
const TABLES_TAG: &[u8; 2] = b"OT";

/// Appends the optics-table part: nothing for `Off`, the tag and revision for `On`.
fn encode_tables(out: &mut Encoder, mode: OpticsTables) {
    if mode == OpticsTables::On {
        out.bytes(TABLES_TAG);
        out.u32(OPTICS_TABLES_REVISION);
    }
}

/// The canonical byte writer.
struct Encoder(Vec<u8>);

impl Encoder {
    fn bytes(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    #[cfg(feature = "beam")]
    fn f64(&mut self, value: f64) {
        self.0.extend_from_slice(&value.to_bits().to_le_bytes());
    }

    fn vec3(&mut self, value: glam::Vec3) {
        for component in value.to_array() {
            self.f32(component);
        }
    }

    /// A length-prefixed byte string.
    fn text(&mut self, text: &str) {
        self.u32(u32::try_from(text.len()).unwrap_or(u32::MAX));
        self.bytes(text.as_bytes());
    }
}

/// Tag of a noise metric in the encoding; a fixed number per variant, never reused.
const fn metric_tag(metric: NoiseMetric) -> u8 {
    match metric {
        NoiseMetric::RelativeLstar => 0,
        NoiseMetric::AbsoluteDeltaE => 1,
    }
}

/// The canonical byte encoding of the inputs, documented in the module docs.
#[must_use]
pub fn canonical_bytes(inputs: &CacheKeyInputs<'_>) -> Vec<u8> {
    let settings = inputs.settings.sanitized();
    let mut out = Encoder(Vec::with_capacity(256 + inputs.planes.len() * 16));
    out.bytes(MAGIC);
    out.u32(CACHE_FORMAT_VERSION);
    encode_scene(&mut out, inputs);

    out.u32(settings.tile);
    out.u32(settings.k0);
    out.u32(settings.k_max);
    out.f32(settings.cutoff);
    out.f32(REFERENCE_NM);
    out.f32(settings.dispersion_gate);
    out.f32(settings.target_noise);
    out.u8(metric_tag(settings.metric));
    encode_tables(&mut out, inputs.optics_tables);
    out.0
}

/// Writes the parts of the inputs that describe the scene: planes, finishes, material, camera,
/// stone transform and frame (everything before the build settings).
fn encode_scene(out: &mut Encoder, inputs: &CacheKeyInputs<'_>) {
    out.u32(u32::try_from(inputs.planes.len()).unwrap_or(u32::MAX));
    for plane in inputs.planes {
        for component in plane.normal {
            out.f32(component);
        }
        out.f32(plane.d);
    }

    encode_finishes(out, inputs);

    let material = inputs.material;
    out.vec3(material.c_axis);
    match material.alpha_axis {
        Some(axis) => {
            out.u8(1);
            out.vec3(axis);
        }
        None => out.u8(0),
    }
    out.text(&format!("{material:?}"));

    let camera = inputs.camera;
    out.vec3(camera.origin);
    out.vec3(camera.forward);
    out.vec3(camera.right);
    out.vec3(camera.up);
    out.f32(camera.fov_tan);

    for value in inputs.stone_transform.to_cols_array() {
        out.f32(value);
    }

    out.u32(inputs.width);
    out.u32(inputs.height);
    out.u32(inputs.max_bounces);
}

/// First bytes of the canonical encoding of a beam leaf-cache key.
#[cfg(feature = "beam")]
const BEAM_MAGIC: &[u8; 7] = b"PCBKEY1";

/// Tag of a region mode in the beam key encoding; a fixed number per variant, never reused.
#[cfg(feature = "beam")]
const fn region_mode_tag(mode: crate::renderer::beam::RegionMode) -> u8 {
    match mode {
        crate::renderer::beam::RegionMode::EndIntersection => 0,
        crate::renderer::beam::RegionMode::PerNode => 1,
    }
}

/// The canonical byte encoding of a beam leaf-cache key (beam guide section 14.10).
///
/// The scene part is the one of [`canonical_bytes`] (so no lighting), followed by the beam
/// settings that change leaves and the beam format version. The path-cache build settings
/// (`K0`, `K_MAX`, noise target) are not part of it.
///
/// | Part | Content |
/// |---|---|
/// | magic | `PCBKEY1` (7 bytes) |
/// | format | `BEAM_CACHE_FORMAT_VERSION`, then [`CACHE_FORMAT_VERSION`] (`u32` each) |
/// | scene | planes, finishes, material, camera, transform, frame (as in [`canonical_bytes`]) |
/// | beam | `tile` (`u32`), `dir_tolerance_rad`, `chord_tolerance_px` (`f64` bits), `weight_tolerance`, `cutoff`, `lighting_tolerance` (`f32` bits), `spectral_nodes`, `absorption_subnodes`, `max_split_depth` (`u32`), region mode tag (`u8`) |
/// | tables | only with the optics tables on: `OT`, [`OPTICS_TABLES_REVISION`] |
#[cfg(feature = "beam")]
#[must_use]
pub fn beam_canonical_bytes(
    inputs: &CacheKeyInputs<'_>,
    beam: &crate::renderer::beam::BeamSettings,
    tile: u32,
) -> Vec<u8> {
    let mut out = Encoder(Vec::with_capacity(256 + inputs.planes.len() * 16));
    out.bytes(BEAM_MAGIC);
    out.u32(crate::renderer::beam::leaf_cache::BEAM_CACHE_FORMAT_VERSION);
    out.u32(CACHE_FORMAT_VERSION);
    encode_scene(&mut out, inputs);

    out.u32(tile);
    out.f64(beam.dir_tolerance_rad);
    out.f64(beam.chord_tolerance_px);
    out.f32(beam.weight_tolerance);
    out.f32(beam.cutoff);
    // Kept leaves are handed back without refinement, so a finer lighting tolerance must
    // trace afresh.
    out.f32(beam.lighting_tolerance);
    out.u32(beam.spectral_nodes);
    out.u32(beam.absorption_subnodes);
    out.u32(beam.max_split_depth);
    out.u8(region_mode_tag(beam.region_mode));
    encode_tables(&mut out, inputs.optics_tables);
    out.0
}

/// The key of a beam leaf cache (beam guide section 14.10): the lighting-independent scene of
/// `inputs`, the beam settings that change leaves and `BEAM_CACHE_FORMAT_VERSION`.
///
/// `tile` is the adaptive tile size in pixels. A pose, design, material or resolution change
/// changes the key; a lighting, exposure or background change does not. The path-cache build
/// settings of `inputs` are ignored. Its [`Display`](fmt::Display) form is the folder name, shared
/// with path-cache folders under one root.
#[cfg(feature = "beam")]
#[must_use]
pub fn beam_cache_key(
    inputs: &CacheKeyInputs<'_>,
    beam: &crate::renderer::beam::BeamSettings,
    tile: u32,
) -> CacheKey {
    let mut hasher = StableHasher::new();
    hasher.write(&beam_canonical_bytes(inputs, beam, tile));
    CacheKey(hasher.finish())
}

/// Writes the non-default finishes by plane index, so a short slice and a padded one agree.
fn encode_finishes(out: &mut Encoder, inputs: &CacheKeyInputs<'_>) {
    let default = FacetFinish::default();
    let marked: Vec<(usize, FacetFinish)> = inputs
        .facet_finishes
        .iter()
        .copied()
        .take(inputs.planes.len())
        .enumerate()
        .filter(|&(_, finish)| finish != default)
        .collect();
    out.u32(u32::try_from(marked.len()).unwrap_or(u32::MAX));
    for (index, finish) in marked {
        out.u32(u32::try_from(index).unwrap_or(u32::MAX));
        out.text(&format!("{finish:?}"));
    }
}

/// The identity of a path cache; its [`Display`](fmt::Display) form is the cache's folder name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheKey([u8; 16]);

impl CacheKey {
    /// The key of these inputs.
    #[must_use]
    pub fn new(inputs: &CacheKeyInputs<'_>) -> Self {
        let mut hasher = StableHasher::new();
        hasher.write(&canonical_bytes(inputs));
        Self(hasher.finish())
    }

    /// The key of a frame scene built with `settings`; `stone_transform` is the identity when the
    /// planes are already in world space.
    #[must_use]
    pub fn from_scene(
        scene: &FrameScene<'_>,
        stone_transform: Mat4,
        settings: &BuildSettings,
    ) -> Self {
        Self::new(&CacheKeyInputs::from_scene(
            scene,
            stone_transform,
            settings,
        ))
    }

    /// A key from its 16 bytes, for example read back from a folder name.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The 16 bytes of the key.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Parses the 32 lowercase hex digits of [`Display`](fmt::Display); `None` for anything else.
    #[must_use]
    pub fn from_hex(text: &str) -> Option<Self> {
        if text.len() != 32 || !text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return None;
        }
        let mut bytes = [0_u8; 16];
        for (slot, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
            let digits = std::str::from_utf8(pair).ok()?;
            *slot = u8::from_str_radix(digits, 16).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}
