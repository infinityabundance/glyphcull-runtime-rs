//! Selection (Architecture.md §3.8; mirrors the JS `src/selection/selection.ts`):
//! selection is logical, rendering is geometric.
//!
//! A [`TextPosition`] is a run chunk id plus a character offset into the run's
//! payload text — independent of pixels, so a selection stays stable across
//! re-materialization and scrolling. [`Selection`] is an ordered pair (start ≤
//! end in document order). Hit testing projects a document point onto the
//! nearest glyph; `range_quads` projects a selection back onto laid-out lines;
//! `copy_text` extracts plain text from chunk content with the documented
//! boundary policy:
//!
//! ```text
//! between runs of the same block        → ''        (the source text re-joins)
//! between blocks (paragraph boundary)   → '\n'
//! between cells of the same table row   → '\t'
//! between cells of different rows       → '\n'
//! `br` chunks                           → '\n'      (explicit hard break)
//! ```
//!
//! Everything here is a pure function of (document, layout, point/selection):
//! no state, no wall clock, deterministic. Document-order indices are computed
//! from the dense pre-order `all_ids` (a `Vec` indexed by chunk id), so the
//! comparison is O(1) after one O(n) index build per call, exactly like the JS
//! `orderIndex` Map.
//!
//! Offsets are char-based (the JS uses UTF-16 code units; DESIGN.md R3). For
//! BMP text (all fixtures) the two are identical.

use std::cmp::Ordering;

use crate::document::{is_block_kind, DocumentModel};
use crate::layout::layout::{LayoutEngine, LineLayout, RunLayout};
use crate::reader::chunk::{ChunkKind, ChunkRecord};

/// A point in document coordinates (CSS pixels).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// The x coordinate.
    pub x: f32,
    /// The y coordinate.
    pub y: f32,
}

/// A logical text position: a run chunk + a character offset in its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    /// The run chunk id.
    pub chunk_id: u32,
    /// Character offset into the run's payload text.
    pub offset: usize,
}

/// A normalized selection: `start` ≤ `end` in document order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The selection start.
    pub start: TextPosition,
    /// The selection end.
    pub end: TextPosition,
}

/// A selection highlight quad (document pixels), consumed by the draw list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionQuad {
    /// The quad's left edge.
    pub x: f32,
    /// The quad's top edge.
    pub y: f32,
    /// The quad's width.
    pub w: f32,
    /// The quad's height.
    pub h: f32,
}

/// The pre-order document index of every chunk id: `position[i]` is the
/// pre-order position of chunk `i + 1` (ids are dense, SPEC.md §2.2).
fn order_index(doc: &DocumentModel<'_>) -> Vec<usize> {
    let ids = doc.all_ids();
    let mut position = vec![0usize; ids.len()];
    for (i, id) in ids.iter().enumerate() {
        if let Some(slot) = position.get_mut((*id as usize).saturating_sub(1)) {
            *slot = i;
        }
    }
    position
}

/// The pre-order position of a chunk id (0 for out-of-range ids, mirroring
/// the JS `index.get(id) ?? 0`).
fn index_of(position: &[usize], chunk_id: u32) -> usize {
    position
        .get((chunk_id as usize).saturating_sub(1))
        .copied()
        .unwrap_or(0)
}

/// Compare two positions in document order: `Less`, `Equal`, or `Greater`.
#[must_use]
pub fn compare_positions(doc: &DocumentModel<'_>, a: TextPosition, b: TextPosition) -> Ordering {
    let index = order_index(doc);
    let ia = index_of(&index, a.chunk_id);
    let ib = index_of(&index, b.chunk_id);
    match ia.cmp(&ib) {
        Ordering::Equal => a.offset.cmp(&b.offset),
        other => other,
    }
}

/// Order two positions into a normalized selection (start ≤ end).
#[must_use]
pub fn normalize_selection(doc: &DocumentModel<'_>, a: TextPosition, b: TextPosition) -> Selection {
    if compare_positions(doc, a, b) == Ordering::Greater {
        Selection { start: b, end: a }
    } else {
        Selection { start: a, end: b }
    }
}

/// Whether a selection covers no text (start === end).
#[must_use]
pub fn is_collapsed(selection: Selection) -> bool {
    selection.start.chunk_id == selection.end.chunk_id
        && selection.start.offset == selection.end.offset
}

