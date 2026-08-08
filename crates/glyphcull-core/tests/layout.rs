//! Layout engine tests: full-document layout of the golden package, the
//! sequential frontier, lists, code blocks, quotes, determinism, and geometry
//! — mirrors the JS `test/layout/layout.test.ts` and adds synthetic
//! table/image/hr coverage the golden fixture cannot express.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::rc::Rc;

use glyphcull_core::document::build_document;
use glyphcull_core::layout::layout::{
    list_marker_text, list_style, BlockLayout, LayoutEngine, LayoutOptions,
};
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;

/// Run a closure against a golden-package engine. The package and model are
/// locals of this frame, so the engine's borrows are trivially sound.
fn with_golden_engine<R>(width: f32, dpr: f32, f: impl FnOnce(&mut LayoutEngine<'_>) -> R) -> R {
    let pkg = parse(common::pipeline_golden()).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr,
            content_width: width,
        },
    );
    f(&mut engine)
}

/// Run a closure against an engine over the given package bytes.
fn with_engine<R>(
    bytes: Vec<u8>,
    width: f32,
    dpr: f32,
    f: impl FnOnce(&mut LayoutEngine<'_>) -> R,
) -> R {
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr,
            content_width: width,
        },
    );
    f(&mut engine)
}

fn records<'b>(engine: &'b LayoutEngine<'_>) -> Vec<&'b BlockLayout> {
    engine
        .records_all()
        .values()
        .map(|rc| rc.as_ref())
        .collect()
}

// ---------------------------------------------------------------------------
// Synthetic packages

/// A 2×2 table with paragraphs inside the cells. `spans` are the
/// `(colspan, rowspan)` extras for cells 5..8 (row-major); `texts` fill the
/// four runs.
fn table_package(spans: &[(u16, u16)], texts: &[&str]) -> Vec<u8> {
    let chunks = vec![
        common::TestChunk {
            id: 1,
            kind: 1, // document
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
            ordinal: 1,
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
            ordinal: 2,
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
            ordinal: 3,
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
            ordinal: 4,
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
            ordinal: 5,
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
            ordinal: 6,
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
            ordinal: 7,
            depth: 3,
            ..Default::default()
        },
        common::TestChunk {
            id: 9,
            kind: 8, // paragraph
            parent_id: 5,
            first_child_id: 13,
            last_child_id: 13,
            ordinal: 8,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 10,
            kind: 8, // paragraph
            parent_id: 6,
            first_child_id: 14,
            last_child_id: 14,
            ordinal: 9,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 11,
            kind: 8, // paragraph
            parent_id: 7,
            first_child_id: 15,
            last_child_id: 15,
            ordinal: 10,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 12,
            kind: 8, // paragraph
            parent_id: 8,
            first_child_id: 16,
            last_child_id: 16,
            ordinal: 11,
            depth: 4,
            ..Default::default()
        },
        common::TestChunk {
            id: 13,
            kind: 18, // run
            parent_id: 9,
            content_index: 1,
            ordinal: 12,
            depth: 5,
            ..Default::default()
        },
        common::TestChunk {
            id: 14,
            kind: 18, // run
            parent_id: 10,
            content_index: 2,
            ordinal: 13,
            depth: 5,
            ..Default::default()
        },
        common::TestChunk {
            id: 15,
            kind: 18, // run
            parent_id: 11,
            content_index: 3,
            ordinal: 14,
            depth: 5,
            ..Default::default()
        },
        common::TestChunk {
            id: 16,
            kind: 18, // run
            parent_id: 12,
            content_index: 4,
            ordinal: 15,
            depth: 5,
            ..Default::default()
        },
    ];
    let extras: Vec<Vec<u8>> = spans
        .iter()
        .enumerate()
        .map(|(i, (cs, rs))| {
            let mut data = Vec::with_capacity(4);
            data.extend_from_slice(&cs.to_le_bytes());
            data.extend_from_slice(&rs.to_le_bytes());
            common::extra_bytes(5 + i as u32, 2, &data) // ExtraKind::CellSpan = 2
        })
        .collect();
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(16, 1, 4, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(&chunks, &extras),
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
    ])
}

