//! The miss/escape block of the bounce loop, including the bounce off a Tray Reflection's glossy
//! floor. A child module of `bounce_body`, so it sees [`PathState`]'s private fields.

use super::{Flow, PathState, depolarize_and_scale, record_termination};
use crate::optics::raytracer::{
    camera::Ray,
    environment::environment_nee_pdf,
    scattering::balance_heuristic,
    transport::{PathTermination, bounce::accumulate_miss_radiance},
};
use glam::Vec3;

impl PathState<'_> {
    /// The ray exited or missed the gemstone: the environment source is sampled and the path
    /// ends as `Escaped` (unless it meets a glossy tray, below). A camera ray that misses the
    /// stone (bounce 0) takes exactly the same lookup as an exit at any later bounce -- the lit
    /// backdrop tray below the horizon is part of the environment function, so a window and the
    /// surround show the same world.
    /// At bounce 0 the Stokes intensity is 1 and the MIS weight 1, so the value is the
    /// environment in that direction bit for bit.
    ///
    /// A pending NEE-eligible carry (a scatter whose continuation escapes directly, or after a
    /// polished transmit out, see `dispatch_polished_bounce`) makes this escape the competing
    /// phase-sampled continuation of the same light-sampling technique. It is weighted by the
    /// balance heuristic so the two contributions sum to the true value. The light density is
    /// evaluated at the carried INTERIOR direction, the measure `nee_contribution_hg_scatter`
    /// sampled in, not at `current_ray.dir`, which after a transmit out is exterior.
    /// `map_or(1.0, ..)` gives full weight when no carry is live, including every trace with NEE
    /// disabled.
    ///
    /// **Tray Reflection.** With a glossy tray (`environment::TrayMirror`)
    /// and a downward escape that meets its plane, the escape is not the end of the path.
    /// The radiance seen at the plane point `P` is
    /// `(1 - F) * tray(P) + F * L(P, reflected)` with the Schlick reflectance `F`
    /// (wavelength independent, unpolarized), and this block evaluates exactly that: the
    /// matte term at once, the mirrored term by following the reflected ray with
    /// throughput `F`:
    ///
    /// 1. the matte part `(1 - F) * tray(P)` is deposited through the ordinary escape lookup
    ///    (so the Stone Shadow is included) with the usual MIS weight times `1 - F`;
    /// 2. everything gathered so far is flushed into `nee_xyz` with this moment's
    ///    `path_pdf`/`compat` ([`PathState::flush_radiance_to_nee`]), so what came before the
    ///    floor is never re-weighted by what follows;
    /// 3. the Stokes vectors are depolarized and scaled by `F` (the floor is modelled
    ///    unpolarized), the plane-of-incidence frame is dropped, and the ray restarts along
    ///    the mirrored direction from the hit point.
    ///
    /// The reflected ray may hit the stone or escape upward. A later escape can meet the floor
    /// again. The MIS carry was consumed by this miss, so the next escape takes weight one. No
    /// termination is recorded and the path goes on (`Flow::Next`). Without a floor hit this is
    /// a plain escape.
    ///
    /// Two documented approximations (physics.md section 9). Surface glare applies at bounce 0
    /// only, so a camera ray that bounces off the floor first reaches the stone at bounce 1
    /// and the stone seen in the tray keeps its full first-surface glare (the GPU twins do the
    /// same). And only bounce 0 captures a primary hit for the denoiser, so the mirrored
    /// stone has none (see `capture_primary_hit`).
    #[inline]
    pub(super) fn handle_miss(
        &mut self,
        bounce: u32,
        phase_pdf_for_mis_this_check: Option<(f32, Vec3)>,
        termination_out: &mut Option<&mut (u32, PathTermination)>,
    ) -> Flow {
        let environment = self.scene.environment;
        let mis_weight = phase_pdf_for_mis_this_check.map_or(1.0, |(phase_pdf, interior_dir)| {
            let light_pdf = environment_nee_pdf(environment, interior_dir);
            balance_heuristic(phase_pdf, light_pdf)
        });
        let floor = environment
            .tray_mirror()
            .and_then(|mirror| mirror.hit(self.current_ray.origin, self.current_ray.dir));
        let Some(floor) = floor else {
            accumulate_miss_radiance(
                &self.exit_split_ctx,
                self.current_ray.origin,
                self.current_ray.dir,
                &self.lambdas,
                &self.stokes,
                mis_weight,
                &mut self.radiance,
            );
            self.path_escaped = true;
            record_termination(termination_out, bounce, PathTermination::Escaped);
            return Flow::Stop;
        };
        accumulate_miss_radiance(
            &self.exit_split_ctx,
            self.current_ray.origin,
            self.current_ray.dir,
            &self.lambdas,
            &self.stokes,
            mis_weight * (1.0 - floor.fresnel),
            &mut self.radiance,
        );
        // The reference tracer and the attribution recorder belong to the path cache, which
        // declines a Tray Reflection: a floor bounce breaks the recorder's invariant that a
        // miss after bounce 0 follows a facet event.
        debug_assert!(
            self.reference.is_none() && self.events.is_none(),
            "the path cache (reference tracer and attribution recorder) declines a Tray Reflection"
        );
        self.flush_radiance_to_nee();
        depolarize_and_scale(&mut self.stokes, floor.fresnel);
        self.prev_plane_normal = None;
        self.current_ray = Ray {
            origin: floor.restart(),
            dir: floor.reflected,
        };
        self.current_k = floor.reflected;
        Flow::Next
    }
}
