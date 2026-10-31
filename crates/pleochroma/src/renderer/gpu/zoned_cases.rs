//! The zoned stones the GPU checks render.
//!
//! One fixture list shared by the Tier 3 statistical image comparisons
//! (`estimator_check::run_image_comparison_zoned`), the GPU harness and the `gpu_backend`
//! tests, so each of them exercises the same shapes.
//!
//! Every stone (except [`zoned_opal_material`]) is a colourless diamond whose absorption is replaced by zones, sized to
//! [`ZONED_PATH_SCALE`] millimetres per model unit (a round brilliant of model girdle radius
//! about 1 is then about 6 mm wide). Zone geometry is in stone millimetres, origin at the model
//! origin, the stone's axis along +Y.

use glam::{DVec3, Vec3};

use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    inclusions::bragg::StructureSpec,
    materials::{CrystalSystem, GemMaterial, OpticalCharacter},
    zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
};

/// Millimetres per model unit of every fixture below.
pub const ZONED_PATH_SCALE: f32 = 3.0;

/// One zoned test stone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZonedImageCase {
    /// Two halves split by the plane `x = 0`: absorbs red / absorbs blue. Exercises the half space.
    Bicolour,
    /// Watermelon: a trigonal prism tube (axis +Y) inside a base zone, capped by a half space.
    /// Exercises the prism and a second override.
    Watermelon,
    /// Two sectors about +Y, one narrow and one wider than pi. Exercises both sector forms.
    Sectors,
    /// A slab and a cylinder tube. Exercises the slab and the cylinder.
    SlabAndTube,
    /// [`Self::Bicolour`] with a 1.2 mm smoothstep boundary. Exercises the soft kernel.
    SoftBicolour,
    /// The prism-tube stone with a 0.8 mm smoothstep boundary. Exercises the soft kernel on a
    /// curved/kinked depth.
    SoftWatermelon,
    /// A prism tube whose zone tensor is dichroic (`AbsorptionTensor::uniaxial`, green Along,
    /// blue-green Across) on a trigonal host with birefringence 0.02. The only case with a
    /// pleochroic zone: it exercises the per-ray eigenmode evaluation of the zone alphas, which
    /// every other case (isotropic tensors) never reaches.
    DichroicRim,
}

impl ZonedImageCase {
    /// Every case, in report order.
    pub const ALL: [Self; 7] = [
        Self::Bicolour,
        Self::Watermelon,
        Self::Sectors,
        Self::SlabAndTube,
        Self::SoftBicolour,
        Self::SoftWatermelon,
        Self::DichroicRim,
    ];

    /// A short label for reports.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Bicolour => "zoned bicolour (half space)",
            Self::Watermelon => "zoned watermelon (prism tube + cap)",
            Self::Sectors => "zoned sectors (narrow + wide)",
            Self::SlabAndTube => "zoned slab + cylinder tube",
            Self::SoftBicolour => "zoned bicolour, soft 1.2 mm boundary",
            Self::SoftWatermelon => "zoned watermelon, soft 0.8 mm boundary",
            Self::DichroicRim => "zoned dichroic rim (uniaxial tensor, trigonal host)",
        }
    }

    /// The zoning of this case.
    #[must_use]
    pub fn zoning(self) -> ZonedAbsorption {
        match self {
            Self::Bicolour => bicolour(0.0),
            Self::SoftBicolour => bicolour(1.2),
            Self::Watermelon => watermelon(0.0),
            Self::SoftWatermelon => watermelon(0.8),
            Self::Sectors => sectors(),
            Self::SlabAndTube => slab_and_tube(),
            Self::DichroicRim => dichroic_rim(),
        }
    }

    /// The stone: a colourless diamond (a birefringent trigonal host for
    /// [`Self::DichroicRim`]) with this case's zones and [`ZONED_PATH_SCALE`].
    #[must_use]
    pub fn material(self) -> GemMaterial {
        let host = if self == Self::DichroicRim {
            trigonal_host()
        } else {
            GemMaterial::diamond()
        };
        host.with_zoning(self.zoning())
            .with_absorption_path_scale(ZONED_PATH_SCALE)
    }
}

/// The diamond with a uniaxial trigonal optical description: crystal system Trigonal,
/// birefringence 0.02 and the c axis along +Y (the stone axis), so the tracers' host test
/// (`crystal_system != Cubic && |birefringence| > 1e-4`) is an anisotropic host.
fn trigonal_host() -> GemMaterial {
    let mut host = GemMaterial::diamond();
    host.crystal_system = CrystalSystem::Trigonal;
    host.optical_character = OpticalCharacter::UniaxialPositive;
    host.birefringence_delta = 0.02;
    host.c_axis = Vec3::Y;
    host
}

fn band(center_nm: f32, width_nm: f32, peak_per_mm: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        center_nm,
        width_nm,
        peak_per_mm,
    )]))
}

fn clear() -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(Vec::new()))
}

fn red_absorbing() -> ZoneAbsorption {
    band(620.0, 50.0, 0.25)
}

fn blue_absorbing() -> ZoneAbsorption {
    band(450.0, 40.0, 0.30)
}

fn green_absorbing() -> ZoneAbsorption {
    band(540.0, 45.0, 0.30)
}

const fn zoned(base: ZoneAbsorption, zones: Vec<Zone>, softness_mm: f32) -> ZonedAbsorption {
    ZonedAbsorption {
        frame: ZoneFrame::IDENTITY,
        base,
        zones,
        boundary_softness_mm: softness_mm,
        follows_axis: false,
    }
}