/// A document with a single image chunk referencing a 100×50 image.
fn image_package() -> Vec<u8> {
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
            kind: 16, // image
            parent_id: 1,
            content_index: 1,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
    ];
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 1, 0, 1),
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
            payload: common::cont_payload(&[], &[0]),
        },
        common::TestSection {
            kind: 6,
            compression: 1,
            payload: common::imgs_payload(&[common::TestImage {
                width: 100,
                height: 50,
                format: 0,
                data: vec![0u8; 100 * 50 * 4],
            }]),
        },
    ])
}

/// A document with a single `hr` chunk.
fn hr_package() -> Vec<u8> {
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
            kind: 21, // hr
            parent_id: 1,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
    ];
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(2, 1, 0, 0, 0),
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
    ])
}

// ---------------------------------------------------------------------------
// Golden layout

#[test]
fn lays_out_the_whole_golden_document_with_increasing_y_positions() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        assert!(engine.frontier_exhausted());
        let mut records = records(engine);
        records.sort_by(|a, b| a.y.total_cmp(&b.y));
        assert!(!records.is_empty());
        let mut prev_y = -1.0_f32;
        for record in records {
            assert!(record.y >= prev_y, "y must not decrease");
            prev_y = record.y;
            assert!(record.w.is_finite(), "finite width");
            assert!(record.h >= 0.0, "non-negative height");
        }
    });
}

#[test]
fn produces_text_lines_with_glyph_instances_for_paragraphs() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let paragraph = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Paragraph && !r.lines.is_empty())
            .expect("a paragraph with lines");
        assert!(!paragraph.lines.is_empty());
        for line in &paragraph.lines {
            assert!(!line.glyphs.is_empty(), "line has glyphs");
            assert!(line.baseline > line.y, "baseline below the line top");
            for glyph in &line.glyphs {
                assert!(glyph.x.is_finite(), "glyph x finite");
                assert!(glyph.y.is_finite(), "glyph y finite");
            }
        }
    });
}

#[test]
fn lays_out_the_heading_with_a_single_line_near_the_top() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let heading = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Heading1)
            .expect("heading");
        assert_eq!(heading.lines.len(), 1);
        // The heading's margin-top offsets it from the document origin.
        assert!(heading.y >= 0.0);
        assert!(heading.y < 50.0, "heading y {}", heading.y);
        // The heading text is 'Golden'.
        let text: String = heading.lines[0]
            .runs
            .iter()
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(text, "Golden");
    });
}

#[test]
fn frontier_extend_to_lays_out_only_the_blocks_needed_to_cover_the_viewport() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(50.0); // cover only the top 50px
        assert!(!engine.frontier_exhausted());
        let before = engine.records_all().len();
        engine.extend_to(10_000.0);
        assert!(engine.records_all().len() > before);
        assert!(engine.frontier_exhausted());
    });
}

#[test]
fn materialize_is_idempotent_and_advances_the_frontier_once() {
    with_golden_engine(800.0, 1.0, |engine| {
        let first = engine.next_frontier_block().expect("frontier block");
        let a = engine.materialize(first).expect("materialized");
        let b = engine.materialize(first).expect("re-materialized");
        assert!(
            Rc::ptr_eq(&a, &b),
            "idempotent materialize returns the same record"
        );
        assert_ne!(engine.next_frontier_block(), Some(first));
    });
}

#[test]
fn materialize_rejects_non_block_chunks() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let paragraph = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Paragraph && !r.lines.is_empty())
            .expect("paragraph");
        let run_id = paragraph.lines[0].runs[0].chunk_id;
        // Inline runs and the structural root are not block kinds.
        assert!(engine.materialize(run_id).is_none());
        assert!(engine.materialize(1).is_none());
    });
}

#[test]
fn exposes_run_geometry_for_visibility_and_hit_testing() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let paragraph = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Paragraph && !r.lines.is_empty())
            .expect("paragraph");
        let run_chunk_id = paragraph.lines[0].runs[0].chunk_id;
        let rect = engine.rect(run_chunk_id).expect("run rect");
        assert!(rect.w > 0.0);
        assert!(rect.h > 0.0);
        // The block rect matches the record.
        let block_rect = engine.rect(paragraph.chunk_id).expect("block rect");
        assert_eq!(block_rect.x, paragraph.x);
        assert_eq!(block_rect.y, paragraph.y);
        assert_eq!(block_rect.w, paragraph.w);
        assert_eq!(block_rect.h, paragraph.h);
    });
}

