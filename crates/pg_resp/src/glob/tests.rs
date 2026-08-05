use super::*;

fn m(pattern: &str, text: &str) -> bool {
    glob_match(pattern.as_bytes(), text.as_bytes())
}

#[test]
fn literal_exact_match() {
    assert!(m("hello", "hello"));
    assert!(!m("hello", "hellx"));
    assert!(!m("hello", "hell"));
    assert!(!m("hello", "helloo"));
}

#[test]
fn star_matches_any_sequence_including_empty() {
    assert!(m("*", ""));
    assert!(m("*", "anything"));
    assert!(m("foo*", "foo"));
    assert!(m("foo*", "foobar"));
    assert!(m("*bar", "bar"));
    assert!(m("*bar", "foobar"));
    assert!(m("foo*bar", "foobazbar"));
    assert!(!m("foo*bar", "foobaz"));
}

#[test]
fn question_mark_matches_exactly_one_char() {
    assert!(m("h?llo", "hello"));
    assert!(m("h?llo", "hallo"));
    assert!(!m("h?llo", "hllo"));
    assert!(!m("h?llo", "heello"));
}

#[test]
fn character_class_matches_any_one_of_set() {
    assert!(m("h[ae]llo", "hello"));
    assert!(m("h[ae]llo", "hallo"));
    assert!(!m("h[ae]llo", "hillo"));
}

#[test]
fn negated_character_class() {
    assert!(m("h[^ae]llo", "hillo"));
    assert!(!m("h[^ae]llo", "hello"));
    assert!(!m("h[^ae]llo", "hallo"));
}

#[test]
fn character_range() {
    assert!(m("[a-z]ey", "key"));
    assert!(!m("[a-z]ey", "5ey"));
    assert!(m("[0-9]ey", "5ey"));
    // reversed range is order-corrected per the digest
    assert!(m("[z-a]ey", "key"));
}

#[test]
fn escape_allows_literal_special_chars() {
    assert!(m(r"\*", "*"));
    assert!(!m(r"\*", "x"));
    assert!(m(r"\?", "?"));
    assert!(m(r"a\[b", "a[b"));
}

#[test]
fn multiple_consecutive_stars_collapse() {
    assert!(m("a**b", "ab"));
    assert!(m("a**b", "axxxb"));
}

#[test]
fn case_sensitive_by_design() {
    assert!(!m("Hello", "hello"));
    assert!(m("Hello", "Hello"));
}

#[test]
fn star_alone_matches_everything() {
    assert!(m("*", "k1"));
    assert!(m("*", ""));
    assert!(m("*", "anything at all"));
}
