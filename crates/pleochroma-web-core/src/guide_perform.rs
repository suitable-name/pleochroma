//! What Next does for the learner in the browser, without the window.
//!
//! A lesson step may carry a recipe (`pleochroma_editor::guide::Perform`) that names what the
//! learner would do, in the desktop's words. [`plan`] turns one into the [`Atom`]s the page
//! carries out through its own callbacks, one after the other. A recipe the page cannot do
//! comes back as a [`Gap`], and its step keeps the Skip Step button.
//!
//! The plan is all or nothing: a gap anywhere in a sequence refuses the whole recipe, so a step
//! never ends half done. Nothing here reads the design. The page resolves a tier name to a row
//! when it runs the atom, because an earlier atom of the same recipe may have moved the rows.

use pleochroma_cut_core::{
    FreshDesignSpec,
    design::{ConcaveTier, ConcaveTool},
};
use pleochroma_editor::{
    guide::{
        CMD_ADOPT_ALL, CMD_COMPARE, CMD_DELETE, CMD_DUPLICATE, CMD_MOVE_DOWN, CMD_MOVE_UP,
        CMD_SOLVE, CMD_UNDO, Perform, StepSeries, TierEntry, viewing_events,
    },
    loading::{ConcaveTierFormFields, concave_tier_form_fields},
};
use std::fmt;

/// A palette command of the desktop's, as the browser has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// The Solve button.
    Solve,
    /// Undo.
    Undo,
    /// Duplicate the selected tier.
    Duplicate,
    /// Delete the selected tier.
    Delete,
    /// Move the selected tier one place earlier.
    MoveUp,
    /// Move the selected tier one place later.
    MoveDown,
    /// The tier table's Adopt all.
    AdoptAll,
    /// Design > Snapshot design.
    Snapshot,
    /// Design > Compare to snapshot.
    Compare,
    /// The Render tab.
    LiveRender,
    /// The Solid view's Snap pill.
    Snap,
    /// The Solid view's Slice pill.
    Slice,
    /// The keyboard shortcuts list.
    Shortcuts,
    /// The manual.
    Manual,
}

impl Command {
    /// Every command, in the order of the desktop's table.
    pub const ALL: [Self; 14] = [
        Self::Solve,
        Self::Undo,
        Self::Duplicate,
        Self::Delete,
        Self::MoveUp,
        Self::MoveDown,
        Self::AdoptAll,
        Self::Snapshot,
        Self::Compare,
        Self::LiveRender,
        Self::Snap,
        Self::Slice,
        Self::Shortcuts,
        Self::Manual,
    ];

    /// The id the shared recipes use (`pleochroma_editor::guide::PERFORM_COMMANDS`).
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Solve => CMD_SOLVE,
            Self::Undo => CMD_UNDO,
            Self::Duplicate => CMD_DUPLICATE,
            Self::Delete => CMD_DELETE,
            Self::MoveUp => CMD_MOVE_UP,
            Self::MoveDown => CMD_MOVE_DOWN,
            Self::AdoptAll => CMD_ADOPT_ALL,
            Self::Snapshot => "solve.snapshot",
            Self::Compare => CMD_COMPARE,
            Self::LiveRender => "view.live_render",
            Self::Snap => "view.snap",
            Self::Slice => "view.slice",
            Self::Shortcuts => "help.shortcuts",
            Self::Manual => "help.manual",
        }
    }

    /// The command with this id.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|command| command.id() == id)
    }

    /// The lesson event the page itself reports once it has done this command, for the three
    /// commands that are plain view toggles with nothing to wait for.
    ///
    /// The other commands report from the place their action succeeds (a solve finishing, a
    /// snapshot being taken), so the executor leaves those events to that place.
    #[must_use]
    pub const fn reported_event(self) -> Option<&'static str> {
        match self {
            Self::LiveRender => Some(viewing_events::LIVE_RENDER_OPENED),
            Self::Snap => Some(viewing_events::SNAP_TOGGLED),
            Self::Slice => Some(viewing_events::SLICE_STARTED),
            _ => None,
        }
    }

    /// Whether the page can carry it out from Rust.
    ///
    /// The user guide is a Slint dialog whose `open` flag only Slint reaches today
    /// (`UserGuideModel` is not in `app.slint`'s export block); name it there and this turns
    /// `true`. The shortcuts list (`ShortcutsModel`) is exported.
    #[must_use]
    pub const fn in_reach(self) -> bool {
        !matches!(self, Self::Manual)
    }
}

