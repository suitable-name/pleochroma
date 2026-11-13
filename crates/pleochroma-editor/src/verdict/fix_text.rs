//! The words a [`FixAction`] shows: the button's label, its tooltip, and the heading and button
//! of the question asked before a fix that carries one ([`super::Reason::confirm`]).

use super::FixAction;

impl FixAction {
    /// The button's label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::SnapToTeeth { .. } => "Snap to Teeth",
            Self::RemoveVanished { .. } => "Remove",
            Self::MoveAfter { .. } => "Move Later",
            Self::SteepenPavilion { .. } => "Steepen",
            Self::AddTable => "Add a Table",
            Self::SnapToMeets => "Snap to Meets",
        }
    }

    /// What the button does, for its tooltip.
    #[must_use]
    pub const fn hint(&self) -> &'static str {
        match self {
            Self::SnapToTeeth { .. } => {
                "Moves each index position of this tier to the nearest gear tooth."
            }
            Self::RemoveVanished { .. } => {
                "Removes the facets that are cut away. One undo brings them back."
            }
            Self::MoveAfter { .. } => {
                "Moves this tier to just after the tier it meets, so that tier is cut first."
            }
            Self::SteepenPavilion { .. } => {
                "Raises this facet to the critical angle plus 2 degrees. It turns about its \
                 girdle corners, so the girdle outline stays where it is."
            }
            Self::AddTable => {
                "Adds a flat table facet at a height that keeps all the crown facets."
            }
            Self::SnapToMeets => {
                "Moves the depth of each tier with no meet point, when one is close, so it cuts \
                 to the nearest point already on the stone. One undo puts the depths back."
            }
        }
    }

    /// Whether the question for this fix comes with its plan ([`super::FixPlan::confirm`]) and
    /// is asked once planning is done, before the edit is applied: the snap to meet points names
    /// the facets it would cut away, which only planning finds. A desktop or web caller that
    /// asks after planning skips the reason's own question ([`super::Reason::confirm`]) for such
    /// a fix; a caller that only asks before planning still has that one.
    #[must_use]
    pub const fn asks_after_planning(&self) -> bool {
        matches!(self, Self::SnapToMeets)
    }

    /// The heading of the question asked before the fix is applied, for a reason (or a plan)
    /// that carries one.
    #[must_use]
    pub const fn confirm_heading(&self) -> &'static str {
        match self {
            Self::RemoveVanished { .. } => "Remove What Is Cut Away?",
            Self::SnapToMeets => "Move Depths to Meet Points?",
            Self::SnapToTeeth { .. }
            | Self::MoveAfter { .. }
            | Self::SteepenPavilion { .. }
            | Self::AddTable => "Apply This Fix?",
        }
    }

    /// The button that runs the fix in that question.
    #[must_use]
    pub const fn confirm_button(&self) -> &'static str {
        match self {
            Self::RemoveVanished { .. } => "Remove",
            Self::SnapToMeets => "Move",
            Self::SnapToTeeth { .. }
            | Self::MoveAfter { .. }
            | Self::SteepenPavilion { .. }
            | Self::AddTable => "Apply",
        }
    }
}
