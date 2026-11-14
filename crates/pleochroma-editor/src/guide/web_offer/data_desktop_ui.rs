//! Web wording for lessons whose desktop text follows the redesigned desktop screens.
//!
//! The desktop's tier form has no Save Tier or Apply buttons, its tier list has row menus and an
//! Add Tier menu, and its Solve button holds the Auto-Solve menu. The browser build keeps the
//! older controls, so each edit below puts the older wording back. They run after the lesson's
//! own overlay (`data_*`), and every `from` text is the desktop line word for word; the test that
//! applies the overlays to the catalogue reports one that no longer matches.

use super::overlay::{GuideOverlay, StepOverlay, TextEdit};

/// The tier-list wording: the desktop says list, the browser says table.
fn list_is_table() -> TextEdit {
    TextEdit::sub("tier list", "tier table")
}

/// A field that applies on Enter on the desktop is saved with a button on the web.
fn enter_is_save() -> TextEdit {
    TextEdit::sub(" and press Enter.", " and click Save Tier.")
}

/// The same for a clause that follows a comma.
fn then_enter_is_save() -> TextEdit {
    TextEdit::sub(", then press Enter.", ", then click Save Tier.")
}

/// The desktop's Meets segment names.
fn segment_note(note: &str) -> TextEdit {
    TextEdit::sub(note, "")
}

/// The auto-solve wording of the two Auto-Solve lines the desktop shares between lessons.
fn auto_solve_edits() -> Vec<TextEdit> {
    vec![
        TextEdit::line(
            "Open the chevron on the right of the Solve button (the Auto-Solve menu) and choose Auto-Solve Off.",
            "Open the Auto list next to the Solve button and choose Off.",
        ),
        TextEdit::line(
            "If it already says Off, choose Auto-Solve 1 s and then Auto-Solve Off again.",
            "If it already says Off, choose 1 s and then Off again.",
        ),
        TextEdit::line("the Auto-Solve menu marks Off.", "the Auto list reads Off."),
        TextEdit::line("the Auto-Solve menu set to Off", "the Auto list set to Off"),
    ]
}

/// The overlays of this file.
pub(super) fn overlays() -> Vec<GuideOverlay> {
    vec![
        add_a_tier(),
        edit_a_tier(),
        index_shorthands(),
        meets_named(),
        meets_mm(),
        adopt_imported(),
        pin_to_mast(),
        arithmetic(),
        tier_notes(),
        cheater_offset(),
        tier_move(),
        multi_select(),
        concave_cylinder_cone(),
        concave_sphere_disc(),
        solve(),
        auto_solve(),
        status(),
        verdict_fixes(),
        preform(),
        yield_and_carat(),
        walkthrough(),
        output_save(),
        cutting_mode(),
    ]
}

fn add_a_tier() -> GuideOverlay {
    GuideOverlay::new("tiers-add-a-tier")
        .edit(TextEdit::sub("Click Add This Tier.", "Click Add Tier."))
        .edit(TextEdit::sub(
            "Add This Tier writes a new row; when you pick an existing row, a change applies as you press Enter or leave the field.",
            "Add Tier writes a new row; Save Tier, shown when you pick a row, changes that row.",
        ))
        .edit(segment_note(" (Facets segment)"))
        .edit(list_is_table())
}

