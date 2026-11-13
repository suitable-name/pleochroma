//! The built-in Ametrine: bicolour quartz with an amethyst half and a citrine half.
//!
//! Both halves are colored from the chromophore catalogue's quartz iron recipes (the citrine
//! half at about 0.4 times the amethyst half's iron, see [`AMETRINE_CITRINE_FE_PPMA`]).
//!
//! The two zone tensors are resolved once per process (`zone_tensors`), so
//! [`GemMaterial::all_materials`] stays cheap and every call returns the same bytes.

use super::GemMaterial;
use crate::optics::{
    absorption::AbsorptionTensor,
    chromophore::{ChromophoreCatalogue, ColorRecipe, resolve},
    zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
};
use glam::DVec3;
use std::sync::OnceLock;

/// Iron of the amethyst half, in ppma of all atoms (the unit of the quartz host's iron).
///
/// This is 80 ppmw Fe in quartz converted through the host's number density:
/// `80e-6 * 2.65 g/cm3 / 55.845 g/mol * N_A / 7.968712e22 cm^-3 * 1e6 = 28.7`. 80 ppmw is the
/// Anahi (Bolivia) amethyst that the catalogue test
/// `measured_color_tests::recipe_vs_measured_amethyst_e_perp_c` checks the recipe against. The
/// recipe's treatment is `gamma_irradiation`, which turns 10 % of the iron into the `[FeO4]0`
/// amethyst centre.
pub const AMETRINE_AMETHYST_FE_PPMA: f64 = 28.7;

/// Iron of the citrine half, in ppma of all atoms: about 0.4 times the amethyst half's.
///
/// The recipe's treatments are `gamma_irradiation` and then `thermal_anneal_450`: the catalogue's
/// heated-amethyst-to-citrine chain, in which the anneal removes the `[FeO4]0` centre and creates
/// the nano-inclusion `Fe3+` chromophore from 50 % of the iron.
///
/// No measurement anchors this strength: the catalogue has no iron measurement of an ametrine's
/// citrine sector, and the conversion fractions are treatment yields. At this iron the half
/// matches the Citrine built-in's color, and the amethyst half stays at the measured Anahi iron.
/// More iron makes most of the stone orange, since most light crosses both halves; less iron
/// tips the stone toward pink.
pub const AMETRINE_CITRINE_FE_PPMA: f64 = 11.0;

/// Treatments of the amethyst half's recipe.
const AMETHYST_TREATMENTS: &[&str] = &["gamma_irradiation"];

/// Treatments of the citrine half's recipe, in the catalogue's own order.
const CITRINE_TREATMENTS: &[&str] = &["gamma_irradiation", "thermal_anneal_450"];

/// The absorption of quartz holding `iron_ppma` of iron after `treatments`, per millimetre.
///
/// The catalogue is embedded in the binary, so the recipe resolving is an invariant of this
/// module and not something a caller can trigger.
fn quartz_iron_tensor(iron_ppma: f64, treatments: &[&str]) -> AbsorptionTensor {
    let catalogue = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("quartz", catalogue.data_version);
    assert!(
        recipe.set_amount("Fe", iron_ppma),
        "the iron amount is finite"
    );
    recipe.treatments = treatments.iter().map(|id| (*id).to_owned()).collect();
    let (tensor, _warnings) = resolve(&recipe, catalogue)
        .expect("the embedded catalogue resolves the quartz iron recipes");
    tensor
}

/// The amethyst and the citrine tensor, resolved once per process.
fn zone_tensors() -> &'static (AbsorptionTensor, AbsorptionTensor) {
    static TENSORS: OnceLock<(AbsorptionTensor, AbsorptionTensor)> = OnceLock::new();
    TENSORS.get_or_init(|| {
        (
            quartz_iron_tensor(AMETRINE_AMETHYST_FE_PPMA, AMETHYST_TREATMENTS),
            quartz_iron_tensor(AMETRINE_CITRINE_FE_PPMA, CITRINE_TREATMENTS),
        )
    })
}

/// The zoning of the built-in Ametrine: amethyst everywhere (the base zone) and citrine on the
/// `x >= 0` side of the plane `x = 0`.
///
/// The plane contains the crystal's c axis (`+Y`, the table normal), like the boundary between
/// the amethyst and the citrine growth sectors of a real ametrine. The boundary is sharp, as the
/// Anahi boundaries are. A half space through the origin is scale-free, so the same geometry
/// fits a stone of any size and needs no "relative to stone" handling. Both zones are per
/// millimetre.
///
/// The zones follow the crystal axis ([`ZonedAbsorption::follows_axis`]): a stone whose Crystal
/// Axis is set elsewhere keeps the boundary on its axis, and the render
/// (`render_setup::apply_material_overrides`) turns it with
/// [`ZoneFrame::along_axis`](crate::optics::zoning::ZoneFrame::along_axis), which keeps the
/// boundary running straight down through the table, so face up the split lies on the axis line.
#[must_use]
pub fn ametrine_zoning() -> ZonedAbsorption {
    let (amethyst, citrine) = zone_tensors();
    let mut zoning = ZonedAbsorption::new(ZoneAbsorption::per_mm(amethyst.clone()));
    zoning.follows_axis = true;
    zoning.zones.push(Zone {
        shape: ZoneShape::HalfSpace {
            normal: DVec3::X,
            offset: 0.0,
        },
        absorption: ZoneAbsorption::per_mm(citrine.clone()),
    });
    zoning
}

impl GemMaterial {
    /// The built-in Ametrine: Anahi-type bicolour quartz with one straight boundary, amethyst on
    /// `x < 0` and citrine on `x >= 0` (see [`ametrine_zoning`]).
    ///
    /// The optics are Citrine's, bit for bit (it is taken from
    /// that entry, so the two cannot drift apart): the same quartz host, ordinary and
    /// extraordinary dispersion, birefringence and c axis `+Y`, no scattering and no edge
    /// rounding. The colors are the catalogue's quartz iron recipes per millimetre (see
    /// [`AMETRINE_AMETHYST_FE_PPMA`] and [`AMETRINE_CITRINE_FE_PPMA`]), so the material is
    /// [`AbsorptionUnit::PerMm`](super::AbsorptionUnit::PerMm) and its plain
    /// [`absorption`](Self::absorption) is the amethyst half.
    ///
    /// Not modelled: the six-sector pinwheel of a whole crystal section (a cut stone shows one or
    /// two sectors, not the star), and a diffuse boundary (`boundary_softness_mm` is `0`). A
    /// straight half-space boundary is a chosen cut configuration, not the growth-sector geometry
    /// of every crystal.
    pub(super) fn built_in_material_ametrine() -> Self {
        let citrine = Self::built_in_materials_amethyst_through_citrine()
            .into_iter()
            .find(|material| material.name == "Citrine")
            .expect("the Citrine built-in is defined next to Amethyst");
        Self {
            name: "Ametrine".to_owned(),
            ..citrine
        }
        .with_zoning(ametrine_zoning())
    }
}
