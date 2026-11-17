//! Scenarios: the material facts of the Stone tab and the Material Reference window.
//!
//! `material_reference_stone_card` opens a template design on the Stone tab and checks the identity
//! card facts: the Mohs, Toughness and specific gravity chips, the cleavage line, no fold card of
//! its own, and `Material Details...` opening the Material Reference window with a material
//! selected. It also checks the two states without material data: a custom material without gem
//! data (one line that leads to the Material Editor) and no material (nothing is shown).
//!
//! `material_reference_window` drives the window: the table and its footer, the sort headers (a
//! second click reverses), the filter chips and their counts, the search field, the B-G / F-C
//! switch, a row's details page (spectrum mode, variants, Back), Copy Table (the clipboard capture),
//! Escape, and opening the window again on another material. The window is swept at its default and
//! its minimum size on both pages.

use super::{
    Scenario,
    editor::{Card, create_template, show_edit_tab},
    existing_install,
    main_window::{click, select_render_view, select_top_tab_model},
};
use crate::support::{
    Ctx, WindowView,
    app::{pump, wait_until},
    coverage,
    query::is_clickable_role,
};
use pleochroma_cut::{
    EditorModel, MainWindow, MaterialReferenceModel, MaterialReferenceWindow, ParametersModel,
    harness_support,
};
use slint::{
    ComponentHandle, LogicalSize, Model,
    platform::{Key, WindowEvent},
};
use std::time::Duration;

/// The view name used in findings.
const VIEW_NAME: &str = "MaterialReferenceWindow";
/// The label of the removed Gem Data fold card: it must no longer be on the Stone tab.
const CARD_LABEL: &str = "Gem Data";
/// The identity card button that opens the window.
const DETAILS_BUTTON: &str = "Material Details...";
/// The line of a custom material without gem data (its first part; the link follows).
const EMPTY_LINE: &str = "No gem data yet";
/// The accessible label of the link that opens the Material Editor from that line.
const EDIT_LINK: &str = "Edit Material...";
/// The search field's placeholder.
const SEARCH_PLACEHOLDER: &str = "Search by name, system or cleavage";
/// The filter chips with their `MaterialReferenceModel.filter` index.
const FILTERS: [(&str, i32); 5] = [
    ("Natural", 1),
    ("Synthetic", 2),
    ("Glass", 3),
    ("My Materials", 4),
    ("All", 0),
];
/// The window's default size.
const DEFAULT_SIZE: (f32, f32) = (1180.0, 780.0);
/// The window's minimum size.
const MIN_SIZE: (f32, f32) = (640.0, 480.0);
/// A built-in material every build knows.
const KNOWN_MATERIAL: &str = "Diamond";

/// The scenarios of this file.
pub fn scenarios() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "material_reference_stone_card",
            seed: existing_install,
            run: stone_card,
        },
        Scenario {
            name: "material_reference_window",
            seed: existing_install,
            run: reference_window,
        },
    ]
}

/// Whether some accessible label of `view` contains `part`.
fn any_label_contains(view: &WindowView, part: &str) -> bool {
    view.labels().iter().any(|label| label.contains(part))
}

/// Activates the first clickable element of `view` labelled `wanted`, or else the first whose label
/// starts with it (a chip may append its count), and records the activation for the coverage gate.
/// Returns whether one was found.
fn press(view: &WindowView, wanted: &str) -> bool {
    let exact = view
        .by_label(wanted)
        .into_iter()
        .find(|e| e.accessible_role().is_some_and(is_clickable_role));
    let found = exact.or_else(|| {
        view.elements().into_iter().find(|e| {
            e.accessible_role().is_some_and(is_clickable_role)
                && e.accessible_label()
                    .is_some_and(|label| label.starts_with(wanted))
        })
    });
    found.is_some_and(|element| {
        coverage::record_activation(&element);
        element.invoke_accessible_default_action();
        true
    })
}

/// Sends Escape to `window`.
fn press_escape(window: &slint::Window) {
    window.dispatch_event(WindowEvent::KeyPressed {
        text: Key::Escape.into(),
    });
    window.dispatch_event(WindowEvent::KeyReleased {
        text: Key::Escape.into(),
    });
    pump(200);
}