fn edit_a_tier() -> GuideOverlay {
    GuideOverlay::new("tiers-edit-a-tier")
        .edit(TextEdit::sub(
            "A change applies to that same row when you press Enter or leave the field; there is no Save button.",
            "Save Tier writes your changes back to that same row.",
        ))
        .edit(TextEdit::sub(
            "the Pavilion Main row shows 41.00 as its angle.",
            "the Pavilion Main row shows 41.00 in the ANGLE column.",
        ))
        .edit(TextEdit::sub(
            "says Tier # with the row number, so you know which row the change goes to.",
            "says Edit Tier with the row number, so you know which row Save Tier will change.",
        ))
        .edit(TextEdit::sub(
            "Editing a field changes the row you picked. + Add Tier starts a new row, which only appears when you click Add This Tier.",
            "Save Tier changes the row you picked. + Add Tier starts a new row.",
        ))
        .edit(TextEdit::sub(
            "the form then offers Add This Tier.",
            "the form title then says Add Tier.",
        ))
        .edit(TextEdit::sub(
            "changes you have not applied when you pick another row",
            "unsaved changes when you pick another row",
        ))
        .edit(enter_is_save())
        .edit(then_enter_is_save())
        .edit(segment_note(" (Facets segment)"))
        .edit(list_is_table())
        .step(StepOverlay::of("Edit, Add and Undo").retitle("Save, Add and Undo"))
}

fn index_shorthands() -> GuideOverlay {
    GuideOverlay::new("tiers-index-shorthands")
        .edit(TextEdit::sub("Click Add This Tier.", "Click Add Tier."))
        .edit(TextEdit::sub(
            "until Add This Tier or an applied edit succeeds.",
            "until Add Tier or Save Tier succeeds.",
        ))
        .edit(segment_note(" (Facets segment)"))
}

fn meets_named() -> GuideOverlay {
    GuideOverlay::new("tiers-meets-named-facets")
        .edit(TextEdit::sub(
            " (Vertex segment); it applies at once.",
            ", then click Save Tier.",
        ))
        .edit(segment_note(" (Facets segment)"))
        .edit(then_enter_is_save())
        .edit(enter_is_save())
}

fn meets_mm() -> GuideOverlay {
    GuideOverlay::new("tiers-meets-mm-targets")
        .edit(TextEdit::line(
            "Girdle Diameter (mm): 6.5, then press Enter (the value applies when you press Enter or leave the field).",
            "Girdle Diameter (mm): 6.5, then click Apply Yield Inputs.",
        ))
        .edit(TextEdit::sub(
            "Pick Scale (Exact Scale Value) or Facets again in Meets to remove a target.",
            "Choose Exact Scale Value or Named Facet(s) again in Meets to remove a target; saving does it for you.",
        ))
        .edit(segment_note(" (Depth segment)"))
        .edit(segment_note(" (Table segment)"))
        .edit(segment_note(" (Girdle segment)"))
        .edit(then_enter_is_save())
}

fn adopt_imported() -> GuideOverlay {
    GuideOverlay::new("tiers-adopt-imported-meets").edit(TextEdit::line(
        "Open the Tier List Menu above the tier list and click Adopt All.",
        "Click Adopt All above the tier table.",
    ))
}

fn pin_to_mast() -> GuideOverlay {
    GuideOverlay::new("tiers-pin-to-mast")
        .edit(segment_note(" (Facets segment)"))
        .edit(then_enter_is_save())
}

fn arithmetic() -> GuideOverlay {
    GuideOverlay::new("tiers-arithmetic")
        .edit(TextEdit::sub(
            "worked out when you press Enter or leave the field.",
            "worked out when you save.",
        ))
        .edit(segment_note(" (Scale segment)"))
        .edit(then_enter_is_save())
        .edit(enter_is_save())
}

fn tier_notes() -> GuideOverlay {
    GuideOverlay::new("tiers-tier-notes")
        .step(StepOverlay::of("Add a note").line(
            "Type check the meet here, then press Enter or click away.",
            "Type check the meet here, then click Set or press Enter.",
        ))
        .edit(TextEdit::sub(
            "A note applies when you leave the field; there is no save step for it.",
            "A note takes effect at once; there is no Save Tier step for it.",
        ))
}

fn cheater_offset() -> GuideOverlay {
    GuideOverlay::new("tiers-cheater-offset").edit(TextEdit::line(
        "Press Enter, or click away; there is no Set button.",
        "Click Set, or press Enter.",
    ))
}

