//! The GUI's cache over the primary-ray-only guide-buffer prepass.
//!
//! A remote-sourced image (radiance only, no guide buffers over the wire) can still be denoised
//! by the same edge-avoiding À-Trous filter the local path uses.
//!
//! # Where the prepass lives
//!
//! The prepass itself ([`generate_guide_buffers_into_geom_with_index`],
//! [`generate_guide_buffers_cancellable_geom_with_index`], [`GuideBuffers`]) lives in
//! `pleochroma::renderer::guide_pass`, so a coordinator can compute the identical guides for
//! its denoised `DISPLAY_FRAME`s. It is re-exported here so every GUI call site keeps this
//! module's path. Only the cache stays GUI-side: its key hashes the planes with
//! `render_thread::hash_planes`, which the GUI's other frame caches share.
//!
//! # Why caching, and on what key
//!
//! The guide buffers depend only on the camera (including roll, pan and field of view), the
//! active facet geometry, and the material's `n_d` (the path signature refracts at it)
//! -- never on which backend produced the radiance, and never on light direction or
//! exposure. [`GuideCache`] recomputes only when
//! [`GuideCache::key_for_camera`]'s key (resolution + camera + a hash of the geometry +
//! `n_d`) differs from the last call, not on every `FRAME` redraw of an in-progress
//! accumulation.
//!
//! # Why it runs off the UI thread
//!
//! One prepass costs ~82 ms at 1920x1080 and ~288 ms at 3840x2160. At 4K that is enough to
//! freeze the Slint event loop, so the remote render start runs it on a background thread via
//! [`generate_guide_buffers_cancellable_geom_with_index`], overlapping it with the network
//! round trip.
//! Cancellation is cooperative, through an `Arc<AtomicBool>` checked between rows. If the pose
//! changes before a background run finishes, `gui::remote` abandons it and starts a new one.
//! [`GuideCache::ensure_geom_cancellable`] runs synchronously on its caller's thread.
//! `gui::remote` hands a background result to [`GuideCache::adopt`] only after it checks, with
//! [`GuideCache::key_for_camera`]/[`GuideCache::matches_key`], that the result matches the pose
//! on screen. A frame that arrives before its background run finishes gets a plain tonemap; a
//! later redraw denoises it.

use pleochroma::{
    geometry::tool::StoneGeometry, optics::raytracer::Camera, render_setup::hash_geometry,
};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(test)]
use pleochroma::optics::raytracer::DEFAULT_FOV_DEG;

// The pure prepass moved to `pleochroma::renderer::guide_pass` (so a coordinator can
// compute the same guides for its denoised display frames); re-exported so every GUI
// call site keeps its `bridge::frame_cache::guide_pass::...` path.
pub use pleochroma::renderer::guide_pass::{
    GuideBuffers, generate_guide_buffers_cancellable_geom_with_index,
    generate_guide_buffers_into_geom_with_index,
};
// Only the tests build guides synchronously into a fresh allocation.
#[cfg(test)]
pub use pleochroma::renderer::guide_pass::generate_guide_buffers;

/// The guide-buffer identity: resolution, camera, geometry, and refractive index.
///
/// Lighting and exposure are excluded. `planes_hash` is
/// `pleochroma::render_setup::hash_geometry` of the planes and tools; for a planar stone it equals
/// the `hash_planes` key the other per-design caches use.
///
/// `pub`, not private: `gui::remote`'s background guide-generation path tags its result
/// with this same key (via [`GuideCache::key_for_camera`]). Fields stay private so a
/// `GuideKey` can only be built by the cache's key builders, never hand-assembled with a
/// mismatched or stale hash.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideKey {
    width: u32,
    height: u32,
    origin: glam::Vec3,
    forward: glam::Vec3,
    right: glam::Vec3,
    up: glam::Vec3,
    fov_tan: f32,
    planes_hash: u64,
    /// The active material's `n_d`: the path signature refracts at it (`0.0` = none).
    n_d: f32,
}

