//! Property tests for layout (proptest).
//!
//! Properties:
//! - Knuth–Plass line breaks cover the item range contiguously, never split
//!   a box, and keep every non-final line within the tolerance.
//! - Line breaking is deterministic.
//! - Full-document layout is deterministic for arbitrary paragraph text.
//! - Layout geometry is sane for arbitrary text and content widths.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use proptest::prelude::*;

use glyphcull_core::document::build_document;
use glyphcull_core::layout::kp::{line_break, KpItem};
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;

/// Build KP items for a whitespace-separated word list (box widths derive
/// from the word lengths; glue is fixed).
fn kp_items(words: &[String]) -> Vec<KpItem> {
    let mut items = Vec::new();
    for (i, word) in words.iter().enumerate() {
        items.push(KpItem::Box {
            width: word.chars().count() as f64 * 8.0 + 2.0,
        });
        if i + 1 < words.len() {
            items.push(KpItem::Glue {
                width: 4.0,
                stretch: 2.0,
                shrink: 1.0,
            });
        }
    }
    items.push(KpItem::Penalty {
        width: 0.0,
        penalty: f64::NEG_INFINITY,
    });
    items
}

/// A breakpoint is a glue or a finite penalty (boxes are never split).
fn is_breakpoint(item: &KpItem) -> bool {
    match item {
        KpItem::Glue { .. } => true,
        KpItem::Penalty { penalty, .. } => *penalty < f64::INFINITY,
        KpItem::Box { .. } => false,
    }
}

/// A document with one paragraph whose single run carries `text`.
fn package_for_text(text: &str) -> Vec<u8> {
    let chunks = vec![
        common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            first_child_id: 2,
            last_child_id: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 2,
            kind: 8, // paragraph
            parent_id: 1,
            first_child_id: 3,
            last_child_id: 3,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
        common::TestChunk {
            id: 3,
            kind: 18, // run
            parent_id: 2,
            content_index: 1,
            ordinal: 2,
            depth: 2,
            ..Default::default()
        },
    ];
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(3, 1, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(&chunks, &[]),
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&[text], &[]),
        },
    ])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Chosen lines cover the paragraph contiguously, end at breakpoints, and
    /// respect the tolerance on every non-final line.
    #[test]
    fn line_breaks_cover_the_range_and_never_split_boxes(
        words in proptest::collection::vec("[a-z]{1,12}", 1..30),
        width_bucket in 0usize..560,
    ) {
        let line_width = 40.0 + width_bucket as f64;
        let items = kp_items(&words);
        let lines = line_break(&items, line_width, 100.0, 10.0);
        prop_assert!(!lines.is_empty(), "a forced final break always terminates");
        prop_assert_eq!(lines[0].start, 0, "first line starts at the head");
        for i in 1..lines.len() {
            prop_assert_eq!(lines[i].start, lines[i - 1].end + 1, "contiguous lines");
        }
        prop_assert_eq!(
            lines[lines.len() - 1].end,
            items.len() - 1,
            "the last line reaches the forced break"
        );
        for line in &lines {
            prop_assert!(is_breakpoint(&items[line.end]), "never split a box");
            prop_assert!(line.badness >= 0.0, "badness is non-negative");
            prop_assert!(line.ratio >= -1.0, "ratio {}", line.ratio);
            if line.end < items.len() - 1 {
                // Non-final lines respect the tolerance; the final line may
                // carry an unbreakable overflow via emergency stretch.
                prop_assert!(line.ratio <= 100.0, "ratio {}", line.ratio);
            }
        }
    }

    /// Identical input yields identical breaks.
    #[test]
    fn line_breaking_is_deterministic(
        words in proptest::collection::vec("[a-z]{1,10}", 1..25),
        width_bucket in 0usize..560,
    ) {
        let line_width = 40.0 + width_bucket as f64;
        let items = kp_items(&words);
        let a = line_break(&items, line_width, 100.0, 10.0);
        let b = line_break(&items, line_width, 100.0, 10.0);
        prop_assert_eq!(a, b);
    }

    /// Two independent layouts of the same package are byte-identical.
    #[test]
    fn layout_is_deterministic(text in "[a-z ]{0,80}") {
        let bytes = package_for_text(&text);
        let lay = |b: &[u8]| {
            let pkg = parse(b).expect("parse");
            let doc = build_document(&pkg).expect("build");
            let mut engine =
                LayoutEngine::new(&doc, LayoutOptions { dpr: 1.0, content_width: 400.0 });
            engine.extend_to(f64::INFINITY);
            engine.records_all().clone()
        };
        let a = lay(&bytes);
        let b = lay(&bytes);
        prop_assert_eq!(a, b);
    }

    /// Geometry invariants: the frontier exhausts, block heights are
    /// non-negative, lines sit below their tops, and non-mark glyphs advance
    /// monotonically along the line.
    #[test]
    fn layout_geometry_is_sane(
        text in "[a-z ]{1,60}",
        width_bucket in 40usize..800,
    ) {
        let bytes = package_for_text(&text);
        let pkg = parse(&bytes).expect("parse");
        let doc = build_document(&pkg).expect("build");
        let mut engine = LayoutEngine::new(
            &doc,
            LayoutOptions {
                dpr: 1.0,
                content_width: width_bucket as f32,
            },
        );
        engine.extend_to(f64::INFINITY);
        prop_assert!(engine.frontier_exhausted());
        let paragraph = engine
            .records_all()
            .values()
            .find(|r| r.kind == ChunkKind::Paragraph)
            .expect("paragraph record");
        prop_assert!(paragraph.w >= 0.0);
        prop_assert!(paragraph.h >= 0.0);
        for line in &paragraph.lines {
            prop_assert!(line.baseline > line.y, "baseline below the line top");
            let mut prev_x = -1.0_f32;
            for g in &line.glyphs {
                if g.mark_of.is_none() {
                    prop_assert!(g.x >= prev_x, "advances are monotonic");
                    prev_x = g.x;
                }
            }
        }
    }
}
