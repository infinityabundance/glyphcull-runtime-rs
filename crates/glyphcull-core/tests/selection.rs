//! Selection tests (TESTING.md §2 unit/selection): hit testing at glyph
//! boundaries, position ordering/normalization, range→quad projection with
//! per-line merging, and the copy extraction policy (paragraphs → newlines,
//! table cells → tabs, rows → newlines) over the golden and synthetic
//! packages — mirrors the JS `test/selection/selection.test.ts`.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use glyphcull_core::document::{build_document, DocumentModel};
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;
use glyphcull_core::selection::{
    compare_positions, copy_text, hit_test_point, is_collapsed, normalize_selection, range_quads,
    Point, Selection, TextPosition,
};

/// Run a closure against a fully laid-out golden document.
fn with_golden<R>(f: impl FnOnce(&DocumentModel<'_>, &mut LayoutEngine<'_>) -> R) -> R {
    let pkg = parse(common::pipeline_golden()).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    f(&doc, &mut engine)
}

/// Build a document model from chunks + texts (single default style).
fn with_synthetic<R>(
    chunks: &[common::TestChunk],
    texts: &[&str],
    f: impl FnOnce(&DocumentModel<'_>, &mut LayoutEngine<'_>) -> R,
) -> R {
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(chunks.len() as u32, 1, texts.len() as u32, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(chunks, &[]),
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(texts, &[]),
        },
    ]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    f(&doc, &mut engine)
}

const fn pos(chunk_id: u32, offset: usize) -> TextPosition {
    TextPosition { chunk_id, offset }
}

// ---------------------------------------------------------------------------
// Positions and ordering

#[test]
fn orders_by_document_order_then_offset() {
    with_golden(|doc, _engine| {
        use std::cmp::Ordering::{Equal, Greater, Less};
        assert_eq!(compare_positions(doc, pos(3, 3), pos(3, 5)), Less);
        assert_eq!(compare_positions(doc, pos(3, 5), pos(3, 3)), Greater);
        assert_eq!(compare_positions(doc, pos(3, 4), pos(3, 4)), Equal);
        // Runs are document-order leaves: run 3 (heading) precedes run 5.
        assert_eq!(compare_positions(doc, pos(3, 6), pos(5, 0)), Less);
        assert_eq!(compare_positions(doc, pos(22, 0), pos(3, 0)), Greater);
    });
}

#[test]
fn normalize_selection_orders_reversed_anchors() {
    with_golden(|doc, _engine| {
        let a = pos(3, 1);
        let b = pos(22, 4);
        assert_eq!(
            normalize_selection(doc, a, b),
            Selection { start: a, end: b }
        );
        assert_eq!(
            normalize_selection(doc, b, a),
            Selection { start: a, end: b }
        );
    });
}

#[test]
fn is_collapsed_only_for_identical_positions() {
    assert!(is_collapsed(Selection {
        start: pos(3, 2),
        end: pos(3, 2)
    }));
    assert!(!is_collapsed(Selection {
        start: pos(3, 2),
        end: pos(3, 3)
    }));
}

#[test]
fn golden_chunk_structure_matches_the_js_vectors() {
    // The JS suite pins these ids; a fixture change must fail loudly here.
    with_golden(|doc, _engine| {
        assert_eq!(doc.chunk(2).expect("c2").kind, ChunkKind::Heading1);
        assert_eq!(doc.chunk(3).expect("c3").kind, ChunkKind::Run);
        assert_eq!(doc.chunk(5).expect("c5").kind, ChunkKind::Run);
        assert_eq!(doc.chunk(22).expect("c22").kind, ChunkKind::Run);
        assert_eq!(doc.direct_text(3), Some("Golden"));
        assert_eq!(doc.direct_text(22), Some("quote"));
    });
}

// ---------------------------------------------------------------------------
// hitTestPoint

#[test]
fn hits_the_nearest_glyph_center() {
    with_golden(|_doc, engine| {
        let heading = engine.record(2).expect("heading record").clone();
        let line = &heading.lines[0];
        for glyph in &line.glyphs {
            if glyph.mark_of.is_some() {
                continue;
            }
            // A point in the glyph's left half lands on the glyph itself.
            let hit = hit_test_point(
                engine,
                Point {
                    x: glyph.x + glyph.advance_px * 0.25,
                    y: glyph.y,
                },
            );
            assert_eq!(
                hit,
                Some(TextPosition {
                    chunk_id: glyph.run_chunk_id,
                    offset: glyph.offset_in_text,
                })
            );
        }
    });
}

