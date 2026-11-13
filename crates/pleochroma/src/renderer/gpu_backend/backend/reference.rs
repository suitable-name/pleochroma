//! The beam residual pass through [`GpuBackend`] (beam guide sections 12.3 and 14.7): the
//! reference estimator over the live pixels, with the kept test.
//!
//! The pass is [`GpuBackend::try_accumulate_geom_surface_live_cancellable`] with the reference
//! megakernel (exact index matching, no exit splitting, branch signature, the kept hook) in place
//! of the shipped one: it adds the samples of every live pixel into `accum` and the odd ones into
//! `half_b`, and a path that a beam of `kept` covers contributes nothing.
//!
//! The kept regions of a batch reach the pass in one of two ways.
//! [`GpuBackend::try_accumulate_reference_live_cancellable`] takes the host tables and uploads
//! them for every pass. An engine that runs many passes over one batch uploads once with
//! [`GpuBackend::upload_reference_kept`] and calls
//! [`GpuBackend::try_accumulate_reference_live_uploaded_cancellable`] with the set.
//!
//! The reference megakernel is built from the plain shader text, which has no optics-table
//! binding. With the optics tables on (`--optics-tables`) the CPU reference reads tables that
//! the GPU pass would not, so the pass declines and the CPU runs the residual (see
//! [`reference_declined_for_tables`]).

use std::sync::atomic::{AtomicBool, Ordering};

use glam::Vec3;

use super::GpuBackend;
use crate::{
    geometry::tool::{GPU_MAX_TOOL_PRIMITIVES, ToolPrimitive},
    optics::{
        raytracer::surface::SurfaceScene,
        tables::{OpticsTables, default_optics_tables},
    },
    renderer::{
        adaptive::LiveTiles,
        beam::beam::PackedKept,
        buffers::GpuSurfaceScene,
        gpu::{
            GpuFrameError, GpuFrameRenderer, GpuFrameScene,
            beam::{engine::SlotSums, kept::KeptBufferSet, reference_source::reference_live_fits},
            frame::{LivePass, LiveSums, TurnRequest, classify_material},
        },
        gpu_backend::{GpuAccumulate, GpuSceneRef, turnstile::CHUNKS_PER_TURN},
    },
};

/// `true` when the GPU reference pass must decline because the optics lookup tables are `tables`.
///
/// The CPU reference reads the table setting, but the reference megakernel is compiled from the
/// plain shader text and never binds the table buffer. Comparing it with a table-on CPU
/// reference would mix two different optics, so the caller runs the residual on the CPU, which
/// honors the tables. A table variant of the reference would also need a table binding.
#[must_use]
pub const fn reference_declined_for_tables(tables: OpticsTables) -> bool {
    matches!(tables, OpticsTables::On)
}

/// Whether the reference pass may run at all: `false` (logged) when the optics tables are on.
///
/// Every route into the reference pass asks this: the host entries and the GPU-resident run,
/// so a resident residual cannot run the analytic reference against table-built beam leaves.
fn reference_admitted() -> bool {
    if reference_declined_for_tables(default_optics_tables()) {
        tracing::debug!(
            "GPU declined the beam residual pass: the optics tables are on, the reference \
             megakernel has no table variant"
        );
        return false;
    }
    true
}

/// Where the kept buffers of a reference pass come from.
#[derive(Clone, Copy)]
enum KeptSource<'k> {
    /// Host tables, uploaded for this pass.
    Packed(&'k PackedKept),
    /// A set the caller uploaded once for the batch.
    Uploaded(&'k KeptBufferSet),
}

/// Where the sums of a reference pass are added.
enum SumTarget<'s> {
    /// The whole frame: `accum` and `half_b`, one entry per frame pixel.
    Frame(&'s mut [Vec3], &'s mut [Vec3]),
    /// The batch's compact slots.
    Compact(SlotSums<'s>),
}

impl SumTarget<'_> {
    /// Whether the buffers have the lengths the target promises for a frame of `num_pixels`.
    ///
    /// The frame form panics on a wrong length (the documented contract of the frame entries); the
    /// compact form reports it, so the caller declines.
    fn fits(&self, num_pixels: usize) -> bool {
        match self {
            Self::Frame(accum, half_b) => {
                assert_eq!(
                    accum.len(),
                    num_pixels,
                    "accumulation buffer must have one entry per pixel"
                );
                assert_eq!(
                    half_b.len(),
                    num_pixels,
                    "half-buffer B must have one entry per pixel"
                );
                true
            }
            Self::Compact(sums) => {
                sums.sum.len() == sums.slots.len() && sums.half_b.len() == sums.slots.len()
            }
        }
    }

    /// Adds the live pixel `pixel` (row-major in a frame `width` wide) its `sum` and odd-sample
    /// `half`; a pixel without a slot is dropped.
    fn add(&mut self, width: u32, pixel: usize, (sum, half): (Vec3, Vec3)) {
        match self {
            Self::Frame(accum, half_b) => {
                accum[pixel] += sum;
                half_b[pixel] += half;
            }
            Self::Compact(sums) => {
                if let Some(slot) = sums.slots.slot_of_index(width, pixel) {
                    sums.sum[slot] += sum;
                    sums.half_b[slot] += half;
                }
            }
        }
    }
}

