//! Shared scene bank for event-wavefront comparison and diagnostics.

use crate::{
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, FacetFinish},
    },
    renderer::env_map::EnvironmentMap,
};

/// One scene of the equivalence run.
pub struct TestCase<'a> {
    pub name: &'static str,
    pub material: GemMaterial,
    pub finishes: &'a [FacetFinish],
    pub env: EnvironmentSource<'a>,
    pub max_bounces: u32,
}

const fn case<'a>(
    name: &'static str,
    material: GemMaterial,
    finishes: &'a [FacetFinish],
    env: EnvironmentSource<'a>,
) -> TestCase<'a> {
    TestCase {
        name,
        material,
        finishes,
        env,
        max_bounces: 8,
    }
}

/// A small deterministic 16 x 8 HDR environment with structured, uneven radiance.
#[must_use]
pub fn synthetic_hdr_map() -> EnvironmentMap {
    let cols = 16;
    let rows = 8;
    let mut pixels = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            let red = (((col * 3 + row * 5) % 7) as f32).mul_add(0.4, 0.1);
            let green = (((col * 2 + row * 3) % 5) as f32).mul_add(0.3, 0.1);
            let blue = (((col * 5 + row * 2) % 6) as f32).mul_add(0.5, 0.1);
            pixels.push([red, green, blue]);
        }
    }
    EnvironmentMap::from_rgb(cols, rows, pixels).expect("valid synthetic env map")
}

/// The representative scenes for material `class` (1 = isotropic, 2 = uniaxial,
/// 3 = biaxial; anything else falls back to a single diamond scene).
///
/// # Panics
///
/// Panics if a representative material is missing from the built-in catalogue.
#[must_use]
pub fn equivalence_cases<'a>(
    class: u32,
    studio_env: EnvironmentSource<'a>,
    frosted_finishes: &'a [FacetFinish],
    synthetic_hdr: &'a EnvironmentMap,
) -> Vec<TestCase<'a>> {
    let diamond = GemMaterial::by_name("Diamond").expect("Diamond is built-in");
    let spinel = GemMaterial::by_name("Spinel").expect("Spinel is built-in");
    let zircon = GemMaterial::by_name("Zircon").expect("Zircon is built-in");
    let quartz = GemMaterial::by_name("Quartz").expect("Quartz is built-in");
    let rutile = GemMaterial::by_name("Rutile").expect("Rutile is built-in");
    let tanzanite = GemMaterial::by_name("Tanzanite").expect("Tanzanite is built-in");
    let ruby = GemMaterial::by_name("Ruby").expect("Ruby is built-in");
    let spinel_scattering = spinel.clone().with_scattering(1.0, 0.3);
    // HDR + scattering: Studio scenes have NEE off, so only this case can see an NEE or
    // MIS bug in the scattering deposit.
    let ruby_scattering = ruby.with_scattering(1.0, 0.3);
    let hdr = EnvironmentSource::HdrMap(synthetic_hdr);

    match class {
        1 => vec![
            case("Diamond, daylight", diamond.clone(), &[], studio_env),
            case("Spinel, daylight", spinel, &[], studio_env),
            case(
                "Diamond, frosted girdle",
                diamond.clone(),
                frosted_finishes,
                studio_env,
            ),
            case(
                "Spinel, scattering",
                spinel_scattering.clone(),
                &[],
                studio_env,
            ),
            case("Diamond, synthetic HDR", diamond.clone(), &[], hdr),
            case(
                "Spinel, scattering, synthetic HDR",
                spinel_scattering,
                &[],
                hdr,
            ),
            TestCase {
                max_bounces: 3,
                ..case(
                    "Diamond, max bounces 3 (survivors)",
                    diamond,
                    &[],
                    studio_env,
                )
            },
        ],
        2 => vec![
            case("Zircon, daylight", zircon.clone(), &[], studio_env),
            case(
                "Zircon, frosted girdle",
                zircon.clone(),
                frosted_finishes,
                studio_env,
            ),
            case("Zircon, synthetic HDR", zircon, &[], hdr),
            case("Ruby, scattering, synthetic HDR", ruby_scattering, &[], hdr),
            case("Quartz, daylight", quartz.clone(), &[], studio_env),
            case("Quartz, synthetic HDR", quartz, &[], hdr),
            case("Rutile, daylight", rutile.clone(), &[], studio_env),
            case("Rutile, synthetic HDR", rutile, &[], hdr),
        ],
        3 => vec![
            case("Tanzanite, daylight", tanzanite.clone(), &[], studio_env),
            case("Tanzanite, synthetic HDR", tanzanite, &[], hdr),
        ],
        _ => vec![case("Diamond, daylight", diamond, &[], studio_env)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        optics::raytracer::LightingPreset,
        renderer::gpu::frame::{classify_material, material_class},
    };

    #[test]
    fn every_scene_uses_its_real_material_class() {
        let hdr = synthetic_hdr_map();
        let studio = LightingPreset::Daylight.studio(1.0, 0.4, 0.35);
        for class in 1..=3 {
            for case in equivalence_cases(class, studio, &[], &hdr) {
                assert_eq!(
                    classify_material(&case.material) & material_class::KIND_MASK,
                    class,
                    "{}",
                    case.name
                );
            }
        }
    }
}
