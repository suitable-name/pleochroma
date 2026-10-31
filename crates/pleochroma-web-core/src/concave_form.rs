//! The Tier tab's concave form without the window.
//!
//! It holds the tool combo's mapping, a blank draft, the labels of the Keep Draft / Discard
//! Draft choice, the guard against saving over a tier that changed under the draft, and the
//! save itself ([`apply_save`]).
//!
//! The page's `editor::inspector::concave` decides WHEN to seed and push (it reads and writes
//! `InspectorModel`); what a draft is, whether it is stale and what a save does to the design
//! are these plain functions over an `EditorSession`.
//!
//! A save is `parse_concave_tier_form` -> `concave_name_clash_message` -> `concave_tier_save_edit`
//! -> `EditorSession::apply`.

use pleochroma_cut_core::design::{ConcaveTool, Design};
use pleochroma_editor::{
    loading::{
        ConcaveTierFormFields, concave_tier_form_fields, parse_concave_tier_form,
        tier_form_error_field,
    },
    session::EditorSession,
    tier_save::{concave_name_clash_message, concave_tier_save_edit},
};

/// The combo index of tool `code` (the desktop's order: CYL, CON, CIR, DSC, SPH); the one
/// mapping the lessons' recipes use too.
pub use crate::guide_perform::tool_index;

/// The message of a save refused because the tier changed under the draft.
pub const STALE_MESSAGE: &str = "This concave tier changed in the design while you were editing it. \
                                 Use Reload From Design, or Keep Draft to write your draft over the change.";

/// The message of a save aimed at a concave tier that is gone.
pub const GONE_MESSAGE: &str =
    "This concave tier no longer exists. Use New Concave Tier to add it again.";

/// Where a discarded draft goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pending {
    /// Load concave tier `n`.
    Concave(usize),
    /// Show the flat form (flat tier `n` is selected).
    Flat(usize),
    /// A blank concave draft.
    Blank,
    /// A blank flat draft ("New Flat Tier Instead").
    NewFlat,
}

/// The label of the button that discards the draft for `pending`.
#[must_use]
pub fn pending_label(pending: Pending) -> String {
    match pending {
        Pending::Concave(index) => format!("Discard Draft, Load Concave #{}", index + 1),
        Pending::Flat(index) => format!("Discard Draft, Load Tier #{}", index + 1),
        Pending::Blank => "Discard Draft, Start New".to_owned(),
        Pending::NewFlat => "Discard Draft, Start New Flat Tier".to_owned(),
    }
}

/// What the form was seeded from: the guard against saving over a changed tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seed {
    /// The concave tier the form describes; `None` for an "Add" draft.
    pub index: Option<usize>,
    /// The design generation at seeding.
    pub generation: u64,
    /// The tier's fields as seeded (full precision).
    pub baseline: ConcaveTierFormFields,
    /// The user chose Keep Draft on the "changed in the design" notice: saving may overwrite.
    pub acknowledged: bool,
}

impl Seed {
    /// Whether the tier this draft is for no longer reads as it did when the draft started.
    ///
    /// Only a generation that moved can have changed it, so an untouched design is cheap.
    #[must_use]
    pub fn is_stale(&self, design: &Design, generation: u64) -> bool {
        if generation == self.generation {
            return false;
        }
        let Some(index) = self.index else {
            return false;
        };
        design
            .concave_tiers
            .get(index)
            .map(concave_tier_form_fields)
            .as_ref()
            != Some(&self.baseline)
    }
}

/// The tool code of combo index `index`.
#[must_use]
pub fn tool_code(index: i32) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|index| ConcaveTool::ALL.get(index).copied())
        .map_or("CYL", ConcaveTool::code)
}

/// A blank draft: a stroked cylinder, everything else for the cutter to give.
///
/// The angle has no default on purpose: the rule is `0 < |angle| < 90`, so Save asks for it.
#[must_use]
pub fn blank_fields() -> ConcaveTierFormFields {
    ConcaveTierFormFields {
        tool: "CYL".to_owned(),
        reciprocating: true,
        ..ConcaveTierFormFields::default()
    }
}

/// What a successful save reports.
#[derive(Debug)]
pub struct Saved {
    /// Where the tier now sits in the concave list.
    pub index: usize,
    /// Whether it was added (rather than modified).
    pub added: bool,
    /// The tier's name, for the toast.
    pub name: String,
}

/// A failed save: the message and the field it concerns.
#[derive(Debug)]
pub struct SaveError {
    /// What went wrong, in the form's words.
    pub message: String,
    /// The form field the message belongs under (`""` for none).
    pub field: &'static str,
}

impl SaveError {
    fn new(message: String) -> Self {
        let field = tier_form_error_field(&message);
        Self { message, field }
    }
}

