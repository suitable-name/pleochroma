//! Inclusions of a rough in the planner window: "Add inclusion...", the list with its Remove
//! buttons, and the entry point a tool that locates an inclusion adds it through.
//!
//! An inclusion is a closed mesh inside the rough: stones keep clear of it, and the weight
//! and the yield count it as material (see the core's mesh `inclusion` module). Every rough
//! takes them. An imported mesh keeps its inclusions in its registered mesh, so adding or
//! removing one changes the model's base. A block, a cylinder or a pebble keeps its base (its
//! sizes and its cuts stay editable) and its inclusions in the model's own set. Either way the
//! change is one undo step.
//!
//! An inclusion is one of four kinds, chosen in the window: a mesh file, or a sphere, an
//! ellipsoid or a box of a typed size. A mesh file of an imported mesh rough is read in the
//! rough file's coordinates; for any other rough the inclusion is placed by typed numbers (the
//! centre of the inclusion in mm from the rough's corner, the rough's centre where a field is
//! empty).
//!
//! The work (reading the file, building the meshes, checking the inclusion against the rough)
//! runs on a worker thread ([`mesh_task::spawn`]); no `RefCell` borrow is held across the file
//! dialog or a job. The logic that matters is in [`crate::mesh_io`] and the core, where the
//! tests run.
//!
//! # For a tool that locates inclusions
//!
//! [`add_inclusion_to_rough`] takes the inclusion as a [`RoughMesh`] in the rough's own frame
//! (millimetres, the rough's bounding box at the origin), whatever found it, and a margin;
//! it needs no file. A mesh built from points and triangles is `RoughMesh::new(&points,
//! &triangles)`.

use super::{
    editing::resync,
    host::{Host, on_idle_host},
    locate::open_locate,
    mesh_task,
    obj_import::read_mesh_file,
    saved::{show_error, show_status},
};
use crate::{
    RoughPlanModel,
    gui::pickers::{PickerFilter, PickerKind, PickerRequest, pick},
    mesh_io::{
        InclusionKind, MeshUnit, Placement, add_inclusion, centre_text, inclusion_in_rough_frame,
        inclusion_parts, inclusion_row_text, parse_margin_mm, parse_placement, primitive_in_rough,
        remove_inclusion,
    },
};
use pleochroma_cut_core::rough_plan::{
    InclusionSet, RoughBase, RoughMesh, RoughModel, shape::hull,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{path::PathBuf, rc::Rc};

/// Shown when the rough changed while an inclusion was being read, added or removed.
const ROUGH_CHANGED_MESSAGE: &str =
    "The rough changed while the inclusion was being read. Do it again.";

/// Registers the inclusion callbacks on the window's `RoughPlanModel`. While a plan runs they
/// are ignored (the controls are disabled in the window as well).
pub(super) fn setup_inclusion_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_add_inclusion(|| on_idle_host(start_add));
    model.on_remove_inclusion(|row| on_idle_host(|host| remove(host, row)));
    model.on_locate_inclusion(|| on_idle_host(open_locate));
}

/// The session's model as it is now (cheap: the inclusion set is shared).
fn current_model(host: &Host) -> RoughModel {
    host.session.borrow().model.clone()
}

/// Whether `base` is an imported mesh, the one rough whose inclusions are in its file's
/// coordinates and whose inclusion is always a mesh file.
const fn is_mesh_rough(base: &RoughBase) -> bool {
    matches!(base, RoughBase::Hull { .. })
}

/// Mirrors the inclusions of the model into the window: that every rough can take them, which
/// controls apply to this rough, the rough's centre, and one line per inclusion. Called
/// whenever the model is mirrored.
pub(super) fn push(host: &Host) {
    let rough = current_model(host);
    let base = rough.base;
    // The inclusion set holds its volumes and extents already: nothing is built here (a derived
    // mesh is built on the worker thread, never on this one).
    let rows: Vec<SharedString> = rough
        .inclusion_infos()
        .iter()
        .enumerate()
        .map(|(i, info)| inclusion_row_text(i, info.extents_mm, info.volume_mm3).into())
        .collect();
    // Photos are traced through a mesh: a convex outline without one cannot be used.
    let has_mesh = matches!(base, RoughBase::Hull { id, .. } if hull::mesh(id).is_some());
    let model = host.window.global::<RoughPlanModel>();
    model.set_inclusions_possible(true);
    model.set_inclusion_placement_possible(!is_mesh_rough(&base));
    model.set_locate_possible(has_mesh);
    model.set_inclusion_rows(ModelRc::new(VecModel::from(rows)));
    push_centre(host);
}