impl GpuBackend {
    /// [`Self::try_accumulate_geom_surface_live_cancellable`] with the reference estimator and the
    /// kept test: one residual pass of a beam render.
    ///
    /// Adds the samples `sample_offset..sample_offset + spp` to `accum` at every live pixel, and the
    /// ones with an odd absolute index to `half_b`, exactly as the live entry does, but traced by
    /// the reference megakernel: a channel survives a refraction only if its indices are bitwise
    /// the hero's, nothing is split, and a channel whose branch signature, image point and
    /// wavelength fall in a region of `kept` (`n_lambda` spectral nodes) is zeroed. An empty `kept`
    /// zeroes nothing.
    ///
    /// Declines (`accum` and `half_b` untouched, [`GpuAccumulate::Declined`]) wherever the live
    /// entry does, and also when the reference variant does not match its anchors, when the
    /// adapter allows fewer storage buffers per stage than the megakernel, the live list and the
    /// kept buffers need, when the kept buffers do not fit one storage binding, or when the optics
    /// tables are on ([`reference_declined_for_tables`]); the caller then runs this batch's
    /// residual on the CPU. [`GpuAccumulate::Cancelled`] leaves both untouched.
    ///
    /// # Panics
    ///
    /// Panics if `accum` or `half_b` is not `scene.width * scene.height` long.
    #[expect(
        clippy::too_many_arguments,
        reason = "the live entry point's parameter list plus the kept regions and N_lambda"
    )]
    pub fn try_accumulate_reference_live_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        live: &LiveTiles,
        kept: &PackedKept,
        n_lambda: u32,
        accum: &mut [Vec3],
        half_b: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        self.accumulate_reference(
            (scene, tools, surface),
            (sample_offset, spp),
            live,
            (KeptSource::Packed(kept), n_lambda),
            SumTarget::Frame(accum, half_b),
            cancel,
        )
    }

    /// [`Self::try_accumulate_reference_live_cancellable`] adding into the batch's compact
    /// slots. `sums` holds the batch's
    /// [`PixelSlots`](crate::renderer::gpu::beam::engine::PixelSlots) and two slices of that
    /// many pixels, which replace the two whole-frame buffers.
    ///
    /// The pass is the same, so every live pixel with a slot receives the same sums bit for
    /// bit; a live pixel without a slot is dropped. Buffers of another length decline
    /// ([`GpuAccumulate::Declined`]) instead of panicking.
    #[expect(
        clippy::too_many_arguments,
        reason = "the live entry point's parameter list plus the kept regions and N_lambda"
    )]
    pub fn try_accumulate_reference_live_compact_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        live: &LiveTiles,
        kept: &PackedKept,
        n_lambda: u32,
        sums: SlotSums<'_>,
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        self.accumulate_reference(
            (scene, tools, surface),
            (sample_offset, spp),
            live,
            (KeptSource::Packed(kept), n_lambda),
            SumTarget::Compact(sums),
            cancel,
        )
    }

    /// [`Self::try_accumulate_reference_live_cancellable`] with the kept regions already uploaded:
    /// `kept` came from [`Self::upload_reference_kept`] (one upload per batch), so the pass uploads
    /// only its live pixel list.
    ///
    /// The result is the same pass bit for bit. Declines, besides the conditions of the packed
    /// entry, when `kept` was uploaded for another `n_lambda`. A set belongs to the device it was
    /// uploaded on: after [`Self::is_lost`] turned `true` upload it again (the engines retire
    /// instead).
    ///
    /// # Panics
    ///
    /// Panics if `accum` or `half_b` is not `scene.width * scene.height` long.
    #[expect(
        clippy::too_many_arguments,
        reason = "the live entry point's parameter list plus the uploaded kept set and N_lambda"
    )]
    pub fn try_accumulate_reference_live_uploaded_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        live: &LiveTiles,
        kept: &KeptBufferSet,
        n_lambda: u32,
        accum: &mut [Vec3],
        half_b: &mut [Vec3],
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        self.accumulate_reference(
            (scene, tools, surface),
            (sample_offset, spp),
            live,
            (KeptSource::Uploaded(kept), n_lambda),
            SumTarget::Frame(accum, half_b),
            cancel,
        )
    }

    /// [`Self::try_accumulate_reference_live_uploaded_cancellable`] adding into the batch's compact
    /// slots, as [`Self::try_accumulate_reference_live_compact_cancellable`] does.
    #[expect(
        clippy::too_many_arguments,
        reason = "the live entry point's parameter list plus the uploaded kept set and N_lambda"
    )]
    pub fn try_accumulate_reference_live_uploaded_compact_cancellable(
        &self,
        scene: &GpuSceneRef<'_>,
        tools: &[ToolPrimitive],
        surface: &SurfaceScene<'_>,
        sample_offset: u32,
        spp: u32,
        live: &LiveTiles,
        kept: &KeptBufferSet,
        n_lambda: u32,
        sums: SlotSums<'_>,
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        self.accumulate_reference(
            (scene, tools, surface),
            (sample_offset, spp),
            live,
            (KeptSource::Uploaded(kept), n_lambda),
            SumTarget::Compact(sums),
            cancel,
        )
    }

    /// Uploads the kept regions of a batch once, for
    /// [`Self::try_accumulate_reference_live_uploaded_cancellable`].
    ///
    /// `None` (a decline, logged) when there is no usable renderer, the adapter binds too few
    /// storage buffers for the reference pass, or the buffers do not fit one storage binding.
    #[must_use]
    pub fn upload_reference_kept(&self, kept: &PackedKept, n_lambda: u32) -> Option<KeptBufferSet> {
        let mutex = self.renderer.as_ref()?;
        if self.lost.load(Ordering::Acquire) && !self.try_recover() {
            return None;
        }
        let renderer = self.lock_renderer(mutex)?;
        let context = renderer.context();
        let available = context.device.limits().max_storage_buffers_per_shader_stage;
        if !reference_live_fits(available) {
            tracing::info!(
                available,
                "GPU declined to upload the beam kept regions: too few storage buffers per stage"
            );
            return None;
        }
        let set = KeptBufferSet::upload(context, kept, n_lambda);
        drop(renderer);
        if set.is_none() {
            tracing::info!("GPU declined to upload the beam kept regions: they do not fit");
        }
        set
    }

    /// The body of the reference entries: `(scene, tools, surface)`, `(sample_offset, spp)`, the
    /// live tiles, the kept source with `n_lambda`, where the two sums go, the cancel flag.
    fn accumulate_reference(
        &self,
        (scene, tools, surface): (&GpuSceneRef<'_>, &[ToolPrimitive], &SurfaceScene<'_>),
        (sample_offset, spp): (u32, u32),
        live: &LiveTiles,
        (kept, n_lambda): (KeptSource<'_>, u32),
        mut target: SumTarget<'_>,
        cancel: &AtomicBool,
    ) -> GpuAccumulate {
        let num_pixels = scene.width as usize * scene.height as usize;
        if !target.fits(num_pixels) {
            tracing::debug!("GPU declined: the compact sums do not match the pixel slots");
            return GpuAccumulate::Declined;
        }
        if live.width() != scene.width || live.height() != scene.height {
            tracing::debug!("GPU declined: the live tiles describe another frame size");
            return GpuAccumulate::Declined;
        }
        if tools.len() > GPU_MAX_TOOL_PRIMITIVES {
            tracing::debug!(tools = tools.len(), "GPU declined: too many concave tools");
            return GpuAccumulate::Declined;
        }
        if !reference_admitted() {
            return GpuAccumulate::Declined;
        }
        if !self.work_admits(scene, spp) {
            return GpuAccumulate::Declined;
        }
        if self.lost.load(Ordering::Acquire) && !self.try_recover() {
            return GpuAccumulate::Declined;
        }
        let Some(mutex) = &self.renderer else {
            return GpuAccumulate::Declined;
        };
        if spp == 0 || live.is_empty() {
            return GpuAccumulate::Done;
        }

        let gpu_scene = GpuFrameScene {
            camera: scene.camera,
            width: scene.width,
            height: scene.height,
            planes: scene.planes,
            facet_finishes: scene.facet_finishes,
            material: scene.material,
            max_bounces: scene.max_bounces,
            environment: scene.environment,
        };
        let gpu_surface = GpuSurfaceScene::from_scene(surface);
        let request = TurnRequest {
            scene: &gpu_scene,
            tools,
            pipeline_class: classify_material(gpu_scene.material),
            sample_offset,
            spp,
            cancel: Some(cancel),
            max_chunks: self
                .configured_limits()
                .map_or(CHUNKS_PER_TURN, |limits| limits.chunks_per_turn),
            surface: Some(&gpu_surface),
            // The reference tracer is the branching yardstick on every build.
            estimator: Some(crate::optics::raytracer::Estimator::Branching),
        };

        let Some(pass) = self.begin_pass(mutex, live, (kept, n_lambda)) else {
            return GpuAccumulate::Declined;
        };
        let live_count = pass.pixels().len();
        let mut scratch_sum = self.take_scratch(live_count);
        let mut scratch_half = self.take_scratch(live_count);
        let outcome = self.run_live_turns(
            mutex,
            &request,
            &pass,
            &mut LiveSums {
                sum: &mut scratch_sum,
                half_b: &mut scratch_half,
            },
        );
        if outcome == GpuAccumulate::Done {
            for (virtual_pixel, &pixel) in pass.pixels().iter().enumerate() {
                target.add(
                    scene.width,
                    pixel as usize,
                    (scratch_sum[virtual_pixel], scratch_half[virtual_pixel]),
                );
            }
        }
        self.return_scratch(scratch_sum);
        self.return_scratch(scratch_half);
        outcome
    }

    /// The pass of `kept`: uploads the tables, or reuses the uploaded set (only the live pixel list
    /// is uploaded then).
    fn begin_pass(
        &self,
        mutex: &std::sync::Mutex<GpuFrameRenderer>,
        live: &LiveTiles,
        (kept, n_lambda): (KeptSource<'_>, u32),
    ) -> Option<LivePass> {
        match kept {
            KeptSource::Packed(packed) => {
                self.begin_reference_live_pass(mutex, live, packed, n_lambda)
            }
            KeptSource::Uploaded(set) => {
                self.begin_reference_live_pass_uploaded(mutex, live, set, n_lambda)
            }
        }
    }

    /// The pass over `live` with the kept set `set` uploaded earlier for `n_lambda` nodes; `None` (a
    /// decline, logged) for another `n_lambda`, the optics tables on, or an unusable renderer.
    pub(super) fn begin_reference_live_pass_uploaded(
        &self,
        mutex: &std::sync::Mutex<GpuFrameRenderer>,
        live: &LiveTiles,
        set: &KeptBufferSet,
        n_lambda: u32,
    ) -> Option<LivePass> {
        if !reference_admitted() {
            return None;
        }
        if set.n_lambda != n_lambda {
            tracing::debug!(
                uploaded = set.n_lambda,
                asked = n_lambda,
                "GPU declined the beam residual pass: the kept set is for another N_lambda"
            );
            return None;
        }
        let renderer = self.lock_renderer(mutex)?;
        Self::declined_pass(renderer.begin_reference_live_pass_uploaded(live, set.clone()))
    }

    /// Uploads the live pixel list and the kept buffers under the renderer lock; `None` (a decline,
    /// logged) if the renderer is unusable, the adapter binds too few storage buffers, or the
    /// buffers do not fit the device.
    pub(super) fn begin_reference_live_pass(
        &self,
        mutex: &std::sync::Mutex<GpuFrameRenderer>,
        live: &LiveTiles,
        kept: &PackedKept,
        n_lambda: u32,
    ) -> Option<LivePass> {
        if !reference_admitted() {
            return None;
        }
        let renderer = self.lock_renderer(mutex)?;
        Self::declined_pass(renderer.begin_reference_live_pass(live, kept, n_lambda))
    }

    /// The pass of `result`, or `None` after logging why the batch's residual runs on the CPU.
    fn declined_pass(result: Result<LivePass, GpuFrameError>) -> Option<LivePass> {
        match result {
            Ok(pass) => Some(pass),
            Err(GpuFrameError::DeviceLimits { needed, available }) => {
                tracing::info!(
                    needed,
                    available,
                    "GPU declined the beam residual pass: too few storage buffers per stage, \
                     the batch's residual runs on the CPU"
                );
                None
            }
            Err(error) => {
                tracing::info!(
                    "GPU declined the beam residual pass, the batch's residual runs on the \
                     CPU: {error}"
                );
                None
            }
        }
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use super::*;

    /// The one admission every route (host and resident) asks, so the resident run declines
    /// exactly when the host entry does.
    #[test]
    fn the_common_reference_admission_follows_the_table_decision() {
        assert_eq!(
            reference_admitted(),
            !reference_declined_for_tables(default_optics_tables())
        );
    }

    #[test]
    fn the_reference_pass_declines_exactly_when_the_optics_tables_are_on() {
        assert!(reference_declined_for_tables(OpticsTables::On));
        assert!(!reference_declined_for_tables(OpticsTables::Off));
        // The process default is off unless a flag enables it.
        assert_eq!(
            reference_declined_for_tables(default_optics_tables()),
            default_optics_tables() == OpticsTables::On
        );
    }
}
