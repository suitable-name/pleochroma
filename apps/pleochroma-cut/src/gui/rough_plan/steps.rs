//! The four steps of the planner's left column and the window's header line.
//!
//! Each step has a done flag, a folded one-line summary and an error line. [`refresh`] rebuilds
//! them from the session and the window after a model change and on the window's timer. [`build`]
//! is the pure part: it turns the plain [`StepFacts`] into the texts, so the rules are tested
//! without a window.
//!
//! - **Rough** is done when the model is valid: it has a size, evaluates, and no scan is being
//!   read. Its error is the model error.
//! - **Crystal Axis** is done when the axis is known, or when the material has none.
//! - **Shape Cuts** is done when every cut row is valid (no cuts at all is valid).
//! - **Plan Settings** is always done: its fields have defaults.

use super::{base::is_blank, cut_rows::fmt_num, host::Host};
use crate::RoughPlanModel;
use pleochroma_cut_core::{rough_plan::RoughBase, yield_metrics::carat_weight};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The window's `RoughFinding.level` of a finding that needs no attention (a plain "ok").
const FINDING_OK: i32 = 0;

/// The separator of the parts of a summary line.
const DOT: &str = " \u{b7} ";

/// What kind of rough the first step sums up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Shape {
    /// A block.
    Block,
    /// A cylinder.
    Cylinder {
        /// Its diameter in mm.
        diameter_mm: f64,
        /// Its length in mm.
        length_mm: f64,
    },
    /// A water-worn pebble.
    Pebble,
    /// A 3D scan (an imported mesh file).
    Scan,
}

/// What the Crystal Axis step knows about the rough's axis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AxisFact {
    /// The material has no crystal axis (it is isotropic): the step does not apply.
    NotApplicable,
    /// The material has one and the rough's axis is Unknown.
    Unknown,
    /// The axis is set; the text is the axis in words.
    Set(String),
}

/// Everything the four steps are worded from, as plain values.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StepFacts {
    /// The kind of rough.
    pub(super) shape: Shape,
    /// The rough's bounding box in mm.
    pub(super) extents_mm: [f64; 3],
    /// Whether the rough has no size yet.
    pub(super) blank: bool,
    /// The material's name, empty when none is picked.
    pub(super) material: String,
    /// The model's weight in carat, when it evaluates and the material is known.
    pub(super) carat: Option<f64>,
    /// The model error, empty for a valid model.
    pub(super) model_error: String,
    /// What the background mesh job is called while a scan is being read, else empty.
    pub(super) reading: String,
    /// The scan's file name, empty for a rough that is not a scan (or one not named yet).
    pub(super) scan_file: String,
    /// How many findings of the scan's import report need a look.
    pub(super) scan_notes: usize,
    /// What is known about the rough's crystal axis.
    pub(super) axis: AxisFact,
    /// The error of the last axis pick, fit or typed value, empty when there is none.
    pub(super) axis_error: String,
    /// How many shape cuts the model has.
    pub(super) cuts: usize,
    /// The first error among the cut rows, empty when every row is valid.
    pub(super) cut_error: String,
    /// The most different designs a layout may use.
    pub(super) max_designs: i32,
    /// The most stones a layout may hold, `0` for no limit.
    pub(super) max_stones: i32,
    /// Whether the candidate designs are the library filter's (else the whole library).
    pub(super) use_filter: bool,
    /// The count of the candidate set as the window words it ("321", "counting..." or "").
    pub(super) candidate_count: String,
}

/// What the steps and the header show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct StepsView {
    /// Whether each of the four steps is complete.
    pub(super) done: [bool; 4],
    /// The one-line summary of each step.
    pub(super) summary: [String; 4],
    /// The error line of each step, empty for none.
    pub(super) error: [String; 4],
    /// The line under the window's title.
    pub(super) subtitle: String,
}

/// "1 note", "2 notes".
fn count_text(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

/// `"30 \u{d7} 60 \u{d7} 20 mm"`: the extents with at most two decimals.
fn size_text(extents_mm: [f64; 3]) -> String {
    let [x, y, z] = extents_mm.map(|extent| fmt_num(extent, 2));
    format!("{x} \u{d7} {y} \u{d7} {z} mm")
}

/// The weight as "94.40 ct".
fn carat_text(carat: f64) -> String {
    format!("{carat:.2} ct")
}

/// The word for a kind of rough.
const fn kind_word(shape: Shape) -> &'static str {
    match shape {
        Shape::Block => "block",
        Shape::Cylinder { .. } => "cylinder",
        Shape::Pebble => "pebble",
        Shape::Scan => "scan",
    }
}