/// The window, when it is open.
fn open_window() -> Option<MaterialReferenceWindow> {
    harness_support::material_reference_window()
}

/// The window's view.
fn window_view(window: &MaterialReferenceWindow) -> WindowView {
    WindowView::of(VIEW_NAME, window)
}

/// The number of table rows the window holds.
fn row_count(model: &MaterialReferenceModel<'_>) -> usize {
    model.get_rows().row_count()
}

/// The name of table row `index`.
fn row_name(model: &MaterialReferenceModel<'_>, index: usize) -> String {
    model
        .get_rows()
        .row_data(index)
        .map(|row| row.name.to_string())
        .unwrap_or_default()
}

/// The name of the selected row, empty when none is selected.
fn selected_name(model: &MaterialReferenceModel<'_>) -> String {
    usize::try_from(model.get_selected())
        .map(|index| row_name(model, index))
        .unwrap_or_default()
}

/// The count the filter chip `index` shows.
fn filter_count(model: &MaterialReferenceModel<'_>, index: i32) -> Option<usize> {
    let slot = usize::try_from(index).ok()?;
    let count = model.get_filter_counts().row_data(slot)?;
    usize::try_from(count).ok()
}

/// Sweeps the open window at its default and its minimum size, then restores the default.
fn sweep_window(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow, label: &str) {
    for (width, height) in [DEFAULT_SIZE, MIN_SIZE] {
        window.window().set_size(LogicalSize::new(width, height));
        pump(150);
        let view = window_view(window);
        ctx.sweep_view(&view, label);
    }
    window
        .window()
        .set_size(LogicalSize::new(DEFAULT_SIZE.0, DEFAULT_SIZE.1));
    pump(150);
}

/// Closes the window with Escape (the key the window answers); hides it when the key did not close
/// it, and says so in a finding. Returns whether Escape closed it.
fn close_with_escape(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow, check: &str) -> bool {
    press_escape(window.window());
    let closed = open_window().is_none();
    ctx.expect(
        closed,
        check,
        "Escape did not close the Material Reference window",
    );
    if !closed {
        // GAP: Escape is not handled; the window is hidden the way its close button would.
        let _ = window.hide();
        pump(200);
    }
    closed
}

