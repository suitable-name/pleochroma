//! The material the plan is for, as a `GemMaterial`: the crystal axis panel hides for an
//! isotropic one, a color-named rule needs the material's presets, and a custom (catalogue)
//! material has its own default rule.
//!
//! A built-in material is known at once. A catalogue material is read from the library on a
//! thread of its own (the database lock may be held by a long search) and kept for the session;
//! until it arrives the panel treats the material as unknown.

use crate::{
    RoughPlanModel,
    gui::{
        rough_colour::preview::resolve_host,
        rough_plan::host::{Host, on_host},
    },
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};
use tracing::warn;

/// The name of the material the form has picked.
pub(in crate::gui::rough_plan) fn picked_name(host: &Host) -> Option<String> {
    let index =
        usize::try_from(host.window.global::<RoughPlanModel>().get_material_index()).ok()?;
    host.session
        .borrow()
        .choices
        .get(index)
        .map(|choice| choice.name.clone())
}

/// The material called `name` when it is known now: a built-in one, or a catalogue one already
/// looked up. For a catalogue material not looked up yet the lookup is started (the window syncs
/// when it arrives) and the answer is `None`.
pub(in crate::gui::rough_plan) fn known_material(
    host: &Rc<Host>,
    name: &str,
) -> Option<GemMaterial> {
    if let Some(material) = GemMaterial::by_name(name) {
        return Some(material);
    }
    let (cached, asked) = {
        let session = host.session.borrow();
        (
            session.axis.materials.get(name).cloned(),
            session.axis.asked.contains(name),
        )
    };
    if cached.is_some() {
        return cached;
    }
    if !asked {
        host.session
            .borrow_mut()
            .axis
            .asked
            .insert(name.to_string());
        spawn_lookup(host, name.to_string());
    }
    None
}

/// The material called `name`, built-in or from the library (for a worker thread: it waits for the
/// database lock).
pub(in crate::gui::rough_plan) fn resolve(db: &Mutex<Database>, name: &str) -> Option<GemMaterial> {
    GemMaterial::by_name(name).or_else(|| {
        let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
        resolve_host(&guard, name).map(|owned| owned.material())
    })
}

/// Reads catalogue material `name` on a thread of its own and syncs the window when it is in.
fn spawn_lookup(host: &Rc<Host>, name: String) {
    let db = Arc::clone(&host.db);
    let window = host.window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("rough-axis-material".to_string())
        .spawn(move || {
            let found = resolve(&db, &name);
            let _ = window.upgrade_in_event_loop(move |_window| {
                on_host(|host| {
                    if let Some(material) = found {
                        host.session
                            .borrow_mut()
                            .axis
                            .materials
                            .insert(name, material);
                        super::sync(host);
                    }
                });
            });
        });
    if let Err(error) = spawned {
        warn!("Rough planner: could not start the material lookup: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_in_material_is_known_without_the_library() {
        assert!(GemMaterial::by_name("Sapphire").is_some());
        assert!(GemMaterial::by_name("No Such Material").is_none());
    }
}