/// `text` with its first letter upper-cased.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// The size of the rough in words: the box, or for a cylinder its diameter and length.
fn shape_size_text(facts: &StepFacts) -> String {
    match facts.shape {
        Shape::Cylinder {
            diameter_mm,
            length_mm,
        } => format!(
            "\u{d8} {} \u{d7} {} mm",
            fmt_num(diameter_mm, 2),
            fmt_num(length_mm, 2)
        ),
        Shape::Block | Shape::Pebble | Shape::Scan => size_text(facts.extents_mm),
    }
}

/// The folded line of the Rough step.
fn rough_summary(facts: &StepFacts) -> String {
    if !facts.reading.is_empty() {
        return facts.reading.clone();
    }
    if facts.blank {
        return "Pick a shape and enter its size".to_string();
    }
    let mut parts = Vec::new();
    if facts.shape == Shape::Scan {
        parts.push(if facts.scan_file.is_empty() {
            "Scan".to_string()
        } else {
            format!("Scan {}", facts.scan_file)
        });
        parts.extend(facts.carat.map(carat_text));
        if facts.scan_notes > 0 {
            parts.push(count_text(facts.scan_notes, "note"));
        }
    } else {
        parts.push(format!(
            "{} {}",
            capitalised(kind_word(facts.shape)),
            shape_size_text(facts)
        ));
        if !facts.material.is_empty() {
            parts.push(facts.material.clone());
        }
        parts.extend(facts.carat.map(carat_text));
    }
    parts.join(DOT)
}

/// The folded line of the Crystal Axis step.
fn axis_summary(facts: &StepFacts) -> String {
    match &facts.axis {
        AxisFact::NotApplicable => "This material has no crystal axis".to_string(),
        AxisFact::Set(sentence) if !sentence.is_empty() => sentence.clone(),
        AxisFact::Set(_) | AxisFact::Unknown => "Unknown".to_string(),
    }
}

/// The folded line of the Shape Cuts step.
fn cuts_summary(facts: &StepFacts) -> String {
    let count = if facts.cuts == 0 {
        "none yet".to_string()
    } else {
        count_text(facts.cuts, "cut")
    };
    if facts.shape == Shape::Scan {
        format!("Face cuts only on a scan{DOT}{count}")
    } else if facts.cuts == 0 {
        "No cuts yet".to_string()
    } else {
        count
    }
}

/// "current filter, 321 designs", "whole library, counting..." or just the set's name when
/// there is no count yet.
fn candidates_text(use_filter: bool, count: &str) -> String {
    let set = if use_filter {
        "current filter"
    } else {
        "whole library"
    };
    if count.starts_with(|c: char| c.is_ascii_digit()) {
        let noun = if count == "1" { "design" } else { "designs" };
        format!("{set}, {count} {noun}")
    } else {
        set.to_string()
    }
}

/// The folded line of the Plan Settings step.
fn settings_summary(facts: &StepFacts) -> String {
    let designs = format!(
        "Up to {} design{}",
        facts.max_designs,
        if facts.max_designs == 1 { "" } else { "s" }
    );
    let mut parts = vec![designs];
    let stones = usize::try_from(facts.max_stones).unwrap_or(0);
    if stones > 0 {
        parts.push(format!("up to {}", count_text(stones, "stone")));
    }
    parts.push(candidates_text(facts.use_filter, &facts.candidate_count));
    parts.join(DOT)
}

/// The line under the window's title: the rough in one line.
fn subtitle(facts: &StepFacts) -> String {
    if facts.blank {
        return "No rough yet".to_string();
    }
    let kind = kind_word(facts.shape);
    let mut parts = vec![if facts.material.is_empty() {
        capitalised(kind)
    } else {
        format!("{} {kind}", facts.material)
    }];
    if facts.shape == Shape::Scan && !facts.scan_file.is_empty() {
        parts.push(facts.scan_file.clone());
    }
    parts.push(shape_size_text(facts));
    parts.extend(facts.carat.map(carat_text));
    if facts.cuts > 0 {
        parts.push(count_text(facts.cuts, "shape cut"));
    }
    parts.join(DOT)
}