/// A template design with the Parameters dock open on the Stone tab, every fold card closed. The
/// tab reads `ParametersModel.stone_folds` when it mounts, so the bits are set first and the tab is
/// mounted again through another one.
fn open_stone_tab(ctx: &mut Ctx<'_>) -> bool {
    show_edit_tab(ctx.app);
    if !ctx.expect(
        create_template(ctx.app, Card::First),
        "template-design",
        "no design could be created from a template",
    ) {
        return false;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    ctx.wait_for(
        "the new design's first solve ends",
        Duration::from_secs(30),
        || !editor.get_solve_running(),
    );
    select_top_tab_model(ctx.app, 0);
    select_render_view(ctx.app, 0);
    let dock = ui.global::<ParametersModel>();
    dock.set_stone_folds(0);
    dock.set_open(true);
    dock.invoke_show_tab(1);
    pump(300);
    dock.invoke_show_tab(0);
    pump(600);
    true
}

/// Opens the window on `material` through the card's callback, waits for it and returns it.
fn open_on(ctx: &mut Ctx<'_>, ui: &MainWindow, material: &str) -> Option<MaterialReferenceWindow> {
    ui.global::<EditorModel>()
        .invoke_open_material_reference(material.into());
    coverage::record_callback("EditorModel.open_material_reference");
    let opened = ctx.wait_for(
        "the Material Reference window opens",
        Duration::from_secs(10),
        || open_window().is_some(),
    );
    let window = open_window().filter(|_| opened)?;
    pump(300);
    Some(window)
}

// ---------------------------------------------------------------------------------------------
// T-H1: the Stone tab card
// ---------------------------------------------------------------------------------------------

/// The material facts of the identity card: the chips, the button, the empty and hidden states.
#[expect(
    clippy::too_many_lines,
    reason = "one sequential UI scenario: the chips, the button, the empty and hidden states"
)]
fn stone_card(ctx: &mut Ctx<'_>) {
    if !open_stone_tab(ctx) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();

    // The facts show for the template material, with no fold card of their own.
    let shown = ctx.wait_for(
        "the material facts are pushed",
        Duration::from_secs(10),
        || editor.get_gem_card_shown(),
    );
    if !ctx.expect(
        shown,
        "gem-card-shown",
        "EditorModel.gem_card_shown stays false for the template design material",
    ) {
        return;
    }
    let view = ctx.app.main_view();
    ctx.expect(
        !view.has(CARD_LABEL),
        "gem-card-removed",
        format!("the Stone tab still has a {CARD_LABEL:?} fold card"),
    );

    // The chips: Mohs always has a value for a built-in material, and each chip shows.
    let mohs = editor.get_gem_mohs().to_string();
    let toughness = editor.get_gem_toughness().to_string();
    ctx.expect(
        !mohs.is_empty(),
        "gem-chip-mohs-value",
        "EditorModel.gem_mohs is empty for the template design material",
    );
    ctx.expect(
        toughness.is_empty() || toughness.starts_with("Toughness "),
        "gem-chip-toughness-value",
        format!("the toughness chip text reads {toughness:?}, expected \"Toughness ...\""),
    );
    ctx.expect(
        any_label_contains(&view, &format!("Mohs hardness {mohs}")),
        "gem-chip-mohs",
        format!("the Stone tab shows no \"Mohs hardness {mohs}\" chip"),
    );
    if !toughness.is_empty() {
        ctx.expect(
            any_label_contains(&view, &toughness),
            "gem-chip-toughness",
            format!("the Stone tab shows no {toughness:?} chip"),
        );
    }
    let sg = editor.get_gem_sg().to_string();
    if !sg.is_empty() {
        ctx.expect(
            any_label_contains(&view, &format!("Specific gravity {sg}")),
            "gem-chip-sg",
            format!("the Stone tab shows no \"Specific gravity {sg}\" chip"),
        );
    }
    let cleavage = editor.get_gem_cleavage().to_string();
    if !cleavage.is_empty() {
        ctx.expect(
            any_label_contains(&view, &format!("Cleavage: {cleavage}")),
            "gem-cleavage-line",
            format!("the Stone tab shows no \"Cleavage: {cleavage}\" line"),
        );
    }
    ctx.expect(
        !editor.get_gem_empty(),
        "gem-not-empty",
        "EditorModel.gem_empty is set for a built-in material",
    );
    ctx.check_health("Stone tab, material facts");
    ctx.sweep_main_at_sizes("Stone tab, material facts");

    // Material Details... opens the window with a material selected.
    let view = ctx.app.main_view();
    ctx.expect(
        view.has(DETAILS_BUTTON),
        "gem-card-button",
        format!("the identity card has no {DETAILS_BUTTON:?} button"),
    );
    let pressed = click(ctx.app, DETAILS_BUTTON);
    ctx.expect(
        pressed,
        "gem-card-details-click",
        format!("the {DETAILS_BUTTON:?} button was not found"),
    );
    if pressed {
        coverage::record_callback("EditorModel.open_material_reference");
    } else {
        // GAP: the button is not reachable by label; the callback it fires is driven.
        editor.invoke_open_material_reference("".into());
        coverage::record_callback("EditorModel.open_material_reference");
    }
    let opened = ctx.wait_for(
        "Material Details... opens the window",
        Duration::from_secs(10),
        || open_window().is_some(),
    );
    ctx.expect(
        opened,
        "gem-card-opens-window",
        "Material Details... did not open the Material Reference window",
    );
    if let Some(window) = open_window().filter(|_| opened) {
        pump(300);
        let model = window.global::<MaterialReferenceModel>();
        ctx.expect(
            row_count(&model) > 0 && model.get_selected() >= 0,
            "gem-card-selects",
            format!(
                "the window holds {} row(s) with row {} selected",
                row_count(&model),
                model.get_selected()
            ),
        );
        ctx.expect(
            model.get_page() == 0,
            "gem-card-table-page",
            "the window opens on the details page instead of the table",
        );
        ctx.check_health("Material Reference opened from the card");
        close_with_escape(ctx, &window, "gem-card-window-escape");
    }

    custom_empty_and_hidden(ctx, &ui);
}

