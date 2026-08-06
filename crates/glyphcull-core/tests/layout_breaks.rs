//! Text breaking tests: tokenization vectors for the UAX #29-grounded
//! subset (words, punctuation, CJK, forced breaks) — mirrors the JS
//! `test/layout/breaks.test.ts` vector for vector.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use glyphcull_core::layout::breaks::{is_combining_mark, tokenize_for_breaking, BreakClass};

fn text(s: &str) -> Vec<String> {
    tokenize_for_breaking(s)
        .iter()
        .map(|t| t.text.clone())
        .collect()
}

fn breaks(s: &str) -> Vec<BreakClass> {
    tokenize_for_breaking(s)
        .iter()
        .map(|t| t.break_after)
        .collect()
}

#[test]
fn keeps_words_intact_and_breaks_at_spaces() {
    assert_eq!(text("hello world"), ["hello", " ", "world"]);
    assert_eq!(
        breaks("hello world"),
        [
            BreakClass::Allowed,
            BreakClass::Space,
            BreakClass::Forbidden
        ]
    );
}

#[test]
fn collapses_whitespace_runs_into_one_glue_token() {
    assert_eq!(text("a   b"), ["a", "   ", "b"]);
}

#[test]
fn attaches_closing_punctuation_to_the_preceding_word() {
    assert_eq!(text("Hello, world!"), ["Hello,", " ", "world!"]);
    assert_eq!(
        breaks("Hello, world!"),
        [
            BreakClass::Allowed,
            BreakClass::Space,
            BreakClass::Forbidden
        ]
    );
}

#[test]
fn does_not_break_after_opening_punctuation() {
    assert_eq!(text("(parenthetical"), ["(parenthetical"]);
}

#[test]
fn does_not_break_inside_contractions() {
    assert_eq!(text("don\u{2019}t"), ["don\u{2019}t"]);
}

#[test]
fn breaks_between_cjk_ideographs_and_attaches_cjk_punctuation() {
    let tokens = tokenize_for_breaking("\u{4e2d}\u{6587}\u{3002}\u{4e2d}");
    assert_eq!(
        tokens.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
        ["\u{4e2d}", "\u{6587}\u{3002}", "\u{4e2d}"]
    );
    assert_eq!(tokens[0].break_after, BreakClass::Allowed);
    assert_eq!(tokens[1].break_after, BreakClass::Allowed);
    assert_eq!(tokens[2].break_after, BreakClass::Forbidden);
}

#[test]
fn treats_explicit_newlines_as_forced_breaks() {
    assert_eq!(
        breaks("a\nb"),
        [
            BreakClass::Allowed,
            BreakClass::Forced,
            BreakClass::Forbidden
        ]
    );
}

#[test]
fn handles_empty_and_whitespace_only_text() {
    assert!(tokenize_for_breaking("").is_empty());
    let tokens = tokenize_for_breaking("   ");
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].text, "   ");
    assert_eq!(tokens[0].break_after, BreakClass::Space);
}

#[test]
fn newline_inside_a_space_run_is_absorbed_like_the_js_regex() {
    // The JS `\s` matches `\n`, so a run that starts at a space swallows any
    // following newlines into the glue token (the `\n` branch only fires when
    // the tokenizer is positioned on the newline itself).
    assert_eq!(text("a \n b"), ["a", " \n ", "b"]);
    assert_eq!(
        breaks("a \n b"),
        [
            BreakClass::Allowed,
            BreakClass::Space,
            BreakClass::Forbidden
        ]
    );
}

#[test]
fn is_deterministic() {
    let s = "The quick brown fox, jumps over the lazy dog.";
    assert_eq!(tokenize_for_breaking(s), tokenize_for_breaking(s));
}

#[test]
fn combining_mark_ranges() {
    assert!(is_combining_mark(0x0301)); // combining acute
    assert!(is_combining_mark(0x20d0)); // combining left arrow above
    assert!(is_combining_mark(0xfe20)); // combining ligature left half
    assert!(!is_combining_mark(0x0041)); // 'A'
    assert!(!is_combining_mark(0x4e2d)); // CJK ideograph
}
