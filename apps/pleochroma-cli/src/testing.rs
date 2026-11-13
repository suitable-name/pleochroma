//! Fixtures and helpers for the tests.

use crate::outcome::Outcome;
use pleochroma_cut_core::{ConstraintTier, Design, MaterialSelection, PreformSpec, ScheduleMeta};
use pleochroma_editor::templates::gallery_design;
use std::path::PathBuf;

/// The standard round brilliant template of the New Design gallery, in diamond.
pub fn template() -> Design {
    gallery_design(1)
}

/// A 1.72 round brilliant with no material name, for the retarget tests: retargeting it to
/// index 1.7681 with a third of the shift on the crown gives a valid result with Shift alone.
pub fn brilliant_at_172() -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.material = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.72),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    design.ensure_tier_ids();
    design
}

/// The concave-facet fixture of the cut core: a 16-tooth brilliant with a groove and a dimple.
pub fn concave() -> Design {
    Design::concave_fixture()
}

/// [`concave`] without its concave tiers: the flat shell a scoring path that ignored the tools
/// would have measured. Same flat tiers, same preform, same material.
pub fn concave_shell() -> Design {
    let mut flat = concave();
    flat.concave_tiers.clear();
    flat.concave_tier_ids.clear();
    flat
}

/// The published Pentalume design (`designs/Pentalume_Sapphire`), with its tiers named by their
/// codes.
///
/// Some of its pavilion and crown tiers reach no meet point, and its tiny P3 and C3 cut into the
/// girdle line at the corners of the clipped pentagon.
pub const PENTALUME_ASC: &str = "GemCad 5.0\n\
    g 80 0\n\
    y 5 y\n\
    I 1.7680773735046387\n\
    H Pentalume Sapphire\n\
    a -43 0.7130808923813132 0 n P1 16 32 48 64\n\
    a -40.5 0.7661935481511087 8 n P2 24 40 56 72\n\
    a -44 0.7955073176906 76 n P3 4 12 20 28 36 44 52 60 68\n\
    a 90 1 0 n G1 16 32 48 64 G Set stone size.\n\
    a 90 1.13 8 n G2 24 40 56 72\n\
    a 34 0.5944270003043362 0 n C1 16 32 48 64\n\
    a 30 0.6018060796608384 8 n C2 24 40 56 72\n\
    a 33 0.6346315320439263 76 n C3 4 12 20 28 36 44 52 60 68\n\
    a 0 0.35 n T\n";

/// Cam's Portuguese (PC 45.140, public domain), verbatim: a true meet-point design whose every
/// tier but the anchors meets a point already on the stone, with a level girdle line.
pub const CAMS_ASC: &str = "GemCad 5.0_2017\n\
    g 96 0.0\n\
    y 12 y\n\
    I 2.16\n\
    H PC 45.140  Cam's Portuguese\n\
    H by Greg Ninedorf, 2014\n\
    H Copyright assigned to the public domain.\n\
    a -90.00000 1.07325 96 n 1 88 80 72 64 56 48 40 32 24 16 8 G Set the size and shape\n\
    a -61.71792 0.79638 96 n 2 88 80 72 64 56 48 40 32 24 16 8 G Set the girdle line\n\
    a -51.97569 0.68189 92 n 3 84 76 68 60 52 44 36 28 20 12 4 G Meet the girdle line\n\
    a -47.01841 0.65290 96 n 4 88 80 72 64 56 48 40 32 24 16 8 G Meet 2\n\
    a -42.00000 0.64937 92 n 5 84 76 68 60 52 44 36 28 20 12 4 G Meet 3\n\
    a 32.00000 0.88823 96 n a 8 16 24 32 40 48 56 64 72 80 88 G Set the girdle width\n\
    a 28.00000 0.85428 4 n b 12 20 28 36 44 52 60 68 76 84 92 G Meet the girdle\n\
    a 23.00000 0.83175 96 n c 8 16 24 32 40 48 56 64 72 80 88 G Meet a\n\
    a 0.00000 0.68812 96 n t G Meet b\n\
    F This design works for R.I. 1.76 - 2.16\n";

/// The `.asc` text `text` opened as the file `name`, the way the commands open one.
pub fn asc(name: &str, text: &str) -> crate::load::Loaded {
    crate::load::from_bytes(name, text.as_bytes()).expect("the fixture opens")
}

/// The name, angle and index positions of every tier: what must survive a file round trip.
pub fn shape(design: &Design) -> Vec<(String, f64, Vec<f64>)> {
    design
        .tiers
        .iter()
        .map(|tier| (tier.name.clone(), tier.angle_deg, tier.indices.clone()))
        .collect()
}

/// A path for `name` in a folder of this test process, which is created.
pub fn temp_path(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("pleochroma-cli-tests-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&folder);
    folder.join(name)
}

/// Writes `design` as a `.psdd` file called `name` and returns its path as text.
pub fn write_design(name: &str, design: &Design) -> String {
    let loaded = crate::load::Loaded::in_memory(design.clone(), name);
    let text = loaded
        .design_text(design, &crate::materials::Catalogue::default())
        .expect("the fixture can be written");
    let path = temp_path(name);
    std::fs::write(&path, text).expect("the fixture folder is writable");
    path.to_string_lossy().into_owned()
}

/// Runs the command line made of `parts`, in this process.
pub fn run(parts: &[&str]) -> Outcome {
    let args: Vec<String> = parts.iter().map(|part| (*part).to_string()).collect();
    crate::run_command_line(&args)
}

/// What `outcome` wrote to the file at `path`, if it wrote one.
pub fn file_text<'a>(outcome: &'a Outcome, path: &str) -> Option<&'a str> {
    outcome
        .files
        .iter()
        .find(|file| file.path.to_string_lossy() == path)
        .map(|file| file.text.as_str())
}