/// What the steps and the header show for `facts`.
#[must_use]
pub(super) fn build(facts: &StepFacts) -> StepsView {
    StepsView {
        done: [
            !facts.blank && facts.model_error.is_empty() && facts.reading.is_empty(),
            facts.axis != AxisFact::Unknown,
            facts.cut_error.is_empty(),
            true,
        ],
        summary: [
            rough_summary(facts),
            axis_summary(facts),
            cuts_summary(facts),
            settings_summary(facts),
        ],
        error: [
            facts.model_error.clone(),
            facts.axis_error.clone(),
            facts.cut_error.clone(),
            String::new(),
        ],
        subtitle: subtitle(facts),
    }
}

/// The kind of rough of `base`.
const fn shape_of(base: &RoughBase) -> Shape {
    match *base {
        RoughBase::Block { .. } => Shape::Block,
        RoughBase::Cylinder {
            diameter_mm,
            length_mm,
            ..
        } => Shape::Cylinder {
            diameter_mm,
            length_mm,
        },
        RoughBase::Pebble { .. } => Shape::Pebble,
        RoughBase::Hull { .. } => Shape::Scan,
    }
}

/// Reads the facts off the session and the window. `None` while the session is being written
/// to (the next refresh reads it).
fn collect(host: &Host) -> Option<StepFacts> {
    let model = host.window.global::<RoughPlanModel>();
    let session = host.session.try_borrow().ok()?;
    let base = session.model.base;
    let material = usize::try_from(model.get_material_index())
        .ok()
        .and_then(|index| session.choices.get(index));
    let carat = session
        .model_measure
        .as_ref()
        .zip(material)
        .filter(|(_, choice)| choice.specific_gravity > 0.0)
        .map(|(measure, choice)| carat_weight(measure.volume_mm3, choice.specific_gravity));
    let rows = model.get_cuts();
    let cut_error = (0..rows.row_count())
        .filter_map(|index| rows.row_data(index))
        .map(|row| row.error.to_string())
        .find(|error| !error.is_empty())
        .unwrap_or_default();
    let findings = model.get_scan_findings();
    let scan_notes = (0..findings.row_count())
        .filter_map(|index| findings.row_data(index))
        .filter(|finding| finding.level > FINDING_OK)
        .count();
    let axis_error = if model.get_axis_note_level() == 3 {
        model.get_axis_note().to_string()
    } else {
        String::new()
    };
    let scan = matches!(base, RoughBase::Hull { .. });
    let axis = if !model.get_axis_available() {
        AxisFact::NotApplicable
    } else if model.get_axis_set() {
        AxisFact::Set(model.get_axis_sentence().to_string())
    } else {
        AxisFact::Unknown
    };
    let use_filter = model.get_use_filter();
    let candidate_count = if use_filter {
        model.get_filter_count_text()
    } else {
        model.get_library_count_text()
    };
    Some(StepFacts {
        shape: shape_of(&base),
        extents_mm: base.bounding_box_extents(),
        blank: is_blank(&session.model),
        material: material
            .map(|choice| choice.name.clone())
            .unwrap_or_default(),
        carat,
        model_error: model.get_model_error().to_string(),
        reading: if model.get_mesh_busy() {
            model.get_mesh_busy_text().to_string()
        } else {
            String::new()
        },
        scan_file: if scan {
            model.get_scan_file().to_string()
        } else {
            String::new()
        },
        scan_notes: if scan { scan_notes } else { 0 },
        axis,
        axis_error,
        cuts: session.model.cuts.len(),
        cut_error,
        max_designs: model.get_max_designs(),
        max_stones: model.get_max_stones(),
        use_filter,
        candidate_count: candidate_count.to_string(),
    })
}

/// Sets a list property to `wanted` unless it already holds exactly that, so an unchanged
/// list is no change for the window.
fn set_list<T: Clone + PartialEq + 'static>(
    current: &ModelRc<T>,
    wanted: &[T],
    set: impl FnOnce(ModelRc<T>),
) {
    let same = current.row_count() == wanted.len()
        && wanted
            .iter()
            .enumerate()
            .all(|(index, item)| current.row_data(index).as_ref() == Some(item));
    if !same {
        set(ModelRc::new(VecModel::from(wanted.to_vec())));
    }
}

/// Writes `view` into the window; what did not change is left alone.
fn push(host: &Host, view: &StepsView) {
    let model = host.window.global::<RoughPlanModel>();
    set_list(&model.get_step_done(), &view.done, |list| {
        model.set_step_done(list);
    });
    let texts = |items: &[String; 4]| -> Vec<SharedString> {
        items.iter().map(|text| text.as_str().into()).collect()
    };
    set_list(&model.get_step_summary(), &texts(&view.summary), |list| {
        model.set_step_summary(list);
    });
    set_list(&model.get_step_error(), &texts(&view.error), |list| {
        model.set_step_error(list);
    });
    if model.get_rough_subtitle().as_str() != view.subtitle {
        model.set_rough_subtitle(view.subtitle.as_str().into());
    }
}