fn bicolour(softness_mm: f32) -> ZonedAbsorption {
    zoned(
        red_absorbing(),
        vec![Zone {
            shape: ZoneShape::HalfSpace {
                normal: DVec3::X,
                offset: 0.0,
            },
            absorption: blue_absorbing(),
        }],
        softness_mm,
    )
}

/// The bicolour fixture written the other way round.
///
/// The BLUE band is the base and the RED band is the override on the `-X` half space. Physically
/// the same stone as
/// [`ZonedImageCase::Bicolour`] (red for `x < 0`, blue for `x > 0`), so a renderer that handles
/// zone-crossing segments consistently renders both identically.
#[must_use]
pub fn mirrored_bicolour_material() -> GemMaterial {
    GemMaterial::diamond()
        .with_zoning(zoned(
            blue_absorbing(),
            vec![Zone {
                shape: ZoneShape::HalfSpace {
                    normal: -DVec3::X,
                    offset: 0.0,
                },
                absorption: red_absorbing(),
            }],
            0.0,
        ))
        .with_absorption_path_scale(ZONED_PATH_SCALE)
}

/// Precious Opal (White) with the bicolour zones and its default play-of-color structure.
///
/// A zoned host that also holds a Bragg domain structure, the stone `bounce_scatter_sample_zoned_bragg`
/// renders. Deliberately not in [`ZonedImageCase::ALL`]: that list drives the per-segment echo
/// baselines and the mean-relative-tolerance loop, and a Bragg stone's noise is another question.
///
/// # Panics
///
/// Panics if `"Precious Opal (White)"` is ever removed or loses its default structure.
#[must_use]
pub fn zoned_opal_material() -> GemMaterial {
    let name = "Precious Opal (White)";
    GemMaterial::by_name(name)
        .expect("\"Precious Opal (White)\" is a built-in material")
        .with_zoning(bicolour(0.0))
        .with_absorption_path_scale(ZONED_PATH_SCALE)
        .with_structure_spec(
            &StructureSpec::default_for(name).expect("the opal has a default structure"),
        )
}

fn watermelon(softness_mm: f32) -> ZonedAbsorption {
    zoned(
        green_absorbing(),
        vec![
            Zone {
                shape: ZoneShape::CoaxialPrism {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    n_sides: 3,
                    r_in: 0.0,
                    r_out: 1.8,
                    phase: 0.4,
                },
                absorption: red_absorbing(),
            },
            Zone {
                shape: ZoneShape::HalfSpace {
                    normal: DVec3::Y,
                    offset: 0.8,
                },
                absorption: blue_absorbing(),
            },
        ],
        softness_mm,
    )
}

/// A clear stone with a dichroic trigonal-prism rim (axis +Y, apothems 1.0 to 1.8 mm): the
/// Along (ordinary) band absorbs green, the Across (extraordinary) band blue-green.
///
/// The zone's tensor is the one a two-colour pattern zone makes
/// (`AbsorptionTensor::uniaxial(along, across)`); the table carries both band sets.
fn dichroic_rim() -> ZonedAbsorption {
    zoned(
        clear(),
        vec![Zone {
            shape: ZoneShape::CoaxialPrism {
                axis_point: DVec3::ZERO,
                axis_dir: DVec3::Y,
                n_sides: 3,
                r_in: 1.0,
                r_out: 1.8,
                phase: 0.4,
            },
            absorption: ZoneAbsorption::per_mm(AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::new(540.0, 45.0, 0.30)],
                vec![AbsorptionBand::new(490.0, 50.0, 0.30)],
            )),
        }],
        0.0,
    )
}

fn sectors() -> ZonedAbsorption {
    zoned(
        clear(),
        vec![
            Zone {
                shape: ZoneShape::Sector {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    angle_from: 0.3,
                    angle_to: 1.9,
                },
                absorption: red_absorbing(),
            },
            Zone {
                shape: ZoneShape::Sector {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    angle_from: 2.4,
                    angle_to: 5.9,
                },
                absorption: blue_absorbing(),
            },
        ],
        0.0,
    )
}

fn slab_and_tube() -> ZonedAbsorption {
    zoned(
        clear(),
        vec![
            Zone {
                shape: ZoneShape::Slab {
                    normal: DVec3::Z,
                    offset_min: -1.2,
                    offset_max: 1.2,
                },
                absorption: green_absorbing(),
            },
            Zone {
                shape: ZoneShape::CoaxialCylinder {
                    axis_point: DVec3::ZERO,
                    axis_dir: DVec3::Y,
                    r_in: 0.6,
                    r_out: 1.6,
                },
                absorption: red_absorbing(),
            },
        ],
        0.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixture_is_valid_and_gpu_renderable() {
        for case in ZonedImageCase::ALL {
            let material = case.material();
            let zoning = material.zoning.as_ref().expect("zoned");
            assert_eq!(zoning.validate(), Ok(()), "{}", case.label());
            assert!(material.gpu_supported(), "{}", case.label());
        }
    }

    #[test]
    fn the_mirrored_bicolour_is_valid_and_gpu_renderable() {
        let material = mirrored_bicolour_material();
        let zoning = material.zoning.as_ref().expect("zoned");
        assert_eq!(zoning.validate(), Ok(()));
        assert!(material.gpu_supported());
    }

    #[test]
    fn the_zoned_opal_is_valid_gpu_renderable_and_structured() {
        let material = zoned_opal_material();
        let zoning = material.zoning.as_ref().expect("zoned");
        assert_eq!(zoning.validate(), Ok(()));
        assert!(material.gpu_supported());
        assert!(
            material
                .oriented_inclusions()
                .expect("the opal has an inclusion set")
                .structure()
                .is_some()
        );
    }
}