/// Why the browser cannot do a recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gap {
    /// A step series whose rungs are linked: the browser has no relations.
    LinkedSteps,
    /// Removing a relation between tiers.
    Relation,
    /// A Solid view with a tracer over it (view mode 2 or 5): the browser has none.
    SolidWithTracer(i32),
    /// A dialog the page cannot open from Rust yet.
    Unreachable(Command),
    /// A recipe naming something that does not exist.
    Unknown(String),
}

impl fmt::Display for Gap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LinkedSteps => {
                f.write_str("linked step series need relations, which the browser does not have")
            }
            Self::Relation => f.write_str("relations between tiers are not in the browser"),
            Self::SolidWithTracer(mode) => write!(
                f,
                "view mode {mode} (Solid with a tracer over it) is not in the browser"
            ),
            Self::Unreachable(command) => write!(
                f,
                "the browser cannot open that dialog for you yet ({})",
                command.id()
            ),
            Self::Unknown(what) => f.write_str(what),
        }
    }
}

/// Why the page stopped carrying out a recipe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The control that refused it already told the learner (a toast, a message under the field).
    Reported,
    /// Not yet told: say it.
    Say(String),
}

impl Stop {
    /// A stop that has not been reported yet, with the words to report it in.
    #[must_use]
    pub fn say(message: impl Into<String>) -> Self {
        Self::Say(message.into())
    }
}

/// The concave form's twelve fields, as the inspector's draft holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConcaveForm {
    /// The fields as text, exactly as `parse_concave_tier_form` reads them back.
    pub fields: ConcaveTierFormFields,
    /// The tool combo's index (0 CYL, 1 CON, 2 CIR, 3 DSC, 4 SPH).
    pub tool_index: i32,
}

