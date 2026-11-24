//! Collation of library strings (04 §6.1): the browser's
//! [`natural_cmp`](yoton_apps::browse::natural_cmp) through its
//! byte-comparable key ([`yoton_index::key::sort_key`]), with leading
//! articles optionally ignored for artists and albums.

use yoton_apps::browse::letter_of;

use crate::format::Kind;

/// `s` without a leading English article ("The ", "A ", "An "), if
/// anything is left after it.
#[must_use]
pub fn strip_article(s: &str) -> &str {
    for a in ["the ", "a ", "an "] {
        if s.len() > a.len() && s.as_bytes()[..a.len()].eq_ignore_ascii_case(a.as_bytes()) {
            let rest = s[a.len()..].trim_start();
            if !rest.is_empty() {
                return rest;
            }
        }
    }
    s
}

/// The part of `s` that sorts: artists and albums lose a leading article
/// when `articles` is set.
#[must_use]
pub fn sort_name(s: &str, kind: Kind, articles: bool) -> &str {
    if articles && matches!(kind, Kind::Artist | Kind::Album) { strip_article(s) } else { s }
}

/// Letter-jump letter of a string (`0` = none).
pub(crate) fn letter_code(s: &str, kind: Kind, articles: bool) -> u32 {
    letter_of(sort_name(s, kind, articles)).map_or(0, u32::from)
}

/// Writes the sort key of `s` into `out` (truncated to its length) and
/// returns the bytes written.
pub(crate) fn key(s: &str, kind: Kind, articles: bool, out: &mut [u8]) -> usize {
    yoton_index::key::sort_key(sort_name(s, kind, articles), out).min(out.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn articles() {
        assert_eq!(strip_article("The Beatles"), "Beatles");
        assert_eq!(strip_article("the  who"), "who");
        assert_eq!(strip_article("A Tribe Called Quest"), "Tribe Called Quest");
        assert_eq!(strip_article("An"), "An");
        assert_eq!(strip_article("The "), "The ");
        assert_eq!(strip_article("Theory"), "Theory");
        assert_eq!(sort_name("The Doors", Kind::Title, true), "The Doors");
        assert_eq!(sort_name("The Doors", Kind::Artist, false), "The Doors");
    }
}
