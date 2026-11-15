//! The surface finish on its way to the GPU.
//!
//! A CPU [`SurfaceScene`] (the facet surfaces, the faceting machine and the abraded-edge setting)
//! reaches the kernels in two pieces, neither of which needs a binding of its own:
//!
//! - the per-facet words go into the finish buffer (binding 8) through
//!   [`encode_facet_surfaces`](super::encode_facet_surfaces), the packed `u32` that the CPU
//!   decodes with [`FacetSurface::from_packed`];
//! - the edge band and the machine go into the spare slots of the material's inclusion block:
//!   `edge = [width, alpha, halo_rq_nm, arm_offset_deg]` and `haze.zw = [a, b]`. The sweep of
//!   the stone over the lap is carried as the contact radius over the mast distance,
//!   `x = a + u b` for the draw `u` in `[0, 1]` (`a = inner / mast`, `b = (outer - inner) / mast`,
//!   both zero for an arm through the lap centre). The scratch angle is then
//!   `arm_offset + asin(1 / x)`, clamped, which is [`FacetingMachine::angle_at_radius_deg`] with
//!   the contact point held outside the arm's line.
//!
//! The directions the scratches are measured against ([`ScratchBasis`]) go into the block's two
//! scratch rows: `scratch_up = [up.xyz, 1]` and `scratch_side = [side.xyz, 0]`. A stone as designed
//! leaves both rows zero, and the shader then runs the unrotated arithmetic.
//!
//! An ideal scene touches nothing: the material block stays zero and the words are the plain
//! finish words, so such a frame is byte-identical to one without this module.

use crate::optics::raytracer::{
    FacetFinish,
    surface::{EdgeAbrasion, FacetSurface, FacetingMachine, ScratchBasis, SurfaceScene},
};

use super::{GpuGemMaterial, encode_facet_surfaces};

/// An owned copy of a [`SurfaceScene`], held by a GPU renderer between frames.
#[derive(Clone, Debug, Default)]
pub struct GpuSurfaceScene {
    facet_surfaces: Vec<FacetSurface>,
    machine: FacetingMachine,
    scratch: ScratchBasis,
    edge: EdgeAbrasion,
}

impl GpuSurfaceScene {
    /// A copy of `scene`.
    #[must_use]
    pub fn from_scene(scene: &SurfaceScene<'_>) -> Self {
        Self {
            facet_surfaces: scene.facet_surfaces.to_vec(),
            machine: scene.machine,
            scratch: scene.scratch,
            edge: scene.edge,
        }
    }

    /// `true` when nothing in the scene departs from a perfect polish.
    #[must_use]
    pub fn is_ideal(&self) -> bool {
        !self.edge.active() && self.facet_surfaces.iter().all(|surface| surface.is_ideal())
    }

    /// `true` when the CPU tracer's `SurfaceScene::is_ideal` holds for the scene this was copied
    /// from: no facet surface is listed at all and no edge is abraded.
    ///
    /// Stricter than [`Self::is_ideal`], which also accepts a list of ideal surfaces. The CPU
    /// tracer runs exit collection exactly when this holds, so the GPU engines ask the same
    /// question (`GpuTransportParams::with_estimator`) and a hybrid frame never mixes estimators
    /// over a listed but ideal surface.
    #[must_use]
    pub fn is_ideal_for_cpu(&self) -> bool {
        self.facet_surfaces.is_empty() && !self.edge.active()
    }

    /// `true` when a ray can take the ground arm even though no facet finish is frosted: a Ground
    /// facet surface, or the abraded-edge band. The event wavefront schedules its ground arm
    /// kernel on this as well as on the finishes.
    #[must_use]
    pub fn needs_ground_arm(&self) -> bool {
        self.edge.ground_active()
            || self
                .facet_surfaces
                .iter()
                .any(|surface| surface.is_ground())
    }

    /// The binding-8 words of `num_planes` facets: the facet surfaces over the finishes (see
    /// [`encode_facet_surfaces`]).
    #[must_use]
    pub fn encode_finishes(&self, finishes: &[FacetFinish], num_planes: usize) -> Vec<u32> {
        encode_facet_surfaces(&self.facet_surfaces, finishes, num_planes)
    }