/// The combo index of the tool with this code (the desktop's order); 0 for an unknown code.
#[must_use]
pub fn tool_index(code: &str) -> i32 {
    ConcaveTool::ALL
        .iter()
        .position(|tool| tool.code().eq_ignore_ascii_case(code.trim()))
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

/// The form `tier` fills in when a lesson types it.
#[must_use]
pub fn concave_form(tier: &ConcaveTier) -> ConcaveForm {
    let fields = concave_tier_form_fields(tier);
    let tool_index = tool_index(&fields.tool);
    ConcaveForm { fields, tool_index }
}

/// One thing the page does for the learner.
#[derive(Clone, Debug, PartialEq)]
pub enum Atom {
    /// A new design from the New Design dialog's Empty form. The unsaved-changes question
    /// comes first, so it may finish later than the atoms before it.
    NewDesign {
        /// The dialog's "Start From" card (0 is Empty).
        template_index: i32,
        /// The design the form describes.
        spec: Box<FreshDesignSpec>,
    },
    /// The Tier form's Add Tier or Save Tier.
    Tier(TierEntry),
    /// The concave form's Add or Save.
    Concave {
        /// The concave row to pick first; `None` is "+ Add Concave Tier".
        edit: Option<String>,
        /// The tier, typed in.
        form: Box<ConcaveForm>,
    },
    /// Picks the row called this.
    Select(String),
    /// The Steps panel's Generate (never linked: that is a [`Gap`]).
    Steps(StepSeries),
    /// "Mirror to other block" on a tier.
    Mirror {
        /// The tier to mirror.
        tier: String,
        /// The name suffix.
        suffix: String,
    },
    /// The Tier form's Note field and Set button.
    Note {
        /// The tier the note belongs to.
        tier: String,
        /// The note; blank clears it.
        text: String,
    },
    /// The Tier form's Cheater Offset field and Set button.
    Cheater {
        /// The tier the offset belongs to.
        tier: String,
        /// The offset in degrees; blank clears it.
        text: String,
    },
    /// Apply Material with this material picked.
    Material(String),
    /// The Preform tab's Girdle Diameter and Apply Yield Inputs.
    Yield {
        /// The girdle diameter in millimetres.
        girdle_diameter_mm: f64,
    },
    /// The view the shared view mode (0 Solid, 1 Path, 3 Diagram, 4 Ray) stands for.
    ViewMode(i32),
    /// The inspector's tab (0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History).
    InspectorTab(i32),
    /// A palette command.
    Command(Command),
}

/// The atoms that carry out `perform`, in order.
///
/// # Errors
///
/// The first [`Gap`] in the recipe: the whole recipe is refused, never a part of it.
pub fn plan(perform: &Perform) -> Result<Vec<Atom>, Gap> {
    let mut atoms = Vec::new();
    collect(perform, &mut atoms)?;
    Ok(atoms)
}

fn collect(perform: &Perform, out: &mut Vec<Atom>) -> Result<(), Gap> {
    match perform {
        Perform::NewDesign {
            template_index,
            spec,
        } => out.push(Atom::NewDesign {
            template_index: *template_index,
            spec: spec.clone(),
        }),
        Perform::Tier(entry) => out.push(Atom::Tier(entry.clone())),
        Perform::ConcaveTier { edit, tier } => out.push(Atom::Concave {
            edit: edit.clone(),
            form: Box::new(concave_form(tier)),
        }),
        Perform::SelectTier(name) => out.push(Atom::Select(name.clone())),
        Perform::Steps(series) => {
            if series.linked {
                return Err(Gap::LinkedSteps);
            }
            out.push(Atom::Steps(series.clone()));
        }
        Perform::MirrorTier { tier, suffix } => out.push(Atom::Mirror {
            tier: tier.clone(),
            suffix: suffix.clone(),
        }),
        Perform::ClearRelation(_) => return Err(Gap::Relation),
        Perform::TierNote { tier, text } => out.push(Atom::Note {
            tier: tier.clone(),
            text: text.clone(),
        }),
        Perform::CheaterOffset { tier, text } => out.push(Atom::Cheater {
            tier: tier.clone(),
            text: text.clone(),
        }),
        Perform::Material(name) => out.push(Atom::Material(name.clone())),
        Perform::Yield { girdle_diameter_mm } => out.push(Atom::Yield {
            girdle_diameter_mm: *girdle_diameter_mm,
        }),
        Perform::ViewMode(mode) => out.push(view_mode(*mode)?),
        Perform::InspectorTab(tab) => {
            if !(0..=4).contains(tab) {
                return Err(Gap::Unknown(format!("inspector tab {tab} does not exist")));
            }
            out.push(Atom::InspectorTab(*tab));
        }
        Perform::Command(id) => {
            let command = Command::from_id(id)
                .ok_or_else(|| Gap::Unknown(format!("{id:?} is not a command the page knows")))?;
            if !command.in_reach() {
                return Err(Gap::Unreachable(command));
            }
            out.push(Atom::Command(command));
        }
        Perform::Sequence(parts) => {
            for part in parts {
                collect(part, out)?;
            }
        }
    }
    Ok(())
}

fn view_mode(mode: i32) -> Result<Atom, Gap> {
    match mode {
        0 | 1 | 3 | 4 => Ok(Atom::ViewMode(mode)),
        2 | 5 => Err(Gap::SolidWithTracer(mode)),
        _ => Err(Gap::Unknown(format!("view mode {mode} does not exist"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::design::ToolMotion;
    use pleochroma_editor::{
        guide::{EVENTS, GuideCatalog, PERFORM_COMMANDS, WebFeatures, lesson_start_spec},
        loading::parse_concave_tier_form,
    };

    #[test]
    fn a_stop_says_what_it_was_given() {
        assert!(matches!(Stop::say("no"), Stop::Say(text) if text == "no"));
        assert_ne!(Stop::say("no"), Stop::Reported);
    }

    fn groove(tool: ConcaveTool, tool_angle_deg: Option<f64>, motion: ToolMotion) -> ConcaveTier {
        ConcaveTier {
            name: "Groove".to_owned(),
            angle_deg: -42.0,
            indices: vec![0.0, 24.0, 48.0, 72.0],
            instructions: String::new(),
            tool,
            tool_azimuth_deg: 0.0,
            displacement: [0.0; 3],
            diameter_ratio: 0.25,
            tool_angle_deg,
            motion,
        }
    }

    #[test]
    fn a_sequence_is_flattened_in_order() {
        let recipe = Perform::select("Crown Main").then(Perform::command(CMD_DUPLICATE));
        let atoms = plan(&recipe).expect("a plain sequence plans");
        assert_eq!(
            atoms,
            [
                Atom::Select("Crown Main".to_owned()),
                Atom::Command(Command::Duplicate)
            ]
        );
        let moved = plan(&Perform::move_up("Girdle", 2)).expect("a move plans");
        assert_eq!(moved.len(), 3, "the pick, then one move per place");
        assert_eq!(moved[2], Atom::Command(Command::MoveUp));
    }

    #[test]
    fn every_shared_command_has_a_browser_command_and_back() {
        for id in PERFORM_COMMANDS {
            let command = Command::from_id(id)
                .unwrap_or_else(|| panic!("the browser does not know the command {id:?}"));
            assert_eq!(command.id(), *id);
        }
        assert_eq!(
            Command::ALL.len(),
            PERFORM_COMMANDS.len(),
            "a command the shared list lacks"
        );
        assert_eq!(Command::from_id("no.such.command"), None);
    }

    #[test]
    fn only_the_plain_view_toggles_report_their_own_event() {
        let reporting: Vec<Command> = Command::ALL
            .into_iter()
            .filter(|command| command.reported_event().is_some())
            .collect();
        assert_eq!(
            reporting,
            [Command::LiveRender, Command::Snap, Command::Slice]
        );
        for command in reporting {
            let name = command.reported_event().expect("filtered above");
            assert!(EVENTS.contains(&name), "{name} is not a lesson event");
        }
    }

    #[test]
    fn a_recipe_the_page_cannot_do_is_a_named_gap() {
        let series = |linked| {
            Perform::Steps(StepSeries {
                name: "Step".to_owned(),
                start: "30".to_owned(),
                step: "4".to_owned(),
                count: 3,
                indices: "0:12:96".to_owned(),
                anchor: String::new(),
                linked,
            })
        };
        assert!(plan(&series(false)).is_ok());
        assert_eq!(plan(&series(true)), Err(Gap::LinkedSteps));
        assert_eq!(
            plan(&Perform::ClearRelation("Step3".to_owned())),
            Err(Gap::Relation)
        );
        assert_eq!(plan(&Perform::ViewMode(2)), Err(Gap::SolidWithTracer(2)));
        assert_eq!(plan(&Perform::ViewMode(5)), Err(Gap::SolidWithTracer(5)));
        assert!(matches!(plan(&Perform::ViewMode(9)), Err(Gap::Unknown(_))));
        assert!(matches!(
            plan(&Perform::InspectorTab(5)),
            Err(Gap::Unknown(_))
        ));
        assert!(matches!(
            plan(&Perform::command("no.such.command")),
            Err(Gap::Unknown(_))
        ));
        assert_eq!(
            plan(&Perform::command("help.shortcuts")),
            Ok(vec![Atom::Command(Command::Shortcuts)])
        );
        assert_eq!(
            plan(&Perform::command(Command::Manual.id())),
            Err(Gap::Unreachable(Command::Manual))
        );
        for view in [0, 1, 3, 4] {
            assert_eq!(
                plan(&Perform::ViewMode(view)),
                Ok(vec![Atom::ViewMode(view)])
            );
        }
        for tab in 0..=4 {
            assert!(plan(&Perform::InspectorTab(tab)).is_ok());
        }
    }

    #[test]
    fn a_gap_anywhere_refuses_the_whole_recipe() {
        let recipe = Perform::solve()
            .then(Perform::ViewMode(2))
            .then(Perform::undo());
        assert_eq!(plan(&recipe), Err(Gap::SolidWithTracer(2)));
    }

    #[test]
    fn every_gap_says_something_to_the_learner() {
        for gap in [
            Gap::LinkedSteps,
            Gap::Relation,
            Gap::SolidWithTracer(2),
            Gap::Unreachable(Command::Manual),
            Gap::Unknown("a thing".to_owned()),
        ] {
            let text = gap.to_string();
            assert!(!text.is_empty() && !text.ends_with('.'), "{text:?}");
        }
    }

    #[test]
    fn a_new_design_keeps_its_card_and_spec() {
        let spec = lesson_start_spec(0);
        let recipe = Perform::NewDesign {
            template_index: 0,
            spec: Box::new(spec.clone()),
        };
        assert_eq!(
            plan(&recipe),
            Ok(vec![Atom::NewDesign {
                template_index: 0,
                spec: Box::new(spec)
            }])
        );
    }

    #[test]
    fn the_tool_index_follows_the_combo_order() {
        for (at, tool) in ConcaveTool::ALL.iter().enumerate() {
            assert_eq!(
                tool_index(tool.code()),
                i32::try_from(at).expect("a handful of tools")
            );
            assert_eq!(
                tool_index(&tool.code().to_lowercase()),
                tool_index(tool.code())
            );
        }
        assert_eq!(tool_index("CYL"), 0);
        assert_eq!(tool_index("nonsense"), 0);
    }

    #[test]
    fn a_concave_tier_fills_the_form_with_the_texts_the_form_reads_back() {
        let wheel = groove(ConcaveTool::Disc, Some(90.0), ToolMotion::Plunge);
        let form = concave_form(&wheel);
        assert_eq!(form.fields.name, "Groove");
        assert_eq!(form.fields.tool, "DSC");
        assert_eq!(form.tool_index, tool_index("DSC"));
        assert!(!form.fields.reciprocating);
        assert_eq!(form.fields.tool_angle_deg, "90");
        // Saved as the form saves it, the draft makes the same tier back.
        let parsed = parse_concave_tier_form(&form.fields, 96).expect("the draft parses");
        assert_eq!(parsed, wheel);
        let plain = concave_form(&groove(
            ConcaveTool::Cylinder,
            None,
            ToolMotion::Reciprocating,
        ));
        assert_eq!(plain.tool_index, 0);
        assert!(plain.fields.reciprocating);
        assert!(plain.fields.tool_angle_deg.is_empty());
    }

    #[test]
    fn every_recipe_of_every_offered_lesson_plans_or_names_a_dialog_gap() {
        let mut with_recipe = 0_usize;
        for features in [
            WebFeatures::ALL,
            WebFeatures {
                library: false,
                ..WebFeatures::ALL
            },
        ] {
            for guide in GuideCatalog::new().web_guides(features) {
                for step in &guide.steps {
                    let Some(recipe) = &step.perform else {
                        continue;
                    };
                    with_recipe += 1;
                    match plan(recipe) {
                        Ok(atoms) => assert!(
                            !atoms.is_empty(),
                            "{} / {}: an empty plan",
                            guide.id,
                            step.title
                        ),
                        // Only the user guide dialog is out of reach; every other gap would
                        // mean an offered lesson asks for something the browser lacks.
                        Err(Gap::Unreachable(_)) => {}
                        Err(gap) => panic!("{} / {}: {gap}", guide.id, step.title),
                    }
                }
            }
        }
        assert!(with_recipe > 0, "no offered lesson has a recipe");
    }
}