/// The identity card of a custom material without gem data, and with no material.
///
/// A custom material without gem data needs the Material Editor save flow; the fields are
/// written the way the push does for it (`gem_empty`, no chip values), and the rendering of
/// them is what is checked.
fn custom_empty_and_hidden(ctx: &mut Ctx<'_>, ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    editor.set_gem_card_shown(true);
    editor.set_gem_empty(true);
    editor.set_gem_sg("".into());
    editor.set_gem_mohs("".into());
    editor.set_gem_toughness("".into());
    editor.set_gem_cleavage("".into());
    editor.set_gem_cleavage_warns(false);
    pump(400);
    let view = ctx.app.main_view();
    ctx.expect(
        any_label_contains(&view, EMPTY_LINE),
        "gem-card-empty-line",
        format!("the card of a custom material without gem data shows no {EMPTY_LINE:?} line"),
    );
    ctx.expect(
        view.has(EDIT_LINK),
        "gem-card-empty-link",
        format!("the empty line has no {EDIT_LINK:?} link"),
    );
    ctx.expect(
        !any_label_contains(&view, "Mohs hardness"),
        "gem-card-empty-no-chips",
        "a custom material without gem data still shows a Mohs chip",
    );
    ctx.check_health("Stone tab, custom material without gem data");
    ctx.sweep_main_at_sizes("Stone tab, custom material without data");

    editor.set_gem_empty(false);
    editor.set_gem_card_shown(false);
    pump(400);
    let view = ctx.app.main_view();
    ctx.expect(
        !view.has(DETAILS_BUTTON) && !any_label_contains(&view, EMPTY_LINE),
        "gem-card-hidden",
        "the material facts show although the design has no material",
    );
    ctx.check_health("Stone tab, no material facts");
}

// ---------------------------------------------------------------------------------------------
// T-H2: the window
// ---------------------------------------------------------------------------------------------

/// Opens the window through the Help menu (or its callback) with a design open.
fn open_from_menu(ctx: &mut Ctx<'_>, ui: &MainWindow) -> Option<MaterialReferenceWindow> {
    let reached = click(ctx.app, "Help") && click(ctx.app, "Material Reference...");
    if reached {
        let opened = ctx.wait_for(
            "Help > Material Reference... opens the window",
            Duration::from_secs(10),
            || open_window().is_some(),
        );
        if let Some(window) = open_window().filter(|_| opened) {
            pump(300);
            return Some(window);
        }
        ctx.expect(
            false,
            "mr-menu-opens-window",
            "Help > Material Reference... did not open the window",
        );
    }
    // GAP: the menu item is not reachable by label; the card's callback opens the window.
    open_on(ctx, ui, KNOWN_MATERIAL)
}

/// Sorts by Hardness through its header, twice, and checks the order changes and reverses.
fn check_sort(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow) {
    let model = window.global::<MaterialReferenceModel>();
    let first_by_name = row_name(&model, 0);
    ctx.expect(
        model.get_sort_column() == 0 && !model.get_sort_descending(),
        "mr-default-sort",
        format!(
            "the default sort is column {} ({}), expected Name ascending",
            model.get_sort_column(),
            if model.get_sort_descending() {
                "descending"
            } else {
                "ascending"
            }
        ),
    );
    let pressed = press(&window_view(window), "Sort by Hardness");
    ctx.expect(
        pressed,
        "mr-sort-header",
        "no header button labelled \"Sort by Hardness...\" was found",
    );
    if !pressed {
        // GAP: the header is not reachable by label; the callback it fires is driven. The column
        // index is the plan's `Column` order.
        model.invoke_sort_clicked(7);
        coverage::record_callback("MaterialReferenceModel.sort_clicked");
    }
    pump(250);
    let column = model.get_sort_column();
    let descending = model.get_sort_descending();
    ctx.expect(
        column != 0,
        "mr-sort-column",
        "pressing the Hardness header did not change the sort column",
    );
    let first_by_hardness = row_name(&model, 0);
    ctx.expect(
        first_by_hardness != first_by_name,
        "mr-sort-order",
        format!("the first row is {first_by_hardness:?} by name and by hardness"),
    );
    if !press(&window_view(window), "Sort by Hardness") {
        model.invoke_sort_clicked(column);
        coverage::record_callback("MaterialReferenceModel.sort_clicked");
    }
    pump(250);
    ctx.expect(
        model.get_sort_column() == column && model.get_sort_descending() != descending,
        "mr-sort-reverses",
        "a second press of the same header did not reverse the order",
    );
    ctx.check_health("Material Reference sorted by Hardness");
    sweep_window(ctx, window, "Material Reference, sorted by Hardness");
}

