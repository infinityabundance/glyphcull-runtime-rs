//! Glyph measurement: advances, kerning, and mark attachment from the
//! MSDF atlas (SPEC.md §2.5; mirrors the JS `src/layout/measure.ts`).
//!
//! Scope (DESIGN.md D7): per-codepoint glyph selection with combining-mark
//! attachment (marks advance 0 and are positioned at the base glyph's
//! origin). Kerning pairs from the atlas are applied between adjacent
//! non-mark codepoints. A codepoint with no glyph record renders as a
//! fallback "tofu" box (no outline, advance = 0.5 em) — the compiler
//! reports missing codepoints at compile time, so this is a defensive
//! fallback, never a silent substitution.
//!
//! Precision: the JS runtime computes in f64 (its `number`). This module
//! widens the atlas's f32 metrics to f64 for measurement arithmetic so the
//! results match the JS runtime's semantics at tolerance level; the layout
//! engine casts to f32 only at the geometry boundary (see DESIGN.md D24).

use crate::layout::breaks::is_combining_mark;
use crate::reader::glyph::{Atlas, GlyphRecord};

/// The measured geometry of one glyph in a run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphMetric<'a> {
    /// The codepoint (Unicode scalar value).
    pub codepoint: u32,
    /// Advance in document pixels at the run's font size (kerning-adjusted).
    pub advance_px: f64,
    /// Whether the glyph is a combining mark (advance 0, overlays the base).
    pub is_mark: bool,
    /// Whether the atlas has an outline for this codepoint.
    pub has_outline: bool,
    /// The glyph record when present (box, page, flags).
    pub glyph: Option<&'a GlyphRecord>,
}

/// One measured text run.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredRun<'a> {
    /// The glyphs in text order (one per codepoint).
    pub glyphs: Vec<GlyphMetric<'a>>,
    /// Total advance width in pixels.
    pub width_px: f64,
}

/// The glyph record for a codepoint in an atlas, or `None` when missing
/// (defensive tofu fallback; the compiler reports missing codepoints).
///
/// Atlas glyph records are sorted by codepoint (SPEC.md §2.5), so the lookup
/// is a binary search — O(log n), deterministic.
#[must_use]
pub fn glyph_for(atlas: &Atlas, codepoint: u32) -> Option<&GlyphRecord> {
    atlas
        .glyphs
        .binary_search_by(|g| g.codepoint.cmp(&codepoint))
        .ok()
        .and_then(|i| atlas.glyphs.get(i))
}

/// The kerning adjustment in em for a pair, or `None` when the atlas has no
/// pair for `(left, right)`.
///
/// Kerning pairs are sorted by (left, right) (SPEC.md §2.5), so the lookup is
/// a binary search — O(log n), deterministic.
#[must_use]
pub fn kerning_adjust(atlas: &Atlas, left: u32, right: u32) -> Option<f32> {
    atlas
        .kerning
        .binary_search_by(|p| (p.left, p.right).cmp(&(left, right)))
        .ok()
        .and_then(|i| atlas.kerning.get(i))
        .map(|pair| pair.adjust)
}