/// Caches guide buffers until their [`GuideKey`] changes.
///
/// The camera, geometry and refractive index determine when the cache is stale;
/// additional radiance samples do not invalidate it.
///
/// `buffers` is a plain (never-`Option`) field, deliberately: `key` starts `None`, so
/// the first successful [`Self::ensure_geom_cancellable`] call populates `buffers`
/// before returning them. A cancelled prepass returns `None` instead of exposing
/// incomplete guides.
#[derive(Debug)]
pub struct GuideCache {
    key: Option<GuideKey>,
    buffers: GuideBuffers,
    /// Counts completed prepasses run by this cache.
    ///
    /// Tests use this to observe cache hits/misses without inspecting buffer contents;
    /// production code does not read it.
    generation: u64,
}

impl Default for GuideCache {
    fn default() -> Self {
        Self::new()
    }
}

impl GuideCache {
    /// Creates an empty instance.
    #[must_use]
    pub fn new() -> Self {
        Self {
            key: None,
            buffers: GuideBuffers::miss(0, 0),
            generation: 0,
        }
    }

    /// How many times the guide buffers were regenerated. Test-only hook.
    #[cfg(test)]
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the guide buffers valid for `(width, height, yaw, pitch, distance,
    /// planes)`, regenerating the primary-ray prepass only if that key differs from the
    /// last call -- an unchanged pose/gem on a subsequent call (e.g. a later `FRAME`
    /// event from the same in-progress remote render) is a cache hit and costs nothing
    /// beyond the key comparison.
    ///
    /// Test-only: live code uses [`Self::key_for_camera`] and
    /// [`Self::ensure_geom_cancellable`]. Uses `n_d = 0.0`, i.e. no path signature.
    #[cfg(test)]
    pub fn ensure(
        &mut self,
        width: u32,
        height: u32,
        yaw: f32,
        pitch: f32,
        distance: f32,
        planes: &[pleochroma::geometry::plane::GpuFacetPlane],
    ) -> &GuideBuffers {
        self.ensure_geom(
            width,
            height,
            yaw,
            pitch,
            distance,
            StoneGeometry::planes_only(planes),
            0.0,
        )
    }

    /// [`Self::ensure`] for a stone with concave tools: the primary rays hit the
    /// notches too, so the denoiser's depth/normal/facet-id edges follow them.
    ///
    /// `n_d` is the active material's `DispersionModel::n_d()`; the path signature
    /// refracts through the stone at that index, so it is part of the key and a
    /// material change regenerates the buffers. `n_d <= 1.0` means no signature.
    /// This synchronous, zero-roll wrapper is only used by tests.
    #[cfg(test)]
    // Pose, geometry and index are one flat key; a bundle struct would only rename them.
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_geom(
        &mut self,
        width: u32,
        height: u32,
        yaw: f32,
        pitch: f32,
        distance: f32,
        geom: StoneGeometry<'_>,
        n_d: f32,
    ) -> &GuideBuffers {
        let key = Self::key_for_geom(width, height, yaw, pitch, distance, geom, n_d);
        self.ensure_geom_cancellable(key, geom, &AtomicBool::new(false))
            .expect("an uncancelled prepass always completes")
    }

    /// Regenerates stale guides, publishing their key only after every row completes.
    ///
    /// Cancellation leaves the reusable storage unkeyed: partially written guides must
    /// never be served as either the previous camera's image or the cancelled one.
    pub fn ensure_geom_cancellable(
        &mut self,
        key: GuideKey,
        geom: StoneGeometry<'_>,
        cancel: &AtomicBool,
    ) -> Option<&GuideBuffers> {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if self.key.as_ref() != Some(&key) {
            self.key = None;
            let camera = Camera {
                origin: key.origin,
                forward: key.forward,
                right: key.right,
                up: key.up,
                fov_tan: key.fov_tan,
            };
            let completed = generate_guide_buffers_into_geom_with_index(
                key.width,
                key.height,
                &camera,
                geom,
                key.n_d,
                cancel,
                &mut self.buffers,
            );
            if !completed || cancel.load(Ordering::Relaxed) {
                return None;
            }
            self.key = Some(key);
            self.generation += 1;
        }
        Some(&self.buffers)
    }