/// Presses every filter chip and checks the row count against the chip's count.
fn check_filters(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow) {
    let model = window.global::<MaterialReferenceModel>();
    ctx.expect(
        model.get_filter_counts().row_count() == 5,
        "mr-filter-counts",
        format!(
            "the window holds {} filter counts, expected 5",
            model.get_filter_counts().row_count()
        ),
    );
    for (label, index) in FILTERS {
        let pressed = press(&window_view(window), label);
        if !pressed || model.get_filter() != index {
            ctx.expect(
                false,
                "mr-filter-chip",
                format!("the {label:?} chip did not select filter {index}"),
            );
            // GAP: the chip is not reachable by label; the callback it fires is driven.
            model.invoke_filter_chosen(index);
            coverage::record_callback("MaterialReferenceModel.filter_chosen");
        }
        pump(200);
        let expected = filter_count(&model, index);
        ctx.expect(
            expected == Some(row_count(&model)),
            "mr-filter-rows",
            format!(
                "filter {label:?}: the table holds {} row(s), its chip counts {expected:?}",
                row_count(&model)
            ),
        );
        if index == 0 {
            ctx.expect(
                row_count(&model) >= 39,
                "mr-all-rows",
                format!(
                    "the All filter lists {} row(s), expected the 39 built-ins at least",
                    row_count(&model)
                ),
            );
        }
        if index == 3 {
            sweep_window(ctx, window, "Material Reference, Glass filter");
        }
    }
    ctx.check_health("Material Reference filter chips");
}

/// The search field narrows the table and clearing it restores it.
fn check_search(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow) {
    let model = window.global::<MaterialReferenceModel>();
    let all = row_count(&model);
    ctx.expect(
        window_view(window).has(SEARCH_PLACEHOLDER),
        "mr-search-field",
        format!("the window has no search field with the placeholder {SEARCH_PLACEHOLDER:?}"),
    );
    // The field's own edit handler runs on typing; the model write and the callback are what it
    // does.
    model.set_query("garnet".into());
    model.invoke_query_edited("garnet".into());
    coverage::record_callback("MaterialReferenceModel.query_edited");
    pump(250);
    let found = row_count(&model);
    ctx.expect(
        found > 0 && found < all,
        "mr-search-narrows",
        format!("the query \"garnet\" leaves {found} of {all} row(s)"),
    );
    model.set_query("zzzzqq".into());
    model.invoke_query_edited("zzzzqq".into());
    pump(250);
    ctx.expect(
        row_count(&model) == 0,
        "mr-search-none",
        format!(
            "a query nothing matches leaves {} row(s)",
            row_count(&model)
        ),
    );
    ctx.check_health("Material Reference search without a match");
    sweep_window(ctx, window, "Material Reference, search without a match");
    model.set_query("".into());
    model.invoke_query_edited("".into());
    pump(250);
    ctx.expect(
        row_count(&model) == all,
        "mr-search-clears",
        format!(
            "clearing the query leaves {} of {all} row(s)",
            row_count(&model)
        ),
    );
}

