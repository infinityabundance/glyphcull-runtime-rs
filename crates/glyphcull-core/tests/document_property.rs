//! Property tests for the document model (proptest).
//!
//! Properties:
//! - Every randomly generated valid chunk tree builds into a trusted model
//!   whose walk visits every chunk exactly once with consistent depths.
//! - Byte mutations of the golden package never panic `build_document`: the
//!   outcome is a successful model or a typed [`DocumentError`].
//! - Building is deterministic: identical input yields an identical model.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use proptest::prelude::*;

use glyphcull_core::document::{build_document, DocumentErrorKind};
use glyphcull_core::reader::parse;

/// A generated tree node: a chunk kind code plus children.
#[derive(Clone, Debug)]
struct GenNode {
    kind: u8,
    children: Vec<GenNode>,
}

/// Any chunk kind code (SPEC.md §2.2: 1..=21).
fn gen_kind() -> impl Strategy<Value = u8> {
    any::<u8>().prop_map(|b| (b % 21) + 1)
}

fn leaf() -> impl Strategy<Value = GenNode> {
    gen_kind().prop_map(|kind| GenNode {
        kind,
        children: Vec::new(),
    })
}

/// A subtree of at most `max_depth` levels, each node with 0..=4 children.
fn gen_subtree(max_depth: u32) -> impl Strategy<Value = GenNode> {
    if max_depth == 0 {
        leaf().boxed()
    } else {
        prop_oneof![
            leaf(),
            (
                gen_kind(),
                proptest::collection::vec(gen_subtree(max_depth - 1), 0..=4)
            )
                .prop_map(|(kind, children)| GenNode { kind, children }),
        ]
        .boxed()
    }
}

/// A flattened pre-order node with its ring metadata filled in.
#[derive(Clone, Debug)]
struct FlatNode {
    kind: u8,
    parent: u32,
    depth: u32,
    children: Vec<u32>,
}

/// Pre-order flatten with dense ids; returns the node's id.
fn assign(
    node: &GenNode,
    parent: u32,
    depth: u32,
    next_id: &mut u32,
    out: &mut Vec<FlatNode>,
) -> u32 {
    let id = *next_id;
    *next_id += 1;
    let slot = out.len();
    out.push(FlatNode {
        kind: node.kind,
        parent,
        depth,
        children: Vec::new(),
    });
    let mut children = Vec::with_capacity(node.children.len());
    for child in &node.children {
        children.push(assign(child, id, depth + 1, next_id, out));
    }
    out[slot].children = children;
    id
}

/// Convert the flattened tree into CHNK records with consistent rings.
fn to_chunks(flat: &[FlatNode]) -> Vec<common::TestChunk> {
    let mut chunks = Vec::with_capacity(flat.len());
    for (i, node) in flat.iter().enumerate() {
        let id = i as u32 + 1;
        // Document/List/Table/TableRow are structural (SPEC.md §2.2).
        let structural = matches!(node.kind, 1 | 10 | 13 | 14);
        let flags = if structural { 0x10 } else { 0 };
        let first = node.children.first().copied().unwrap_or(0);
        let last = node.children.last().copied().unwrap_or(0);
        let mut prev = 0;
        let mut next = 0;
        if node.parent != 0 {
            let siblings = &flat[node.parent as usize - 1].children;
            if let Some(index) = siblings.iter().position(|&c| c == id) {
                if index > 0 {
                    prev = siblings[index - 1];
                }
                if index + 1 < siblings.len() {
                    next = siblings[index + 1];
                }
            }
        }
        chunks.push(common::TestChunk {
            id,
            kind: node.kind,
            flags,
            style_id: 0,
            parent_id: node.parent,
            prev_id: prev,
            next_id: next,
            first_child_id: first,
            last_child_id: last,
            content_index: 0,
            ordinal: i as u32,
            depth: node.depth,
        });
    }
    chunks
}

/// A random valid document: a `document` root wrapping a random forest.
fn gen_chunks() -> impl Strategy<Value = Vec<common::TestChunk>> {
    proptest::collection::vec(gen_subtree(4), 1..=8).prop_map(|forest| {
        let root = GenNode {
            kind: 1,
            children: forest,
        };
        let mut flat = Vec::new();
        let mut next_id = 1;
        assign(&root, 0, 0, &mut next_id, &mut flat);
        to_chunks(&flat)
    })
}

/// The package for a generated chunk list (1 default style, no content).
fn package_for(chunks: &[common::TestChunk]) -> Vec<u8> {
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(chunks.len() as u32, 1, 0, 0, 0),
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
    ])
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Every generated tree builds; the walk visits every chunk exactly once
    /// and every non-root chunk's depth is parent.depth + 1.
    #[test]
    fn generated_trees_build(chunks in gen_chunks()) {
        let count = chunks.len() as u32;
        let bytes = package_for(&chunks);
        let pkg = parse(&bytes).expect("package parses");
        let doc = build_document(&pkg).expect("model builds");

        let ids = doc.all_ids();
        assert_eq!(ids.len() as u32, count);
        let unique: std::collections::HashSet<u32> = ids.iter().copied().collect();
        assert_eq!(unique.len() as u32, count, "pre-order walk is a bijection");

        for chunk in doc.chunks() {
            assert_eq!(chunk.id, chunk.ordinal + 1, "dense ordinal");
            if chunk.id != 1 {
                let parent = doc.chunk(chunk.parent_id).expect("parent exists");
                assert_eq!(chunk.depth, parent.depth + 1, "depth consistency");
            }
        }
    }

    /// Single-byte mutations of the golden never panic `build_document`.
    #[test]
    fn golden_mutations_never_panic_untyped(
        position in 0..common::pipeline_golden().len(),
        value in any::<u8>(),
    ) {
        let mut mutated = common::pipeline_golden().to_vec();
        mutated[position] = value;
        if let Ok(pkg) = parse(&mutated) {
            match build_document(&pkg) {
                Ok(doc) => {
                    let _ = doc.all_ids();
                    let _ = doc.plain_text(1);
                }
                Err(err) => {
                    assert!(
                        matches!(
                            err.kind,
                            DocumentErrorKind::MissingSection
                                | DocumentErrorKind::InvalidChunkGraph
                                | DocumentErrorKind::DanglingReference
                                | DocumentErrorKind::CountMismatch
                                | DocumentErrorKind::InvalidContent
                        ),
                        "unexpected kind {:?}",
                        err.kind
                    );
                }
            }
        }
    }
}

/// Building is deterministic: identical input yields an identical model.
#[test]
fn building_is_deterministic() {
    let golden = common::pipeline_golden();
    let a = parse(golden).expect("first parse");
    let b = parse(golden).expect("second parse");
    let doc_a = build_document(&a).expect("build a");
    let doc_b = build_document(&b).expect("build b");
    assert_eq!(doc_a.chunks(), doc_b.chunks());
    assert_eq!(doc_a.styles(), doc_b.styles());
    assert_eq!(doc_a.content(), doc_b.content());
    assert_eq!(doc_a.plain_text(1), doc_b.plain_text(1));
    assert_eq!(doc_a.all_ids(), doc_b.all_ids());
}
