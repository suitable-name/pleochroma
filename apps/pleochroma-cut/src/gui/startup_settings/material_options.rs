//! Material option lists shared by the renderer settings and Stone inspector.
use crate::{EditorModel, MainWindow, ViewportModel};
use pleochroma::optics::materials::GemMaterial;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// Maps the previously selected display name into a refreshed option list.
fn preserved_material_index(old_options: &[String], old_index: i32, new_options: &[String]) -> i32 {
    usize::try_from(old_index)
        .ok()
        .and_then(|index| old_options.get(index))
        .and_then(|selected| {
            new_options
                .iter()
                .position(|option| option.eq_ignore_ascii_case(selected))
        })
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

/// Where the material selected at `old_index` of `old_options` sits in `new_options`, by name
/// (case-insensitive); `None` when nothing was selected or the material is gone (the index is
/// then left alone, as it was before the list was sorted).
fn followed_material_index(
    old_options: &[String],
    old_index: i32,
    new_options: &[String],
) -> Option<i32> {
    let selected = usize::try_from(old_index)
        .ok()
        .and_then(|index| old_options.get(index))?;
    new_options
        .iter()
        .position(|option| option.eq_ignore_ascii_case(selected))
        .and_then(|index| i32::try_from(index).ok())
}

pub(in crate::gui) fn refresh_material_options(ui: &MainWindow, custom_mats: &[GemMaterial]) {
    // One alphabetical list, built-ins and custom materials together (the browser app's
    // header combo comes from the same builder).
    let names = pleochroma_editor::material::render_material_options(custom_mats);
    let viewport = ui.global::<ViewportModel>();
    // A custom material saved or deleted now moves entries of this alphabetical list, so the
    // selected material is kept by NAME: the index follows it to its new place.
    let old_names: Vec<String> = viewport
        .get_material_options()
        .iter()
        .map(|name| name.to_string())
        .collect();
    let old_index = viewport.get_selected_material_index();
    let followed = followed_material_index(&old_names, old_index, &names);
    let render_options: Vec<SharedString> = names.into_iter().map(Into::into).collect();
    viewport.set_material_options(std::rc::Rc::new(VecModel::from(render_options)).into());
    if let Some(index) = followed.filter(|&index| index != old_index) {
        viewport.set_selected_material_index(index);
    }

    let editor = ui.global::<EditorModel>();
    let current = editor.get_material_combo_options();
    let old_options: Vec<String> = current.iter().map(|name| name.to_string()).collect();
    let options = pleochroma_editor::material::design_material_options(custom_mats);
    let unchanged = old_options == options;
    if !unchanged {
        let index =
            preserved_material_index(&old_options, editor.get_material_combo_index(), &options);
        editor.set_material_combo_options(ModelRc::new(VecModel::from(
            options
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
        // Refreshing choices is presentation-only. Keep the same displayed option selected
        // without calling the design-material apply callback.
        editor.set_material_combo_index(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The render combo's list is one alphabetical list, and a custom material saved into it
    /// moves entries: the selected material is followed by name, not left on its old index.
    #[test]
    fn the_render_list_is_alphabetical_and_the_selection_follows_its_name() {
        let before = pleochroma_editor::material::render_material_options(&[]);
        assert!(
            before
                .windows(2)
                .all(|pair| pair[0].to_lowercase() <= pair[1].to_lowercase()),
            "{before:?}"
        );
        let custom = GemMaterial::new_custom("Aaa first", 1.5, 0.01, 0.0, [0.0; 3]);
        let after = pleochroma_editor::material::render_material_options(&[custom]);
        assert_eq!(after[0], "Aaa first");
        let quartz = before.iter().position(|n| n == "Quartz").expect("Quartz");
        let followed =
            followed_material_index(&before, i32::try_from(quartz).expect("small"), &after)
                .expect("Quartz is still listed");
        assert_eq!(after[usize::try_from(followed).expect("index")], "Quartz");
        assert_eq!(followed_material_index(&before, -1, &after), None);
        assert_eq!(followed_material_index(&before, 9_999, &after), None);
        assert_eq!(followed_material_index(&after, 0, &before), None);
    }

    #[test]
    fn adding_a_custom_material_preserves_the_custom_ri_selection() {
        let previous = pleochroma_editor::material::design_material_options(&[]);
        let custom = GemMaterial::new_custom("Tutorial glass", 1.5, 0.01, 0.0, [0.0; 3]);
        let refreshed = pleochroma_editor::material::design_material_options(&[custom]);
        let previous_sentinel = previous
            .iter()
            .position(|name| name == "Custom RI…")
            .expect("Custom RI option exists before refresh");
        let selected = preserved_material_index(
            &previous,
            i32::try_from(previous_sentinel).expect("small option index"),
            &refreshed,
        );

        let selected = usize::try_from(selected).expect("a refreshed selection is an index");

        assert!(refreshed.iter().any(|name| name == "Tutorial glass"));
        assert_eq!(refreshed[selected], "Custom RI…");
        assert_ne!(selected, previous_sentinel);
    }
}
