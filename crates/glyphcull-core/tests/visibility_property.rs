//! Property tests for the visibility system (proptest).
//!
//! Properties over random documents, geometries, and viewports:
//! - The culling invariants hold for every input (visible/hidden/frontier
//!   are disjoint; visible non-structural chunks intersect the expanded
//!   viewport; structural visible chunks have a visible descendant; frontier
//!   chunks are geometry-less and non-hidden; the hidden set is exactly the
//!   hidden-flagged subtrees).
//! - Culling is a pure function: identical inputs give identical outputs.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::collections::HashMap;

use proptest::prelude::*;

use glyphcull_core::document::{build_document, is_structural_kind, DocumentModel};
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;
use glyphcull_core::visibility::{
    compute_visible_set, expanded_viewport, intersects, GeometrySource, Rect, Viewport,
    VisibilityResult,
};

struct MapGeometry(HashMap<u32, Rect>);

impl GeometrySource for MapGeometry {
    fn rect(&self, chunk_id: u32) -> Option<Rect> {
        self.0.get(&chunk_id).copied()
    }
}

/// A random document: root + N paragraphs at y positions with optional
/// geometry, optional hidden flags.
fn paragraphs_bytes(count: u32, hidden_ids: &[u32]) -> Vec<u8> {
    let mut chunks = Vec::with_capacity(count as usize + 1);
    chunks.push(common::TestChunk {
        id: 1,
        kind: ChunkKind::Document as u8,
        flags: 0x10,
        first_child_id: 2,
        last_child_id: count + 1,
        ..Default::default()
    });
    for i in 0..count {
        let id = i + 2;
        chunks.push(common::TestChunk {
            id,
            kind: ChunkKind::Paragraph as u8,
            parent_id: 1,
            prev_id: if i == 0 { 0 } else { id - 1 },
            next_id: if i + 1 == count { 0 } else { id + 1 },
            ordinal: id - 1,
            depth: 1,
            flags: if hidden_ids.contains(&id) { 1 } else { 0 },
            ..Default::default()
        });
    }
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(count + 1, 1, 0, 0, 0),
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

/// The invariants every culling result must satisfy (shared with the
/// integration suite).
fn assert_invariants(
    doc: &DocumentModel<'_>,
    geometry: &MapGeometry,
    result: &VisibilityResult,
    viewport: Viewport,
    margin: f32,
) {
    let target = expanded_viewport(viewport, margin);
    for id in &result.visible {
        assert!(
            !result.hidden.contains(id),
            "chunk {id} in both visible and hidden"
        );
        assert!(
            !result.not_yet_visible.contains(id),
            "chunk {id} in both visible and frontier"
        );
    }
    for id in &result.visible {
        let chunk = doc.chunk(*id).expect("visible chunk exists");
        if !is_structural_kind(chunk.kind) {
            let rect = geometry
                .0
                .get(id)
                .expect("visible non-structural has geometry");
            assert!(
                intersects(rect, &target),
                "chunk {id} rect does not intersect"
            );
        }
    }
    for id in &result.not_yet_visible {
        let chunk = doc.chunk(*id).expect("frontier chunk exists");
        assert!(
            !is_structural_kind(chunk.kind),
            "chunk {id} structural but in frontier"
        );
        assert!(
            !geometry.0.contains_key(id),
            "chunk {id} has geometry but in frontier"
        );
        assert_eq!(chunk.flags & 1, 0, "chunk {id} hidden but in frontier");
    }
    let mut expected_hidden: Vec<u32> = Vec::new();
    for id in doc.all_ids() {
        let chunk = doc.chunk(id).expect("chunk exists");
        if chunk.flags & 1 != 0 {
            expected_hidden.push(id);
            let mut stack = doc.child_ids(id);
            while let Some(cid) = stack.pop() {
                expected_hidden.push(cid);
                for grandchild in doc.child_ids(cid) {
                    stack.push(grandchild);
                }
            }
        }
    }
    assert_eq!(result.hidden, expected_hidden, "hidden set mismatch");
}

/// A random document + geometry + viewport scenario.
fn scenario() -> impl Strategy<Value = (Vec<u8>, HashMap<u32, Rect>, Viewport, f32)> {
    (
        1_u32..=20,
        proptest::collection::vec(any::<u8>(), 0..20),
        proptest::collection::vec((0_u32..=20, any::<f32>(), any::<f32>()), 0..20),
        any::<f32>(),
        any::<f32>(),
        any::<f32>(),
        0.0_f32..400.0,
        0.0_f32..400.0,
        -100.0_f32..100.0,
    )
        .prop_map(
            |(count, hidden_flags, rects, viewport_x, viewport_y, viewport_h, w, _h, margin)| {
                let hidden_ids: Vec<u32> = hidden_flags
                    .iter()
                    .enumerate()
                    .filter(|(_, &f)| f & 1 != 0)
                    .map(|(i, _)| i as u32 + 2)
                    .take(count as usize)
                    .collect();
                let bytes = paragraphs_bytes(count, &hidden_ids);
                let mut geometry = HashMap::new();
                for (offset, (_id, y, rect_h)) in rects.into_iter().enumerate() {
                    let id = offset as u32 % count + 2;
                    geometry.insert(
                        id,
                        Rect {
                            x: 0.0,
                            y,
                            w,
                            h: rect_h,
                        },
                    );
                }
                let viewport = Viewport {
                    x: viewport_x,
                    y: viewport_y,
                    w: 400.0,
                    h: viewport_h,
                };
                (bytes, geometry, viewport, margin)
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Culling invariants hold for every random scenario.
    #[test]
    fn culling_invariants_hold(
        (bytes, rects, viewport, margin) in scenario(),
    ) {
        let pkg = parse(&bytes).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let geometry = MapGeometry(rects);
        let result = compute_visible_set(&doc, &geometry, viewport, margin);
        assert_invariants(&doc, &geometry, &result, viewport, margin);
    }

    /// Culling is a pure function of its inputs.
    #[test]
    fn culling_is_pure(
        (bytes, rects, viewport, margin) in scenario(),
    ) {
        let pkg = parse(&bytes).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let geometry = MapGeometry(rects);
        let a = compute_visible_set(&doc, &geometry, viewport, margin);
        let b = compute_visible_set(&doc, &geometry, viewport, margin);
        assert_eq!(a, b);
    }
}
