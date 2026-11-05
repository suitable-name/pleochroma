//! Typical zoning of the stone's species, applied in one step from the Color popup and the Stone
//! tab (`Zoning.apply_typical_pattern`).
//!
//! The pattern is built on the color the stone has without a pattern
//! ([`Target::own_material`]), its colors are solved on a background thread (the Color Pattern
//! editor's solver, `color_pattern::solve_pattern_colors`) and the result goes through the same
//! guarded [`Port::apply`] as an edit in the editor: the open design takes it as one undo step, a
//! preview keeps it for the view only. A stone or material that changed while the colors were
//! matched is refused, never recolored. A Watermelon or Reverse Watermelon then also turns the
//! Crystal Axis through the table (the Through the Table preset, a step of its own), the way
//! watermelons are cut.

use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use pleochroma_cut_core::material::pattern::{
    ColorPattern, typical_index, typical_patterns, wants_axis_through_table,
};
use pleochroma_editor::lch_color::reference_path_mm;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{Port, Target, effective_width};
use crate::{
    CrystalAxisModel, MainWindow, ViewportModel, Zoning,
    gui::{
        color_pattern::{solve_pattern_colors, zone_variants},
        rough_colour::material_zones::registered_vault,
        show_toast,
    },
};

/// What the Slint `Zoning` global shows of the typical zoning of the stone.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct TypicalState {
    /// The species the patterns are typical for ("Sapphire"); empty when it has none.
    pub species: String,
    /// The labels of the patterns, in menu order.
    pub labels: Vec<&'static str>,
    /// The entry the stone's pattern came from, `-1` for none.
    pub index: i32,
}

impl Default for TypicalState {
    fn default() -> Self {
        Self {
            species: String::new(),
            labels: Vec::new(),
            index: -1,
        }
    }
}

/// The typical patterns of the captured stone's species, and the one its pattern came from.
pub(super) fn state_of(target: &Target) -> TypicalState {
    let name = target.material.name.as_str();
    let entries = typical_patterns(name);
    let template = target.pattern.as_ref().and_then(|pattern| pattern.template);
    TypicalState {
        species: if entries.is_empty() {
            String::new()
        } else {
            name.trim().to_owned()
        },
        labels: entries.iter().map(|entry| entry.label).collect(),
        index: typical_index(name, template)
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1),
    }
}

/// Writes `state` to the `Zoning` global.
pub(super) fn push(zoning: &Zoning<'_>, state: &TypicalState) {
    zoning.set_typical_pattern_species(state.species.as_str().into());
    zoning.set_typical_pattern_labels(ModelRc::new(VecModel::from(
        state
            .labels
            .iter()
            .map(|label| SharedString::from(*label))
            .collect::<Vec<_>>(),
    )));
    zoning.set_typical_pattern_index(state.index);
}

/// A typical pattern whose colors are being matched.
struct Pending {
    ticket: u64,
    port: Port,
    /// The stone as it was when the entry was picked: the apply refuses another one.
    expected: RefCell<Target>,
    cancel: Arc<AtomicBool>,
}

thread_local! {
    /// The one request in flight (a newer pick supersedes it).
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// Hands out the ticket of each request, so a late result of a superseded one is dropped.
static TICKET: AtomicU64 = AtomicU64::new(0);

impl Port {
    /// Applies entry `index` of the stone's typical patterns: builds it on the stone's own
    /// color, matches its colors in the background and then applies it. The error is the
    /// sentence for why it cannot start; a later failure is reported as a toast.
    pub(super) fn apply_typical(&self, ui: &MainWindow, index: usize) -> Result<(), String> {
        let target = self.capture()?;
        let entry = *typical_patterns(&target.material.name)
            .get(index)
            .ok_or_else(|| "This material has no such typical zoning.".to_owned())?;
        let width = effective_width(target.width);
        // The species' color variants that fit this stone: a zone whose color is one of them (the
        // rubellite core of a tourmaline, the yellow rim of a parti-color sapphire) takes it.
        // The cutter's saved colors of this material come from the main window's vault.
        let vault = registered_vault();
        let variants = zone_variants(&target.material, vault.as_deref());
        // The stone's own color per direction: a pleochroic stone never gets a one-color zone.
        let own = self.own_color(&target, width, &variants)?;
        start(
            self,
            ui,
            target,
            entry.for_stone_on(&own, &variants),
            reference_path_mm(Some(width)),
        );
        Ok(())
    }
}

/// Starts matching the colors of `pattern` and remembers what to apply them to.
fn start(port: &Port, ui: &MainWindow, target: Target, pattern: ColorPattern, path_mm: f64) {
    let ticket = TICKET.fetch_add(1, Ordering::SeqCst) + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    let previous = PENDING.with(|slot| {
        slot.borrow_mut().replace(Pending {
            ticket,
            port: port.clone(),
            expected: RefCell::new(target),
            cancel: Arc::clone(&cancel),
        })
    });
    if let Some(previous) = previous {
        previous.cancel.store(true, Ordering::SeqCst);
    }
    ui.global::<Zoning>().set_typical_pattern_busy(true);
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let solved = solve_pattern_colors(pattern, path_mm, &cancel);
        let _ = weak.upgrade_in_event_loop(move |ui| finish(&ui, ticket, solved));
    });
}