fn tier_move() -> GuideOverlay {
    GuideOverlay::new("tiers-move")
        .edit(TextEdit::line(
            "Open the \u{2026} menu (Actions for Tier N) at the end of the Girdle row; it holds Move Up and Move Down.",
            "Find the up and down buttons at the end of the Girdle row.",
        ))
        .edit(TextEdit::line(
            "Click Move Up once, or pick the row and press Alt+Up.",
            "Click the up button once, or pick the row and press Alt+Up.",
        ))
}

fn multi_select() -> GuideOverlay {
    GuideOverlay::new("tiers-multi-select").edit(TextEdit::line(
        "Open the Tier List Menu above the tier list and click Select to show tick boxes, then tick Crown Main and Pavilion Main.",
        "Click Select above the tier table to show tick boxes, then tick Crown Main and Pavilion Main.",
    ))
}

fn concave_cylinder_cone() -> GuideOverlay {
    GuideOverlay::new("tiers-concave-cylinder-cone")
        .edit(TextEdit::line(
            "Tool: click the Cylinder tile",
            "Tool: Cylinder (CYL)",
        ))
        .edit(TextEdit::line(
            "Tool: click the Cone tile",
            "Tool: Cone (CON)",
        ))
        .edit(TextEdit::line(
            "Stroke: Back and Forth (Reciprocating).",
            "Tick Reciprocating (stroked back and forth).",
        ))
}

fn concave_sphere_disc() -> GuideOverlay {
    GuideOverlay::new("tiers-concave-sphere-disc")
        .edit(TextEdit::line(
            "Tool: click the Sphere tile",
            "Tool: Sphere (SPH)",
        ))
        .edit(TextEdit::line(
            "Tool: click the Disc tile",
            "Tool: Disc (DSC)",
        ))
        .edit(TextEdit::line(
            "Stroke: One Way (plunge).",
            "Leave Reciprocating unticked.",
        ))
        .edit(TextEdit::line(
            "Stroke: Back and Forth.",
            "Tick Reciprocating.",
        ))
        .edit(TextEdit::line(
            "Set Stroke to One Way.",
            "Untick Reciprocating.",
        ))
        .edit(TextEdit::sub(
            "so leave Stroke on One Way.",
            "so leave Reciprocating unticked.",
        ))
        .edit(TextEdit::sub(
            "so set Stroke to Back and Forth this time.",
            "so tick Reciprocating this time.",
        ))
}

fn solve() -> GuideOverlay {
    GuideOverlay::new("solving-solve")
        .edits(auto_solve_edits())
        .edit(TextEdit::line(
            "Open the chevron on the Solve button. Auto-Solve 300 ms is the usual choice: a small design then solves itself shortly after you stop typing.",
            "Open the Auto list. 300 ms is the usual choice: a small design then solves itself shortly after you stop typing.",
        ))
        .edit(TextEdit::line(
            "Choose Auto-Solve Off if you would rather press Solve yourself.",
            "Choose Off if you would rather press Solve yourself.",
        ))
        .edit(TextEdit::sub("such as editing a tier,", "such as Save Tier,"))
        .edit(enter_is_save())
        .edit(list_is_table())
}

fn auto_solve() -> GuideOverlay {
    GuideOverlay::new("solving-auto-solve")
        .edit(TextEdit::line(
            "Find the chevron on the right of the Solve button (the Auto-Solve menu): Auto-Solve Off, 150 ms, 300 ms, 1 s and 3 s.",
            "Find the Auto list next to the Solve button: Off, 150 ms, 300 ms, 1 s and 3 s.",
        ))
        .edit(TextEdit::line(
            "Open the Auto-Solve menu (the chevron on the Solve button) and choose Auto-Solve Off.",
            "Open the Auto list and choose Off.",
        ))
        .edit(TextEdit::line(
            "If it already says Off, choose Auto-Solve 1 s and then Auto-Solve Off again.",
            "If it already says Off, choose 1 s and then Off again.",
        ))
        .edit(TextEdit::line("the Auto-Solve menu marks Off.", "the Auto list reads Off."))
        .edit(TextEdit::line(
            "the Auto-Solve menu set to Off",
            "the Auto list set to Off",
        ))
        .edit(TextEdit::line(
            "Open the Auto-Solve menu (the chevron on the Solve button) and choose Auto-Solve 300 ms.",
            "Open the Auto list and choose 300 ms.",
        ))
        .edit(TextEdit::line(
            "the Auto-Solve menu marks 300 ms.",
            "the Auto list reads 300 ms.",
        ))
        .edit(TextEdit::line(
            "the Auto-Solve menu set to a time",
            "the Auto list set to a time",
        ))
        .edit(enter_is_save())
        .edit(list_is_table())
}

