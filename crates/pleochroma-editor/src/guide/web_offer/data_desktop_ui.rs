//! Web wording for lessons whose desktop text follows the redesigned desktop screens.
//!
//! The desktop's tier form has no Save Tier or Apply buttons, its tier list has row menus and an
//! Add Tier menu and no column headings (a detail line under the indices says what a tier meets,
//! a state mark stands for the solve), its Solve button holds the Auto-Solve menu, and a narrow
//! Solid view folds Snap and Slice into its Show menu. The browser build keeps the
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
        quick_add(),
        inline_angle(),
        index_shorthands(),
        meets_named(),
        meets_mm(),
        adopt_imported(),
        pin_to_mast(),
        mirror_to_other_block(),
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
        drag_handles(),
        slice(),
    ]
}

fn add_a_tier() -> GuideOverlay {
    GuideOverlay::new("tiers-add-a-tier")
        .edit(TextEdit::line(
            "a G1 row in the tier list: its code, G1, stands before the angle.",
            "a G1 row in the tier list, with G1 in the CODE column.",
        ))
        .edit(TextEdit::line(
            "a P1 row with the code P1, and meets G1 in the detail line under its indices.",
            "a P1 row with P1 in the CODE column and meets G1 in the Meets column.",
        ))
        .edit(TextEdit::line(
            "a C1 row with the code C1.",
            "a C1 row with C1 in the CODE column.",
        ))
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
        .edit(TextEdit::line(
            "the detail line under P1's indices reads meets Girdle, and the status strip says: Pavilion has no anchor: add a tier with an exact scale value.",
            "P1's Meets column reads meets Girdle, and the status strip says: Pavilion has no anchor: add a tier with an exact scale value.",
        ))
        .edit(TextEdit::line(
            "the detail line under P1's indices shows its pinned scale value again, and the status strip no longer says the pavilion has no anchor.",
            "P1's Meets column shows its pinned scale value again, and the status strip no longer says the pavilion has no anchor.",
        ))
        .edit(TextEdit::sub(
            "A change applies to that same row when you press Enter or leave the field; there is no Save button.",
            "Save Tier writes your changes back to that same row.",
        ))
        .edit(TextEdit::sub(
            "the Pavilion Main row shows 41.00 as its angle.",
            "the Pavilion Main row shows 41.00 in the ANGLE column.",
        ))
        .edit(TextEdit::sub(
            "shows the tier's name and a line such as Pavilion, tier 1 of 4, so you know which row the change goes to.",
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

// The browser's tier table has headed columns (CODE, ANGLE, Indices, Meets, MAST, SOLVE, PIN) and
// buttons on the row; the desktop's row has neither.
fn quick_add() -> GuideOverlay {
    GuideOverlay::new("tiers-quick-add")
        .edit(TextEdit::line(
            "a Girdle row with the code G1, pinned to scale 1.",
            "a Girdle row with G1 in the CODE column, pinned to scale 1.",
        ))
        .edit(TextEdit::line(
            "a Culet row with the code Culet, pinned to scale 0.88.",
            "a Culet row with Culet in the CODE column, pinned to scale 0.88.",
        ))
}

fn inline_angle() -> GuideOverlay {
    GuideOverlay::new("tiers-inline-angle")
        .edit(TextEdit::line(
            "The angle of a tier row is editable in place, with no trip to the Tier tab.",
            "The ANGLE column is editable in place, with no trip to the Tier tab.",
        ))
        .edit(TextEdit::line(
            "Double-click the angle of the Pavilion Main row, or pick the row and press F2.",
            "Double-click the ANGLE cell of the Pavilion Main row, or pick the row and press F2.",
        ))
}

fn mirror_to_other_block() -> GuideOverlay {
    GuideOverlay::new("tiers-mirror-to-other-block")
        .edit(TextEdit::line(
            "a new last row with the angle 40.00 and the same indices.",
            "a new last row with 40.00 in the ANGLE column and the same indices.",
        ))
        .edit(TextEdit::line(
            "a new last row with the angle 34.50 and a P code.",
            "a new last row with 34.50 in the ANGLE column and a P code in the CODE column.",
        ))
}

// The browser's Solid toolbar keeps the Snap and Slice pills; it has no Show menu.
fn drag_handles() -> GuideOverlay {
    GuideOverlay::new("viewing-drag-handles")
        .edit(TextEdit::line(
            "By default an angle snaps to 0.1 degrees and a depth to 0.01. Snap turns that off.",
            "By default an angle snaps to 0.1 degrees and a depth to 0.01. The Snap pill turns that off.",
        ))
        .edit(TextEdit::line(
            "Click Snap in the toolbar above the picture. In a narrow view it is in the Show menu under the view pills: click Show, then Snap.",
            "Click the Snap pill in the toolbar.",
        ))
        .edit(TextEdit::line(
            "Snap is no longer lit.",
            "The Snap pill is no longer lit.",
        ))
        .edit(TextEdit::line(
            "The index handle always moves in whole teeth, whatever Snap says.",
            "The index handle always moves in whole teeth, whatever the pill says.",
        ))
        .edit(TextEdit::line(
            "Snap clicked",
            "The Snap pill clicked",
        ))
        .edit(TextEdit::line(
            "Click Snap again, in the toolbar or in the Show menu.",
            "Click the Snap pill again.",
        ))
        .edit(TextEdit::line(
            "Snap is lit.",
            "The Snap pill is lit.",
        ))
}

fn slice() -> GuideOverlay {
    GuideOverlay::new("viewing-slice")
        .edit(TextEdit::line(
            "Click Slice in the toolbar above the picture (or press S with the stone in focus). In a narrow view it is in the Show menu under the view pills: click Show, then Slice.",
            "Click the Slice pill in the toolbar (or press S with the stone in focus).",
        ))
        .edit(TextEdit::line(
            "Slice is lit and the line under the toolbar says what to do.",
            "The Slice pill is lit and the line under the toolbar says what to do.",
        ))
}

fn index_shorthands() -> GuideOverlay {
    GuideOverlay::new("tiers-index-shorthands")
        .edit(TextEdit::line(
            "the G1 row lists the indices 0, 12, 24, 36, 48, 60, 72, 84.",
            "G1's Indices column lists 0, 12, 24, 36, 48, 60, 72, 84.",
        ))
        .edit(TextEdit::sub("Click Add This Tier.", "Click Add Tier."))
        .edit(TextEdit::sub(
            "until Add This Tier or an applied edit succeeds.",
            "until Add Tier or Save Tier succeeds.",
        ))
        .edit(segment_note(" (Facets segment)"))
}

fn meets_named() -> GuideOverlay {
    GuideOverlay::new("tiers-meets-named-facets")
        .edit(TextEdit::line(
            "the detail line under Upper Girdle's indices reads meets Lower Girdle.",
            "Upper Girdle's Meets column reads meets Lower Girdle.",
        ))
        .edit(TextEdit::line(
            "the Table row's detail line reads meet point.",
            "the Table row's Meets column reads meet.",
        ))
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
        .edit(TextEdit::sub(
            "measured along the mast in millimeters.",
            "measured the way the MAST column's millimeter tooltip measures it.",
        ))
        .edit(TextEdit::line(
            "the Pavilion Main row's detail line reads 2.2 mm depth.",
            "the Pavilion Main row's Meets column reads 2.2 mm depth.",
        ))
        .edit(TextEdit::line(
            "the Table row's detail line reads table 3.2 mm.",
            "the Table row's Meets column reads table 3.2 mm.",
        ))
        .edit(TextEdit::line(
            "the Girdle row's detail line reads girdle 0.3 mm.",
            "the Girdle row's Meets column reads girdle 0.3 mm.",
        ))
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
    GuideOverlay::new("tiers-adopt-imported-meets")
        .edit(TextEdit::line(
            "Open the Tier List Menu above the tier list and click Adopt All.",
            "Click Adopt All above the tier table.",
        ))
        .edit(TextEdit::line(
            "Open the \u{2026} menu (Actions for Tier N) at the end of a tier row, or right-click the row: it offers Adopt. In the Tier tab, a cyan line starting File says stands under Solved.",
            "Look for Adopt buttons in the tier table, and for a cyan line starting File says under Solved in the Tier tab.",
        ))
        .edit(TextEdit::line(
            "Adopt in the \u{2026} menu of a tier row.",
            "an Adopt button in the tier table.",
        ))
        .edit(TextEdit::line(
            "Open the \u{2026} menu of one row of the tier table and click Adopt.",
            "Click Adopt on one row of the tier table.",
        ))
        .edit(TextEdit::line(
            "that row's detail line changes from a pinned scale value to a meet, and its menu no longer offers Adopt.",
            "that row's Meets column changes from a pinned value to a meet, and its Adopt button goes.",
        ))
        .edit(TextEdit::line(
            "no row's \u{2026} menu offers Adopt any more.",
            "no Adopt button is left in the tier table.",
        ))
}

fn pin_to_mast() -> GuideOverlay {
    GuideOverlay::new("tiers-pin-to-mast")
        .edit(TextEdit::line(
            "the Upper Girdle row's detail line reads meets Lower Girdle.",
            "the Upper Girdle row's Meets column reads meets Lower Girdle.",
        ))
        .edit(TextEdit::line(
            "Open the \u{2026} menu (Actions for Tier N) at the end of the Upper Girdle row and click Pin.",
            "In the Upper Girdle row, click Pin in the PIN column.",
        ))
        .edit(TextEdit::line(
            "the row's detail line shows a pinned scale value, and its menu no longer offers Pin.",
            "the Meets column shows a pinned scale value, and the Pin button goes.",
        ))
        .edit(TextEdit::sub(
            "The menu offers no Pin while a tier is unsolved or already pinned.",
            "There is no Pin button while a tier is unsolved or already pinned.",
        ))
        .edit(segment_note(" (Facets segment)"))
        .edit(then_enter_is_save())
}

fn arithmetic() -> GuideOverlay {
    GuideOverlay::new("tiers-arithmetic")
        .edit(TextEdit::line(
            "the Table row's detail line reads scale 0.35.",
            "the Table row's Meets column reads 0.35.",
        ))
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
            "a Groove row whose detail line, under its indices, reads Concave tool.",
            "a Groove row with a Tool badge in the MEETS column and Tool cut in the SOLVE column.",
        ))
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
        .edit(TextEdit::sub(
            "The state mark at the right of each tier row changes from a tick to a triangle (Not solved yet) instead of claiming a result that might no longer be true",
            "The MAST and SOLVE columns of the tier table show placeholders instead of numbers that might no longer be true",
        ))
        .edit(TextEdit::line(
            "The state mark at the right of each tier row is a tick again.",
            "The MAST and SOLVE columns of the tier table show numbers again.",
        ))
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
        // The browser's tier table keeps its headed columns.
        .edit(TextEdit::line(
            "the new G1 row shows its code, G1, before the angle.",
            "the new G1 row reads G1 in the CODE column.",
        ))
        .edit(TextEdit::line(
            "the mark at the right end of the P1 row reads window -0.5\u{b0} in red.",
            "the P1 row's MARGIN column reads -0.5\u{b0} in red.",
        ))
        .edit(TextEdit::line(
            "the C1 row shows its code, C1, before the angle.",
            "the C1 row reads C1 in the CODE column.",
        ))
        .edit(TextEdit::line(
            "The RI and Critical chips update, and the mark at the right end of the P1 row turns green and reads +15.6\u{b0}.",
            "The RI and Critical chips update, and P1's MARGIN column turns green and reads +15.6\u{b0}.",
        ))
        .edit(TextEdit::line(
            "Read the small line under each tier's indices in the tier list.",
            "Find the ORBIT column in the tier list.",
        ))
        .edit(TextEdit::line(
            "Check that the lines of G1, P1 and C1 show no orbit count. A complete orbit is not listed. Scroll the tier list if a row is out of view.",
            "Check that G1, P1 and C1 each read orbit x8.",
        ))
        .edit(TextEdit::line(
            "no row's line shows a count such as 6/8 orbit.",
            "no row shows an amber count such as 6/8 orbit.",
        ))
        .edit(TextEdit::line(
            "A count means an index is missing or mistyped: compare that tier's Indices with 0, 12, 24, 36, 48, 60, 72, 84, or, in the Advanced interface, choose Complete orbit in that row's \u{2026} menu.",
            "An amber count means an index is missing or mistyped: compare that tier's Indices with 0, 12, 24, 36, 48, 60, 72, 84, or use the ORBIT cell's Complete orbit action.",
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
            "Press Space or D, or click the button that reads like P1 Done \u{b7} Next, to mark the step done. The page goes on to the next step.",
            "Press Space or D, or click Mark Step Done, to mark the step done.",
        ))
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
