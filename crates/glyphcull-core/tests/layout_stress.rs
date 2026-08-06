//! Stress tests for layout: very deep quote chains, very wide documents, and
//! a long paragraph whose line breaking must not blow up the active list.
//!
//! Layout of containers is recursive (it mirrors the JS engine exactly); the
//! recursion depth is the document depth, so the deep-chain test runs on a
//! dedicated large-stack thread (see DESIGN.md D26 for the host stack
//! contract).

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use glyphcull_core::document::build_document;
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;

/// Run a closure on a thread with the given native stack (bytes).
fn with_large_stack<R: Send + 'static>(
    stack_size: usize,
    f: impl FnOnce() -> R + Send + 'static,
) -> R {
    std::thread::Builder::new()
        .stack_size(stack_size)
        .spawn(f)
        .expect("spawn large-stack thread")
        .join()
        .expect("join large-stack thread")
}

/// A package whose outermost top-level block is a `depth`-deep chain of
/// quotes ending in a paragraph with a single run.
fn deep_quote_package(depth: u32) -> Vec<u8> {
    let paragraph_id = depth + 2;
    let run_id = depth + 3;
    let mut chunks = Vec::with_capacity(run_id as usize);
    chunks.push(common::TestChunk {
        id: 1,
        kind: 1,
        flags: 0x10,
        first_child_id: 2,
        last_child_id: 2,
        ..Default::default()
    });
    for i in 0..depth {
        let id = i + 2;
        let is_leaf = i + 1 == depth;
        chunks.push(common::TestChunk {
            id,
            kind: 9, // quote
            parent_id: if i == 0 { 1 } else { id - 1 },
            first_child_id: if is_leaf { paragraph_id } else { id + 1 },
            last_child_id: if is_leaf { paragraph_id } else { id + 1 },
            ordinal: i + 1,
            depth: i + 1,
            ..Default::default()
        });
    }
    chunks.push(common::TestChunk {
        id: paragraph_id,
        kind: 8, // paragraph
        parent_id: depth + 1,
        first_child_id: run_id,
        last_child_id: run_id,
        ordinal: depth + 1,
        depth: depth + 1,
        ..Default::default()
    });
    chunks.push(common::TestChunk {
        id: run_id,
        kind: 18, // run
        parent_id: paragraph_id,
        content_index: 1,
        ordinal: depth + 2,
        depth: depth + 2,
        ..Default::default()
    });
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(run_id, 1, 1, 0, 0),
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
            payload: common::cont_payload(&["deep"], &[]),
        },
    ])
}

/// A package whose root has `count` paragraph children, each with one run.
fn wide_package(count: u32) -> Vec<u8> {
    let run_base = count + 2;
    let mut chunks = Vec::with_capacity((count * 2 + 1) as usize);
    chunks.push(common::TestChunk {
        id: 1,
        kind: 1,
        flags: 0x10,
        first_child_id: 2,
        last_child_id: count + 1,
        ..Default::default()
    });
    for i in 0..count {
        let id = i + 2;
        chunks.push(common::TestChunk {
            id,
            kind: 8, // paragraph
            parent_id: 1,
            prev_id: if i == 0 { 0 } else { id - 1 },
            next_id: if i + 1 == count { 0 } else { id + 1 },
            first_child_id: run_base + i,
            last_child_id: run_base + i,
            ordinal: id - 1,
            depth: 1,
            ..Default::default()
        });
    }
    for i in 0..count {
        let id = run_base + i;
        chunks.push(common::TestChunk {
            id,
            kind: 18, // run
            parent_id: i + 2,
            content_index: i + 1,
            ordinal: id - 1,
            depth: 2,
            ..Default::default()
        });
    }
    let texts: Vec<String> = (0..count).map(|i| format!("paragraph {i} text")).collect();
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(count * 2 + 1, 1, count, 0, 0),
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
            payload: common::cont_payload(&text_refs, &[]),
        },
    ])
}