fn status() -> GuideOverlay {
    GuideOverlay::new("solving-status")
        .edit(TextEdit::sub(
            " (Vertex segment); it applies at once.",
            ", then click Save Tier.",
        ))
        .edit(segment_note(" (Scale segment)"))
        .edit(TextEdit::sub(
            "Press Enter. Pressing Ctrl+Z",
            "Click Save Tier. Pressing Ctrl+Z",
        ))
        .edit(TextEdit::sub(
            "Add Anchor in a row's ... menu (Actions for Tier N) does the same",
            "The tier table's Add Anchor button on a block's rows does the same",
        ))
        .edit(list_is_table())
}

fn verdict_fixes() -> GuideOverlay {
    GuideOverlay::new("solving-verdict-fixes")
        .edit(enter_is_save())
        .edit(list_is_table())
}

fn preform() -> GuideOverlay {
    GuideOverlay::new("solving-preform").edit(list_is_table())
}

fn yield_and_carat() -> GuideOverlay {
    GuideOverlay::new("solving-yield-carat")
        .edit(TextEdit::line(
            "In the Yield group, set Stone width (Girdle Diameter, mm) to 8 and press Enter.",
            "In the Yield group, set Girdle Diameter (mm) to 8.",
        ))
        .edit(TextEdit::line(
            "Yield values apply when you press Enter or leave the field; there is no Apply button for them.",
            "Click Apply Yield Inputs.",
        ))
        .edit(TextEdit::line(
            "the Yield group shows the 8 mm width, and the Stone tab's Stone width reads the same.",
            "Apply Yield Inputs is no longer highlighted.",
        ))
        .edit(list_is_table())
}

fn walkthrough() -> GuideOverlay {
    GuideOverlay::new("new-design-walkthrough")
        .edit(TextEdit::sub("Click Add This Tier.", "Click Add Tier."))
        .edit(segment_note(" (Scale segment)"))
        .edit(TextEdit::line(
            "Press Enter in the field: the value applies when you press Enter or leave it.",
            "Click Apply Yield Inputs.",
        ))
        .edit(TextEdit::sub(
            "or with Add Anchor in the row's \u{2026} menu (Actions for Tier N).",
            "or with the tier table's Add Anchor button.",
        ))
        .edit(list_is_table())
}

fn output_save() -> GuideOverlay {
    GuideOverlay::new("output-save")
        .edit(TextEdit::sub(
            "in the Tier tab and press Enter.",
            "in the Tier tab, then click Save Tier.",
        ))
        .edit(list_is_table())
}

fn cutting_mode() -> GuideOverlay {
    GuideOverlay::new("output-cutting-mode")
        .edit(TextEdit::line(
            "Each page shows the tier, its angle in large figures, the indices to cut as chips and a large index wheel. A strip of named steps runs along the page (windowed when there are more than 20), and a Next preview shows the step after this one.",
            "Each page shows the tier, its angle in large figures, the indices to cut as chips, and a picture of the stone after the step beside the index wheel.",
        ))
        .edit(TextEdit::line(
            "Space or D marks the step done and goes on: the button reads like P3 Done \u{b7} Next. Esc closes cutting mode.",
            "Space or D marks the step done and goes on. Esc closes cutting mode.",
        ))
        .edit(TextEdit::sub(
            "the Cutting Instructions page hold",
            "the Schedule tab hold",
        ))
}