/// Hit test a document point against the laid-out text: the nearest line
/// (smallest vertical distance, document order on ties) and, within it, the
/// nearest glyph center. Returns the run position, or `None` when the
/// document has no laid-out text (images and rulers carry no text positions).
#[must_use]
pub fn hit_test_point(layout: &LayoutEngine<'_>, point: Point) -> Option<TextPosition> {
    let mut best: Option<(&LineLayout, f32)> = None;
    for block in layout.records_all().values() {
        for line in &block.lines {
            if line.runs.is_empty() {
                continue;
            }
            let top = line.y;
            let bottom = line.y + line.height_px;
            let dy = if point.y < top {
                top - point.y
            } else if point.y > bottom {
                point.y - bottom
            } else {
                0.0
            };
            match best {
                None => best = Some((line, dy)),
                Some((_, best_dy)) => {
                    if dy < best_dy {
                        best = Some((line, dy));
                    }
                }
            }
        }
    }
    let (line, _) = best?;
    Some(position_in_line(line, point.x))
}

/// The run position nearest `x` on a laid-out line.
fn position_in_line(line: &LineLayout, x: f32) -> TextPosition {
    // Marks ride with their base glyph (advance 0) and never anchor a position.
    for glyph in &line.glyphs {
        if glyph.mark_of.is_some() {
            continue;
        }
        if x < glyph.x + glyph.advance_px / 2.0 {
            return TextPosition {
                chunk_id: glyph.run_chunk_id,
                offset: glyph.offset_in_text,
            };
        }
    }
    if let Some(last) = line.glyphs.iter().rev().find(|g| g.mark_of.is_none()) {
        return TextPosition {
            chunk_id: last.run_chunk_id,
            offset: last.offset_in_text + 1,
        };
    }
    // No glyphs (e.g. missing atlas): anchor at the first run's start.
    if let Some(first) = line.runs.first() {
        return TextPosition {
            chunk_id: first.chunk_id,
            offset: first.start,
        };
    }
    // Unreachable: every laid-out line carries at least one run (R2).
    TextPosition {
        chunk_id: 0,
        offset: 0,
    }
}

/// Project a selection onto the laid-out lines as highlight quads, in document
/// order, merged per line where pieces are contiguous. A collapsed selection
/// yields no quads. Runs without glyph geometry fall back to a proportional
/// rect inside the run box.
#[must_use]
pub fn range_quads(layout: &LayoutEngine<'_>, selection: Selection) -> Vec<SelectionQuad> {
    if is_collapsed(selection) {
        return Vec::new();
    }
    let doc = layout.document();
    let index = order_index(doc);
    let start_index = index_of(&index, selection.start.chunk_id);
    let end_index = index_of(&index, selection.end.chunk_id);
    let mut quads: Vec<SelectionQuad> = Vec::new();
    for block in layout.records_all().values() {
        for line in &block.lines {
            let mut pieces: Vec<SelectionQuad> = Vec::new();
            for run in &line.runs {
                let run_index = index_of(&index, run.chunk_id);
                if run_index < start_index || run_index > end_index {
                    continue;
                }
                let (from, to) = if run_index == start_index && run_index == end_index {
                    (
                        run.start.max(selection.start.offset),
                        run.end.min(selection.end.offset),
                    )
                } else if run_index == start_index {
                    (run.start.max(selection.start.offset), run.end)
                } else if run_index == end_index {
                    (run.start, run.end.min(selection.end.offset))
                } else {
                    (run.start, run.end)
                };
                if from >= to {
                    continue;
                }
                pieces.push(covered_piece(line, run, from, to));
            }
            // Merge contiguous pieces on the same line (a selection within one
            // run is already a single piece; adjacent full runs merge).
            let mut merged: Option<SelectionQuad> = None;
            for piece in pieces {
                match merged {
                    None => merged = Some(piece),
                    Some(m) => {
                        if piece.x <= m.x + m.w + 0.5 {
                            merged = Some(SelectionQuad {
                                x: m.x,
                                y: m.y,
                                w: piece.x + piece.w - m.x,
                                h: m.h,
                            });
                        } else {
                            quads.push(m);
                            merged = Some(piece);
                        }
                    }
                }
            }
            if let Some(m) = merged {
                quads.push(m);
            }
        }
    }
    quads
}

/// The highlight rect of a covered sub-range `[from, to)` of a run on a line.
fn covered_piece(line: &LineLayout, run: &RunLayout, from: usize, to: usize) -> SelectionQuad {
    let mut x_min = f32::INFINITY;
    let mut x_max = f32::NEG_INFINITY;
    let mut found = false;
    for glyph in &line.glyphs {
        if glyph.run_chunk_id != run.chunk_id
            || glyph.offset_in_text < from
            || glyph.offset_in_text >= to
        {
            continue;
        }
        found = true;
        x_min = x_min.min(glyph.x);
        x_max = x_max.max(glyph.x + glyph.advance_px);
    }
    if !found {
        // No glyph geometry (missing atlas): proportional within the run box.
        let run_chars = run.text.chars().count().max(1) as f32;
        let f0 = (from.saturating_sub(run.start)) as f32 / run_chars;
        let f1 = (to.saturating_sub(run.start)) as f32 / run_chars;
        x_min = run.x + run.width * f0;
        x_max = run.x + run.width * f1;
    }
    SelectionQuad {
        x: x_min,
        y: line.y,
        w: (x_max - x_min).max(0.0),
        h: line.height_px,
    }
}