#[test]
fn lists_produce_markers() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let items: Vec<&BlockLayout> = records(engine)
            .into_iter()
            .filter(|r| r.kind == ChunkKind::ListItem)
            .collect();
        assert_eq!(items.len(), 2);
        for item in items {
            assert!(item.marker.is_some(), "list item has a marker");
            assert_eq!(item.children.len(), 1, "the implicit paragraph");
            assert_eq!(item.children[0].kind, ChunkKind::Paragraph);
            assert!(!item.children[0].lines.is_empty());
            assert!(!item.children[0].lines[0].runs.is_empty());
        }
    });
}

#[test]
fn code_blocks_are_preformatted_no_wrapping() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let code = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::CodeBlock)
            .expect("code block");
        // 'code block\n' splits into two preformatted lines (the trailing
        // newline is preserved verbatim); the first carries the text.
        assert_eq!(code.lines.len(), 2);
        assert_eq!(code.lines[0].runs[0].text, "code block");
        assert_eq!(code.lines[0].ratio, 0.0);
        // Every line is a single (non-wrapped) run.
        for line in &code.lines {
            assert!(line.runs.len() <= 1);
        }
    });
}

#[test]
fn quotes_indent_their_children() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let quote = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Quote)
            .expect("quote");
        assert!(!quote.children.is_empty());
        assert!(quote.children[0].x > quote.x, "children are indented");
    });
}

#[test]
fn is_deterministic_identical_input_yields_identical_layouts() {
    let mut engine_a = None;
    let mut engine_b = None;
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        engine_a = Some(engine.records_all().clone());
    });
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        engine_b = Some(engine.records_all().clone());
    });
    assert_eq!(engine_a, engine_b, "layouts must be identical");
}

#[test]
fn extend_to_zero_materializes_nothing() {
    with_golden_engine(800.0, 1.0, |engine| {
        engine.extend_to(0.0);
        assert!(engine.records_all().is_empty());
        assert!(!engine.frontier_exhausted());
    });
}

#[test]
fn wrapped_paragraph_preserves_every_token_exactly_once() {
    // A narrow width with short words forces many multi-word lines — the
    // case where the JS index mapping corrupts the partition. The Rust
    // runtime must assign each token to exactly one line, contiguously, with
    // no empty lines (DESIGN.md R2).
    let text = "aa bb cc dd ee ff gg hh ii jj kk ll mm nn oo pp qq rr ss tt uu vv ww xx yy zz";
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
            kind: 8,
            parent_id: 1,
            first_child_id: 3,
            last_child_id: 3,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
        common::TestChunk {
            id: 3,
            kind: 18,
            parent_id: 2,
            content_index: 1,
            ordinal: 2,
            depth: 2,
            ..Default::default()
        },
    ];
    let bytes = common::build_package(&[
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
    ]);
    with_engine(bytes, 60.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let paragraph = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Paragraph && !r.lines.is_empty())
            .expect("paragraph");
        assert!(
            paragraph.lines.len() > 1,
            "the paragraph genuinely wraps ({} lines)",
            paragraph.lines.len()
        );
        // No empty lines, and the run char offsets partition the source text
        // contiguously in document order.
        let mut cursor = 0usize;
        for line in &paragraph.lines {
            assert!(
                !line.runs.is_empty(),
                "every line carries at least one token"
            );
            let mut line_chars = 0usize;
            for run in &line.runs {
                assert_eq!(run.start, cursor + line_chars, "run offsets contiguous");
                line_chars += run.text.chars().count();
            }
            cursor += line_chars;
        }
        assert_eq!(
            cursor,
            text.chars().count(),
            "every token appears exactly once"
        );
        // The full reconstruction (spaces are their own tokens) equals the
        // source text verbatim.
        let reconstructed: String = paragraph
            .lines
            .iter()
            .flat_map(|l| l.runs.iter())
            .map(|r| r.text.as_str())
            .collect();
        assert_eq!(reconstructed, text);
    });
}

// ---------------------------------------------------------------------------
// Synthetic: tables