/// The B-G / F-C switch.
fn check_scale(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow) {
    let model = window.global::<MaterialReferenceModel>();
    let b_g = "B\u{2013}G";
    let f_c = "F\u{2013}C";
    ctx.expect(
        model.get_dispersion_bg(),
        "mr-scale-default",
        "the window does not open on the B-G dispersion figure",
    );
    for (label, bg) in [(f_c, false), (b_g, true)] {
        let pressed = press(&window_view(window), label);
        if !pressed || model.get_dispersion_bg() != bg {
            ctx.expect(
                false,
                "mr-scale-switch",
                format!("the {label:?} segment did not set dispersion_bg to {bg}"),
            );
            // GAP: the segment is not reachable by label; the callback it fires is driven.
            model.invoke_scale_chosen(bg);
            coverage::record_callback("MaterialReferenceModel.scale_chosen");
        }
        pump(200);
    }
    ctx.check_health("Material Reference dispersion switch");
}

/// Copy Table puts a tab-separated table on the (captured) clipboard.
fn check_copy(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow) {
    let model = window.global::<MaterialReferenceModel>();
    let _ = harness_support::take_clipboard_writes();
    let pressed = press(&window_view(window), "Copy Table");
    ctx.expect(
        pressed,
        "mr-copy-button",
        "the Copy Table button was not found",
    );
    if !pressed {
        // GAP: the button is not reachable by label; the callback it fires is driven.
        model.invoke_copy_table();
        coverage::record_callback("MaterialReferenceModel.copy_table");
    }
    let mut copied: Vec<String> = Vec::new();
    let _ = wait_until(Duration::from_secs(3), || {
        copied.extend(harness_support::take_clipboard_writes());
        !copied.is_empty()
    });
    ctx.expect(
        copied.iter().any(|text| text.contains('\t')),
        "mr-copy-table",
        "Copy Table put no tab-separated text on the clipboard",
    );
}

/// A row's details: open it, the cards, spectrum mode, variants, back.
fn check_details(ctx: &mut Ctx<'_>, window: &MaterialReferenceWindow) {
    let model = window.global::<MaterialReferenceModel>();
    model.set_selected(1);
    pump(150);
    let selected = selected_name(&model);
    let pressed = press(&window_view(window), "Open Details");
    ctx.expect(
        pressed,
        "mr-open-details-button",
        "the Open Details button was not found",
    );
    if !pressed || model.get_page() != 1 {
        // GAP: the button is not reachable by label; the callback it fires is driven.
        model.invoke_row_opened(model.get_selected());
        coverage::record_callback("MaterialReferenceModel.row_opened");
    }
    pump(400);
    ctx.expect(
        model.get_page() == 1,
        "mr-details-page",
        "opening a row did not show the details page",
    );
    let view = window_view(window);
    let titles = [
        "refractive index",
        "optics",
        "durability and cutting",
        "absorption spectrum",
        "color by direction and light",
        "color variants",
        "sources",
    ];
    let lower: Vec<String> = view.labels().iter().map(|l| l.to_lowercase()).collect();
    for title in titles {
        ctx.expect(
            lower.iter().any(|label| label.contains(title)),
            "mr-details-card",
            format!("the details page of {selected:?} shows no {title:?} card"),
        );
    }
    ctx.expect(
        view.has("All Materials"),
        "mr-details-back-button",
        "the details page has no All Materials button",
    );
    ctx.check_health("Material Reference details page");
    sweep_window(ctx, window, "Material Reference, details page");

    // Spectrum mode, then back to the default.
    for (label, mode) in [("Transmission", 1), ("Absorption", 0)] {
        if !press(&window_view(window), label) {
            // GAP: the segment is not reachable by label; the callback it fires is driven.
            model.invoke_spectrum_mode(mode);
            coverage::record_callback("MaterialReferenceModel.spectrum_mode");
        }
        pump(250);
    }
    sweep_window(ctx, window, "Material Reference, spectrum modes");

    // The first variant (Material Default) is always there: select it and use it for the design.
    model.invoke_variant_selected(0);
    coverage::record_callback("MaterialReferenceModel.variant_selected");
    pump(250);
    model.invoke_use_variant(0);
    coverage::record_callback("MaterialReferenceModel.use_variant");
    pump(400);
    ctx.check_health("Material Reference variant used for the design");

    // Back keeps the selection.
    let pressed = press(&window_view(window), "All Materials");
    if !pressed || model.get_page() != 0 {
        ctx.expect(
            false,
            "mr-back",
            "All Materials did not return to the table page",
        );
        // GAP: the button is not reachable by label; the callback it fires is driven.
        model.invoke_back();
        coverage::record_callback("MaterialReferenceModel.back");
    }
    pump(300);
    ctx.expect(
        selected_name(&model) == selected,
        "mr-back-keeps-selection",
        format!(
            "the selection was {selected:?} before the details page and {:?} after Back",
            selected_name(&model)
        ),
    );
    ctx.check_health("Material Reference back on the table");
}

