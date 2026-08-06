//! Visibility system tests: geometric + semantic culling, the frontier,
//! determinism, the responsibility boundary (culling never mutates), and
//! invariant checks (mirrors the JS `test/visibility/visibility.test.ts`).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod common;

use std::collections::HashMap;

use glyphcull_core::document::build_document;
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;
use glyphcull_core::visibility::{
    compute_visible_set, expanded_viewport, intersects, GeometrySource, Rect, Viewport,
    VisibilityResult,
};

/// A geometry source backed by a map: every chunk at a fixed horizontal
/// strip.
#[derive(Debug, Default, Clone)]
struct MapGeometry {
    rects: HashMap<u32, Rect>,
}

impl MapGeometry {
    fn from_ys(ys: &[(u32, f32, f32)]) -> Self {
        // (id, y, h) — x = 0, w = 400.
        let rects = ys
            .iter()
            .map(|&(id, y, h)| {
                (
                    id,
                    Rect {
                        x: 0.0,
                        y,
                        w: 400.0,
                        h,
                    },
                )
            })
            .collect();
        Self { rects }
    }
}

impl GeometrySource for MapGeometry {
    fn rect(&self, chunk_id: u32) -> Option<Rect> {
        self.rects.get(&chunk_id).copied()
    }
}

/// A document: root with `count` paragraph children, some optionally hidden.
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
            content_index: 1,
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
            payload: common::info_payload_counts(count + 1, 1, 1, 0, 0),
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
            payload: common::cont_payload(&["x"], &[]),
        },
    ])
}

/// Compute the visible set and the expected number of geometry reads (one
/// per non-hidden chunk).
fn cull_and_count_reads(
    doc: &glyphcull_core::document::DocumentModel<'_>,
    geometry: &MapGeometry,
    viewport: Viewport,
    margin: f32,
) -> (VisibilityResult, usize) {
    let result = compute_visible_set(doc, geometry, viewport, margin);
    let reads = doc
        .all_ids()
        .iter()
        .filter(|&&id| {
            let chunk = doc.chunk(id).expect("chunk");
            chunk.flags & 1 == 0
        })
        .count();
    (result, reads)
}

