//! The text the library-search tutorial types: the shortest start of the first listed title
//! that really shortens the list.

/// The shortest start of `first_title` (at least three characters, not ending in a blank) that
/// leaves some, but not all, of `rows` listed. A row is `(title, designer)`; the search matches
/// either one, ignoring case. `None` when every start matches every row, or no row at all (then
/// typing cannot show a shorter list).
#[must_use]
pub fn shortening_query(first_title: &str, rows: &[(String, String)]) -> Option<String> {
    let title = first_title.trim();
    let chars: Vec<char> = title.chars().collect();
    (3..=chars.len())
        .map(|len| chars[..len].iter().collect::<String>())
        .filter(|start| !start.ends_with(char::is_whitespace))
        .find(|start| {
            let needle = start.to_lowercase();
            let listed = rows
                .iter()
                .filter(|(title, designer)| {
                    title.to_lowercase().contains(&needle)
                        || designer.to_lowercase().contains(&needle)
                })
                .count();
            listed > 0 && listed < rows.len()
        })
}
