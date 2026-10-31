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
/// it at its old path.
///
/// This is a *starting* vocabulary, not an exhaustive one: the real catalogue
/// contains free-text scraped shape strings this list doesn't cover (see
/// `Database::get_unique_shapes`, which unions this list with whatever actually
/// appears in `diagram_details.shape` so neither source drops the other's values).
/// Dropped from this list: `"Cushion"`, `"Trillion"`, `"Barion"`, `"Briolette"`,
/// `"Rhombus"`. `Database::get_unique_shapes`'s doc comment already establishes that
/// the real catalogue's `diagram_details.shape` values are free-text, not drawn from
/// this vocabulary, and the shape filter (`search::build_search_predicate`) matches it
/// with plain `=`, never a substring/prefix match -- so a picker entry that never
/// appears as an EXACT scraped `shape` string is worse than not offering it at all: it
/// silently returns zero results for a shape the catalogue actually has designs of,
/// under a different exact spelling (e.g. as part of a compound shape name). Chose
/// dropping the five over switching the filter to substring/prefix matching, since that
/// would also loosen every other exact shape (a "Heart" search should not also surface
/// "Heart-Shaped Cushion"). A hand-edited or already-seeded `shape_vocabulary` row for
/// one of these five (see `Database::migrate_shape_vocabulary`'s `INSERT OR IGNORE`) is
/// left in place on an existing install -- this only changes what a FRESH seed offers.
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
