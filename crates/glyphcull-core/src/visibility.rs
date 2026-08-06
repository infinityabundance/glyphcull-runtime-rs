//! The visibility system (Architecture.md §3.3; mirrors the JS
//! `src/visibility/visibility.ts`).
//!
//! Culling determines what should exist right now — nothing more. It walks
//! the chunk graph in document order, applies semantic culling (the `hidden`
//! flag excludes a chunk and its whole subtree), then geometric culling (a
//! renderable chunk is visible iff its laid-out geometry intersects the
//! viewport expanded by a margin). Chunks with no geometry yet are *not yet
//! visible* (beyond the materialization frontier), never merely absent.
//! Structural chunks (document, list, table, row) carry no geometry of their
//! own and are visible iff a descendant is visible.
//!
//! **Responsibility boundary**: culling only determines. It never
//! materializes, never generates glyphs, never paints — and it never mutates
//! the geometry source or the document. The visible set is a pure function
//! of (document, geometry, viewport, margin), which is what makes it
//! deterministic and testable.

use std::collections::HashSet;

use crate::document::{is_structural_kind, DocumentModel};
use crate::reader::chunk::flags;

/// An axis-aligned rectangle in document (CSS pixel) coordinates.
///
/// Coordinates are `f32` — the SPEC's glyph and layout metrics are `f32`, and
/// the renderer consumes `f32` geometry; the JS runtime's `number` (f64) is
/// equivalent for the inclusive-of-edges intersection test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// The geometry provider visibility consults (implemented by layout).
pub trait GeometrySource {
    /// The laid-out geometry of a chunk, or `None` when not yet materialized.
    fn rect(&self, chunk_id: u32) -> Option<Rect>;
}

/// A viewport: the visible document window in document coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

/// The result of one culling pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisibilityResult {
    /// Visible chunks in document order (renderable + structural context).
    pub visible: Vec<u32>,
    /// Chunks excluded by semantic culling (whole hidden subtrees), in walk
    /// order (the hidden chunk first, then its subtree).
    pub hidden: Vec<u32>,
    /// Chunks beyond the materialization frontier (no geometry yet), in
    /// document order.
    pub not_yet_visible: Vec<u32>,
}

/// Axis-aligned rectangle intersection (inclusive of edges).
#[must_use]
pub fn intersects(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}

/// The viewport expanded by `margin` pixels on every side.
#[must_use]
pub fn expanded_viewport(viewport: Viewport, margin: f32) -> Rect {
    Rect {
        x: viewport.x - margin,
        y: viewport.y - margin,
        w: viewport.w + margin * 2.0,
        h: viewport.h + margin * 2.0,
    }
}

/// Compute the visible set.
///
/// The walk is in document order (pre-order from the root), using an explicit
/// stack so arbitrarily deep documents cannot overflow the native stack (see
/// DESIGN.md D17). Semantic culling prunes hidden subtrees entirely;
/// geometry-less renderable chunks are reported as `not_yet_visible`;
/// structural chunks are reported visible when any descendant is visible.
///
/// The geometry source is consulted exactly once per non-hidden chunk.
pub fn compute_visible_set(
    doc: &DocumentModel<'_>,
    geometry: &dyn GeometrySource,
    viewport: Viewport,
    margin: f32,
) -> VisibilityResult {
    let target = expanded_viewport(viewport, margin);
    let mut hidden: Vec<u32> = Vec::new();
    let mut not_yet_visible: Vec<u32> = Vec::new();
    // Chunks whose own geometry intersects the target. Membership only (no
    // iteration), so the set's hash order never leaks into output.
    let mut own_visible: HashSet<u32> = HashSet::new();

    // Pass 1 (pre-order): semantic culling + geometric culling per chunk,
    // mirroring the JS recursive walk with an explicit stack.
    let mut stack = vec![doc.root().id];
    while let Some(id) = stack.pop() {
        let chunk = doc.chunk(id);
        let Some(chunk) = chunk else {
            continue;
        };
        if chunk.flags & flags::HIDDEN != 0 {
            // Semantic culling: the whole subtree is excluded, exactly like
            // the JS inner stack (the hidden chunk first, then a depth-first
            // walk of its descendants).
            hidden.push(id);
            let mut sub: Vec<u32> = doc.child_ids(id);
            while let Some(cid) = sub.pop() {
                hidden.push(cid);
                for grandchild in doc.child_ids(cid) {
                    sub.push(grandchild);
                }
            }
            continue;
        }
        let structural = is_structural_kind(chunk.kind);
        match geometry.rect(id) {
            Some(rect) => {
                if intersects(&rect, &target) {
                    own_visible.insert(id);
                }
            }
            None if !structural => {
                // Beyond the materialization frontier: not yet visible,
                // never absent.
                not_yet_visible.push(id);
            }
            None => {}
        }
        let children = doc.child_ids(id);
        for child in children.iter().rev() {
            stack.push(*child);
        }
    }

    // Pass 2 (reverse pre-order): a structural chunk is visible iff it has a
    // visible descendant; renderable chunks are visible iff their own
    // geometry intersects. Children are processed before their parents, so
    // the aggregation is bottom-up.
    let ids = doc.all_ids();
    let mut visible_set: HashSet<u32> = HashSet::new();
    for id in ids.iter().rev() {
        let chunk = doc.chunk(*id);
        let Some(chunk) = chunk else {
            continue;
        };
        let structural = is_structural_kind(chunk.kind);
        let child_visible = if structural {
            doc.child_ids(*id)
                .iter()
                .any(|child| visible_set.contains(child))
        } else {
            false
        };
        if own_visible.contains(id) || child_visible {
            visible_set.insert(*id);
        }
    }

    // Emit in document order (chunk ids are dense in document order).
    let visible: Vec<u32> = ids
        .iter()
        .filter(|id| visible_set.contains(id))
        .copied()
        .collect();
    VisibilityResult {
        visible,
        hidden,
        not_yet_visible,
    }
}
