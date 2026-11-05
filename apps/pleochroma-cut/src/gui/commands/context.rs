//! Which commands the palette offers on the page on screen.
//!
//! The palette used to list every command everywhere, so the Live Render offered Add Tier,
//! Solve, the inspector tabs and the Edit view's modes, none of which does anything there.
//! Each command now belongs to the pages where it makes sense, and the palette lists only the
//! commands of the page on screen. Within that list a command that cannot run right now (no
//! tier selected, nothing to undo) still shows, greyed with its reason, as before.
//!
//! The rule is [`pages_of`]: a few groups by command id, then the category (every tier and
//! solve command is a designing command). The guide's own runs of a command
//! (`commands::run_unlocked`) do not go through it.

use super::{
    registry::{Category, Command},
    state::{CommandState, Fact},
};

/// A page of the main window, as far as the palette is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// The 3D tab's Edit view: the design editor.
    Edit,
    /// The 3D tab's Live Render.
    LiveRender,
    /// The Cutting Instructions tab.
    CuttingTable,
    /// The Files & Downloads tab.
    Files,
}

impl Page {
    /// The page `state` was captured on.
    #[must_use]
    pub fn of(state: &CommandState) -> Self {
        if state.has(Fact::CuttingTablePage) {
            Self::CuttingTable
        } else if state.has(Fact::FilesPage) {
            Self::Files
        } else if state.has(Fact::LiveRenderView) {
            Self::LiveRender
        } else {
            Self::Edit
        }
    }
}

const ALL: &[Page] = &[
    Page::Edit,
    Page::LiveRender,
    Page::CuttingTable,
    Page::Files,
];
const EDIT: &[Page] = &[Page::Edit];
const LIVE: &[Page] = &[Page::LiveRender];
const THREE_D: &[Page] = &[Page::Edit, Page::LiveRender];
const EDIT_AND_CUTTING: &[Page] = &[Page::Edit, Page::CuttingTable];
const BUT_3D: &[Page] = &[Page::CuttingTable, Page::Files];
const BUT_LIVE: &[Page] = &[Page::Edit, Page::CuttingTable, Page::Files];
const BUT_EDIT: &[Page] = &[Page::LiveRender, Page::CuttingTable, Page::Files];
const BUT_CUTTING: &[Page] = &[Page::Edit, Page::LiveRender, Page::Files];
const BUT_FILES: &[Page] = &[Page::Edit, Page::LiveRender, Page::CuttingTable];

/// The commands that stop something running: offered on every page, but only while it runs.
const STOPS: &[&str] = &[
    "solve.abandon",
    "solve.deep_cancel",
    "solve.optimize_cancel",
];

/// The designing commands outside the Tiers and Solve groups: the editor's history, variants,
/// view modes, inspector tabs and tools.
fn designing(id: &str) -> bool {
    matches!(
        id,
        "edit.undo"
            | "edit.redo"
            | "history.keep_trial"
            | "file.save_variant"
            | "tools.angle_sweep"
            | "view.solid"
            | "view.path_traced"
            | "view.ray_traced"
            | "view.both"
            | "view.diagram"
            | "view.slice"
            | "view.snap"
    ) || id.starts_with("view.inspector_")
}

/// The pages `command` is offered on.
#[must_use]
pub fn pages_of(command: &Command) -> &'static [Page] {
    match command.id {
        // Going to the page already on screen does nothing.
        "view.tab_3d" => BUT_3D,
        "view.live_render" => BUT_LIVE,
        "view.edit" => BUT_EDIT,
        "view.tab_cutting" => BUT_CUTTING,
        "view.tab_files" => BUT_FILES,
        // The Live Render's own display modes.
        "view.live_solid" | "view.live_traced" | "view.live_ray_traced" => LIVE,
        // The render controls of the two 3D views.
        "tools.stone_properties"
        | "tools.lighting_environment"
        | "tools.render_output"
        | "tools.render_settings"
        | "tools.export_render" => THREE_D,
        // The design's instructions, which the Cutting Instructions tab shows too.
        "edit.edit_as_text" | "tools.cutting_mode" | "file.export_diagram" => EDIT_AND_CUTTING,
        id if STOPS.contains(&id) => ALL,
        id if designing(id) => EDIT,
        _ => match command.category {
            Category::Tiers | Category::Solve => EDIT,
            Category::File | Category::Edit | Category::View | Category::Tools | Category::Help => {
                ALL
            }
        },
    }
}