/// Parses `fields` and applies them as `AddConcaveTier` (`index` `None`) or
/// `ModifyConcaveTier`, with the desktop's checks plus the stale guard.
///
/// # Errors
///
/// The reason, with the field it concerns, when the tier changed under the draft and the draft
/// was not kept, when `index` no longer names a concave tier, when a field does not parse, when
/// another concave tier holds the name, or when the edit is refused.
pub fn apply_save(
    session: &mut EditorSession,
    fields: &ConcaveTierFormFields,
    index: Option<usize>,
    seed: Option<&Seed>,
) -> Result<Saved, SaveError> {
    let design = &session.design;
    if let Some(seed) = seed
        && !seed.acknowledged
        && seed.is_stale(design, session.current_generation())
    {
        return Err(SaveError {
            message: STALE_MESSAGE.to_owned(),
            field: "",
        });
    }
    if index.is_some_and(|index| index >= design.concave_tiers.len()) {
        return Err(SaveError {
            message: GONE_MESSAGE.to_owned(),
            field: "",
        });
    }
    let tier = parse_concave_tier_form(fields, design.meta.gear_teeth).map_err(SaveError::new)?;
    if let Some(message) = concave_name_clash_message(design, index, &tier.name) {
        return Err(SaveError::new(message));
    }
    let edit = concave_tier_save_edit(design, index, tier);
    session
        .apply(edit)
        .map_err(|error| SaveError::new(error.to_string()))?;
    let saved_index = index.unwrap_or_else(|| session.design.concave_tiers.len().saturating_sub(1));
    let name = session
        .design
        .concave_tiers
        .get(saved_index)
        .map(|tier| tier.name.clone())
        .unwrap_or_default();
    Ok(Saved {
        index: saved_index,
        added: index.is_none(),
        name,
    })
}

#[cfg(test)]
mod tests {
    //! The tool mapping, the labels, the save path and the stale guard.

    use super::*;
    use pleochroma_editor::loading::{parse_new_design_form, parse_preform_form};

    /// An empty design (96 index gear) with no tiers, built the way New Design builds one.
    fn session() -> EditorSession {
        let preform = parse_preform_form(1, "1.50", "1.00", "1.50", 96).expect("preform");
        let spec = parse_new_design_form(96, preform, "8", true, 0).expect("spec");
        EditorSession::from_template(spec, 0)
    }

    /// A valid pavilion-side cylinder draft named `name`.
    fn draft(name: &str) -> ConcaveTierFormFields {
        ConcaveTierFormFields {
            name: name.to_owned(),
            angle_deg: "-42".to_owned(),
            indices: "0 12".to_owned(),
            diameter_ratio: "0.2".to_owned(),
            ..blank_fields()
        }
    }

    fn seed_of(session: &EditorSession, index: Option<usize>) -> Seed {
        Seed {
            index,
            generation: session.current_generation(),
            baseline: index
                .and_then(|i| session.design.concave_tiers.get(i))
                .map_or_else(blank_fields, concave_tier_form_fields),
            acknowledged: false,
        }
    }

    #[test]
    fn the_tool_combo_follows_the_desktop_order() {
        let codes: Vec<&str> = (0..5).map(tool_code).collect();
        assert_eq!(codes, ["CYL", "CON", "CIR", "DSC", "SPH"]);
        for (index, code) in codes.iter().enumerate() {
            assert_eq!(tool_index(code), i32::try_from(index).unwrap());
        }
        assert_eq!(tool_index("dsc"), 3, "case-insensitive");
        assert_eq!(tool_index("nonsense"), 0);
        assert_eq!(tool_code(-1), "CYL");
        assert_eq!(tool_code(9), "CYL");
    }

    #[test]
    fn only_a_cone_and_a_disc_take_a_tool_angle() {
        let takers: Vec<i32> = (0..5)
            .filter(|&i| {
                tool_code(i)
                    .parse::<ConcaveTool>()
                    .is_ok_and(ConcaveTool::takes_angle)
            })
            .collect();
        assert_eq!(takers, [1, 3], "the combo indices the Slint form enables");
    }

    #[test]
    fn a_blank_draft_is_a_stroked_cylinder_without_an_angle() {
        let fields = blank_fields();
        assert_eq!(fields.tool, "CYL");
        assert!(fields.reciprocating);
        assert!(fields.angle_deg.is_empty());
        let error = parse_concave_tier_form(&fields, 96).unwrap_err();
        assert_eq!(tier_form_error_field(&error), "angle", "{error}");
    }

    #[test]
    fn the_pending_labels_name_where_the_draft_would_go() {
        assert_eq!(
            pending_label(Pending::Concave(1)),
            "Discard Draft, Load Concave #2"
        );
        assert_eq!(
            pending_label(Pending::Flat(2)),
            "Discard Draft, Load Tier #3"
        );
        assert_eq!(pending_label(Pending::Blank), "Discard Draft, Start New");
        assert_eq!(
            pending_label(Pending::NewFlat),
            "Discard Draft, Start New Flat Tier"
        );
    }

    #[test]
    fn a_new_draft_is_added_at_the_end_and_selected_by_index() {
        let mut session = session();
        let saved = apply_save(&mut session, &draft("A"), None, None)
            .ok()
            .unwrap();
        assert!(saved.added);
        assert_eq!(saved.index, 0);
        assert_eq!(saved.name, "A");
        let saved = apply_save(&mut session, &draft("B"), None, None)
            .ok()
            .unwrap();
        assert_eq!(saved.index, 1, "appended after the first");
        assert_eq!(session.design.concave_tiers.len(), 2);
    }