/// The sum of glyph advances over `glyphs[start..end]` (the JS
/// `sumAdvances`). The end is clamped defensively so callers may pass
/// code-unit lengths that exceed the glyph count on astral text without
/// reading past the end.
#[must_use]
pub fn sum_advances(glyphs: &[GlyphMetric<'_>], start: usize, end: usize) -> f64 {
    glyphs
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .map(|g| g.advance_px)
        .sum()
}

/// Measure a text run against an atlas at a font size. `letter_spacing_px` is
/// added after every non-mark glyph (SPEC.md §2.3, tag 15).
///
/// Mirrors the JS `measureRun` operation for operation: kerning adjusts the
/// *previous non-mark* glyph's advance when the atlas carries a pair for
/// (previous codepoint, current codepoint) and the current glyph exists;
/// marks advance 0; missing glyphs advance 0.5 em with no outline.
#[must_use]
pub fn measure_run<'a>(
    atlas: &'a Atlas,
    text: &str,
    font_size_px: f32,
    letter_spacing_px: f32,
) -> MeasuredRun<'a> {
    let em_to_px = f64::from(font_size_px);
    let mut glyphs: Vec<GlyphMetric<'_>> = Vec::with_capacity(text.len());
    let mut width_px = 0.0_f64;
    let mut prev: Option<u32> = None;
    let mut last_non_mark_index: Option<usize> = None;

    for ch in text.chars() {
        let cp = u32::from(ch);
        let glyph = glyph_for(atlas, cp);
        let is_mark = is_combining_mark(cp) || glyph.is_some_and(|g| g.combining);
        let mut advance_em = glyph.map_or(0.5, |g| f64::from(g.advance));

        // Kerning: adjust the advance of the previous glyph by the pair amount.
        if let Some(left) = prev {
            if glyph.is_some() {
                if let Some(pair_adjust) = kerning_adjust(atlas, left, cp) {
                    if let Some(idx) = last_non_mark_index {
                        // `idx` was recorded as the length before a push, so it
                        // always names an already-pushed glyph.
                        if let Some(previous) = glyphs.get_mut(idx) {
                            let extra = f64::from(pair_adjust) * em_to_px;
                            previous.advance_px += extra;
                            width_px += extra;
                        }
                    }
                }
            }
        }

        if is_mark {
            // Marks advance 0 and attach to the base glyph's origin.
            advance_em = 0.0;
        } else {
            if letter_spacing_px != 0.0 {
                width_px += f64::from(letter_spacing_px);
            }
            last_non_mark_index = Some(glyphs.len());
        }

        let advance_px = advance_em * em_to_px;
        width_px += advance_px;
        glyphs.push(GlyphMetric {
            codepoint: cp,
            advance_px,
            is_mark,
            has_outline: glyph.is_some_and(|g| !g.no_outline),
            glyph,
        });
        prev = Some(cp);
    }

    MeasuredRun { glyphs, width_px }
}

/// The atlas metrics needed for vertical layout, in em units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    /// The ascent in em (above the baseline).
    pub ascent: f32,
    /// The descent in em (positive; below the baseline).
    pub descent: f32,
    /// The line gap in em.
    pub line_gap: f32,
}

/// Metrics of an atlas in em units.
#[must_use]
pub fn atlas_metrics(atlas: &Atlas) -> FontMetrics {
    FontMetrics {
        ascent: atlas.ascent,
        descent: atlas.descent,
        line_gap: atlas.line_gap,
    }
}

/// The ratio of texels to em (for the renderer's distance→pixel mapping).
#[must_use]
pub fn texels_per_em_px(atlas: &Atlas) -> f32 {
    atlas.texels_per_em()
}

#[cfg(test)]
mod tests {
    //! Unit tests for the measurement helpers against a hand-built atlas.
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;
    use crate::reader::glyph::{Atlas, GlyphRecord, KerningPair};

    fn glyph(codepoint: u32, advance: f32, no_outline: bool, combining: bool) -> GlyphRecord {
        GlyphRecord {
            codepoint,
            advance,
            bearing_x: 0.0,
            bearing_y: 0.0,
            box_x: 0,
            box_y: 0,
            box_w: 1,
            box_h: 1,
            page_index: 0,
            no_outline,
            combining,
        }
    }

