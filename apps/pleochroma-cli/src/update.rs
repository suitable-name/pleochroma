//! The new-version notice printed before a command runs.
//!
//! `main.rs` asks GitHub whether a newer Pleochroma release exists and then prints one `info:`
//! line to standard error. Standard output stays deterministic and never gets it. The check is
//! skipped with [`NO_UPDATE_CHECK_FLAG`] (removed from the arguments before parsing) or when
//! [`NO_UPDATE_CHECK_ENV`] is set; an up-to-date result or any failure prints nothing. The rules
//! live in `pleochroma-update` and are tested there; [`run`](crate::run) and the tests never
//! make a network call.

pub use pleochroma_update::{NO_UPDATE_CHECK_ENV, NO_UPDATE_CHECK_FLAG, take_update_check};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_help_names_the_flag_and_the_variable() {
        let page = crate::help::text(crate::args::Topic::Root);
        assert!(page.contains(NO_UPDATE_CHECK_FLAG), "{page}");
        assert!(page.contains(NO_UPDATE_CHECK_ENV), "{page}");
    }
}