/// Rebuilds the steps and the header line from the session and the window. Called after every
/// model change, and by the window's own timer.
pub(super) fn refresh(host: &Host) {
    if let Some(facts) = collect(host) {
        push(host, &build(&facts));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid 30 x 60 x 20 mm block of tourmaline with four cuts and the default settings.
    fn block() -> StepFacts {
        StepFacts {
            shape: Shape::Block,
            extents_mm: [30.0, 60.0, 20.0],
            blank: false,
            material: "Tourmaline".to_string(),
            carat: Some(94.4),
            model_error: String::new(),
            reading: String::new(),
            scan_file: String::new(),
            scan_notes: 0,
            axis: AxisFact::Set("Along the length".to_string()),
            axis_error: String::new(),
            cuts: 4,
            cut_error: String::new(),
            max_designs: 2,
            max_stones: 0,
            use_filter: true,
            candidate_count: "321".to_string(),
        }
    }

    fn scan() -> StepFacts {
        StepFacts {
            shape: Shape::Scan,
            extents_mm: [30.1, 58.7, 21.4],
            carat: Some(92.8),
            scan_file: "tourmaline_042.stl".to_string(),
            scan_notes: 2,
            cuts: 0,
            ..block()
        }
    }

    #[test]
    fn a_valid_block_sums_up_like_the_mockup() {
        let view = build(&block());
        assert_eq!(view.done, [true, true, true, true]);
        assert_eq!(
            view.summary[0],
            "Block 30 \u{d7} 60 \u{d7} 20 mm \u{b7} Tourmaline \u{b7} 94.40 ct"
        );
        assert_eq!(view.summary[1], "Along the length");
        assert_eq!(view.summary[2], "4 cuts");
        assert_eq!(
            view.summary[3],
            "Up to 2 designs \u{b7} current filter, 321 designs"
        );
        assert_eq!(
            view.error,
            [String::new(), String::new(), String::new(), String::new()]
        );
        assert_eq!(
            view.subtitle,
            "Tourmaline block \u{b7} 30 \u{d7} 60 \u{d7} 20 mm \u{b7} 94.40 ct \u{b7} 4 shape cuts"
        );
    }

    #[test]
    fn a_scan_names_its_file_its_weight_and_its_notes() {
        let view = build(&scan());
        assert_eq!(
            view.summary[0],
            "Scan tourmaline_042.stl \u{b7} 92.80 ct \u{b7} 2 notes"
        );
        assert_eq!(view.summary[2], "Face cuts only on a scan \u{b7} none yet");
        assert_eq!(
            view.subtitle,
            "Tourmaline scan \u{b7} tourmaline_042.stl \u{b7} 30.1 \u{d7} 58.7 \u{d7} 21.4 mm \u{b7} 92.80 ct"
        );
        let one_note = StepFacts {
            scan_notes: 1,
            cuts: 2,
            ..scan()
        };
        let view = build(&one_note);
        assert!(view.summary[0].ends_with("1 note"), "{}", view.summary[0]);
        assert_eq!(view.summary[2], "Face cuts only on a scan \u{b7} 2 cuts");
        let quiet = StepFacts {
            scan_notes: 0,
            ..scan()
        };
        assert_eq!(
            build(&quiet).summary[0],
            "Scan tourmaline_042.stl \u{b7} 92.80 ct"
        );
    }

    #[test]
    fn the_rough_step_is_done_only_for_a_valid_sized_model_that_is_not_being_read() {
        assert!(build(&block()).done[0]);
        let blank = StepFacts {
            blank: true,
            carat: None,
            ..block()
        };
        let view = build(&blank);
        assert!(!view.done[0]);
        assert_eq!(view.summary[0], "Pick a shape and enter its size");
        assert_eq!(view.subtitle, "No rough yet");
        assert_eq!(view.error[0], "", "an unsized rough is not an error yet");

        let invalid = StepFacts {
            model_error: "The cuts leave nothing of the rough.".to_string(),
            carat: None,
            ..block()
        };
        let view = build(&invalid);
        assert!(!view.done[0]);
        assert_eq!(view.error[0], "The cuts leave nothing of the rough.");

        let reading = StepFacts {
            reading: "Reading the mesh file...".to_string(),
            ..scan()
        };
        let view = build(&reading);
        assert!(!view.done[0]);
        assert_eq!(view.summary[0], "Reading the mesh file...");
    }

    #[test]
    fn the_axis_step_is_done_when_known_or_when_the_material_has_none() {
        let unknown = StepFacts {
            axis: AxisFact::Unknown,
            ..block()
        };
        let view = build(&unknown);
        assert!(!view.done[1]);
        assert_eq!(view.summary[1], "Unknown");

        let isotropic = StepFacts {
            axis: AxisFact::NotApplicable,
            ..block()
        };
        let view = build(&isotropic);
        assert!(view.done[1]);
        assert_eq!(view.summary[1], "This material has no crystal axis");

        let failed = StepFacts {
            axis_error: "No face was hit.".to_string(),
            ..block()
        };
        assert_eq!(build(&failed).error[1], "No face was hit.");
    }

    #[test]
    fn the_cuts_step_is_done_when_every_row_is_valid_and_none_is_valid_too() {
        let none = StepFacts { cuts: 0, ..block() };
        let view = build(&none);
        assert!(view.done[2]);
        assert_eq!(view.summary[2], "No cuts yet");
        assert_eq!(build(&StepFacts { cuts: 1, ..block() }).summary[2], "1 cut");

        let bad = StepFacts {
            cut_error: "Setback A must be positive.".to_string(),
            ..block()
        };
        let view = build(&bad);
        assert!(!view.done[2]);
        assert_eq!(view.error[2], "Setback A must be positive.");
    }

    #[test]
    fn the_plan_settings_step_is_always_done() {
        let broken = StepFacts {
            blank: true,
            model_error: "x".to_string(),
            cut_error: "y".to_string(),
            axis: AxisFact::Unknown,
            ..block()
        };
        assert!(build(&broken).done[3]);
        assert_eq!(build(&broken).error[3], "");
    }

    #[test]
    fn the_settings_line_names_the_caps_and_the_candidate_set() {
        let capped = StepFacts {
            max_designs: 1,
            max_stones: 6,
            use_filter: false,
            candidate_count: "3,299".to_string(),
            ..block()
        };
        assert_eq!(
            build(&capped).summary[3],
            "Up to 1 design \u{b7} up to 6 stones \u{b7} whole library, 3,299 designs"
        );
        let one = StepFacts {
            max_stones: 1,
            candidate_count: "1".to_string(),
            ..block()
        };
        assert_eq!(
            build(&one).summary[3],
            "Up to 2 designs \u{b7} up to 1 stone \u{b7} current filter, 1 design"
        );
        // While the count is not in, the set is named without it.
        let counting = StepFacts {
            candidate_count: "counting...".to_string(),
            ..block()
        };
        assert!(build(&counting).summary[3].ends_with("current filter"));
        assert_eq!(candidates_text(true, ""), "current filter");
    }

    #[test]
    fn a_cylinder_and_a_pebble_are_worded_by_their_own_sizes() {
        let cylinder = StepFacts {
            shape: Shape::Cylinder {
                diameter_mm: 12.0,
                length_mm: 30.5,
            },
            extents_mm: [12.0, 30.5, 12.0],
            cuts: 0,
            ..block()
        };
        assert_eq!(
            build(&cylinder).summary[0],
            "Cylinder \u{d8} 12 \u{d7} 30.5 mm \u{b7} Tourmaline \u{b7} 94.40 ct"
        );
        let pebble = StepFacts {
            shape: Shape::Pebble,
            material: String::new(),
            carat: None,
            cuts: 0,
            ..block()
        };
        let view = build(&pebble);
        assert_eq!(view.summary[0], "Pebble 30 \u{d7} 60 \u{d7} 20 mm");
        assert_eq!(view.subtitle, "Pebble \u{b7} 30 \u{d7} 60 \u{d7} 20 mm");
    }

    #[test]
    fn a_list_is_set_only_when_it_differs() {
        let current = ModelRc::new(VecModel::from(vec![true, false, true, true]));
        let mut sets = 0;
        set_list(&current, &[true, false, true, true], |_| sets += 1);
        assert_eq!(sets, 0, "the same four values are no change");
        set_list(&current, &[true, true, true, true], |_| sets += 1);
        assert_eq!(sets, 1);
        set_list(&current, &[true, false], |_| sets += 1);
        assert_eq!(sets, 2, "another length is a change");
    }
}