#[test]
fn table_lays_out_rows_and_columns() {
    let bytes = table_package(
        &[(1, 1), (1, 1), (1, 1), (1, 1)],
        &["alpha", "beta", "gamma", "delta"],
    );
    with_engine(bytes, 800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let table = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Table)
            .expect("table");
        let layout = table.table.as_ref().expect("table layout");
        assert_eq!(layout.columns.len(), 2);
        assert_eq!(layout.rows.len(), 2);
        assert_eq!(layout.rows[0].len(), 2);
        assert_eq!(layout.rows[1].len(), 2);
        // The table record's children are the four cells, flattened row-major.
        assert_eq!(table.children.len(), 4);
        assert!(table.h > 0.0);
        for (r, row) in layout.rows.iter().enumerate() {
            for (c, placement) in row.iter().enumerate() {
                assert!(placement.w > 0.0, "cell width");
                assert_eq!(
                    placement.w, placement.cell.w,
                    "cell laid at its column width"
                );
                assert!(placement.h >= placement.cell.h, "placement covers the cell");
                assert_eq!(placement.colspan, 1);
                assert_eq!(placement.rowspan, 1);
                // Column x alignment: x == prefix sum of the preceding widths.
                let expected_x: f32 = layout.columns.iter().take(c).sum();
                assert!(
                    (placement.x - (table.x + expected_x)).abs() < 0.001,
                    "cell x alignment"
                );
                // Row y alignment.
                let expected_y: f32 = layout.rows[..r].iter().map(|row| row[0].h).sum();
                assert!(
                    (placement.y - (table.y + expected_y)).abs() < 0.001,
                    "cell y alignment"
                );
            }
        }
    });
}

#[test]
fn table_honors_colspan() {
    // Row 1: cell 5 spans 2 columns and cell 6 spans 1 → 3 columns total;
    // row 2 keeps two single cells under the first two columns.
    let bytes = table_package(
        &[(2, 1), (1, 1), (1, 1), (1, 1)],
        &["alpha", "beta", "gamma", "delta"],
    );
    with_engine(bytes, 800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let table = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Table)
            .expect("table");
        let layout = table.table.as_ref().expect("table layout");
        assert_eq!(layout.columns.len(), 3);
        assert_eq!(layout.rows.len(), 2);
        assert_eq!(layout.rows[0].len(), 2);
        assert_eq!(layout.rows[1].len(), 2);
        // The spanning cell covers the first two columns.
        let spanning = &layout.rows[0][0];
        assert_eq!(spanning.colspan, 2);
        assert_eq!(spanning.rowspan, 1);
        let first_two: f32 = layout.columns[..2].iter().sum();
        assert!((spanning.w - first_two).abs() < 0.001, "span width");
        assert!(
            (spanning.cell.w - first_two).abs() < 0.001,
            "cell laid at its span width"
        );
        assert_eq!(spanning.x, table.x);
        // The second cell of row 1 sits after the span.
        let second = &layout.rows[0][1];
        assert_eq!(second.colspan, 1);
        assert!((second.x - (table.x + first_two)).abs() < 0.001);
        assert!((second.w - layout.columns[2]).abs() < 0.001);
        // Row 2's cells sit under the first two columns.
        let x1 = layout.rows[1][0].x;
        let x2 = layout.rows[1][1].x;
        assert!((x2 - x1 - layout.columns[0]).abs() < 0.001);
    });
}

#[test]
fn table_honors_rowspan_and_grows_the_last_spanned_row() {
    // Cell 5 (row 1) spans both rows and carries enough text to force the
    // last spanned row to grow (the JS growth rule). The table is narrow, so
    // the long cell's natural width (the text measured per run) scales down
    // and the paragraph genuinely wraps into multiple lines.
    let long = "a b c d e f g h i j k l m n o p q r s t u v w x y z";
    let bytes = table_package(
        &[(1, 2), (1, 1), (1, 1), (1, 1)],
        &[long, "beta", "gamma", "delta"],
    );
    with_engine(bytes, 140.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let table = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Table)
            .expect("table");
        let layout = table.table.as_ref().expect("table layout");
        let spanning = &layout.rows[0][0];
        assert_eq!(spanning.rowspan, 2);
        assert_eq!(spanning.colspan, 1);
        // The spanning cell's placement height covers both row heights (the
        // row-1 sibling cell and the row-2 cells each measure one row).
        let rows_sum: f32 = layout.rows[0][1].h + layout.rows[1][0].h;
        assert!(
            (spanning.h - rows_sum).abs() < 0.001,
            "placement spans both rows"
        );
        // Its own content drove the growth, so the placement height equals the
        // cell's natural height (to f32 precision).
        assert!(
            (spanning.h - spanning.cell.h).abs() < 0.01,
            "grown cell matches its content: {} vs {}",
            spanning.h,
            spanning.cell.h
        );
        // The non-spanning cells of the second row start below the first
        // row's height (the row-1 sibling cell).
        assert!(
            (layout.rows[1][0].y - (table.y + layout.rows[0][1].h)).abs() < 0.001,
            "second row starts below the first row"
        );
    });
}

