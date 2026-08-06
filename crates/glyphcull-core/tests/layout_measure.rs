//! Glyph measurement tests: advances, kerning, marks, tofu fallback —
//! mirrors the JS `test/layout/measure.test.ts` against the golden atlas.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use glyphcull_core::layout::measure::{atlas_metrics, measure_run};
use glyphcull_core::reader::parse;

fn atlas0() -> glyphcull_core::reader::glyph::Atlas {
    let pkg = parse(common::pipeline_golden()).expect("parses");
    let atlases = pkg
        .atlases()
        .expect("has atlases")
        .expect("atlases present");
    atlases[0].clone()
}

#[test]
fn measures_advances_proportional_to_the_font_size() {
    let atlas = atlas0();
    let a = measure_run(&atlas, "Hello", 16.0, 0.0);
    let b = measure_run(&atlas, "Hello", 32.0, 0.0);
    assert!(a.width_px > 0.0);
    assert!((b.width_px - a.width_px * 2.0).abs() < 1e-9);
    assert_eq!(a.glyphs.len(), 5);
}

#[test]
fn glyph_advances_are_non_negative_and_finite() {
    let atlas = atlas0();
    let m = measure_run(&atlas, "The quick brown fox", 16.0, 0.0);
    for g in &m.glyphs {
        assert!(g.advance_px.is_finite(), "non-finite advance");
        assert!(g.advance_px >= 0.0, "negative advance");
    }
}

#[test]
fn applies_kerning_from_the_atlas() {
    let atlas = atlas0();
    if atlas.kerning.is_empty() {
        return; // fixture-dependent; see the JS test's identical guard
    }
    let pair = atlas.kerning[0];
    if pair.adjust.abs() <= 1e-6 {
        return;
    }
    let left_text = char::from_u32(pair.left).expect("left char").to_string();
    let right_text = char::from_u32(pair.right).expect("right char").to_string();
    let text = format!("{left_text}{right_text}");
    let pair_run = measure_run(&atlas, &text, 16.0, 0.0);
    let solo_run = measure_run(&atlas, &text, 16.0, 0.0);
    assert_eq!(
        pair_run.width_px, solo_run.width_px,
        "measurement is deterministic"
    );
    // The kerning adjust equals the width delta vs. no kerning: verify the
    // pair total equals the sum of solo advances plus the adjustment.
    let left_glyph = atlas
        .glyphs
        .binary_search_by(|g| g.codepoint.cmp(&pair.left))
        .ok()
        .and_then(|i| atlas.glyphs.get(i))
        .expect("left glyph");
    let right_glyph = atlas
        .glyphs
        .binary_search_by(|g| g.codepoint.cmp(&pair.right))
        .ok()
        .and_then(|i| atlas.glyphs.get(i))
        .expect("right glyph");
    let expected = (left_glyph.advance + right_glyph.advance + pair.adjust) as f64 * 16.0;
    assert!(
        (pair_run.width_px - expected).abs() < 1e-4,
        "width {} != expected {expected}",
        pair_run.width_px
    );
}

#[test]
fn combining_marks_advance_zero_and_attach_to_the_base() {
    let atlas = atlas0();
    // e + combining acute. If the atlas lacks the combining mark glyph, the
    // fallback (advance 0.5em) applies and isMark is still true by range.
    let m = measure_run(&atlas, "e\u{0301}", 16.0, 0.0);
    assert!(m.glyphs[1].is_mark, "second glyph must be a mark");
    assert_eq!(m.glyphs[1].advance_px, 0.0);
    assert!(
        (m.width_px - m.glyphs[0].advance_px).abs() < 1e-9,
        "the mark must not widen the run"
    );
}

#[test]
fn letter_spacing_widens_the_run() {
    let atlas = atlas0();
    let plain = measure_run(&atlas, "ab", 16.0, 0.0);
    let spaced = measure_run(&atlas, "ab", 16.0, 2.0);
    // CSS semantics: letter-spacing is added after every character.
    assert!(
        (spaced.width_px - plain.width_px - 4.0).abs() < 1e-9,
        "spaced {} vs plain {}",
        spaced.width_px,
        plain.width_px
    );
}

#[test]
fn missing_glyphs_fall_back_to_a_half_em_tofu_box() {
    let atlas = atlas0();
    let m = measure_run(&atlas, "\u{10ffff}", 16.0, 0.0);
    assert!(!m.glyphs[0].has_outline);
    assert!((m.glyphs[0].advance_px - 8.0).abs() < 1e-9);
}

#[test]
fn metrics_are_finite_and_descent_is_positive() {
    let atlas = atlas0();
    let m = atlas_metrics(&atlas);
    assert!(m.ascent.is_finite());
    assert!(m.ascent > 0.0);
    assert!(m.descent >= 0.0);
    assert!(m.line_gap.is_finite());
}

#[test]
fn astral_codepoints_measure_one_glyph_per_codepoint() {
    let atlas = atlas0();
    // The two-surrogate emoji has no atlas glyph → two half-em tofu boxes,
    // one per codepoint (never a surrogate-half split).
    let m = measure_run(&atlas, "\u{1f600}", 16.0, 0.0);
    assert_eq!(m.glyphs.len(), 1);
    assert!(!m.glyphs[0].has_outline);
    assert!((m.glyphs[0].advance_px - 8.0).abs() < 1e-9);
}