    /// `material` with the edge band, the machine and the scratch basis written into its inclusion
    /// block; unchanged for an ideal scene. A basis other than [`ScratchBasis::MODEL`] fills the
    /// two scratch rows.
    #[must_use]
    pub fn apply_to_material(&self, mut material: GpuGemMaterial) -> GpuGemMaterial {
        if self.is_ideal() {
            return material;
        }
        let (a, b) = machine_sweep(&self.machine);
        let block = &mut material.inclusions;
        block.edge = [
            self.edge.width,
            self.edge.alpha,
            self.edge.halo_rq_nm,
            self.machine.arm_offset_deg,
        ];
        block.haze[2] = a;
        block.haze[3] = b;
        if !self.scratch.is_model() {
            block.scratch_up = scratch_row(self.scratch.up, 1.0);
            block.scratch_side = scratch_row(self.scratch.side, 0.0);
        }
        material
    }
}

/// One scratch row: `v` in `.xyz` and `w` in `.w`.
const fn scratch_row(v: glam::Vec3, w: f32) -> [f32; 4] {
    [v.x, v.y, v.z, w]
}

/// The `(a, b)` of the module documentation. `(0, 0)` for a zero mast distance (an arm through
/// the lap centre), which the shader reads as "the scratch angle is the arm offset".
pub fn machine_sweep(machine: &FacetingMachine) -> (f32, f32) {
    let mast = machine.mast_distance_mm;
    if mast == 0.0 {
        return (0.0, 0.0);
    }
    (
        machine.sweep.inner_mm / mast,
        (machine.sweep.outer_mm - machine.sweep.inner_mm) / mast,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::raytracer::surface::{FacetSurface, SweepRange};

    /// The scratch angle the shader computes from `(arm_offset, a, b)` and the draw `u`
    /// (`surf_machine_angle_deg`).
    #[expect(
        clippy::many_single_char_names,
        reason = "a, b, u, x, v are the names of the shader's angle formula"
    )]
    fn shader_angle_deg(arm_offset: f32, a: f32, b: f32, u: f32) -> f32 {
        if a == 0.0 && b == 0.0 {
            return arm_offset;
        }
        let x = u.mul_add(b, a);
        let v = if x.abs() <= 1.0 {
            if x < 0.0 || (x == 0.0 && (a < 0.0 || b < 0.0)) {
                -1.0
            } else {
                1.0
            }
        } else {
            1.0 / x
        };
        arm_offset + v.clamp(-1.0, 1.0).asin().to_degrees()
    }

    fn zeroed_material() -> GpuGemMaterial {
        bytemuck::Zeroable::zeroed()
    }

    fn mast_machine(mast_mm: f32, inner_mm: f32, outer_mm: f32) -> FacetingMachine {
        FacetingMachine {
            arm_offset_deg: 7.5,
            lap_radius_mm: 100.0,
            mast_distance_mm: mast_mm,
            sweep: SweepRange { inner_mm, outer_mm },
        }
    }

    #[test]
    fn an_ideal_scene_leaves_the_material_block_zero() {
        let scene = GpuSurfaceScene::from_scene(&SurfaceScene::IDEAL);
        assert!(scene.is_ideal() && !scene.needs_ground_arm());
        let material = scene.apply_to_material(zeroed_material());
        assert_eq!(material.inclusions.edge, [0.0; 4]);
        assert_eq!(material.inclusions.haze, [0.0; 4]);
        assert_eq!(material.inclusions.scratch_up, [0.0; 4]);
        assert_eq!(material.inclusions.scratch_side, [0.0; 4]);
        // Surfaces that are all ideal are as good as none.
        let listed = GpuSurfaceScene::from_scene(&SurfaceScene {
            facet_surfaces: &[FacetSurface::IDEAL; 3],
            ..SurfaceScene::IDEAL
        });
        assert!(listed.is_ideal());
    }

    /// A listed but ideal surface is ideal for the kernels, not for the CPU's collection rule.
    #[test]
    fn a_listed_ideal_surface_is_not_ideal_for_the_cpu_rule() {
        let none = GpuSurfaceScene::default();
        assert!(none.is_ideal());
        assert!(none.is_ideal_for_cpu());
        let listed = GpuSurfaceScene::from_scene(&SurfaceScene {
            facet_surfaces: &[FacetSurface::IDEAL],
            ..SurfaceScene::IDEAL
        });
        assert!(listed.is_ideal());
        assert!(!listed.is_ideal_for_cpu());
    }

    #[test]
    fn the_shader_form_of_the_sweep_is_the_machines_angle() {
        for (mast, inner, outer) in [
            (30.0, 40.0, 80.0),
            (-30.0, 40.0, 80.0),
            (60.0, 45.0, 90.0),
            (25.0, 0.0, 70.0),
            (30.0, 55.0, 55.0),
        ] {
            let machine = mast_machine(mast, inner, outer);
            let (a, b) = machine_sweep(&machine);
            for i in 0..=20u8 {
                let u = f32::from(i) / 20.0;
                let shader = shader_angle_deg(machine.arm_offset_deg, a, b, u);
                let cpu = machine.sample_angle_deg(u);
                assert!(
                    (shader - cpu).abs() < 2e-4,
                    "mast {mast} sweep {inner}..{outer} u {u}: shader {shader} cpu {cpu}"
                );
            }
        }
        // An arm through the lap centre has one angle.
        let centred = mast_machine(0.0, 40.0, 80.0);
        assert_eq!(machine_sweep(&centred), (0.0, 0.0));
        assert_eq!(
            shader_angle_deg(centred.arm_offset_deg, 0.0, 0.0, 0.37),
            centred.sample_angle_deg(0.37)
        );
    }

    #[test]
    fn a_ground_surface_or_the_band_asks_for_the_ground_arm() {
        let ground = [FacetSurface::ground(0.3, 1.0)];
        let with_ground = GpuSurfaceScene::from_scene(&SurfaceScene {
            facet_surfaces: &ground,
            ..SurfaceScene::IDEAL
        });
        assert!(with_ground.needs_ground_arm() && !with_ground.is_ideal());
        let band = GpuSurfaceScene::from_scene(&SurfaceScene {
            edge: EdgeAbrasion {
                width: 0.02,
                alpha: 0.3,
                halo_rq_nm: 0.0,
            },
            ..SurfaceScene::IDEAL
        });
        assert!(band.needs_ground_arm());
        let polish = [FacetSurface::polished(2.5, 2.0)];
        let polished = GpuSurfaceScene::from_scene(&SurfaceScene {
            facet_surfaces: &polish,
            ..SurfaceScene::IDEAL
        });
        assert!(!polished.needs_ground_arm() && !polished.is_ideal());
    }

    #[test]
    fn the_edge_and_the_machine_land_in_the_spare_slots() {
        let machine = mast_machine(30.0, 40.0, 80.0);
        let scene = GpuSurfaceScene::from_scene(&SurfaceScene {
            facet_surfaces: &[],
            machine,
            scratch: ScratchBasis::MODEL,
            edge: EdgeAbrasion {
                width: 0.02,
                alpha: 0.3,
                halo_rq_nm: 12.0,
            },
        });
        let material = scene.apply_to_material(zeroed_material());
        assert_eq!(material.inclusions.edge, [0.02, 0.3, 12.0, 7.5]);
        assert_eq!(material.inclusions.haze[2], 40.0 / 30.0);
        assert_eq!(material.inclusions.haze[3], 40.0 / 30.0);
        // The other slots of the block are not touched.
        assert_eq!(material.inclusions.haze[..2], [0.0, 0.0]);
        assert_eq!(material.inclusions.header, [0; 4]);
        // A stone as designed leaves the scratch rows zero: the shader runs the unrotated code.
        assert_eq!(material.inclusions.scratch_up, [0.0; 4]);
        assert_eq!(material.inclusions.scratch_side, [0.0; 4]);
    }

    #[test]
    fn a_turned_stones_scratch_basis_lands_in_the_two_scratch_rows() {
        let basis = ScratchBasis::new(
            glam::Vec3::new(0.0, 0.0, 1.0),
            glam::Vec3::new(1.0, 0.0, 0.0),
        );
        let polish = [FacetSurface::polished(2.5, 2.0)];
        let scene = GpuSurfaceScene::from_scene(&SurfaceScene {
            facet_surfaces: &polish,
            scratch: basis,
            ..SurfaceScene::IDEAL
        });
        let material = scene.apply_to_material(zeroed_material());
        assert_eq!(material.inclusions.scratch_up, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(material.inclusions.scratch_side, [1.0, 0.0, 0.0, 0.0]);
        // The rows sit apart from the machine and the edge slots.
        assert_eq!(material.inclusions.edge[3], 0.0);
        assert_eq!(material.inclusions.header, [0; 4]);
        // An ideal scene stays zero whatever the basis.
        let ideal = GpuSurfaceScene::from_scene(&SurfaceScene {
            scratch: basis,
            ..SurfaceScene::IDEAL
        });
        let untouched = ideal.apply_to_material(zeroed_material());
        assert_eq!(untouched.inclusions.scratch_up, [0.0; 4]);
    }
}