    fn atlas() -> Atlas {
        // Glyph records must be sorted by codepoint (SPEC §2.5) — the
        // binary-search lookup depends on it.
        Atlas {
            font_id: 0,
            glyphs: vec![
                glyph(0x20, 0.25, true, false), // space, no outline
                glyph(0x61, 0.5, false, false), // a
                glyph(0x62, 0.6, false, false), // b
            ],
            page_count: 1,
            padding: 2,
            texels_per_em_raw: 32768,
            ascent: 0.8,
            descent: 0.2,
            line_gap: 0.0,
            cap_height: 0.7,
            x_height: 0.5,
            units_per_em: 1000.0,
            family: "test".into(),
            weight: 400,
            italic: false,
            page_width: 64,
            page_height: 64,
            kerning: vec![KerningPair {
                left: 0x61,
                right: 0x62,
                adjust: -0.05,
            }],
            pages: vec![vec![0u8; 64 * 64 * 4]],
        }
    }

    #[test]
    fn advances_scale_with_font_size() {
        let atlas = atlas();
        let a = measure_run(&atlas, "ab", 16.0, 0.0);
        let b = measure_run(&atlas, "ab", 32.0, 0.0);
        assert_eq!(a.glyphs.len(), 2);
        assert!((b.width_px - a.width_px * 2.0).abs() < 1e-9);
    }

    #[test]
    fn kerning_adjusts_the_previous_non_mark_advance() {
        let atlas = atlas();
        let m = measure_run(&atlas, "ab", 16.0, 0.0);
        // Advances and the adjust are f32 atlas values widened to f64 (0.6
        // and 0.05 are not exactly representable in f32).
        let expected = (f64::from(0.5_f32) + f64::from(0.6_f32) + f64::from(-0.05_f32)) * 16.0;
        assert!(
            (m.width_px - expected).abs() < 1e-9,
            "width {} != expected {expected}",
            m.width_px
        );
        let expected_first = (f64::from(0.5_f32) + f64::from(-0.05_f32)) * 16.0;
        assert!(
            (m.glyphs[0].advance_px - expected_first).abs() < 1e-9,
            "first advance {} != {expected_first}",
            m.glyphs[0].advance_px
        );
    }

    #[test]
    fn missing_glyphs_are_half_em_tofu_without_outline() {
        let atlas = atlas();
        let m = measure_run(&atlas, "\u{10ffff}", 16.0, 0.0);
        assert!(!m.glyphs[0].has_outline);
        assert!((m.glyphs[0].advance_px - 8.0).abs() < 1e-9);
        assert!(m.glyphs[0].glyph.is_none());
    }

    #[test]
    fn spaces_are_no_outline_but_have_advances() {
        let atlas = atlas();
        let m = measure_run(&atlas, "a b", 16.0, 0.0);
        assert!(!m.glyphs[1].has_outline);
        assert!(m.glyphs[1].glyph.is_some());
        assert!((m.glyphs[1].advance_px - 0.25 * 16.0).abs() < 1e-9);
    }

    #[test]
    fn combining_marks_advance_zero() {
        let atlas = atlas();
        // 'e' (missing → tofu) + combining acute (mark by range).
        let m = measure_run(&atlas, "e\u{0301}", 16.0, 0.0);
        assert!(m.glyphs[1].is_mark);
        assert_eq!(m.glyphs[1].advance_px, 0.0);
        assert!((m.width_px - m.glyphs[0].advance_px).abs() < 1e-9);
    }

    #[test]
    fn letter_spacing_widens_the_run_after_every_non_mark() {
        let atlas = atlas();
        let plain = measure_run(&atlas, "ab", 16.0, 0.0);
        let spaced = measure_run(&atlas, "ab", 16.0, 2.0);
        assert!((spaced.width_px - plain.width_px - 4.0).abs() < 1e-9);
    }

    #[test]
    fn atlas_glyph_lookup_is_a_binary_search() {
        let atlas = atlas();
        assert!(glyph_for(&atlas, 0x61).is_some());
        assert!(glyph_for(&atlas, 0x63).is_none());
        assert_eq!(kerning_adjust(&atlas, 0x61, 0x62), Some(-0.05));
        assert_eq!(kerning_adjust(&atlas, 0x62, 0x61), None);
    }
}
