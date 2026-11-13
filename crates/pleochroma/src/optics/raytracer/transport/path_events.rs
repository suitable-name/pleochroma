//! A recorder for what one traced path did, for spectral attribution.
//!
//! [`PathEvents`] is filled by the bounce loop through a few hook calls and is never read by
//! it: with no recorder (every plain entry point) nothing here runs and no arithmetic of the
//! trace changes. The events are exactly those the metrics' own classifier cares about, so a
//! sample can be labelled windowed / returned / extinct from them (see
//! `color::metrics::attribution`).

use super::PathTermination;
use crate::{
    optics::raytracer::camera::HitRecord,
    renderer::path_cache::{
        bounce::EventKind,
        signature::{BRANCH_SIG_SEED, branch_step},
    },
};
use glam::Vec3;

/// The first inside-to-outside transition of a path.
#[derive(Clone, Copy, Debug)]
pub struct ExitEvent {
    /// Index of the exit facet in the plane slice (tool facets follow the planes).
    pub facet: usize,
    /// The facet's outward normal (the stone's, not the ray-facing one).
    pub normal: Vec3,
    /// The ray direction after the event (the refracted direction outside the stone).
    pub dir: Vec3,
    /// The loop iteration of the event.
    pub bounce: u32,
}

/// What one traced path did, as the bounce loop saw it.
#[derive(Clone, Copy, Debug)]
pub struct PathEvents {
    /// The first facet event: `(facet, entered)`. `entered` is `false` for the specular
    /// reflection off the outside of the stone (surface glare).
    pub first_facet: Option<(usize, bool)>,
    /// The first inside-to-outside transition, if the path made one.
    pub first_exit: Option<ExitEvent>,
    /// Facet events that left the ray inside the stone (internal reflections) before the
    /// first exit.
    pub bounces_before_exit: u32,
    /// A Henyey-Greenstein scattering event happened.
    pub scattered: bool,
    /// The fluorescence vertex fired.
    pub fluoresced: bool,
    /// The ray entered the stone again after an exit (concave stones).
    pub reentered: bool,
    /// Why the bounce loop stopped.
    pub termination: Option<PathTermination>,
    /// The ray direction when the path missed the stone (after an exit, or the camera ray
    /// that never hit).
    pub escaped_dir: Option<Vec3>,
    /// The two-lane branch signature of the facet events so far (`signature::branch_step`): the
    /// path cache's cover test compares its fold with a record's.
    pub signature: [u32; 2],
}

impl Default for PathEvents {
    fn default() -> Self {
        Self {
            first_facet: None,
            first_exit: None,
            bounces_before_exit: 0,
            scattered: false,
            fluoresced: false,
            reentered: false,
            termination: None,
            escaped_dir: None,
            signature: BRANCH_SIG_SEED,
        }
    }
}

impl PathEvents {
    /// One facet event. `inside_before` is the geometric truth just before the event,
    /// `inside_after` the state after the reflect/refract decision, `dir_after` the ray
    /// direction after it and `is_ext_after` the eigenmode after it.
    pub(crate) fn facet(
        &mut self,
        bounce: u32,
        hit: HitRecord,
        inside_before: bool,
        inside_after: bool,
        dir_after: Vec3,
        is_ext_after: bool,
    ) {
        let kind = if inside_before == inside_after {
            EventKind::Reflect
        } else {
            EventKind::Transmit
        };
        let facet = u32::try_from(hit.facet_idx).unwrap_or(u32::MAX);
        self.signature = branch_step(self.signature, facet, kind, is_ext_after);
        if self.first_facet.is_none() {
            self.first_facet = Some((hit.facet_idx, inside_after));
        }
        let exited = self.first_exit.is_some();
        match (inside_before, inside_after) {
            (true, false) if !exited => {
                self.first_exit = Some(ExitEvent {
                    facet: hit.facet_idx,
                    normal: hit.normal,
                    dir: dir_after,
                    bounce,
                });
            }
            (true, true) if !exited => self.bounces_before_exit += 1,
            (false, true) if exited => self.reentered = true,
            _ => {}
        }
    }

    /// The path's ray missed everything; `dir` is its direction.
    ///
    /// The debug assertion relies on the loop invariant that a miss after bounce 0 always
    /// follows a facet event, since the ray starts outside the stone. A continuation that
    /// escapes without a facet event would break it. A bounce off a Tray Reflection's floor
    /// breaks it too, so the path cache that drives this recorder declines a Tray Reflection.
    pub(crate) const fn miss(&mut self, bounce: u32, dir: Vec3) {
        debug_assert!(
            bounce == 0 || self.first_facet.is_some(),
            "a miss after bounce 0 follows a facet event"
        );
        self.escaped_dir = Some(dir);
    }

    /// A scattering event happened.
    pub(crate) const fn scatter(&mut self) {
        self.scattered = true;
    }

    /// The fluorescence vertex fired.
    pub(crate) const fn fluoresce(&mut self) {
        self.fluoresced = true;
    }

    /// The loop stopped for `termination`.
    pub(crate) const fn terminated(&mut self, termination: PathTermination) {
        self.termination = Some(termination);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(facet_idx: usize, normal: Vec3) -> HitRecord {
        HitRecord {
            t: 1.0,
            normal,
            facet_idx,
        }
    }

    #[test]
    fn an_outside_reflection_is_a_first_facet_that_did_not_enter() {
        let mut events = PathEvents::default();
        events.facet(0, hit(3, Vec3::Y), false, false, Vec3::Y, false);
        assert_eq!(events.first_facet, Some((3, false)));
        assert!(events.first_exit.is_none());
    }

    #[test]
    fn entry_reflections_exit_and_reentry_are_told_apart() {
        let mut events = PathEvents::default();
        events.facet(0, hit(1, Vec3::Y), false, true, -Vec3::Y, false);
        events.facet(1, hit(2, -Vec3::Y), true, true, Vec3::Y, false);
        events.facet(2, hit(4, Vec3::X), true, false, Vec3::X, false);
        // Later interior events do not move the first exit or the count.
        events.facet(3, hit(5, Vec3::Z), false, true, Vec3::Z, false);
        events.facet(4, hit(6, Vec3::X), true, true, Vec3::Z, false);
        events.facet(5, hit(7, Vec3::X), true, false, Vec3::X, false);
        assert_eq!(events.first_facet, Some((1, true)));
        let exit = events.first_exit.expect("the path exited");
        assert_eq!((exit.facet, exit.bounce), (4, 2));
        assert_eq!(events.bounces_before_exit, 1);
        assert!(events.reentered);
    }

    #[test]
    fn facet_events_fold_into_the_branch_signature() {
        let mut events = PathEvents::default();
        assert_eq!(events.signature, BRANCH_SIG_SEED);
        events.facet(0, hit(1, Vec3::Y), false, true, -Vec3::Y, true);
        events.facet(1, hit(2, -Vec3::Y), true, true, Vec3::Y, false);
        let expected = branch_step(
            branch_step(BRANCH_SIG_SEED, 1, EventKind::Transmit, true),
            2,
            EventKind::Reflect,
            false,
        );
        assert_eq!(events.signature, expected);
    }

    #[test]
    fn flags_and_termination_are_recorded() {
        let mut events = PathEvents::default();
        events.scatter();
        events.fluoresce();
        events.terminated(PathTermination::RussianRoulette);
        events.miss(0, Vec3::X);
        assert!(events.scattered && events.fluoresced);
        assert_eq!(events.termination, Some(PathTermination::RussianRoulette));
        assert_eq!(events.escaped_dir, Some(Vec3::X));
    }
}
