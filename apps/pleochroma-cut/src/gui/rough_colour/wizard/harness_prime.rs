//! UI harness only (feature `ui-harness`): puts the open wizard in the state a cutter has after a
//! capture session, so the Rough Color tutorial video can walk every page of the strip.
//!
//! The wizard opens from a saved plan with the plan's stored color: two cached working-resolution
//! photos, the zones and the fit report (see `harness_seed`). That opens only Compare and Accept,
//! because Photos, Calibration, Masks, Surfaces and Fit stand on facts a camera rig and a capture
//! session produce (`steps::StepFacts`): a picked rig, the mesh aligned to it in the Locate window,
//! a stone photo and a white frame for two views, and an applied calibration. None of that can be
//! filmed (it needs the rig, its alignment, the photos and a fit of minutes), and loading a photo
//! through the window would also throw the restored fit away. So this fills those facts directly:
//!
//! * a stored rig: the default eight-camera layout, named [`RIG_NAME`];
//! * the alignment: the rough's mesh centred on the rig's origin;
//! * the files of the first views: a stone photo and a white frame each (the caller writes small
//!   synthetic pictures and passes their paths; nothing is read from them unless Prepare is pressed);
//! * the calibration: the sRGB camera fallback and the first CIE LED kind.
//!
//! The restored views, zones and fit stay as they are, so every step stays reachable. Nothing is
//! stored: the state lives in the window until it closes.

use super::{
    context::AlignmentSnapshot,
    host::{Host, with_host},
    view_model::push_all,
    work::{self, BacklightChoice, CameraChoice},
};
use pleochroma_cut_core::rough_plan::locate::{Projection, RigProfile, Rigid};
use std::{path::PathBuf, rc::Rc};

/// The name of the seeded rig, as the Checklist step's rig list shows it.
pub const RIG_NAME: &str = "Backlit Rig (8 Cameras)";

/// How far the eight cameras stand from the stone, mm.
const RIG_DISTANCE_MM: f64 = 400.0;

/// How far above and below the horizontal the cameras look, degrees.
const RIG_ELEVATION_DEG: f64 = 20.0;

/// The focal length of every camera, in pixels: the 20 mm rough fills about two thirds of the
/// image height at [`RIG_DISTANCE_MM`].
const RIG_FOCAL_PX: f64 = 30_000.0;

/// The size of every camera image, pixels.
const RIG_IMAGE_SIZE: [u32; 2] = [3_200, 2_400];

/// The refractive index the rig assumes for the stone (quartz).
const RIG_STONE_N: f64 = 1.55;

/// The mean white frame colour the calibration is built from, linear RGB.
const WHITE_RGB: [f64; 3] = [0.9, 0.9, 0.9];

/// Half the edge of the seeded rough (a 20 mm cube), mm: the translation that centres it.
const ROUGH_HALF_EDGE_MM: f64 = 10.0;

/// The fewest views the wizard needs photos for before it opens the Calibration step.
const MIN_VIEWS: usize = 2;

/// Fills the open wizard's rig, alignment, photo files and calibration (see the module comment).
/// `photos` holds one `(stone photo, white frame)` pair per view, starting with the first view.
///
/// # Errors
///
/// The wizard is not open, the Rough Planner has no mesh rough, fewer than two pairs are given,
/// or the calibration cannot be built; the sentence says which.
pub fn prime(photos: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    with_host(|host| prime_host(host, photos))
        .unwrap_or_else(|| Err("The rough color wizard is not open.".to_owned()))
}

fn prime_host(host: &Rc<Host>, photos: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    if photos.len() < MIN_VIEWS {
        return Err(format!(
            "The wizard needs photos for at least {MIN_VIEWS} views, {} given.",
            photos.len()
        ));
    }
    {
        let mut state = host.state.borrow_mut();
        let mesh_id = state
            .context
            .as_ref()
            .map(|context| context.mesh_id)
            .ok_or("The Rough Planner has no mesh rough.")?;
        let views = RigProfile::side_layout(
            RIG_DISTANCE_MM,
            RIG_ELEVATION_DEG,
            Projection::Pinhole {
                focal_px: RIG_FOCAL_PX,
            },
            RIG_IMAGE_SIZE,
        );
        state.rigs = vec![RigProfile::new(RIG_NAME, views, RIG_STONE_N)];
        state.rig_index = Some(0);
        state.alignment = Some(AlignmentSnapshot {
            rig_name: RIG_NAME.to_owned(),
            transform: Rigid {
                rotation: [0.0; 3],
                translation: [-ROUGH_HALF_EDGE_MM; 3],
            },
            mesh_id,
            photos: photos
                .iter()
                .map(|(stone, _)| Some(stone.clone()))
                .collect(),
        });
        state.sync_views();
        for (view, (stone, white)) in photos.iter().enumerate() {
            if let Some(files) = state.files.get_mut(view) {
                files.stone = Some(stone.clone());
                files.white = vec![white.clone()];
            }
        }
        state.camera = CameraChoice::Fallback;
        let light = BacklightChoice::Led(state.led);
        let calibration = work::build_calibration(&state.camera, &light, WHITE_RGB)?;
        state.backlight = light;
        state.calibration = Some(calibration);
    }
    push_all(host);
    Ok(())
}