#[test]
fn bounds_before_the_first_and_after_the_last_glyph_of_a_line() {
    with_golden(|_doc, engine| {
        let heading = engine.record(2).expect("heading").clone();
        let line = &heading.lines[0];
        let first = line.glyphs[0];
        let last = line.glyphs[line.glyphs.len() - 1];
        let hit_before = hit_test_point(
            engine,
            Point {
                x: first.x - 10.0,
                y: line.baseline,
            },
        );
        assert_eq!(hit_before, Some(pos(first.run_chunk_id, 0)));
        let hit_after = hit_test_point(
            engine,
            Point {
                x: last.x + last.advance_px + 10.0,
                y: line.baseline,
            },
        );
        assert_eq!(
            hit_after,
            Some(pos(last.run_chunk_id, last.offset_in_text + 1))
        );
    });
}

#[test]
fn boundary_exactly_at_a_glyph_center_lands_after_that_glyph() {
    with_golden(|_doc, engine| {
        let heading = engine.record(2).expect("heading").clone();
        let line = &heading.lines[0];
        let first = line.glyphs[0];
        let center = first.x + first.advance_px / 2.0;
        let before = hit_test_point(
            engine,
            Point {
                x: center - 0.1,
                y: line.baseline,
            },
        );
        assert_eq!(before, Some(pos(first.run_chunk_id, first.offset_in_text)));
        let at = hit_test_point(
            engine,
            Point {
                x: center,
                y: line.baseline,
            },
        );
        assert_eq!(at, Some(pos(first.run_chunk_id, first.offset_in_text + 1)));
    });
}

#[test]
fn clamps_vertically_to_the_nearest_line() {
    with_golden(|_doc, engine| {
        let mut records: Vec<_> = engine.records_all().values().cloned().collect();
        records.sort_by(|a, b| a.y.total_cmp(&b.y));
        let first_record = records.iter().find(|r| !r.lines.is_empty()).expect("first");
        let first_line = &first_record.lines[0];
        let last_record = records.last().expect("last");
        let last_line = last_record.lines.last().expect("last line");
        let above = hit_test_point(
            engine,
            Point {
                x: first_line.baseline,
                y: -1000.0,
            },
        );
        assert_eq!(
            above.map(|p| p.chunk_id),
            Some(first_line.glyphs[0].run_chunk_id)
        );
        let below = hit_test_point(
            engine,
            Point {
                x: last_line.runs[0].x,
                y: 1_000_000.0,
            },
        );
        assert_eq!(
            below.map(|p| p.chunk_id),
            Some(last_line.glyphs[last_line.glyphs.len() - 1].run_chunk_id)
        );
    });
}

#[test]
fn returns_none_for_a_document_without_text() {
    let chunks = vec![common::TestChunk {
        id: 1,
        kind: 1,
        flags: 0x10,
        ..Default::default()
    }];
    with_synthetic(&chunks, &[], |_doc, engine| {
        assert_eq!(hit_test_point(engine, Point { x: 0.0, y: 0.0 }), None);
    });
}

// ---------------------------------------------------------------------------
// rangeQuads

#[test]
fn a_collapsed_selection_yields_no_quads() {
    with_golden(|doc, engine| {
        let selection = normalize_selection(doc, pos(3, 2), pos(3, 2));
        assert!(range_quads(engine, selection).is_empty());
    });
}

#[test]
fn a_full_line_selection_produces_one_merged_quad_per_line() {
    with_golden(|_doc, engine| {
        let heading = engine.record(2).expect("heading").clone();
        let line = &heading.lines[0];
        let selection = Selection {
            start: pos(3, 0),
            end: pos(3, 6),
        };
        let quads = range_quads(engine, selection);
        assert_eq!(quads.len(), 1);
        let quad = quads[0];
        assert!((quad.y - line.y).abs() < 1e-6);
        assert!((quad.h - line.height_px).abs() < 1e-6);
        // The quad spans the run: from the first glyph to the last glyph's end.
        let first = line.glyphs[0];
        let last = line.glyphs[line.glyphs.len() - 1];
        assert!((quad.x - first.x).abs() < 1e-6);
        assert!((quad.x + quad.w - (last.x + last.advance_px)).abs() < 1e-6);
    });
}

#[test]
fn a_partial_selection_clips_to_the_covered_glyphs() {
    with_golden(|_doc, engine| {
        let paragraph = engine
            .records_all()
            .values()
            .map(|rc| rc.as_ref())
            .find(|r| r.kind == ChunkKind::Paragraph && !r.lines.is_empty())
            .expect("paragraph")
            .clone();
        let line = &paragraph.lines[0];
        let run = &line.runs[0]; // 'Deterministic' (chunk 5, chars 0-12)
        let selection = Selection {
            start: pos(run.chunk_id, 2),
            end: pos(run.chunk_id, 5),
        };
        let quads = range_quads(engine, selection);
        assert!(!quads.is_empty());
        let quad = quads[0];
        assert!(quad.w > 0.0);
        assert!(quad.w < run.width);
        assert!(quad.x >= run.x);
        assert!(quad.x + quad.w <= run.x + run.width + 0.5);
    });
}