/// A package with a single paragraph whose run carries `text`.
fn paragraph_package(text: &str) -> Vec<u8> {
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

#[test]
fn deep_quote_chain_lays_out_without_overflowing_the_stack() {
    // Container layout recurses once per quote; the chain runs on a 64 MiB
    // stack thread so the depth is never constrained by the harness thread.
    with_large_stack(64 * 1024 * 1024, || {
        let depth = 5_000_u32;
        let bytes = deep_quote_package(depth);
        let pkg = parse(&bytes).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let mut engine = LayoutEngine::new(
            &doc,
            LayoutOptions {
                dpr: 1.0,
                content_width: 400.0,
            },
        );
        engine.extend_to(f64::INFINITY);
        assert!(engine.frontier_exhausted());
        // Every quote and the innermost paragraph carry a record.
        assert_eq!(engine.records_all().len() as u32, depth + 1);
        // The outermost quote's height accumulates the chain (the paragraph
        // is one line tall).
        let outer = engine.record(2).expect("outermost quote");
        assert_eq!(outer.kind, ChunkKind::Quote);
        assert_eq!(outer.children.len(), 1);
        assert!(outer.h > 0.0);
        // The innermost paragraph is a record with one line.
        let paragraph = engine.record(depth + 2).expect("innermost paragraph");
        assert_eq!(paragraph.kind, ChunkKind::Paragraph);
        assert_eq!(paragraph.lines.len(), 1);
    });
}

#[test]
fn wide_document_lays_out_streaming_block_by_block() {
    let count = 2_000_u32;
    let bytes = wide_package(count);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    // Materialize one top-level block at a time: the frontier advances
    // strictly and the cursor never moves backwards.
    let mut prev_cursor = 0.0_f32;
    while !engine.frontier_exhausted() {
        let id = engine.next_frontier_block().expect("frontier block");
        let before = engine.frontier_y();
        let _ = engine.materialize(id).expect("materialized");
        assert!(engine.frontier_y() >= before, "cursor advances");
        prev_cursor = prev_cursor.max(engine.frontier_y());
    }
    assert_eq!(
        engine.records_all().len() as u32,
        count,
        "one record per paragraph"
    );
    assert!(prev_cursor > 0.0, "the document has height");
}

#[test]
fn long_paragraph_breaks_without_pathological_active_list_growth() {
    // 3000 words → ~6000 KP items and ~3000 breakpoints. The active list is
    // deduplicated by (breakpoint, fitness), so the dynamic program stays
    // fast on long paragraphs (no exponential blowup).
    let words: Vec<String> = (0..3000).map(|i| format!("word{}", i % 97)).collect();
    let text = words.join(" ");
    let bytes = paragraph_package(&text);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 400.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    assert!(engine.frontier_exhausted());
    let paragraph = engine.record(2).expect("paragraph");
    assert!(paragraph.lines.len() > 100, "wraps into many lines");
    // Every line carries at least one run, and the runs partition the source
    // text contiguously (no dropped or duplicated tokens; DESIGN.md R2).
    let mut cursor = 0usize;
    for line in &paragraph.lines {
        assert!(line.y >= 0.0);
        assert!(!line.runs.is_empty(), "no empty lines");
        for run in &line.runs {
            assert_eq!(run.start, cursor, "run offsets contiguous");
            cursor += run.text.chars().count();
        }
    }
    assert_eq!(
        cursor,
        text.chars().count(),
        "every token appears exactly once"
    );
}

#[test]
fn materialize_the_same_block_many_times_is_stable() {
    // Re-materializing the same top-level block returns the identical record
    // (idempotence under repetition, including mixed with frontier advances).
    let bytes = wide_package(20);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    let first = engine.next_frontier_block().expect("first block");
    let a = engine.materialize(first).expect("materialized");
    for _ in 0..50 {
        let again = engine.materialize(first).expect("materialized again");
        assert!(std::rc::Rc::ptr_eq(&a, &again), "record identity is stable");
    }
}