/// Opens the window on a named material, then again on another one while it is open.
fn check_reopen(ctx: &mut Ctx<'_>, ui: &MainWindow) {
    let Some(window) = open_on(ctx, ui, KNOWN_MATERIAL) else {
        ctx.expect(
            false,
            "mr-reopen",
            "the window did not open again after Escape closed it",
        );
        return;
    };
    let model = window.global::<MaterialReferenceModel>();
    model.invoke_filter_chosen(0);
    model.set_query("".into());
    model.invoke_query_edited("".into());
    pump(250);
    let _ = open_on(ctx, ui, KNOWN_MATERIAL);
    ctx.expect(
        selected_name(&model) == KNOWN_MATERIAL,
        "mr-opens-selected",
        format!(
            "the window opened on {KNOWN_MATERIAL:?} selects {:?}",
            selected_name(&model)
        ),
    );
    // Opening it again, while open, selects the material asked for.
    let other = (0..row_count(&model))
        .map(|index| row_name(&model, index))
        .find(|name| !name.is_empty() && name != KNOWN_MATERIAL)
        .unwrap_or_default();
    let _ = open_on(ctx, ui, &other);
    ctx.expect(
        selected_name(&model) == other,
        "mr-reopen-selects",
        format!(
            "opening the open window on {other:?} selects {:?}",
            selected_name(&model)
        ),
    );
    // Preview As is view only: it must leave the design alone and the window open.
    model.invoke_preview_as(other.as_str().into());
    coverage::record_callback("MaterialReferenceModel.preview_as");
    pump(400);
    ctx.check_health("Material Reference, Preview As");
    close_with_escape(ctx, &window, "mr-final-escape");
}

/// The window's whole journey.
fn reference_window(ctx: &mut Ctx<'_>) {
    show_edit_tab(ctx.app);
    if !ctx.expect(
        create_template(ctx.app, Card::First),
        "template-design",
        "no design could be created from a template",
    ) {
        return;
    }
    let ui = ctx.app.ui().clone_strong();
    let editor = ui.global::<EditorModel>();
    ctx.wait_for(
        "the new design's first solve ends",
        Duration::from_secs(30),
        || !editor.get_solve_running(),
    );
    select_top_tab_model(ctx.app, 0);
    select_render_view(ctx.app, 0);

    let Some(window) = open_from_menu(ctx, &ui) else {
        ctx.expect(
            false,
            "mr-window-opens",
            "the Material Reference window did not open",
        );
        return;
    };
    let model = window.global::<MaterialReferenceModel>();
    ctx.expect(
        row_count(&model) >= 39,
        "mr-built-in-rows",
        format!(
            "the table holds {} row(s), expected the 39 built-ins at least",
            row_count(&model)
        ),
    );
    ctx.expect(
        model.get_page() == 0,
        "mr-opens-on-table",
        "the window did not open on the table page",
    );
    ctx.check_health("Material Reference table");
    sweep_window(ctx, &window, "Material Reference, table");

    check_sort(ctx, &window);
    check_filters(ctx, &window);
    check_search(ctx, &window);
    check_scale(ctx, &window);
    check_copy(ctx, &window);
    check_details(ctx, &window);

    if close_with_escape(ctx, &window, "mr-escape") {
        check_reopen(ctx, &ui);
    }
    ctx.check_health("Material Reference closed");
}