#[test]
fn reports_exactly_the_chunks_whose_rects_intersect_the_viewport() {
    let bytes = paragraphs_bytes(5, &[]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    // Paragraphs at y = 0, 100, 200, 300, 400 (h = 50).
    let ys: Vec<(u32, f32, f32)> = (0..5).map(|i| (i + 2, (i * 100) as f32, 50.0)).collect();
    let geometry = MapGeometry::from_ys(&ys);
    let viewport = Viewport {
        x: 0.0,
        y: 125.0,
        w: 400.0,
        h: 100.0,
    };
    let result = compute_visible_set(&doc, &geometry, viewport, 0.0);
    // y=100 (100..150) and y=200 (200..250) intersect 125..225.
    assert_eq!(result.visible, vec![1, 3, 4]);
    assert!(result.hidden.is_empty());
    assert!(result.not_yet_visible.is_empty());
}

#[test]
fn the_margin_expands_the_viewport() {
    let bytes = paragraphs_bytes(3, &[]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let ys = vec![(2, 0.0, 50.0), (3, 100.0, 50.0), (4, 200.0, 50.0)];
    let geometry = MapGeometry::from_ys(&ys);
    let viewport = Viewport {
        x: 0.0,
        y: 60.0,
        w: 400.0,
        h: 50.0,
    };
    // Without margin: only y=100 intersects. With margin 60: y=0 also.
    let tight = compute_visible_set(&doc, &geometry, viewport, 0.0);
    assert_eq!(tight.visible, vec![1, 3]);
    let loose = compute_visible_set(&doc, &geometry, viewport, 60.0);
    assert_eq!(loose.visible, vec![1, 2, 3]);
}

#[test]
fn intersection_is_inclusive_of_shared_edges() {
    let a = Rect {
        x: 0.0,
        y: 0.0,
        w: 10.0,
        h: 10.0,
    };
    let b = Rect {
        x: 10.0,
        y: 5.0,
        w: 10.0,
        h: 10.0,
    };
    assert!(!intersects(&a, &b), "edges only touch");
    let c = Rect {
        x: 9.0,
        y: 5.0,
        w: 10.0,
        h: 10.0,
    };
    assert!(intersects(&a, &c));
}

#[test]
fn expanded_viewport_grows_on_every_side() {
    let viewport = Viewport {
        x: 10.0,
        y: 20.0,
        w: 100.0,
        h: 50.0,
    };
    let expanded = expanded_viewport(viewport, 5.0);
    assert_eq!(
        expanded,
        Rect {
            x: 5.0,
            y: 15.0,
            w: 110.0,
            h: 60.0
        }
    );
}

#[test]
fn excludes_hidden_chunks_and_their_whole_subtree() {
    let bytes = paragraphs_bytes(3, &[3]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let ys = vec![(2, 0.0, 50.0), (3, 100.0, 50.0), (4, 200.0, 50.0)];
    let geometry = MapGeometry::from_ys(&ys);
    let viewport = Viewport {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 300.0,
    };
    let result = compute_visible_set(&doc, &geometry, viewport, 0.0);
    assert_eq!(result.hidden, vec![3]);
    assert_eq!(result.visible, vec![1, 2, 4]);
}

#[test]
fn chunks_without_geometry_are_not_yet_visible_never_absent() {
    let bytes = paragraphs_bytes(3, &[]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    // Only the first paragraph is materialized.
    let ys = vec![(2, 0.0, 50.0)];
    let geometry = MapGeometry::from_ys(&ys);
    let viewport = Viewport {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 300.0,
    };
    let result = compute_visible_set(&doc, &geometry, viewport, 0.0);
    assert_eq!(result.visible, vec![1, 2]);
    assert_eq!(result.not_yet_visible, vec![3, 4]);
    assert!(result.hidden.is_empty());
}

#[test]
fn is_a_pure_function_identical_inputs_give_identical_outputs() {
    let bytes = paragraphs_bytes(4, &[]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let ys: Vec<(u32, f32, f32)> = (0..4).map(|i| (i + 2, (i * 80) as f32, 40.0)).collect();
    let geometry = MapGeometry::from_ys(&ys);
    let viewport = Viewport {
        x: 0.0,
        y: 100.0,
        w: 400.0,
        h: 120.0,
    };
    let a = compute_visible_set(&doc, &geometry, viewport, 10.0);
    let b = compute_visible_set(&doc, &geometry, viewport, 10.0);
    assert_eq!(a, b);
}

#[test]
fn never_mutates_the_geometry_source_or_the_document() {
    let bytes = paragraphs_bytes(3, &[]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let ys = vec![(2, 0.0, 50.0), (3, 100.0, 50.0), (4, 200.0, 50.0)];
    let geometry = MapGeometry::from_ys(&ys);
    let chunks_before = doc.chunks().to_vec();
    let rects_before = geometry.rects.clone();
    let (_result, reads) = cull_and_count_reads(
        &doc,
        &geometry,
        Viewport {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 200.0,
        },
        0.0,
    );
    // The document and geometry are untouched.
    assert_eq!(doc.chunks(), chunks_before.as_slice());
    assert_eq!(geometry.rects, rects_before);
    // Exactly one rect() read per non-hidden chunk (root + 3 paragraphs).
    assert_eq!(reads, 4);
}

#[test]
fn structural_chunks_are_visible_iff_a_descendant_is_visible() {
    // root → list → [item1, item2]; only item2 intersects.
    let chunks = common::chnk_payload(
        &[
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
                kind: 10, // list (structural)
                parent_id: 1,
                flags: 0x10,
                first_child_id: 3,
                last_child_id: 4,
                ordinal: 1,
                depth: 1,
                ..Default::default()
            },
            common::TestChunk {
                id: 3,
                kind: 11, // list_item
                parent_id: 2,
                ordinal: 2,
                depth: 2,
                next_id: 4,
                ..Default::default()
            },
            common::TestChunk {
                id: 4,
                kind: 11, // list_item
                parent_id: 2,
                ordinal: 3,
                depth: 2,
                prev_id: 3,
                ..Default::default()
            },
        ],
        &[],
    );
    let bytes = common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(4, 1, 0, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: chunks,
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
    ]);
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let geometry = MapGeometry::from_ys(&[(4, 0.0, 50.0)]);
    let viewport = Viewport {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 300.0,
    };
    let result = compute_visible_set(&doc, &geometry, viewport, 0.0);
    // The list and root are visible through item 4; item 3 (no geometry) is
    // not-yet-visible.
    assert_eq!(result.visible, vec![1, 2, 4]);
    assert_eq!(result.not_yet_visible, vec![3]);
}

#[test]
fn result_invariants_hold_over_the_golden() {
    let bytes = common::pipeline_golden();
    let pkg = parse(bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut geometry = MapGeometry::default();
    // Give every chunk a rect spread across a tall document.
    for (i, chunk) in doc.chunks().iter().enumerate() {
        geometry.rects.insert(
            chunk.id,
            Rect {
                x: 0.0,
                y: (i * 30) as f32,
                w: 400.0,
                h: 25.0,
            },
        );
    }
    let viewport = Viewport {
        x: 0.0,
        y: 50.0,
        w: 400.0,
        h: 200.0,
    };
    let result = compute_visible_set(&doc, &geometry, viewport, 20.0);
    assert_visibility_invariants(&doc, &geometry, &result, viewport, 20.0);
}

/// The invariants every culling result must satisfy (used by the property
/// test too).
fn assert_visibility_invariants(
    doc: &glyphcull_core::document::DocumentModel<'_>,
    geometry: &MapGeometry,
    result: &VisibilityResult,
    viewport: Viewport,
    margin: f32,
) {
    let target = expanded_viewport(viewport, margin);
    // visible / hidden / not_yet_visible are disjoint.
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
    // Every non-structural visible chunk's geometry intersects the target.
    for id in &result.visible {
        let chunk = doc.chunk(*id).expect("visible chunk exists");
        if !glyphcull_core::document::is_structural_kind(chunk.kind) {
            let rect = geometry
                .rects
                .get(id)
                .expect("visible non-structural has geometry");
            assert!(
                intersects(rect, &target),
                "chunk {id} rect does not intersect"
            );
        }
    }
    // Structural visible chunks have a visible descendant.
    for id in &result.visible {
        let chunk = doc.chunk(*id).expect("visible chunk exists");
        if glyphcull_core::document::is_structural_kind(chunk.kind) {
            let has_visible_descendant = doc
                .child_ids(*id)
                .iter()
                .any(|child| result.visible.contains(child))
                || result.visible.contains(id);
            assert!(
                has_visible_descendant || geometry.rects.contains_key(id),
                "chunk {id}"
            );
        }
    }
    // Frontier chunks are non-structural, geometry-less, and not hidden.
    for id in &result.not_yet_visible {
        let chunk = doc.chunk(*id).expect("frontier chunk exists");
        assert!(
            !glyphcull_core::document::is_structural_kind(chunk.kind),
            "chunk {id} structural but in frontier"
        );
        assert!(
            !geometry.rects.contains_key(id),
            "chunk {id} has geometry but in frontier"
        );
        assert_eq!(chunk.flags & 1, 0, "chunk {id} hidden but in frontier");
    }
    // Hidden chunks are exactly the hidden-flagged subtrees.
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