    /// Computes the [`GuideKey`] for `(width, height, yaw, pitch, distance, planes)`, the
    /// identity [`Self::ensure`] uses. Test-only: live code uses [`Self::key_for_camera`].
    ///
    /// Uses `n_d = 0.0`.
    #[cfg(test)]
    #[must_use]
    pub fn key_for(
        width: u32,
        height: u32,
        yaw: f32,
        pitch: f32,
        distance: f32,
        planes: &[pleochroma::geometry::plane::GpuFacetPlane],
    ) -> GuideKey {
        Self::key_for_geom(
            width,
            height,
            yaw,
            pitch,
            distance,
            StoneGeometry::planes_only(planes),
            0.0,
        )
    }

    /// [`Self::key_for`] for a stone with concave tools, and the material's `n_d` (see
    /// [`Self::ensure_geom`]).
    #[cfg(test)]
    #[must_use]
    pub fn key_for_geom(
        width: u32,
        height: u32,
        yaw: f32,
        pitch: f32,
        distance: f32,
        geom: StoneGeometry<'_>,
        n_d: f32,
    ) -> GuideKey {
        let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG);
        Self::key_for_camera(width, height, &camera, geom, n_d)
    }

    /// Keys the actual eye and image-plane basis, including roll and pan, for tracing and
    /// picking.
    #[must_use]
    pub fn key_for_camera(
        width: u32,
        height: u32,
        camera: &Camera,
        geom: StoneGeometry<'_>,
        n_d: f32,
    ) -> GuideKey {
        GuideKey {
            width,
            height,
            origin: camera.origin,
            forward: camera.forward,
            right: camera.right,
            up: camera.up,
            fov_tan: camera.fov_tan,
            planes_hash: hash_geometry(geom),
            n_d,
        }
    }

    /// Checks whether `key` matches the cache's completed guide buffers.
    ///
    /// A [`Self::ensure_geom_cancellable`] call with this key would be a cache hit.
    /// This checks readiness without running a synchronous prepass.
    #[must_use]
    pub fn matches_key(&self, key: &GuideKey) -> bool {
        self.key.as_ref() == Some(key)
    }

    /// Adopts completed guide buffers from a background prepass.
    ///
    /// Stores a [`generate_guide_buffers_cancellable_geom_with_index`] result for
    /// `key` without running the prepass or incrementing the generation counter,
    /// which tracks only this cache's own prepass runs. The caller is responsible
    /// for `buffers` actually matching `key` -- `gui::remote`'s async path only calls
    /// this after confirming the background result's key matches the pose currently on
    /// screen.
    pub fn adopt(&mut self, key: GuideKey, buffers: GuideBuffers) {
        self.key = Some(key);
        self.buffers = buffers;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::cuts::StandardGemCuts;

    #[test]
    fn rolled_image_planes_have_distinct_guide_keys_without_changing_the_orbit() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let geometry = StoneGeometry::planes_only(&planes);
        let original = Camera::new(0.6, 0.45, 2.4, DEFAULT_FOV_DEG);
        let rolled =
            Camera::new(0.6, 0.45, 2.4, DEFAULT_FOV_DEG).with_roll(std::f32::consts::FRAC_PI_2);
        assert_ne!(
            GuideCache::key_for_camera(32, 24, &original, geometry, 1.5),
            GuideCache::key_for_camera(32, 24, &rolled, geometry, 1.5),
        );
        assert_eq!(original.origin, rolled.origin);
    }

    #[test]
    fn panned_cameras_have_distinct_guide_keys_with_the_same_view_direction() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let geometry = StoneGeometry::planes_only(&planes);
        let original = Camera::new(0.6, 0.45, 2.4, DEFAULT_FOV_DEG);
        let panned = Camera::new(0.6, 0.45, 2.4, DEFAULT_FOV_DEG).with_pan([0.3, -0.2]);
        assert_ne!(
            GuideCache::key_for_camera(32, 24, &original, geometry, 1.5),
            GuideCache::key_for_camera(32, 24, &panned, geometry, 1.5),
        );
        assert_eq!(original.forward, panned.forward);
    }

    #[test]
    fn cancelled_guides_do_not_publish_the_requested_camera() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let geom = StoneGeometry::planes_only(&planes);
        let mut cache = GuideCache::new();
        cache.ensure_geom(8, 8, 0.6, 0.45, 2.4, geom, 1.5);
        let new_key = GuideCache::key_for_geom(8, 8, 0.9, 0.45, 2.4, geom, 1.5);
        assert!(
            cache
                .ensure_geom_cancellable(new_key.clone(), geom, &AtomicBool::new(true))
                .is_none()
        );
        assert!(!cache.matches_key(&new_key));
        assert_eq!(cache.generation(), 1);
        assert!(
            cache
                .ensure_geom_cancellable(new_key.clone(), geom, &AtomicBool::new(false))
                .is_some()
        );
        assert!(cache.matches_key(&new_key));
        assert_eq!(cache.generation(), 2);
    }

    #[test]
    fn guide_cache_reuses_buffers_when_the_key_is_unchanged() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();

        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cache.generation(), 1);

        // Same size, pose and geometry: a cache hit, the generation is unchanged.
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            1,
            "an unchanged pose/geometry must reuse the cached guide buffers"
        );
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cache.generation(), 1);
    }

    #[test]
    fn guide_cache_regenerates_on_yaw_change() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(8, 8, 0.90, 0.45, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            2,
            "a changed yaw must invalidate the cache"
        );
    }

    #[test]
    fn guide_cache_regenerates_on_pitch_or_distance_change() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(8, 8, 0.60, 0.80, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            2,
            "a changed pitch must invalidate the cache"
        );

        cache.ensure(8, 8, 0.60, 0.80, 3.0, &planes);
        assert_eq!(
            cache.generation(),
            3,
            "a changed distance must invalidate the cache"
        );
    }

    #[test]
    fn guide_cache_regenerates_when_the_gem_geometry_changes() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &srb);
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &emerald);
        assert_eq!(
            cache.generation(),
            2,
            "a changed set of cutting instructions must invalidate the cache even with an \
             unchanged camera pose"
        );
    }

    /// The path signature refracts at the material's `n_d`, so a material change with an
    /// unchanged pose must regenerate the guides, and the same index must not.
    #[test]
    fn guide_cache_regenerates_when_the_material_index_changes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let geom = StoneGeometry::planes_only(&planes);
        let mut cache = GuideCache::new();
        cache.ensure_geom(8, 8, 0.60, 0.45, 2.4, geom, 2.417);
        cache.ensure_geom(8, 8, 0.60, 0.45, 2.4, geom, 2.417);
        assert_eq!(cache.generation(), 1);
        cache.ensure_geom(8, 8, 0.60, 0.45, 2.4, geom, 1.52);
        assert_eq!(
            cache.generation(),
            2,
            "a changed n_d must invalidate the cache"
        );
    }

    #[test]
    fn guide_cache_regenerates_on_resolution_change() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(16, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(
            cache.generation(),
            2,
            "a changed output resolution must invalidate the cache"
        );
    }

    /// Light direction is not part of the key: the primary-ray prepass never samples lighting,
    /// so a light-only change must not regenerate the guides.
    #[test]
    fn ensure_signature_has_no_light_parameters() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cache.generation(), 1);
    }

    #[test]
    fn guide_cache_key_for_matches_what_ensure_uses_internally() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GuideCache::new();
        let key = GuideCache::key_for(8, 8, 0.60, 0.45, 2.4, &planes);

        assert!(
            !cache.matches_key(&key),
            "a freshly-constructed cache must not match any key yet"
        );
        cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert!(
            cache.matches_key(&key),
            "the key ensure() just populated must equal key_for()'s independently \
             computed key for the identical pose/geometry"
        );
    }

    #[test]
    fn guide_cache_adopt_installs_externally_computed_buffers_without_recomputing() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let camera = Camera::new(0.60, 0.45, 2.4, DEFAULT_FOV_DEG);
        let buffers = generate_guide_buffers(8, 8, &camera, &planes);
        let key = GuideCache::key_for(8, 8, 0.60, 0.45, 2.4, &planes);

        let mut cache = GuideCache::new();
        cache.adopt(key.clone(), buffers.clone());

        assert_eq!(
            cache.generation(),
            0,
            "adopt() folds in an externally-computed result -- it must not be counted \
             as this cache having run its own prepass"
        );
        assert!(cache.matches_key(&key));

        // A subsequent ensure() for the identical key must be a pure cache hit: same
        // generation, same (adopted) buffers, no recompute triggered.
        let cached = cache.ensure(8, 8, 0.60, 0.45, 2.4, &planes);
        assert_eq!(cached.depth, buffers.depth);
        assert_eq!(cache.generation(), 0);
    }
}
