//! Glob-style pattern matching for KEYS / SCAN's MATCH option (bible §3.4
//! T1/T2). Syntax per docs/refs/valkey-notes.md's digest of Valkey's
//! `stringmatchlen` (util.c): `*` any sequence, `?` single char, `[abc]`
//! class, `[^abc]` negated class, `[a-z]` range (order-corrected), `\x`
//! escape. Case-sensitive, matching KEYS's documented behavior — no nocase
//! mode needed for pg_resp's T1/T2 scope.
//!
//! Implements the documented *syntax and behavior*, not copied from any
//! Redis-licensed source (bible D8) — this is a standard, widely-implemented
//! glob algorithm; the digest describes what it must do, not how Valkey's C
//! code is written.

pub fn glob_match(pattern: &[u8], text: &[u8]) -> bool {
    let mut p = pattern;
    let mut s = text;

    loop {
        match p.first() {
            Some(&b'*') => {
                while p.len() > 1 && p[1] == b'*' {
                    p = &p[1..];
                }
                if p.len() == 1 {
                    return true; // trailing '*' matches anything remaining
                }
                let rest = &p[1..];
                for i in 0..=s.len() {
                    if glob_match(rest, &s[i..]) {
                        return true;
                    }
                }
                return false;
            }
            Some(&b'?') => {
                let Some((&_, s_rest)) = s.split_first() else {
                    return false;
                };
                s = s_rest;
                p = &p[1..];
            }
            Some(&b'[') => {
                let (matched, new_p, consumed_s) = match_class(&p[1..], s);
                if !consumed_s || !matched {
                    return false;
                }
                p = new_p;
                s = &s[1..];
            }
            Some(&b'\\') if p.len() >= 2 => {
                let Some((&sc, s_rest)) = s.split_first() else {
                    return false;
                };
                if p[1] != sc {
                    return false;
                }
                s = s_rest;
                p = &p[2..];
            }
            Some(&pc) => {
                let Some((&sc, s_rest)) = s.split_first() else {
                    return false;
                };
                if pc != sc {
                    return false;
                }
                s = s_rest;
                p = &p[1..];
            }
            None => return s.is_empty(),
        }
    }
}

/// Parses a `[...]` class starting right after the `[`. Returns
/// (does-first-char-of-s-match, pattern-slice-after-the-closing-`]`,
/// whether-there-was-a-character-of-s-to-test-at-all).
fn match_class<'p>(mut p: &'p [u8], s: &[u8]) -> (bool, &'p [u8], bool) {
    let negate = p.first() == Some(&b'^');
    if negate {
        p = &p[1..];
    }
    let subject = s.first().copied();
    let mut matched = false;

    loop {
        match p.first() {
            None => break, // unterminated class: treat leniently, stop here
            Some(&b']') => {
                p = &p[1..];
                break;
            }
            Some(&b'\\') if p.len() >= 2 => {
                if subject == Some(p[1]) {
                    matched = true;
                }
                p = &p[2..];
            }
            Some(&lo) if p.len() >= 3 && p[1] == b'-' && p[2] != b']' => {
                let (mut lo, mut hi) = (lo, p[2]);
                if lo > hi {
                    std::mem::swap(&mut lo, &mut hi);
                }
                if let Some(c) = subject {
                    if c >= lo && c <= hi {
                        matched = true;
                    }
                }
                p = &p[3..];
            }
            Some(&c) => {
                if subject == Some(c) {
                    matched = true;
                }
                p = &p[1..];
            }
        }
    }

    if negate {
        matched = !matched;
    }
    (matched, p, subject.is_some())
}

#[cfg(test)]
mod tests;