#[test]
fn spans_multiple_blocks_in_document_order() {
    with_golden(|_doc, engine| {
        // From the start of the heading to the end of the quote.
        let selection = Selection {
            start: pos(3, 0),
            end: pos(22, 5),
        };
        let quads = range_quads(engine, selection);
        assert!(quads.len() > 1);
        for i in 1..quads.len() {
            assert!(quads[i].y >= quads[i - 1].y);
        }
    });
}

#[test]
fn merges_adjacent_styled_runs_of_one_line_into_a_single_quad() {
    with_golden(|_doc, engine| {
        let paragraph = engine
            .records_all()
            .values()
            .map(|rc| rc.as_ref())
            .find(|r| r.kind == ChunkKind::Paragraph && !r.lines.is_empty())
            .expect("paragraph")
            .clone();
        // The whole paragraph text: all runs of all its lines.
        let first_run = &paragraph.lines[0].runs[0];
        let last_line = &paragraph.lines[paragraph.lines.len() - 1];
        let last_run = &last_line.runs[last_line.runs.len() - 1];
        let selection = Selection {
            start: pos(first_run.chunk_id, first_run.start),
            end: pos(last_run.chunk_id, last_run.end),
        };
        let quads = range_quads(engine, selection);
        assert_eq!(quads.len(), paragraph.lines.len());
        for quad in &quads {
            assert!(quad.w > 0.0);
        }
    });
}

#[test]
fn is_deterministic() {
    with_golden(|_doc, engine| {
        let selection = Selection {
            start: pos(3, 1),
            end: pos(22, 3),
        };
        assert_eq!(
            range_quads(engine, selection),
            range_quads(engine, selection)
        );
    });
}

// ---------------------------------------------------------------------------
// copyText

#[test]
fn copies_a_single_run_the_heading() {
    with_golden(|doc, _engine| {
        let selection = Selection {
            start: pos(3, 0),
            end: pos(3, 6),
        };
        assert_eq!(copy_text(doc, selection), "Golden");
    });
}

#[test]
fn copies_a_partial_run_slice() {
    with_golden(|doc, _engine| {
        let selection = Selection {
            start: pos(3, 1),
            end: pos(3, 4),
        };
        assert_eq!(copy_text(doc, selection), "old");
    });
}

#[test]
fn joins_styled_runs_of_a_paragraph_without_separators() {
    with_golden(|doc, _engine| {
        let selection = Selection {
            start: pos(5, 0),
            end: pos(11, 1),
        };
        assert_eq!(
            copy_text(doc, selection),
            "Deterministic golden fixture with a link."
        );
    });
}

#[test]
fn separates_blocks_with_newlines_preserving_document_order() {
    with_golden(|doc, _engine| {
        let selection = Selection {
            start: pos(3, 0),
            end: pos(22, 5),
        };
        assert_eq!(
            copy_text(doc, selection),
            "Golden\nDeterministic golden fixture with a link.\none\ntwo\ncode block\n\nquote"
        );
    });
}

#[test]
fn a_collapsed_selection_copies_the_empty_string() {
    with_golden(|doc, _engine| {
        let selection = Selection {
            start: pos(3, 2),
            end: pos(3, 2),
        };
        assert_eq!(copy_text(doc, selection), "");
    });
}