#[test]
fn table_cell_spans_come_from_extras() {
    let bytes = table_package(
        &[(1, 1), (2, 2), (1, 1), (1, 1)],
        &["alpha", "beta", "gamma", "delta"],
    );
    with_engine(bytes, 800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let table = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Table)
            .expect("table");
        let layout = table.table.as_ref().expect("table layout");
        // Cell 6 carries colspan 2 rowspan 2; the placement carries the spans.
        assert_eq!(layout.rows[0][1].colspan, 2);
        assert_eq!(layout.rows[0][1].rowspan, 2);
        // The second row's cells carry the default spans.
        assert_eq!(layout.rows[1][0].colspan, 1);
        assert_eq!(layout.rows[1][0].rowspan, 1);
    });
}

// ---------------------------------------------------------------------------
// Synthetic: images

#[test]
fn images_keep_their_intrinsic_aspect_ratio() {
    for (dpr, expected_w, expected_h) in [(1.0, 100.0, 50.0), (2.0, 50.0, 25.0)] {
        let bytes = image_package();
        with_engine(bytes, 800.0, dpr, |engine| {
            engine.extend_to(f64::INFINITY);
            let image = records(engine)
                .into_iter()
                .find(|r| r.kind == ChunkKind::Image)
                .expect("image");
            let quad = image.image.as_ref().expect("image quad");
            assert!((quad.w - expected_w).abs() < 0.001, "dpr {dpr} width");
            assert!((quad.h - expected_h).abs() < 0.001, "dpr {dpr} height");
            // The quad sits at the block origin.
            assert_eq!(quad.x, image.x);
            assert_eq!(quad.y, image.y);
            assert_eq!(quad.image_id, 0);
            // The block box matches the quad.
            assert_eq!(image.w, quad.w);
            assert_eq!(image.h, quad.h);
        });
    }
}

#[test]
fn images_shrink_to_the_content_width_keeping_aspect() {
    let bytes = image_package();
    with_engine(bytes, 60.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let image = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Image)
            .expect("image");
        let quad = image.image.as_ref().expect("image quad");
        assert!((quad.w - 60.0).abs() < 0.001, "clamped width");
        assert!((quad.h - 30.0).abs() < 0.001, "aspect preserved");
    });
}

// ---------------------------------------------------------------------------
// Synthetic: horizontal rule

#[test]
fn hr_renders_a_ruler_at_the_style_midline() {
    let bytes = hr_package();
    with_engine(bytes, 800.0, 1.0, |engine| {
        engine.extend_to(f64::INFINITY);
        let hr = records(engine)
            .into_iter()
            .find(|r| r.kind == ChunkKind::Hr)
            .expect("hr");
        let ruler = hr.ruler.expect("ruler");
        // Default style: 16px font → box h 16, ruler at the midline.
        assert_eq!(hr.h, 16.0);
        assert!((ruler.y - (hr.y + 8.0)).abs() < 0.001);
        assert_eq!(ruler.x, hr.x);
        assert_eq!(ruler.w, hr.w);
    });
}

// ---------------------------------------------------------------------------
// listMarkerText (mirrors the JS vectors; more vectors live in the in-crate
// unit tests)

#[test]
fn marker_text_matches_the_js_vectors() {
    assert_eq!(list_marker_text(list_style::DISC, 1), "\u{2022}");
    assert_eq!(list_marker_text(list_style::CIRCLE, 1), "\u{25e6}");
    assert_eq!(list_marker_text(list_style::SQUARE, 1), "\u{25aa}");
    assert_eq!(list_marker_text(list_style::DECIMAL, 3), "3.");
    assert_eq!(list_marker_text(list_style::LOWER_ALPHA, 1), "a.");
    assert_eq!(list_marker_text(list_style::LOWER_ALPHA, 27), "aa.");
    assert_eq!(list_marker_text(list_style::UPPER_ALPHA, 2), "B.");
    assert_eq!(list_marker_text(list_style::LOWER_ROMAN, 4), "iv.");
    assert_eq!(list_marker_text(list_style::LOWER_ROMAN, 9), "ix.");
    assert_eq!(list_marker_text(list_style::UPPER_ROMAN, 49), "XLIX.");
    assert_eq!(list_marker_text(list_style::NONE, 1), "");
}