/// Refreshes only the rough's centre in the window: what an empty position field means. The
/// sizes of a block, a cylinder or a pebble change it without the model being mirrored.
pub(super) fn push_centre(host: &Host) {
    let base = host.session.borrow().model.base;
    host.window
        .global::<RoughPlanModel>()
        .set_inclusion_centre_text(centre_text(&base).into());
}

/// The "Add inclusion..." button: adds an inclusion of the kind the window has chosen. A typed
/// shape is built at once; a mesh file is asked for with a file dialog. The unit, the margin and
/// the position are the ones in the window now.
fn start_add(host: &Rc<Host>) {
    show_error(host, "");
    let base = host.session.borrow().model.base;
    if let Err(error) = base.validate() {
        show_error(host, &format!("Inclusion not added: {error}"));
        return;
    }
    let model = host.window.global::<RoughPlanModel>();
    let margin_mm = match parse_margin_mm(model.get_inclusion_margin().as_str()) {
        Ok(margin_mm) => margin_mm,
        Err(message) => {
            show_error(host, &message);
            return;
        }
    };
    // An imported mesh takes mesh files only, in its file's coordinates: no kind, no position.
    let (kind, placement) = if is_mesh_rough(&base) {
        (InclusionKind::MeshFile, [None; 3])
    } else {
        let placement = match parse_placement(
            model.get_inclusion_x_mm().as_str(),
            model.get_inclusion_y_mm().as_str(),
            model.get_inclusion_z_mm().as_str(),
        ) {
            Ok(placement) => placement,
            Err(message) => {
                show_error(host, &message);
                return;
            }
        };
        (
            InclusionKind::from_index(model.get_inclusion_kind_index()),
            placement,
        )
    };
    if kind == InclusionKind::MeshFile {
        pick_file(host, margin_mm, placement);
        return;
    }
    let sizes = [
        model.get_inclusion_size_x_mm(),
        model.get_inclusion_size_y_mm(),
        model.get_inclusion_size_z_mm(),
    ];
    match primitive_in_rough(
        kind,
        [sizes[0].as_str(), sizes[1].as_str(), sizes[2].as_str()],
        placement,
        &base,
    ) {
        Ok(inclusion) => add_inclusion_to_rough(host, inclusion, margin_mm),
        Err(message) => show_error(host, &format!("Inclusion not added: {message}")),
    }
}

/// Asks for a mesh file and reads it as an inclusion placed at `placement` (an imported mesh
/// ignores it). The unit is the one in the window now.
fn pick_file(host: &Rc<Host>, margin_mm: f64, placement: Placement) {
    // Read now, before the dialog: the choices are the ones made for this inclusion.
    let unit = MeshUnit::from_index(host.window.global::<RoughPlanModel>().get_mesh_unit_index());
    let Some(main) = host.main.upgrade() else {
        return;
    };
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Add an Inclusion from a Mesh File".to_string()),
        filters: vec![PickerFilter {
            label: "Mesh (OBJ, STL, PLY)".to_string(),
            extensions: ["obj", "stl", "ply"].map(str::to_string).to_vec(),
        }],
        default_file_name: None,
        starting_dir: None,
    };
    pick(&main, request, move |_, path| {
        let Some(path) = path else {
            return;
        };
        on_idle_host(move |host| read_inclusion(host, path, unit, margin_mm, placement));
    });
}

/// Reads the mesh file at `path` (its numbers in `unit`) on a worker thread, moves it into
/// the rough's frame and, when it is ready, adds it with [`add_inclusion_to_rough`].
fn read_inclusion(
    host: &Rc<Host>,
    path: PathBuf,
    unit: MeshUnit,
    margin_mm: f64,
    placement: Placement,
) {
    let base = host.session.borrow().model.base;
    show_error(host, "");
    mesh_task::spawn(
        host,
        "Reading the inclusion...",
        move || {
            let bytes = read_mesh_file(&path)?;
            let extension = path.extension().and_then(|e| e.to_str());
            let (points, triangles) = inclusion_parts(&bytes, extension, unit)?;
            inclusion_in_rough_frame(&base, &points, &triangles, placement)
        },
        move |host, result| match result {
            Ok(inclusion) => add_inclusion_to_rough(host, inclusion, margin_mm),
            Err(message) => show_error(host, &format!("Inclusion not added: {message}")),
        },
    );
}