/// The synthetic 2×2 table (cells → tabs within a row, rows → newlines).
fn table_chunks() -> Vec<common::TestChunk> {
    let mut chunks = vec![
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
            kind: 13, // table
            flags: 0x10,
            parent_id: 1,
            first_child_id: 3,
            last_child_id: 4,
            depth: 1,
            ..Default::default()
        },
        common::TestChunk {
            id: 3,
            kind: 14, // table_row
            flags: 0x10,
            parent_id: 2,
            next_id: 4,
            first_child_id: 5,
            last_child_id: 6,
            depth: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 4,
            kind: 14, // table_row
            flags: 0x10,
            parent_id: 2,
            prev_id: 3,
            first_child_id: 7,
            last_child_id: 8,
            depth: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 5,
            kind: 15, // table_cell
            parent_id: 3,
            next_id: 6,
            first_child_id: 9,
            last_child_id: 9,
            depth: 3,
            ..Default::default()
        },
        common::TestChunk {
            id: 6,
            kind: 15, // table_cell
            parent_id: 3,
            prev_id: 5,
            first_child_id: 10,
            last_child_id: 10,
            depth: 3,
            ..Default::default()
        },
        common::TestChunk {
            id: 7,
            kind: 15, // table_cell
            parent_id: 4,
            next_id: 8,
            first_child_id: 11,
            last_child_id: 11,
            depth: 3,
            ..Default::default()
        },
        common::TestChunk {
            id: 8,
            kind: 15, // table_cell
            parent_id: 4,
            prev_id: 7,
            first_child_id: 12,
            last_child_id: 12,
            depth: 3,
            ..Default::default()
        },
        common::TestChunk {
            id: 9,
            kind: 8, // paragraph
            parent_id: 5,
            first_child_id: 13,
            last_child_id: 13,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 10,
            kind: 8, // paragraph
            parent_id: 6,
            first_child_id: 14,
            last_child_id: 14,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 11,
            kind: 8, // paragraph
            parent_id: 7,
            first_child_id: 15,
            last_child_id: 15,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 12,
            kind: 8, // paragraph
            parent_id: 8,
            first_child_id: 16,
            last_child_id: 16,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 13,
            kind: 18, // run
            parent_id: 9,
            content_index: 1,
            depth: 5,
            ..Default::default()
        },
        common::TestChunk {
            id: 14,
            kind: 18, // run
            parent_id: 10,
            content_index: 2,
            depth: 5,
            ..Default::default()
        },
        common::TestChunk {
            id: 15,
            kind: 18, // run
            parent_id: 11,
            content_index: 3,
            depth: 5,
            ..Default::default()
        },
        common::TestChunk {
            id: 16,
            kind: 18, // run
            parent_id: 12,
            content_index: 4,
            depth: 5,
            ..Default::default()
        },
    ];
    // Dense ordinals (id = ordinal + 1).
    for (i, chunk) in chunks.iter_mut().enumerate() {
        chunk.ordinal = i as u32;
    }
    chunks
}

#[test]
fn table_policy_cells_to_tabs_within_a_row_rows_to_newlines() {
    let chunks = table_chunks();
    with_synthetic(&chunks, &["a", "b", "c", "d"], |doc, _engine| {
        let selection = Selection {
            start: pos(13, 0),
            end: pos(14, 1),
        };
        assert_eq!(copy_text(doc, selection), "a\tb");
        let selection = Selection {
            start: pos(13, 0),
            end: pos(15, 1),
        };
        assert_eq!(copy_text(doc, selection), "a\tb\nc");
        let selection = Selection {
            start: pos(13, 0),
            end: pos(16, 1),
        };
        assert_eq!(copy_text(doc, selection), "a\tb\nc\td");
    });
}

#[test]
fn br_chunks_copy_as_explicit_newlines() {
    let chunks = vec![
        common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            first_child_id: 2,
            last_child_id: 2,
            ordinal: 0,
            ..Default::default()
        },
        common::TestChunk {
            id: 2,
            kind: 8, // paragraph
            parent_id: 1,
            first_child_id: 3,
            last_child_id: 5,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
        common::TestChunk {
            id: 3,
            kind: 18, // run
            parent_id: 2,
            next_id: 4,
            content_index: 1,
            ordinal: 2,
            depth: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 4,
            kind: 20, // br
            parent_id: 2,
            prev_id: 3,
            next_id: 5,
            ordinal: 3,
            depth: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 5,
            kind: 18, // run
            parent_id: 2,
            prev_id: 4,
            content_index: 2,
            ordinal: 4,
            depth: 2,
            ..Default::default()
        },
    ];
    with_synthetic(&chunks, &["before", "after"], |doc, _engine| {
        let selection = Selection {
            start: pos(3, 0),
            end: pos(5, 5),
        };
        assert_eq!(copy_text(doc, selection), "before\nafter");
    });
}

#[test]
fn property_copying_a_block_equals_its_laid_out_run_texts() {
    with_golden(|doc, engine| {
        let blocks: Vec<_> = engine.records_all().values().cloned().collect();
        for record in blocks {
            if record.lines.is_empty() {
                continue;
            }
            // The last text-bearing line (a code block's final empty line has
            // no runs).
            let last_line = record.lines.iter().rev().find(|l| !l.runs.is_empty());
            let Some(last_line) = last_line else {
                continue;
            };
            let first_run = &record.lines[0].runs[0];
            let last_run = &last_line.runs[last_line.runs.len() - 1];
            let selection = Selection {
                start: pos(first_run.chunk_id, first_run.start),
                end: pos(last_run.chunk_id, last_run.end),
            };
            // Copying a block's full span yields its run texts joined
            // (paragraph boundaries inside the block are implicit — soft line
            // breaks carry the source spaces; a trailing source newline is
            // not a laid-out run).
            let expected: String = record
                .lines
                .iter()
                .flat_map(|l| l.runs.iter())
                .map(|r| r.text.as_str())
                .collect();
            assert_eq!(copy_text(doc, selection), expected);
        }
    });
}
