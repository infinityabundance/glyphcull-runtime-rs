//! Stress tests for the document model: very wide and very deep documents
//! validate within bounds, traversals are iterative (no native-stack risk on
//! adversarial depth), and repeated builds are stable.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use glyphcull_core::document::build_document;
use glyphcull_core::reader::{parse, Package};

fn wide_chunks(count: u32) -> Vec<common::TestChunk> {
    // Root with `count` paragraph children; a flat ring.
    let mut chunks = Vec::with_capacity(count as usize + 1);
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
            ordinal: id - 1,
            depth: 1,
            ..Default::default()
        });
    }
    chunks
}

fn deep_chunks(depth: u32) -> Vec<common::TestChunk> {
    // A single chain: each node has exactly one child.
    let mut chunks = Vec::with_capacity(depth as usize + 1);
    for i in 0..=depth {
        let id = i + 1;
        let is_leaf = i == depth;
        let mut chunk = common::TestChunk {
            id,
            kind: if i == 0 { 1 } else { 8 }, // document root, then paragraphs
            flags: if i == 0 { 0x10 } else { 0 },
            parent_id: if i == 0 { 0 } else { id - 1 },
            ordinal: i,
            depth: i,
            ..Default::default()
        };
        if !is_leaf {
            chunk.first_child_id = id + 1;
            chunk.last_child_id = id + 1;
        }
        chunks.push(chunk);
    }
    chunks
}

fn model_of(chunks: &[common::TestChunk], content: &[&str]) -> (Package, Vec<u8>) {
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(
                chunks.len() as u32,
                1,
                content.len() as u32,
                0,
                0,
            ),
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
            payload: common::cont_payload(content, &[]),
        },
    ]);
    let pkg = parse(&bytes).expect("parses");
    let _ = build_document(&pkg).expect("builds");
    (pkg, bytes)
}

#[test]
fn a_hundred_thousand_chunk_document_validates() {
    let count = 100_000_u32;
    let chunks = wide_chunks(count);
    let (pkg, _) = model_of(&chunks, &[]);
    let doc = build_document(&pkg).expect("builds");
    let ids = doc.all_ids();
    assert_eq!(ids.len() as u32, count + 1);
    let unique: std::collections::HashSet<u32> = ids.iter().copied().collect();
    assert_eq!(unique.len() as u32, count + 1);
    // Spot-check the ring: the root's children are consecutive.
    assert_eq!(doc.child_ids(1).len() as u32, count);
    assert_eq!(doc.child_ids(1).first(), Some(&2));
    assert_eq!(doc.child_ids(1).last(), Some(&(count + 1)));
}

#[test]
fn a_ten_thousand_deep_document_validates_and_walks_iteratively() {
    let depth = 10_000_u32;
    let chunks = deep_chunks(depth);
    let (pkg, _) = model_of(&chunks, &[]);
    let doc = build_document(&pkg).expect("builds");
    let ids = doc.all_ids();
    assert_eq!(ids.len() as u32, depth + 1);
    // The deepest chunk is reachable; traversals never recurse, so a chain of
    // 10,000 cannot overflow the native stack.
    assert_eq!(doc.chunk(depth + 1).expect("deepest").depth, depth);
    assert_eq!(doc.child_ids(depth + 1), Vec::<u32>::new());
}

#[test]
fn plain_text_handles_ten_thousand_deep_documents() {
    let depth = 10_000_u32;
    let chunks = deep_chunks(depth);
    // Give the deepest chunk direct text; every other chunk is empty.
    let mut deepest = chunks.last().expect("deepest").clone();
    deepest.kind = 12; // code_block
    deepest.content_index = 1;
    let mut chunks = chunks;
    *chunks.last_mut().expect("deepest") = deepest;
    let (pkg, _) = model_of(&chunks, &["deep"]);
    let doc = build_document(&pkg).expect("builds");
    // Pre-order text: empty paragraphs then the deepest chunk's text.
    assert_eq!(doc.plain_text(1), "deep");
    // Direct text of the deepest chunk.
    assert_eq!(doc.plain_text(depth + 1), "deep");
}

#[test]
fn repeated_builds_are_stable() {
    let golden = common::pipeline_golden();
    let reference = parse(golden).expect("reference parse");
    let reference_doc = build_document(&reference).expect("reference build");
    let reference_ids = reference_doc.all_ids();
    for _ in 0..200 {
        let pkg = parse(golden).expect("parse");
        let doc = build_document(&pkg).expect("build");
        assert_eq!(doc.all_ids(), reference_ids);
        assert_eq!(doc.styles(), reference_doc.styles());
        assert_eq!(doc.plain_text(1), reference_doc.plain_text(1));
    }
}