    #[test]
    fn saving_a_loaded_tier_modifies_it_in_one_undoable_step() {
        let mut session = session();
        apply_save(&mut session, &draft("A"), None, None)
            .ok()
            .unwrap();
        let mut fields = concave_tier_form_fields(&session.design.concave_tiers[0]);
        fields.diameter_ratio = "0.3".to_owned();
        let seed = seed_of(&session, Some(0));
        let saved = apply_save(&mut session, &fields, Some(0), Some(&seed))
            .ok()
            .unwrap();
        assert!(!saved.added);
        assert_eq!(saved.index, 0);
        assert!((session.design.concave_tiers[0].diameter_ratio - 0.3).abs() < 1e-12);
        session.undo().unwrap();
        assert!((session.design.concave_tiers[0].diameter_ratio - 0.2).abs() < 1e-12);
    }

    #[test]
    fn a_save_with_nothing_retyped_round_trips_the_tier() {
        let mut session = session();
        apply_save(&mut session, &draft("A"), None, None)
            .ok()
            .unwrap();
        let before = session.design.concave_tiers[0].clone();
        let fields = concave_tier_form_fields(&before);
        let seed = seed_of(&session, Some(0));
        apply_save(&mut session, &fields, Some(0), Some(&seed))
            .ok()
            .unwrap();
        assert_eq!(session.design.concave_tiers[0], before);
    }

    #[test]
    fn a_bad_field_names_its_bucket() {
        let mut session = session();
        let mut fields = draft("A");
        fields.tool_azimuth_deg = "abc".to_owned();
        let error = apply_save(&mut session, &fields, None, None).err().unwrap();
        assert_eq!(error.field, "theta", "{}", error.message);
        assert!(session.design.concave_tiers.is_empty(), "nothing applied");
        let mut fields = draft("A");
        fields.diameter_ratio = "0".to_owned();
        let error = apply_save(&mut session, &fields, None, None).err().unwrap();
        assert_eq!(error.field, "diameter", "{}", error.message);
    }

    #[test]
    fn a_name_another_concave_tier_holds_is_refused_on_the_name_field() {
        let mut session = session();
        apply_save(&mut session, &draft("A"), None, None)
            .ok()
            .unwrap();
        let error = apply_save(&mut session, &draft("a"), None, None)
            .err()
            .unwrap();
        assert_eq!(error.field, "name", "{}", error.message);
        assert_eq!(session.design.concave_tiers.len(), 1);
        // Saving the tier under its own name is not a clash.
        let seed = seed_of(&session, Some(0));
        apply_save(&mut session, &draft("A"), Some(0), Some(&seed))
            .ok()
            .unwrap();
    }

    #[test]
    fn a_tier_that_changed_under_the_draft_refuses_the_save_until_kept() {
        let mut session = session();
        apply_save(&mut session, &draft("A"), None, None)
            .ok()
            .unwrap();
        let mut seed = seed_of(&session, Some(0));
        // Another edit changes the tier after the draft started.
        let mut other = draft("A");
        other.diameter_ratio = "0.4".to_owned();
        apply_save(&mut session, &other, Some(0), None)
            .ok()
            .unwrap();
        assert!(seed.is_stale(&session.design, session.current_generation()));
        let mut mine = draft("A");
        mine.indices = "0 24".to_owned();
        let error = apply_save(&mut session, &mine, Some(0), Some(&seed))
            .err()
            .unwrap();
        assert_eq!(error.message, STALE_MESSAGE);
        assert!((session.design.concave_tiers[0].diameter_ratio - 0.4).abs() < 1e-12);
        // Keep Draft acknowledges the change: the draft may overwrite it.
        seed.acknowledged = true;
        apply_save(&mut session, &mine, Some(0), Some(&seed))
            .ok()
            .unwrap();
        assert!((session.design.concave_tiers[0].diameter_ratio - 0.2).abs() < 1e-12);
    }

    #[test]
    fn an_edit_elsewhere_that_leaves_the_tier_alone_is_not_stale() {
        let mut session = session();
        apply_save(&mut session, &draft("A"), None, None)
            .ok()
            .unwrap();
        let seed = seed_of(&session, Some(0));
        apply_save(&mut session, &draft("B"), None, None)
            .ok()
            .unwrap();
        assert!(!seed.is_stale(&session.design, session.current_generation()));
        assert!(
            !seed_of(&session, None).is_stale(&session.design, 0),
            "an Add draft never is"
        );
    }

    #[test]
    fn a_draft_for_a_removed_tier_is_refused() {
        let mut session = session();
        let error = apply_save(&mut session, &draft("A"), Some(3), None)
            .err()
            .unwrap();
        assert_eq!(error.message, GONE_MESSAGE);
        assert!(session.design.concave_tiers.is_empty());
    }
}
