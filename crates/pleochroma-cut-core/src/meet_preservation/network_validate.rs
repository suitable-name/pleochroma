//! The solve-and-judge entry points of [`MeetPreservationBaseline`]: the geometry gate on a
//! design it solves itself, with the solve handed back when the caller needs the tiers of the
//! design it just certified.

use super::{MeetPreservationBaseline, blame_candidate};
use crate::{
    design::Design,
    meet_preservation::{PreservationFacts, PreservationPolicy, PreservationViolation, provenance},
};
use pleochroma::geometry::meet_solver::{SolveControl, SolvedTier};

impl MeetPreservationBaseline {
    /// [`Self::validate_with`] that also returns the solve it judged, so a caller that goes on
    /// to measure the certified design does not solve it a second time.
    ///
    /// The tiers are those of `candidate` exactly as passed; the same facts and the same
    /// refusals as [`Self::validate_with`] (bit for bit: it is the same gate on the same solve).
    ///
    /// # Errors
    /// As [`Self::validate_with`].
    pub fn validate_and_solve_with(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<(PreservationFacts, Vec<SolvedTier>), PreservationViolation> {
        self.validate_geometry_solved(
            candidate,
            policy,
            &cancel.map_or_else(SolveControl::default, SolveControl::with_cancel),
            true,
        )
        .map_err(|error| self.annotate(error))
    }

    pub(in crate::meet_preservation) fn validate_geometry(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
        control: &SolveControl<'_>,
        check_authoring: bool,
    ) -> Result<PreservationFacts, PreservationViolation> {
        self.validate_geometry_solved(candidate, policy, control, check_authoring)
            .map(|(facts, _)| facts)
    }

    /// The gate proper: solve `candidate`, judge the solve, return both.
    fn validate_geometry_solved(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
        control: &SolveControl<'_>,
        check_authoring: bool,
    ) -> Result<(PreservationFacts, Vec<SolvedTier>), PreservationViolation> {
        if control.is_cancelled() {
            return Err(PreservationViolation::Cancelled);
        }
        if check_authoring && !provenance::same_authoring(self.original(), candidate) {
            return Err(PreservationViolation::StaleCandidate);
        }
        let _validation = self.telemetry.phase(3);
        let solved = {
            let _solve = self.telemetry.phase(2);
            crate::meet_preservation::solve_checked_controlled(candidate, control)
                .map_err(blame_candidate)?
        };
        let facts = self.validate_solved(candidate, &solved, policy, control, None)?;
        Ok((facts, solved))
    }
}
