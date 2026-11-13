/// The canonical faceting-design shape vocabulary, most-common first and freeform
/// last (deliberate order -- e.g. a GUI shape picker can present it as-is without
/// re-sorting).
///
/// This is what seeds the `shape_vocabulary` table on every `Database::new` (see
/// `Database::migrate_shape_vocabulary`), and it's `pub` so another crate building an
/// import/assignment flow (e.g. `apps/pleochroma-cut`) can offer the same list as a
/// picker without a round trip through the database -- there is exactly one
/// definition of this list, here. It lives in `model` (not under `db`) so a build
/// without the `sqlite` feature, which has no `db`, still has it; `db::sqlite` re-exports
/// it.
///
/// This is a *starting* vocabulary, not an exhaustive one: the catalogue holds free-text
/// shape strings this list does not cover (`Database::get_unique_shapes` unions both).
/// The shape filter matches `diagram_details.shape` with plain `=`, so an entry that never
/// appears as an exact scraped string returns nothing. `"Cushion"`, `"Trillion"`,
/// `"Barion"`, `"Briolette"` and `"Rhombus"` are left out for that reason, rather than
/// offered under a spelling the catalogue does not use. Rows already seeded (see
/// `Database::migrate_shape_vocabulary`, `INSERT OR IGNORE`) stay; only a fresh seed changes.
pub const DEFAULT_SHAPES: &[&str] = &[
    "Round",
    "Oval",
    "Square",
    "Rectangle",
    "Emerald",
    "Pear",
    "Marquise",
    "Heart",
    "Triangle",
    "Hexagon",
    "Octagon",
    "Pentagon",
    "Kite",
    "Shield",
    "Star",
    "Freeform",
];