/// Adds `inclusion`, a closed mesh in the rough's own frame (millimetres, the rough's bounding
/// box at the origin), to the rough with a margin of `margin_mm`, as one undo step. This is the
/// way in for anything that finds an inclusion by other means than a file; the file route
/// goes through it too.
///
/// It must be called on the UI thread. The inclusion is checked against the rough on a worker
/// thread; a refusal (it reaches the surface, lies outside the material, crosses another
/// inclusion, cannot hold its margin) is shown on the window's error line and nothing changes.
pub(super) fn add_inclusion_to_rough(host: &Rc<Host>, inclusion: RoughMesh, margin_mm: f64) {
    add_inclusion_notify(host, inclusion, margin_mm, |_, _| {});
}

/// [`add_inclusion_to_rough`], and `notify` is told how it ended: `Ok` when the inclusion was
/// added (one undo step), `Err` with the reason when it was refused or the rough changed
/// meanwhile (the reason is on the planner's error line as well). `notify` runs on the UI
/// thread; it is not called when the job was overtaken (a newer job, a reset).
///
/// This is how "Locate inclusion from photos" adds the point it found and learns whether it
/// went in, so that it can keep the located inclusion's marks with its rig.
pub(super) fn add_inclusion_notify(
    host: &Rc<Host>,
    inclusion: RoughMesh,
    margin_mm: f64,
    notify: impl FnOnce(&Rc<Host>, Result<(), String>) + Send + 'static,
) {
    let before = current_model(host);
    let job_model = before.clone();
    mesh_task::spawn(
        host,
        "Adding the inclusion...",
        move || add_inclusion(&job_model, &inclusion, margin_mm),
        move |host, result| {
            let outcome = finish(
                host,
                &before,
                result,
                "Inclusion not added:",
                "Inclusion added. Stones keep clear of it, and the weight and the yield count it \
                 as material.",
            );
            notify(host, outcome);
        },
    );
}

/// The Remove button of inclusion `row`.
fn remove(host: &Rc<Host>, row: i32) {
    let Ok(index) = usize::try_from(row) else {
        return;
    };
    show_error(host, "");
    let before = current_model(host);
    let job_model = before.clone();
    mesh_task::spawn(
        host,
        "Removing the inclusion...",
        move || remove_inclusion(&job_model, index),
        move |host, result| {
            let _ = finish(
                host,
                &before,
                result,
                "Inclusion not removed:",
                "Inclusion removed.",
            );
        },
    );
}

/// Whether the session's base and inclusions are the ones of `before` (the cuts and the
/// orientation may have changed meanwhile: an inclusion does not depend on them).
fn unchanged_since(host: &Host, before: &RoughModel) -> bool {
    let session = host.session.borrow();
    session.model.base == before.base && session.model.inclusions == before.inclusions
}

/// Makes `base` and `inclusions` the model's, as one undo step, and shows them.
fn install_inclusions(host: &Rc<Host>, base: RoughBase, inclusions: InclusionSet) {
    {
        let mut session = host.session.borrow_mut();
        session.model.base = base;
        session.model.inclusions = inclusions;
        session.commit();
    }
    resync(host, true);
}

/// The UI-thread end of an inclusion job: installs the new base and inclusions as one undo
/// step when the model still has the ones the job started from, and says what happened. The
/// result is `Ok` when they were installed and the reason (also shown) when they were not.
fn finish(
    host: &Rc<Host>,
    before: &RoughModel,
    result: Result<RoughModel, String>,
    failure: &str,
    success: &str,
) -> Result<(), String> {
    let updated = match result {
        Ok(updated) => updated,
        Err(message) => {
            show_error(host, &format!("{failure} {message}"));
            return Err(message);
        }
    };
    if !unchanged_since(host, before) {
        show_error(host, ROUGH_CHANGED_MESSAGE);
        return Err(ROUGH_CHANGED_MESSAGE.to_owned());
    }
    install_inclusions(host, updated.base, updated.inclusions);
    show_status(host, success);
    Ok(())
}