/// The colors are matched (`None`: they could not be): applies the pattern to the stone it was
/// made for.
fn finish(ui: &MainWindow, ticket: u64, solved: Option<ColorPattern>) {
    let pending = PENDING.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot
            .as_ref()
            .is_some_and(|pending| pending.ticket == ticket)
        {
            slot.take()
        } else {
            None
        }
    });
    // A superseded request: the newer one is still busy and reports for itself.
    let Some(pending) = pending else {
        return;
    };
    ui.global::<Zoning>().set_typical_pattern_busy(false);
    let result = match solved {
        Some(pattern) => {
            let through_table = wants_axis_through_table(&pattern);
            pending
                .port
                .apply(&pending.expected, Some(pattern))
                .map(|()| through_table)
        }
        None => Err("The colors of this zoning could not be matched.".to_owned()),
    };
    match result {
        // A watermelon is cut as a slice across c: its rings show face up only with the Crystal
        // Axis through the table (with a tourmaline's own axis in the table plane, pink and green
        // blend to orange). The same step as the Through the Table preset, after the pattern.
        Ok(true) => ui
            .global::<CrystalAxisModel>()
            .invoke_geometry_preset_chosen(0),
        Ok(false) => {}
        Err(message) => show_toast(ui, &message, "warning"),
    }
    ui.global::<ViewportModel>().invoke_render_color_refresh();
    pending.port.present(ui);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::render_thread::PlanesOwner;
    use pleochroma::optics::materials::GemMaterial;
    use pleochroma_cut_core::{
        MaterialSelection,
        material::pattern::{TemplateId, apply_color_pattern},
    };

    fn target(name: &str, pattern: Option<ColorPattern>) -> Target {
        let material = GemMaterial::by_name(name).expect("built in");
        Target {
            owner: PlanesOwner::Catalogue { entry_id: 1 },
            rendered: material.clone(),
            material,
            custom: Arc::default(),
            width: 7.0,
            design: None,
            pattern,
        }
    }

    fn from_template(name: &str, index: usize) -> ColorPattern {
        let mut pattern = typical_patterns(name)[index].for_stone([40.0, 45.0, 265.0], &[]);
        for at in pattern.unsolved_colors() {
            assert!(pattern.set_color_bands(at, vec![[500.0, 40.0, 0.2]]));
        }
        pattern
    }

    #[test]
    fn the_variants_of_a_sapphire_make_its_parti_color_rim_a_variant() {
        let sapphire = GemMaterial::by_name("Sapphire").expect("built in");
        let variants = zone_variants(&sapphire, None);
        assert!(
            variants
                .iter()
                .any(|variant| variant.key == "sapphire.golden_yellow")
        );
        let entry = typical_patterns("Sapphire")[1];
        assert_eq!(entry.template, TemplateId::PartiColor);
        let pattern = entry.for_stone([40.0, 45.0, 265.0], &variants);
        // The rim takes the Golden Yellow first (2026-11-03; the Yellow is its fallback).
        assert_eq!(
            pattern.base.color.variant_key(),
            Some("sapphire.golden_yellow")
        );
        // The blue core is the Sapphire's Material Default, so nothing is left to solve.
        assert_eq!(
            pattern.zones[0].color.variant_key(),
            Some("sapphire.default")
        );
        assert!(pattern.unsolved_colors().is_empty());
    }

    #[test]
    fn a_species_with_typical_zoning_lists_it_and_marks_the_entry_the_pattern_came_from() {
        let plain = state_of(&target("Sapphire", None));
        assert_eq!(plain.species, "Sapphire");
        assert_eq!(
            plain.labels,
            ["Hexagonal Banding", "Parti Color", "Color Band"]
        );
        assert_eq!(plain.index, -1);

        for (index, expected) in [(0, 0), (1, 1), (2, 2)] {
            let state = state_of(&target("Sapphire", Some(from_template("Sapphire", index))));
            assert_eq!(state.index, expected);
        }
    }

    #[test]
    fn a_hand_made_pattern_or_another_species_marks_nothing() {
        // A watermelon is not a sapphire entry; no template at all is a hand-made pattern.
        let mut other = from_template("Tourmaline", 0);
        assert_eq!(other.template, Some(TemplateId::Watermelon));
        assert_eq!(state_of(&target("Sapphire", Some(other.clone()))).index, -1);
        other.template = None;
        assert_eq!(state_of(&target("Tourmaline", Some(other))).index, -1);
        // The built-in zoned Ametrine has zones of its own but no override, so no entry is marked.
        let ametrine = state_of(&target("Ametrine", None));
        assert_eq!(ametrine.labels, ["Split", "Sectors"]);
        assert_eq!(ametrine.index, -1);
    }

    #[test]
    fn a_species_without_typical_zoning_offers_nothing() {
        let state = state_of(&target("Diamond", Some(from_template("Sapphire", 0))));
        assert_eq!(state, TypicalState::default());
        assert_eq!(state.index, -1);
    }

    #[test]
    fn the_own_color_of_a_stone_ignores_the_pattern_it_has() {
        let pattern = from_template("Sapphire", 0);
        let plain = GemMaterial::sapphire();
        let zoned = apply_color_pattern(plain.clone(), &pattern, 7.0).expect("applies");
        assert!(zoned.zoning.is_some());

        // A design: its selection is resolved again with the pattern dropped.
        let mut design = target("Sapphire", Some(pattern.clone()));
        design.material = zoned.clone();
        design.rendered = zoned.clone();
        design.design = Some(super::super::DesignTarget {
            epoch: 1,
            generation: 1,
            material: MaterialSelection::none().with_color_pattern(Some(pattern.clone())),
        });
        let own = design.own_material();
        assert!(
            own.zoning.is_none(),
            "the pattern is not part of the own color"
        );
        assert_eq!(own.name, "Sapphire");

        // A preview with a pattern: the untinted source. Without one: what it renders.
        let mut preview = target("Sapphire", Some(pattern));
        preview.rendered = zoned;
        assert_eq!(preview.own_material(), preview.material);
        let mut bare = target("Sapphire", None);
        bare.rendered = plain.with_body_color([0.1, 0.2, 0.4]);
        assert_eq!(bare.own_material(), bare.rendered);
    }
}