/// Whether the palette lists `command` in `state`: it belongs to the page on screen, and a
/// command that stops something is listed only while that runs.
#[must_use]
pub fn offered(command: &Command, state: &CommandState) -> bool {
    pages_of(command).contains(&Page::of(state))
        && (!STOPS.contains(&command.id) || command.availability(state).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::commands::table::COMMANDS;

    fn command(id: &str) -> &'static Command {
        COMMANDS
            .iter()
            .find(|command| command.id == id)
            .unwrap_or_else(|| panic!("no command {id}"))
    }

    /// The ready state (a design open, everything possible) on `page`.
    fn on(page: Page) -> CommandState {
        CommandState::ready()
            .with(Fact::EditView, page == Page::Edit)
            .with(Fact::LiveRenderView, page == Page::LiveRender)
            .with(Fact::CuttingTablePage, page == Page::CuttingTable)
            .with(Fact::FilesPage, page == Page::Files)
    }

    #[test]
    fn the_page_follows_the_captured_facts() {
        for page in [
            Page::Edit,
            Page::LiveRender,
            Page::CuttingTable,
            Page::Files,
        ] {
            assert_eq!(Page::of(&on(page)), page);
        }
    }

    /// The owner's report: the Live Render offered the designing commands.
    #[test]
    fn the_live_render_offers_no_designing_command() {
        let live = on(Page::LiveRender);
        for id in [
            "tiers.add",
            "tiers.delete",
            "solve.run",
            "solve.optimize",
            "solve.retarget",
            "edit.undo",
            "view.solid",
            "view.diagram",
            "view.inspector_tier",
            "view.slice",
            "tools.angle_sweep",
            "view.live_render",
        ] {
            assert!(!offered(command(id), &live), "{id} on the Live Render");
        }
        for id in [
            "view.live_traced",
            "tools.render_settings",
            "tools.export_render",
            "file.save",
            "help.manual",
            "view.edit",
            "tools.plan_rough",
        ] {
            assert!(offered(command(id), &live), "{id} on the Live Render");
        }
    }

    #[test]
    fn the_editor_offers_the_designing_commands_and_not_the_live_modes() {
        let edit = on(Page::Edit);
        for id in [
            "tiers.add",
            "solve.run",
            "edit.undo",
            "view.solid",
            "tools.render_settings",
        ] {
            assert!(offered(command(id), &edit), "{id} in the editor");
        }
        for id in ["view.live_solid", "view.edit"] {
            assert!(!offered(command(id), &edit), "{id} in the editor");
        }
    }

    #[test]
    fn the_cutting_table_offers_the_instructions_but_no_tier_or_render_command() {
        let cutting = on(Page::CuttingTable);
        for id in [
            "edit.edit_as_text",
            "tools.cutting_mode",
            "file.export_cutting_sheet",
            "file.export_cutting_sheet_pdf",
        ] {
            assert!(offered(command(id), &cutting), "{id} on the cutting table");
        }
        for id in ["tiers.add", "tools.render_settings", "view.tab_cutting"] {
            assert!(!offered(command(id), &cutting), "{id} on the cutting table");
        }
    }

    #[test]
    fn a_stop_command_is_listed_on_every_page_only_while_its_work_runs() {
        for page in [Page::Edit, Page::LiveRender, Page::Files] {
            let running = on(page);
            assert!(
                offered(command("solve.optimize_cancel"), &running),
                "{page:?}"
            );
            let idle = running.with(Fact::OptimizeRunning, false);
            assert!(
                !offered(command("solve.optimize_cancel"), &idle),
                "{page:?}"
            );
        }
    }

    #[test]
    fn every_command_is_offered_somewhere() {
        for command in COMMANDS {
            assert!(
                !pages_of(command).is_empty(),
                "{} is offered nowhere",
                command.id
            );
        }
    }
}