/// The chunk ids between the selection's endpoints, inclusive, document order.
#[must_use]
pub fn covered_chunk_ids(doc: &DocumentModel<'_>, selection: Selection) -> Vec<u32> {
    if is_collapsed(selection) {
        return Vec::new();
    }
    let index = order_index(doc);
    let start_index = index_of(&index, selection.start.chunk_id);
    let end_index = index_of(&index, selection.end.chunk_id);
    let ids = doc.all_ids();
    ids.into_iter()
        .skip(start_index)
        .take(end_index.saturating_sub(start_index) + 1)
        .collect()
}

/// Extract the plain text covered by a selection, preserving document order
/// with the boundary policy in the module doc. A collapsed selection returns
/// the empty string.
#[must_use]
pub fn copy_text(doc: &DocumentModel<'_>, selection: Selection) -> String {
    if is_collapsed(selection) {
        return String::new();
    }
    let index = order_index(doc);
    let start_index = index_of(&index, selection.start.chunk_id);
    let end_index = index_of(&index, selection.end.chunk_id);
    let mut pieces: Vec<String> = Vec::new();
    let mut parents: Vec<u32> = Vec::new();
    for id in doc.all_ids() {
        let i = index_of(&index, id);
        if i < start_index || i > end_index {
            continue;
        }
        let Some(chunk) = doc.chunk(id) else {
            continue;
        };
        if chunk.kind == ChunkKind::Br {
            pieces.push("\n".to_string());
            parents.push(block_parent(doc, chunk));
            continue;
        }
        let Some(text) = doc.direct_text(id) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        // Slice by char offsets (R3); the byte offset caps at the text end.
        let start_byte = if id == selection.start.chunk_id {
            char_byte_offset(text, selection.start.offset)
        } else {
            0
        };
        let end_byte = if id == selection.end.chunk_id {
            char_byte_offset(text, selection.end.offset)
        } else {
            text.len()
        };
        let end_byte = end_byte.min(text.len()).max(start_byte);
        // The range is provably in bounds (start <= end <= len); the None
        // arm is unreachable and exists only to keep this panic-free.
        let Some(slice) = text.get(start_byte..end_byte) else {
            continue;
        };
        if slice.is_empty() {
            continue;
        }
        pieces.push(slice.to_string());
        parents.push(block_parent(doc, chunk));
    }
    let mut out = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        if i > 0 {
            if let (Some(prev), Some(curr)) = (parents.get(i - 1), parents.get(i)) {
                out.push_str(separator(doc, *prev, *curr));
            }
        }
        out.push_str(piece);
    }
    out
}

/// The byte offset of a char offset in `text` (capped at the end).
fn char_byte_offset(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map_or(text.len(), |(byte, _)| byte)
}

/// The nearest block ancestor of a chunk (itself included).
fn block_parent(doc: &DocumentModel<'_>, chunk: &ChunkRecord) -> u32 {
    let mut id = chunk.id;
    loop {
        let Some(current) = doc.chunk(id) else {
            return id;
        };
        if is_block_kind(current.kind) {
            return id;
        }
        if current.parent_id == 0 {
            return id;
        }
        id = current.parent_id;
    }
}

/// The TableCell ancestor of a block, or `None`.
fn cell_of(doc: &DocumentModel<'_>, block_id: u32) -> Option<u32> {
    let mut id = block_id;
    loop {
        let current = doc.chunk(id)?;
        if current.kind == ChunkKind::TableCell {
            return Some(id);
        }
        if current.parent_id == 0 {
            return None;
        }
        id = current.parent_id;
    }
}

/// The separator between two text pieces (see the module doc).
fn separator(doc: &DocumentModel<'_>, prev_block: u32, next_block: u32) -> &'static str {
    if prev_block == next_block {
        return "";
    }
    let prev_cell = cell_of(doc, prev_block);
    let next_cell = cell_of(doc, next_block);
    if let (Some(prev_cell), Some(next_cell)) = (prev_cell, next_cell) {
        if prev_cell == next_cell {
            return "\n"; // two paragraphs inside one cell
        }
        let prev_row = doc.chunk(prev_cell).map_or(0, |c| c.parent_id);
        let next_row = doc.chunk(next_cell).map_or(0, |c| c.parent_id);
        return if prev_row == next_row { "\t" } else { "\n" };
    }
    "\n"
}
