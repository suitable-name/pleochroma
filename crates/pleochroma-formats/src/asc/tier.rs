//! [`AscTier`]: one `a` record -- a single facet tier at a given angle and
//! mast (height) setting, occurring at one or more index-wheel positions.

use super::meet_instruction::{MeetInstruction, parse_meet_instruction};

/// One `a` record: a single facet tier at a given angle and mast (height) setting,
/// occurring at one or more index-wheel positions.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AscTier {
    /// Signed angle from the girdle plane, in degrees: negative is pavilion, positive is
    /// crown. A zero angle is a flat facet, and the SIGN of the zero records its side:
    /// a sign-negative zero (`-0.0`, [`f64::is_sign_negative`]) is the culet, a positive
    /// zero is the table.
    ///
    /// [`super::parse_asc`] folds both file spellings of the culet (a `0` angle with a
    /// negative distance, or a `-0.000000` token) into the sign-negative zero, so no
    /// consumer needs the file order to tell a culet from a table.
    pub angle_deg: f64,
    /// The "mast" / height setting: how far the facet plane is cut from the stone's
    /// center. [`super::parse_asc`] turns the culet's documented negative distance
    /// into a positive one (moving the sign onto [`Self::angle_deg`]'s zero); any
    /// other value is stored as `GemCAD` wrote it, and callers should take the
    /// magnitude when turning this into a plane offset.
    pub mast: f64,
    /// Facet name(s) as written in the file (e.g. "P1", "C7", "G1", "1", "U"). Empty
    /// when the file leaves the facet unnamed. Several distinct names on one tier are
    /// joined with `/`, in first-seen order with consecutive repeats collapsed. This
    /// folded label is what tier-level consumers (meet resolution, cut-sheet labels)
    /// read; [`Self::index_names`] keeps the per-facet placement.
    pub name: String,
    /// Every index-wheel position at which this tier's facet occurs. Usually integers;
    /// fractional positions occur for angles that do not land on a gear tooth.
    pub indices: Vec<f64>,
    /// Every `n <name>` group in file order, as `(position, name)`. `position` indexes
    /// [`Self::indices`]: `GemCAD` binds a name to the index written just before it
    /// (position 0 for a name before any index), and its diagram labels that facet.
    /// Repeats are kept, since one tier can label two facets with the same name.
    ///
    /// [`super::to_asc_string`] writes names back at these positions. A tier built by
    /// hand (for example exported from the editor) has none, and the writer then puts
    /// [`Self::name`] after the first index, `GemCAD`'s default labelling.
    pub index_names: Vec<(usize, String)>,
    /// Free-text notes after the `G` marker, if any (e.g. "Cut to mast depth X.").
    pub notes: String,
}

impl AscTier {
    /// Every distinct name this tier is known by, split back out of the joined
    /// `name` field (see that field's doc comment for why more than one name folds
    /// into a single `/`-joined string). Empty if the tier is unnamed.
    ///
    /// `/` is the separator and is never escaped: a name that itself contains `/`
    /// (typed by hand, for instance) is indistinguishable from a group of names and
    /// is split into several here.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.name.is_empty() {
            Vec::new()
        } else {
            self.name.split('/').collect()
        }
    }

    /// `true` for a zero-angle tier on the pavilion side (the culet): a
    /// sign-negative zero [`Self::angle_deg`]. See that field's doc comment.
    #[must_use]
    pub const fn is_culet(&self) -> bool {
        self.angle_deg == 0.0 && self.angle_deg.is_sign_negative()
    }

    /// Parses `notes` (the raw text after this tier's `G` marker, if any) into a
    /// structured [`MeetInstruction`]. Returns `None` when there are no notes at all.
    ///
    /// Computed on demand from the raw notes, which [`super::to_asc_string`] writes back
    /// verbatim; no stored parse can drift from the text.
    #[must_use]
    pub fn meet_instruction(&self) -> Option<MeetInstruction> {
        parse_meet_instruction(&self.notes)
    }
}
