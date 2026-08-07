//! Block layout (Architecture.md §3.5; mirrors the JS `src/layout/layout.ts`):
//! materialization turns semantic chunks into renderable geometry.
//!
//! Text blocks (paragraphs, headings, captions, quotes) break lines with the
//! Knuth–Plass algorithm (UAX #29-grounded tokenization, `breaks.rs`,
//! `kp.rs`); code blocks are preformatted; lists emit markers; tables use a
//! deterministic auto layout honoring colspan/rowspan; images keep their
//! intrinsic aspect ratio. Every record is absolute geometry in document
//! pixels — the runtime owns no CSS cascade (styles are resolved in the
//! package).
//!
//! The engine drives a **sequential frontier**: top-level blocks are laid
//! out in document order, so chunks beyond the viewport are never laid out
//! (streaming). `extend_to` advances the frontier until the viewport is
//! covered; `materialize` lays out one top-level block (idempotent).
//!
//! Ownership model (DESIGN.md D24): records are shared via `Rc` so a nested
//! block exists exactly once — the `records` index and its parent's
//! `children` list hold the same allocation, exactly like the JS runtime's
//! object sharing. Without this, an n-deep tree would be duplicated at every
//! ancestor (O(n²) memory). Records are immutable after construction.
//!
//! Precision (DESIGN.md D24): text widths and Knuth–Plass ratios are
//! computed in f64 (matching the JS `number`); geometry is f32 at the record
//! boundary (the crate's `Rect` and renderer contract). The two meet at
//! explicit `as f32` casts.

use std::collections::BTreeMap;
use std::rc::Rc;

use crate::document::{is_block_kind, DocumentModel, ResolvedStyle};
use crate::layout::breaks::{tokenize_for_breaking, BreakClass};
use crate::layout::kp::{line_break, KpItem, KpLine};
use crate::layout::measure::{atlas_metrics, measure_run, sum_advances};
use crate::reader::chunk::{ChunkKind, ChunkRecord, ExtraData};
use crate::reader::glyph::Atlas;
use crate::visibility::{GeometrySource, Rect};

/// The quote indent in CSS px (mirrors the JS `QUOTE_INDENT`).
pub const QUOTE_INDENT: f32 = 24.0;
/// The list marker gutter width in em multiples (mirrors the JS
/// `MARKER_GUTTER`).
pub const MARKER_GUTTER: f32 = 1.6;

/// The list marker styles (SPEC.md §2.3 tag 11; mirrors the JS `ListStyle`).
pub mod list_style {
    /// No marker.
    pub const NONE: u8 = 0;
    /// `•` disc.
    pub const DISC: u8 = 1;
    /// `◦` hollow disc.
    pub const CIRCLE: u8 = 2;
    /// `▪` square.
    pub const SQUARE: u8 = 3;
    /// Decimal counter (`1.`).
    pub const DECIMAL: u8 = 4;
    /// Lowercase alpha counter (`a.`).
    pub const LOWER_ALPHA: u8 = 5;
    /// Uppercase alpha counter (`A.`).
    pub const UPPER_ALPHA: u8 = 6;
    /// Lowercase roman counter (`iv.`).
    pub const LOWER_ROMAN: u8 = 7;
    /// Uppercase roman counter (`IV.`).
    pub const UPPER_ROMAN: u8 = 8;
}

/// One placed glyph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphInstance {
    /// The codepoint (Unicode scalar value).
    pub codepoint: u32,
    /// Baseline origin in document pixels.
    pub x: f32,
    /// Baseline origin in document pixels.
    pub y: f32,
    /// The advance in document pixels.
    pub advance_px: f32,
    /// The atlas (font) id.
    pub atlas_id: u32,
    /// The font size in pixels.
    pub font_size_px: f32,
    /// The text color as u32 RGBA.
    pub color: u32,
    /// The owning run chunk id.
    pub run_chunk_id: u32,
    /// Offset into the run's payload text (in chars).
    pub offset_in_text: usize,
    /// Whether the atlas has an outline for this codepoint.
    pub has_outline: bool,
    /// Index (into the line's glyphs) of the base glyph a mark attaches to.
    pub mark_of: Option<usize>,
}

/// One laid-out text run (a slice of a run chunk's text).
#[derive(Debug, Clone, PartialEq)]
pub struct RunLayout {
    /// The run chunk id.
    pub chunk_id: u32,
    /// The run's text.
    pub text: String,
    /// Offset into the run's payload text (in chars).
    pub start: usize,
    /// End offset into the run's payload text (in chars).
    pub end: usize,
    /// Baseline origin in document pixels.
    pub x: f32,
    /// Baseline origin in document pixels.
    pub y: f32,
    /// Advance width in document pixels.
    pub width: f32,
    /// The run's resolved style.
    pub style: ResolvedStyle,
    /// The atlas (font) id.
    pub atlas_id: u32,
}

/// One laid-out line.
#[derive(Debug, Clone, PartialEq)]
pub struct LineLayout {
    /// Line box top in document pixels.
    pub y: f32,
    /// The baseline of the line.
    pub baseline: f32,
    /// The width the line was set to.
    pub width: f32,
    /// The line box height in pixels.
    pub height_px: f32,
    /// The Knuth–Plass ratio (0 for preformatted lines).
    pub ratio: f64,
    /// The line's runs.
    pub runs: Vec<RunLayout>,
    /// The line's placed glyphs.
    pub glyphs: Vec<GlyphInstance>,
}

/// A list-item marker.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerLayout {
    /// The marker text.
    pub text: String,
    /// Baseline origin in document pixels.
    pub x: f32,
    /// Baseline origin in document pixels.
    pub y: f32,
    /// The marker gutter width in pixels.
    pub width: f32,
}

/// A placed image quad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageQuad {
    /// The image id.
    pub image_id: u32,
    /// The quad's left edge.
    pub x: f32,
    /// The quad's top edge.
    pub y: f32,
    /// The quad's width.
    pub w: f32,
    /// The quad's height.
    pub h: f32,
}

/// A horizontal rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ruler {
    /// The ruler's left edge.
    pub x: f32,
    /// The ruler's vertical center.
    pub y: f32,
    /// The ruler's width.
    pub w: f32,
}

/// A table cell placement.
#[derive(Debug, Clone, PartialEq)]
pub struct CellPlacement {
    /// The laid-out cell block.
    pub cell: Rc<BlockLayout>,
    /// The column span.
    pub colspan: u16,
    /// The row span.
    pub rowspan: u16,
    /// The cell's left edge.
    pub x: f32,
    /// The cell's top edge.
    pub y: f32,
    /// The cell's width.
    pub w: f32,
    /// The cell's height.
    pub h: f32,
}

/// A laid-out table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableLayout {
    /// The table's left edge.
    pub x: f32,
    /// The table's top edge.
    pub y: f32,
    /// The table's width.
    pub w: f32,
    /// The final column widths.
    pub columns: Vec<f32>,
    /// The placements, one row of cells per table row.
    pub rows: Vec<Vec<CellPlacement>>,
}

/// The layout record of one block chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockLayout {
    /// The chunk id.
    pub chunk_id: u32,
    /// The chunk kind.
    pub kind: ChunkKind,
    /// The chunk's resolved style.
    pub style: ResolvedStyle,
    /// Content box left (absolute document pixels).
    pub x: f32,
    /// Content box top (absolute document pixels).
    pub y: f32,
    /// Content box width.
    pub w: f32,
    /// Content box height.
    pub h: f32,
    /// The block's laid-out lines (text blocks).
    pub lines: Vec<LineLayout>,
    /// The block's laid-out children (containers), in document order.
    pub children: Vec<Rc<BlockLayout>>,
    /// The list-item marker, when the block is a list item.
    pub marker: Option<MarkerLayout>,
    /// The placed image quad, when the block is an image.
    pub image: Option<ImageQuad>,
    /// The table layout, when the block is a table.
    pub table: Option<TableLayout>,
    /// The ruler, when the block is an `hr`.
    pub ruler: Option<Ruler>,
}

/// Options for the layout engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    /// Device pixel ratio (affects natural image size).
    pub dpr: f32,
    /// The page content width in CSS pixels.
    pub content_width: f32,
}

/// One measured, styled token (a slice of a run's text).
#[derive(Debug, Clone)]
struct LaidToken {
    text: String,
    run_chunk_id: u32,
    style: ResolvedStyle,
    atlas_id: u32,
    font_size_px: f32,
    start_char: usize,
    end_char: usize,
    width: f64,
    break_after: BreakClass,
}

/// A table row's cell ids and spans (parallel arrays, mirroring the JS).
struct TableRowSpec {
    cell_ids: Vec<u32>,
    /// `(colspan, rowspan)`.
    spans: Vec<(u16, u16)>,
}

/// The layout engine.
///
/// Implements the visibility system's [`GeometrySource`] contract: `rect`
/// answers the laid-out geometry of any chunk (block or run) once laid out.
pub struct LayoutEngine<'a> {
    doc: &'a DocumentModel<'a>,
    dpr: f32,
    content_width: f32,
    /// Every laid-out block (nested included), keyed by chunk id.
    records: BTreeMap<u32, Rc<BlockLayout>>,
    /// Run rectangles for geometry queries (visibility, hit testing).
    run_rects: BTreeMap<u32, Rect>,
    /// Top-level block ids in document order (the frontier).
    top_level_blocks: Vec<u32>,
    /// The frontier head index.
    block_index: usize,
    /// The y position the frontier has reached.
    cursor_y: f32,
}

impl<'a> LayoutEngine<'a> {
    /// Create an engine over the given document model.
    #[must_use]
    pub fn new(doc: &'a DocumentModel<'a>, options: LayoutOptions) -> Self {
        let top_level_blocks = doc.child_ids(doc.root().id);
        Self {
            doc,
            dpr: options.dpr,
            content_width: options.content_width,
            records: BTreeMap::new(),
            run_rects: BTreeMap::new(),
            top_level_blocks,
            block_index: 0,
            cursor_y: 0.0,
        }
    }

    /// The document model this engine lays out.
    #[must_use]
    pub const fn document(&self) -> &'a DocumentModel<'a> {
        self.doc
    }

    /// The next top-level block chunk id to lay out (the frontier head).
    #[must_use]
    pub fn next_frontier_block(&self) -> Option<u32> {
        self.top_level_blocks.get(self.block_index).copied()
    }

    /// Whether the frontier has advanced past every top-level block.
    #[must_use]
    pub fn frontier_exhausted(&self) -> bool {
        self.block_index >= self.top_level_blocks.len()
    }

    /// The y position the frontier has reached.
    #[must_use]
    pub const fn frontier_y(&self) -> f32 {
        self.cursor_y
    }

    /// GeometrySource: the rect of a laid-out chunk (block or run).
    #[must_use]
    pub fn rect(&self, chunk_id: u32) -> Option<Rect> {
        if let Some(block) = self.records.get(&chunk_id) {
            return Some(Rect {
                x: block.x,
                y: block.y,
                w: block.w,
                h: block.h,
            });
        }
        self.run_rects.get(&chunk_id).copied()
    }

    /// The layout record of a laid-out block chunk.
    #[must_use]
    pub fn record(&self, chunk_id: u32) -> Option<&BlockLayout> {
        self.records.get(&chunk_id).map(Rc::as_ref)
    }

    /// All laid-out block records, keyed by chunk id (deterministic order).
    #[must_use]
    pub const fn records_all(&self) -> &BTreeMap<u32, Rc<BlockLayout>> {
        &self.records
    }

    /// Advance the frontier until the layout covers `bottom_y` document
    /// pixels (or every top-level block is laid out).
    ///
    /// `bottom_y` is f64 so callers may pass `f64::INFINITY` to lay out the
    /// whole document (mirrors the JS `extendTo(Number.POSITIVE_INFINITY)`).
    pub fn extend_to(&mut self, bottom_y: f64) {
        let mut guard = 0usize;
        while !self.frontier_exhausted()
            && f64::from(self.cursor_y) < bottom_y
            && guard < self.top_level_blocks.len() + 1
        {
            if let Some(chunk_id) = self.next_frontier_block() {
                let _ = self.materialize(chunk_id);
            }
            guard += 1;
        }
    }

    /// Lay out one top-level block (idempotent). Nested blocks (list items,
    /// quote children, table cells) are laid out as part of their parent.
    ///
    /// Returns the block's record; `None` when the chunk id names no block
    /// kind (mirrors the JS contract for the non-frontier branch).
    pub fn materialize(&mut self, chunk_id: u32) -> Option<Rc<BlockLayout>> {
        if let Some(existing) = self.records.get(&chunk_id) {
            return Some(Rc::clone(existing));
        }
        let index = self.top_level_blocks.iter().position(|&id| id == chunk_id);
        match index {
            None => {
                // Not a top-level block: lay it out nested at the cursor.
                let chunk = self.doc.chunk(chunk_id)?;
                if !is_block_kind(chunk.kind) {
                    return None;
                }
                // `layout_block` registers the record.
                Some(self.layout_block(chunk, 0.0, self.cursor_y, self.content_width))
            }
            Some(index) => {
                let chunk = self.doc.chunk(chunk_id)?;
                let layout = self.layout_block(chunk, 0.0, self.cursor_y, self.content_width);
                self.cursor_y = layout.y + layout.h;
                self.block_index = index + 1;
                Some(layout)
            }
        }
    }

    // ---------------------------------------------------------------------
    // Style helpers

    /// The resolved style of a chunk, falling back to the document default
    /// (mirrors the JS `styles[styleId] ?? styles[0]`).
    fn style_of(&self, chunk_id: u32) -> ResolvedStyle {
        let styles = self.doc.styles();
        self.doc
            .chunk(chunk_id)
            .and_then(|chunk| styles.get(chunk.style_id as usize))
            .copied()
            .or_else(|| styles.first().copied())
            .unwrap_or_default()
    }

    /// The atlas of a resolved style, or `None` when the font id is out of
    /// range (defensive; the compiler validates font ids).
    fn atlas_for(&self, style: ResolvedStyle) -> Option<&'a Atlas> {
        self.doc.atlases().get(style.font_id as usize)
    }

    /// The baseline offset of a line box of the given style.
    fn baseline_offset(&self, style: ResolvedStyle) -> f32 {
        let atlas = self.atlas_for(style);
        let font_size = f64::from(style.font_size_px);
        let line_height = font_size * f64::from(style.line_height);
        let Some(atlas) = atlas else {
            return style.font_size_px * 0.8;
        };
        let m = atlas_metrics(atlas);
        let ascent = f64::from(m.ascent);
        let descent = f64::from(m.descent);
        (ascent * font_size + (line_height - (ascent + descent) * font_size) / 2.0) as f32
    }

    // ---------------------------------------------------------------------
    // Block dispatch

    /// Lay out a block and register it (nested included) so geometry queries
    /// resolve it. Returns the shared record.
    fn layout_block(
        &mut self,
        chunk: &'a ChunkRecord,
        x: f32,
        y: f32,
        width: f32,
    ) -> Rc<BlockLayout> {
        let block = self.layout_block_raw(chunk, x, y, width);
        let rc = Rc::new(block);
        self.records.insert(chunk.id, Rc::clone(&rc));
        rc
    }

    fn layout_block_raw(
        &mut self,
        chunk: &'a ChunkRecord,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let style = self.style_of(chunk.id);
        let box_y = y + style.margin_top;
        match chunk.kind {
            ChunkKind::Paragraph
            | ChunkKind::Heading1
            | ChunkKind::Heading2
            | ChunkKind::Heading3
            | ChunkKind::Heading4
            | ChunkKind::Heading5
            | ChunkKind::Heading6
            | ChunkKind::Caption => self.layout_text_block(chunk.id, style, x, box_y, width),
            ChunkKind::CodeBlock => self.layout_code_block(chunk.id, style, x, box_y, width),
            ChunkKind::Quote => self.layout_quote(chunk.id, style, x, box_y, width),
            ChunkKind::List => self.layout_list(chunk.id, style, x, box_y, width),
            ChunkKind::ListItem => {
                Rc::try_unwrap(self.layout_list_item(chunk.id, style, x, box_y, width, 0))
                    .unwrap_or_else(|rc| (*rc).clone())
            }
            ChunkKind::Image => self.layout_image(chunk.id, style, x, box_y, width),
            ChunkKind::Hr => {
                let ruler = Ruler {
                    x,
                    y: box_y + style.font_size_px * 0.5,
                    w: width,
                };
                BlockLayout {
                    chunk_id: chunk.id,
                    kind: chunk.kind,
                    style,
                    x,
                    y: box_y,
                    w: width,
                    h: style.font_size_px,
                    lines: Vec::new(),
                    children: Vec::new(),
                    marker: None,
                    image: None,
                    table: None,
                    ruler: Some(ruler),
                }
            }
            ChunkKind::Table => self.layout_table(chunk.id, style, x, box_y, width),
            ChunkKind::TableCell => self.layout_cell(chunk.id, style, x, box_y, width),
            // Structural chunks (document, table_row) produce no geometry.
            _ => BlockLayout {
                chunk_id: chunk.id,
                kind: chunk.kind,
                style,
                x,
                y: box_y,
                w: width,
                h: 0.0,
                lines: Vec::new(),
                children: Vec::new(),
                marker: None,
                image: None,
                table: None,
                ruler: None,
            },
        }
    }

    // ---------------------------------------------------------------------
    // Text blocks

    /// One measured, styled token (a slice of a run's text).
    fn laid_token(
        &self,
        run_chunk_id: u32,
        text: &str,
        style: ResolvedStyle,
        start_char: usize,
        end_char: usize,
    ) -> LaidToken {
        let atlas = self.atlas_for(style);
        let font_size = f64::from(style.font_size_px);
        let width = if let Some(atlas) = atlas {
            let measured = measure_run(atlas, text, style.font_size_px, style.letter_spacing);
            sum_advances(&measured.glyphs, 0, measured.glyphs.len())
        } else {
            font_size * 0.5 * text.chars().count() as f64
        };
        LaidToken {
            text: text.to_string(),
            run_chunk_id,
            style,
            atlas_id: style.font_id,
            font_size_px: style.font_size_px,
            start_char,
            end_char,
            width,
            break_after: BreakClass::Forbidden,
        }
    }

    /// Collect the styled token stream of a block's run children.
    fn collect_tokens(&self, chunk_id: u32) -> Vec<LaidToken> {
        let mut tokens: Vec<LaidToken> = Vec::new();
        for run_id in self.doc.child_ids(chunk_id) {
            if self.doc.chunk(run_id).is_none() {
                continue;
            }
            let style = self.style_of(run_id);
            let Some(text) = self.doc.direct_text(run_id) else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            let sub = tokenize_for_breaking(text);
            let mut char_index = 0usize;
            for token in &sub {
                let end = char_index + token.text.chars().count();
                let mut laid = self.laid_token(run_id, &token.text, style, char_index, end);
                laid.break_after = token.break_after;
                tokens.push(laid);
                char_index = end;
            }
        }
        tokens
    }

    fn layout_text_block(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let tokens = self.collect_tokens(chunk_id);
        if tokens.is_empty() {
            return self.empty_block(chunk_id, style, x, y, width);
        }
        let line_height = f64::from(style.font_size_px) * f64::from(style.line_height);
        let mut items: Vec<KpItem> = Vec::with_capacity(tokens.len() * 2);
        let em = f64::from(style.font_size_px);
        for token in &tokens {
            items.push(KpItem::Box { width: token.width });
            match token.break_after {
                BreakClass::Space => items.push(KpItem::Glue {
                    width: em * 0.25,
                    stretch: em * 0.125,
                    shrink: em * 0.0833,
                }),
                BreakClass::Allowed => items.push(KpItem::Glue {
                    width: 0.0,
                    stretch: em * 0.25,
                    shrink: 0.0,
                }),
                BreakClass::Forbidden => {
                    items.push(KpItem::Penalty {
                        width: 0.0,
                        penalty: f64::INFINITY,
                    });
                }
                BreakClass::Forced => {
                    items.push(KpItem::Penalty {
                        width: 0.0,
                        penalty: f64::NEG_INFINITY,
                    });
                }
            }
        }
        items.push(KpItem::Penalty {
            width: 0.0,
            penalty: f64::NEG_INFINITY,
        });

        let breaks = line_break(&items, f64::from(width), 100.0, 10.0);
        let mut lines: Vec<LineLayout> = Vec::with_capacity(breaks.len());
        for (i, br) in breaks.iter().enumerate() {
            let is_first = i == 0;
            let line_x = x + if is_first { style.text_indent } else { 0.0 };
            let line_width = if is_first {
                (width - style.text_indent).max(0.0)
            } else {
                width
            };
            let line_top = (f64::from(y) + i as f64 * line_height) as f32;
            let line = self.build_styled_line(chunk_id, &tokens, br, line_x, line_width, line_top);
            lines.push(line);
        }
        let height = (breaks.len() as f64 * line_height.ceil()) as f32;
        self.block_with_lines(chunk_id, style, x, y, width, height, lines)
    }

    fn build_styled_line(
        &self,
        chunk_id: u32,
        tokens: &[LaidToken],
        br: &KpLine,
        line_x: f32,
        line_width: f32,
        line_top: f32,
    ) -> LineLayout {
        // The line's baseline comes from the block's own style (documented v1
        // simplification: mixed-size runs share the line baseline).
        let block_style = self.style_of(chunk_id);
        let baseline = line_top + self.baseline_offset(block_style);
        let mut runs: Vec<RunLayout> = Vec::new();
        let mut glyphs: Vec<GlyphInstance> = Vec::new();
        // The line's tokens: a KP item is one box (at even indices) plus one
        // break item (glue/penalty, at odd indices) per token, so token `i`
        // occupies items `2i` and `2i+1`. A line spanning items `[start..end]`
        // therefore owns tokens `[start/2 .. (end-1)/2]`.
        //
        // This index mapping is a deliberate correctness fix over the JS
        // runtime, which compares token positions directly against the KP's
        // item indices (DESIGN.md R2): for multi-word lines that over-fills
        // early lines and starves later ones (empty lines). The break
        // geometry (line count, y, heights, ratios) is identical.
        let token_start = br.start / 2;
        let token_end = (br.end - 1) / 2;
        // Phase G (line-start ink guard): the first glyph's ink must not start
        // left of the line origin — a negative left side bearing would paint
        // ink outside the viewport at a line start. Shift the line start right
        // by the overhang plus one document pixel of anti-aliasing margin.
        // Deterministic: computed from the first atlas-bearing token's first
        // glyph (bearing, scale, and the MSDF AA edge all enter through
        // bearing_x · font_size_px). Mirrors the JS runtime exactly.
        let mut line_shift = 0.0_f64;
        for idx in token_start..=token_end {
            let Some(token) = tokens.get(idx) else { break };
            if let Some(atlas) = self.atlas_for(token.style) {
                let probe = measure_run(
                    atlas,
                    &token.text,
                    token.font_size_px,
                    token.style.letter_spacing,
                );
                if let Some(first) = probe.glyphs.first() {
                    if !first.is_mark {
                        if let Some(glyph) = first.glyph {
                            line_shift = f64::from(crate::layout::measure::line_start_shift(
                                glyph.bearing_x,
                                token.font_size_px,
                            ));
                        }
                    }
                }
                break;
            }
        }
        let mut cursor_x = f64::from(line_x) + line_shift;
        let mut token_index = 0usize;
        for token in tokens {
            if token_index < token_start {
                token_index += 1;
                continue;
            }
            if token_index > token_end {
                break;
            }
            // The token's own style drives measurement and appearance.
            let atlas = self.atlas_for(token.style);
            if let Some(atlas) = atlas {
                let measured = measure_run(
                    atlas,
                    &token.text,
                    token.font_size_px,
                    token.style.letter_spacing,
                );
                runs.push(RunLayout {
                    chunk_id: token.run_chunk_id,
                    text: token.text.clone(),
                    start: token.start_char,
                    end: token.end_char,
                    x: cursor_x as f32,
                    y: baseline,
                    width: token.width as f32,
                    style: token.style,
                    atlas_id: token.atlas_id,
                });
                let mut local_mark_base: Option<usize> = None;
                for (gi, metric) in measured.glyphs.iter().enumerate() {
                    if metric.is_mark {
                        let x = match local_mark_base {
                            Some(idx) => glyphs.get(idx).map_or(cursor_x as f32, |g| g.x),
                            None => cursor_x as f32,
                        };
                        glyphs.push(GlyphInstance {
                            codepoint: metric.codepoint,
                            x,
                            y: baseline,
                            advance_px: 0.0,
                            atlas_id: token.atlas_id,
                            font_size_px: token.font_size_px,
                            color: token.style.color,
                            run_chunk_id: token.run_chunk_id,
                            offset_in_text: token.start_char + gi,
                            has_outline: metric.has_outline,
                            mark_of: local_mark_base,
                        });
                    } else {
                        local_mark_base = Some(glyphs.len());
                        glyphs.push(GlyphInstance {
                            codepoint: metric.codepoint,
                            x: cursor_x as f32,
                            y: baseline,
                            advance_px: metric.advance_px as f32,
                            atlas_id: token.atlas_id,
                            font_size_px: token.font_size_px,
                            color: token.style.color,
                            run_chunk_id: token.run_chunk_id,
                            offset_in_text: token.start_char + gi,
                            has_outline: metric.has_outline,
                            mark_of: None,
                        });
                        cursor_x += metric.advance_px;
                    }
                }
            } else {
                runs.push(RunLayout {
                    chunk_id: token.run_chunk_id,
                    text: token.text.clone(),
                    start: token.start_char,
                    end: token.end_char,
                    x: cursor_x as f32,
                    y: baseline,
                    width: token.width as f32,
                    style: token.style,
                    atlas_id: token.atlas_id,
                });
                cursor_x += token.width;
            }
            token_index += 1;
        }
        let block_font_size = f64::from(block_style.font_size_px);
        LineLayout {
            y: line_top,
            baseline,
            width: line_width,
            height_px: (block_font_size * f64::from(block_style.line_height)) as f32,
            ratio: br.ratio,
            runs,
            glyphs,
        }
    }

    fn layout_code_block(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let atlas = match self.atlas_for(style) {
            Some(atlas) => atlas,
            None => return self.empty_block(chunk_id, style, x, y, width),
        };
        let Some(text) = self.doc.direct_text(chunk_id) else {
            return self.empty_block(chunk_id, style, x, y, width);
        };
        if text.is_empty() {
            return self.empty_block(chunk_id, style, x, y, width);
        }
        let font_size = f64::from(style.font_size_px);
        let line_height = font_size * f64::from(style.line_height);
        let baseline = f64::from(self.baseline_offset(style));
        let measured = measure_run(atlas, text, style.font_size_px, style.letter_spacing);
        // `split('\n')` keeps a trailing empty line — preformatted text is
        // preserved verbatim, including a final newline.
        let raw_lines: Vec<&str> = text.split('\n').collect();
        let mut lines: Vec<LineLayout> = Vec::with_capacity(raw_lines.len());
        let mut char_index = 0usize;
        for (i, raw) in raw_lines.iter().enumerate() {
            let line_top = (f64::from(y) + i as f64 * line_height) as f32;
            let mut runs: Vec<RunLayout> = Vec::new();
            let mut glyphs: Vec<GlyphInstance> = Vec::new();
            let mut cursor_x = f64::from(x);
            let raw_chars = raw.chars().count();
            for gi in char_index..char_index + raw_chars {
                if let Some(metric) = measured.glyphs.get(gi) {
                    glyphs.push(GlyphInstance {
                        codepoint: metric.codepoint,
                        x: cursor_x as f32,
                        y: (f64::from(line_top) + baseline) as f32,
                        advance_px: metric.advance_px as f32,
                        atlas_id: style.font_id,
                        font_size_px: style.font_size_px,
                        color: style.color,
                        run_chunk_id: chunk_id,
                        offset_in_text: gi,
                        has_outline: metric.has_outline,
                        mark_of: None,
                    });
                    cursor_x += metric.advance_px;
                }
            }
            if raw_chars > 0 {
                runs.push(RunLayout {
                    chunk_id,
                    text: (*raw).to_string(),
                    start: char_index,
                    end: char_index + raw_chars,
                    x,
                    y: (f64::from(line_top) + baseline) as f32,
                    width: (cursor_x - f64::from(x)) as f32,
                    style,
                    atlas_id: style.font_id,
                });
            }
            lines.push(LineLayout {
                y: line_top,
                baseline: (f64::from(line_top) + baseline) as f32,
                width,
                height_px: line_height as f32,
                ratio: 0.0,
                runs,
                glyphs,
            });
            char_index += raw_chars + 1;
        }
        let height = (raw_lines.len() as f64 * line_height.ceil()) as f32;
        self.block_with_lines(chunk_id, style, x, y, width, height, lines)
    }

    // ---------------------------------------------------------------------
    // Containers

    fn layout_quote(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let mut children: Vec<Rc<BlockLayout>> = Vec::new();
        let mut child_y = y;
        for child_id in self.doc.child_ids(chunk_id) {
            let Some(child) = self.doc.chunk(child_id) else {
                continue;
            };
            let child_layout = self.layout_block(
                child,
                x + QUOTE_INDENT,
                child_y,
                (width - QUOTE_INDENT).max(0.0),
            );
            children.push(Rc::clone(&child_layout));
            child_y = child_layout.y + child_layout.h;
        }
        let height = if children.is_empty() {
            0.0
        } else {
            child_y - y
        };
        self.block_with_children(chunk_id, style, x, y, width, height, children)
    }

    fn layout_list(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let mut value: i64 = 1;
        let mut children: Vec<Rc<BlockLayout>> = Vec::new();
        let mut child_y = y;
        for child_id in self.doc.child_ids(chunk_id) {
            let Some(child) = self.doc.chunk(child_id) else {
                continue;
            };
            // An explicit `list_item_value` extra overrides the running
            // ordinal (0 = auto; mirrors the JS).
            let mut item_value = value;
            for extra in self.doc.extras_for(child.id) {
                if let ExtraData::ListItemValue { value: v } = &extra.data {
                    if *v != 0 {
                        item_value = i64::from(*v);
                    }
                }
            }
            let child_layout = self.layout_list_item(
                child.id,
                self.style_of(child.id),
                x,
                child_y,
                width,
                item_value,
            );
            // `layout_list_item` does not register itself (the JS does); the
            // list registers each item so geometry queries resolve it.
            self.records.insert(child.id, Rc::clone(&child_layout));
            children.push(Rc::clone(&child_layout));
            child_y = child_layout.y + child_layout.h;
            value = item_value + 1;
        }
        let height = child_y - y;
        self.block_with_children(chunk_id, style, x, y, width, height, children)
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_list_item(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
        value: i64,
    ) -> Rc<BlockLayout> {
        let marker_text = list_marker_text(style.list_style, value);
        let marker_width = style.font_size_px * MARKER_GUTTER;
        let content_x = x + marker_width;
        let content_width = (width - marker_width).max(0.0);
        let mut children: Vec<Rc<BlockLayout>> = Vec::new();
        let mut child_y = y;
        for child_id in self.doc.child_ids(chunk_id) {
            let Some(child) = self.doc.chunk(child_id) else {
                continue;
            };
            let child_layout = self.layout_block(child, content_x, child_y, content_width);
            children.push(Rc::clone(&child_layout));
            child_y = child_layout.y + child_layout.h;
        }
        let height = if children.is_empty() {
            (f64::from(style.font_size_px) * f64::from(style.line_height)).ceil() as f32
        } else {
            child_y - y
        };
        // The marker's baseline aligns with the first line of the item
        // content.
        let marker_y = match children.first().and_then(|first| first.lines.first()) {
            Some(first_line) => first_line.baseline,
            None => y + self.baseline_offset(style),
        };
        let block = Rc::new(BlockLayout {
            chunk_id,
            kind: ChunkKind::ListItem,
            style,
            x,
            y,
            w: width,
            h: height,
            lines: Vec::new(),
            children,
            marker: Some(MarkerLayout {
                text: marker_text,
                x,
                y: marker_y,
                width: marker_width,
            }),
            image: None,
            table: None,
            ruler: None,
        });
        // List items are laid out through `layout_list` (which registers
        // them) or through `layout_block` (which registers the unwrapped
        // record) — never both, mirroring the JS registration inside
        // `layoutListItem`.
        block
    }

    fn layout_cell(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let mut children: Vec<Rc<BlockLayout>> = Vec::new();
        let mut child_y = y;
        for child_id in self.doc.child_ids(chunk_id) {
            let Some(child) = self.doc.chunk(child_id) else {
                continue;
            };
            let child_layout = self.layout_block(child, x, child_y, width);
            children.push(Rc::clone(&child_layout));
            child_y = child_layout.y + child_layout.h;
        }
        let height = if children.is_empty() {
            0.0
        } else {
            child_y - y
        };
        self.block_with_children(chunk_id, style, x, y, width, height, children)
    }

    // ---------------------------------------------------------------------
    // Images

    fn layout_image(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let Some(image_id) = self.doc.image_ref(chunk_id) else {
            return self.empty_block(chunk_id, style, x, y, width);
        };
        let Some(image) = self.doc.images().get(image_id as usize) else {
            return self.empty_block(chunk_id, style, x, y, width);
        };
        let natural_w = f64::from(image.width) / f64::from(self.dpr);
        let natural_h = f64::from(image.height) / f64::from(self.dpr);
        let w = f64::from(width).min(natural_w);
        let h = w * (natural_h / natural_w.max(1.0));
        BlockLayout {
            chunk_id,
            kind: ChunkKind::Image,
            style,
            x,
            y,
            w: w as f32,
            h: h as f32,
            lines: Vec::new(),
            children: Vec::new(),
            marker: None,
            image: Some(ImageQuad {
                image_id,
                x,
                y,
                w: w as f32,
                h: h as f32,
            }),
            table: None,
            ruler: None,
        }
    }

    // ---------------------------------------------------------------------
    // Tables (deterministic auto layout honoring colspan/rowspan)

    fn layout_table(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        let chunk = match self.doc.chunk(chunk_id) {
            Some(chunk) => chunk,
            None => return self.empty_block(chunk_id, style, x, y, width),
        };
        // Collect rows, cell ids, and spans (last `cell_span` extra wins,
        // mirroring the JS loop).
        let mut rows: Vec<TableRowSpec> = Vec::new();
        for row_id in self.doc.child_ids(chunk.id) {
            let mut cell_ids: Vec<u32> = Vec::new();
            let mut spans: Vec<(u16, u16)> = Vec::new();
            for cell_id in self.doc.child_ids(row_id) {
                cell_ids.push(cell_id);
                let mut colspan: u16 = 1;
                let mut rowspan: u16 = 1;
                for extra in self.doc.extras_for(cell_id) {
                    if let ExtraData::CellSpan {
                        colspan: cs,
                        rowspan: rs,
                    } = &extra.data
                    {
                        colspan = *cs;
                        rowspan = *rs;
                    }
                }
                spans.push((colspan, rowspan));
            }
            rows.push(TableRowSpec { cell_ids, spans });
        }

        // Column count and natural widths.
        let column_count = rows.iter().fold(1usize, |max, row| {
            let total: usize = row.spans.iter().map(|(cs, _)| usize::from(*cs)).sum();
            max.max(total)
        });
        let mut columns = vec![0.0_f64; column_count];
        for row in &rows {
            let mut col = 0usize;
            for (cell_id, (colspan, _)) in row.cell_ids.iter().zip(&row.spans) {
                let natural = self.cell_natural_width(*cell_id, width);
                let per = if *colspan > 0 {
                    natural / f64::from(*colspan)
                } else {
                    0.0
                };
                for k in 0..usize::from(*colspan) {
                    if let Some(slot) = columns.get_mut(col + k) {
                        *slot = slot.max(per);
                    }
                }
                col += usize::from(*colspan);
            }
        }
        let total_natural: f64 = columns.iter().sum();
        let scale = if total_natural > 0.0 {
            (f64::from(width) / total_natural).min(1.0)
        } else {
            1.0
        };
        let col_widths: Vec<f64> = columns.iter().map(|w| (w * scale).max(20.0)).collect();

        // Row heights (rowspan grows the last spanned row when needed).
        let mut row_heights: Vec<f64> = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut row_h = 0.0_f64;
            let mut col = 0usize;
            for (cell_id, (colspan, rowspan)) in row.cell_ids.iter().zip(&row.spans) {
                if *rowspan <= 1 {
                    let cell_w: f64 = col_widths
                        .iter()
                        .skip(col)
                        .take(usize::from(*colspan))
                        .sum();
                    row_h = row_h.max(f64::from(self.cell_height(*cell_id, cell_w as f32)));
                }
                col += usize::from(*colspan);
            }
            row_heights.push(row_h.max(f64::from(style.font_size_px) * 1.2));
        }
        for (r, row) in rows.iter().enumerate() {
            let mut col = 0usize;
            for (cell_id, (colspan, rowspan)) in row.cell_ids.iter().zip(&row.spans) {
                if *rowspan > 1 {
                    let cell_w: f64 = col_widths
                        .iter()
                        .skip(col)
                        .take(usize::from(*colspan))
                        .sum();
                    let h = f64::from(self.cell_height(*cell_id, cell_w as f32));
                    let span_rows = usize::from(*rowspan).min(rows.len() - r);
                    let current: f64 = row_heights.iter().skip(r).take(span_rows).sum();
                    if h > current {
                        // The last spanned row absorbs the overflow; the index
                        // is in range because `span_rows >= 1`.
                        if let Some(slot) = row_heights.get_mut(r + span_rows - 1) {
                            *slot += h - current;
                        }
                    }
                }
                col += usize::from(*colspan);
            }
        }

        // Lay out cells at their final positions.
        let mut col_x: Vec<f64> = Vec::with_capacity(col_widths.len());
        let mut acc = f64::from(x);
        for w in &col_widths {
            col_x.push(acc);
            acc += w;
        }
        let mut row_y = f64::from(y);
        let mut row_y_positions: Vec<f64> = Vec::with_capacity(row_heights.len());
        for h in &row_heights {
            row_y_positions.push(row_y);
            row_y += h;
        }
        let mut placements: Vec<Vec<CellPlacement>> = Vec::with_capacity(rows.len());
        for (r, row) in rows.iter().enumerate() {
            let mut col = 0usize;
            let mut row_placements: Vec<CellPlacement> = Vec::with_capacity(row.cell_ids.len());
            for (cell_id, (colspan, rowspan)) in row.cell_ids.iter().zip(&row.spans) {
                let cell_x = col_x.get(col).copied().unwrap_or(0.0);
                let cell_w: f64 = col_widths
                    .iter()
                    .skip(col)
                    .take(usize::from(*colspan))
                    .sum();
                let span_rows = usize::from(*rowspan).min(rows.len() - r);
                let cell_h: f64 = row_heights.iter().skip(r).take(span_rows).sum();
                let cell_y = row_y_positions.get(r).copied().unwrap_or(0.0);
                if let Some(cell) = self.doc.chunk(*cell_id) {
                    let layout =
                        self.layout_block(cell, cell_x as f32, cell_y as f32, cell_w as f32);
                    row_placements.push(CellPlacement {
                        cell: layout,
                        colspan: *colspan,
                        rowspan: *rowspan,
                        x: cell_x as f32,
                        y: cell_y as f32,
                        w: cell_w as f32,
                        h: cell_h as f32,
                    });
                }
                col += usize::from(*colspan);
            }
            placements.push(row_placements);
        }
        let table_rows = placements.clone();
        let children: Vec<Rc<BlockLayout>> = placements
            .iter()
            .flat_map(|row| row.iter())
            .map(|p| Rc::clone(&p.cell))
            .collect();
        let table = TableLayout {
            x,
            y,
            w: (acc - f64::from(x)) as f32,
            columns: col_widths.iter().map(|w| *w as f32).collect(),
            rows: table_rows,
        };
        let height = (row_y - f64::from(y)) as f32;
        BlockLayout {
            chunk_id,
            kind: ChunkKind::Table,
            style,
            x,
            y,
            w: table.w,
            h: height,
            lines: Vec::new(),
            children,
            marker: None,
            image: None,
            table: Some(table),
            ruler: None,
        }
    }

    fn cell_height(&mut self, cell_id: u32, width: f32) -> f32 {
        let Some(cell) = self.doc.chunk(cell_id) else {
            return 0.0;
        };
        let layout = self.layout_block(cell, 0.0, 0.0, width);
        layout.h
    }

    fn cell_natural_width(&self, cell_id: u32, max_width: f32) -> f64 {
        let style = self.style_of(cell_id);
        let atlas = self.atlas_for(style);
        let mut max = 0.0_f64;
        for child_id in self.doc.child_ids(cell_id) {
            if let Some(text) = self.doc.direct_text(child_id) {
                if let Some(atlas) = atlas {
                    if !text.is_empty() {
                        let measured =
                            measure_run(atlas, text, style.font_size_px, style.letter_spacing);
                        max = max.max(measured.width_px);
                    }
                }
            }
        }
        max.max(f64::from(style.font_size_px) * 2.0)
            .min(f64::from(max_width))
    }

    // ---------------------------------------------------------------------
    // Record constructors

    fn empty_block(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
    ) -> BlockLayout {
        self.block_with_lines(chunk_id, style, x, y, width, 0.0, Vec::new())
    }

    /// The record constructors mirror the JS private helpers and take the
    /// full geometry tuple; the argument count is intentional (scoped allow).
    #[allow(clippy::too_many_arguments)]
    fn block_with_lines(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        lines: Vec<LineLayout>,
    ) -> BlockLayout {
        // The chunk exists on every layout path; the fallback kind is
        // unreachable and exists only to keep this constructor panic-free.
        let kind = self
            .doc
            .chunk(chunk_id)
            .map_or(ChunkKind::Paragraph, |c| c.kind);
        // Register run rects for geometry queries (visibility, hit testing).
        for line in &lines {
            for run in &line.runs {
                self.run_rects.insert(
                    run.chunk_id,
                    Rect {
                        x: run.x,
                        y: line.y,
                        w: run.width,
                        h: line.height_px,
                    },
                );
            }
        }
        BlockLayout {
            chunk_id,
            kind,
            style,
            x,
            y,
            w: width,
            h: height,
            lines,
            children: Vec::new(),
            marker: None,
            image: None,
            table: None,
            ruler: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn block_with_children(
        &mut self,
        chunk_id: u32,
        style: ResolvedStyle,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        children: Vec<Rc<BlockLayout>>,
    ) -> BlockLayout {
        let kind = self
            .doc
            .chunk(chunk_id)
            .map_or(ChunkKind::Paragraph, |c| c.kind);
        BlockLayout {
            chunk_id,
            kind,
            style,
            x,
            y,
            w: width,
            h: height,
            lines: Vec::new(),
            children,
            marker: None,
            image: None,
            table: None,
            ruler: None,
        }
    }
}

impl<'a> GeometrySource for LayoutEngine<'a> {
    fn rect(&self, chunk_id: u32) -> Option<Rect> {
        LayoutEngine::rect(self, chunk_id)
    }
}

/// Marker text for a list style and value (mirrors the JS `listMarkerText`).
#[must_use]
pub fn list_marker_text(style: u8, value: i64) -> String {
    match style {
        list_style::NONE => String::new(),
        list_style::DISC => "\u{2022}".to_string(),
        list_style::CIRCLE => "\u{25e6}".to_string(),
        list_style::SQUARE => "\u{25aa}".to_string(),
        list_style::DECIMAL => format!("{value}."),
        list_style::LOWER_ALPHA => format!("{}.", to_alpha(value)),
        list_style::UPPER_ALPHA => format!("{}.", to_alpha(value).to_uppercase()),
        list_style::LOWER_ROMAN => format!("{}.", to_roman(value)),
        list_style::UPPER_ROMAN => format!("{}.", to_roman(value).to_uppercase()),
        _ => String::new(),
    }
}

/// The lowercase alphabetic counter (`1 → a`, `27 → aa`; mirrors the JS
/// `toAlpha` — `%`/`/` semantics match JavaScript's truncated operators).
fn to_alpha(value: i64) -> String {
    let mut v = value - 1;
    let mut out = String::new();
    loop {
        // 97 + (v % 26) is always a valid Latin-1 char for v >= -1 (values
        // below 1 only occur defensively and mirror the JS output).
        let ch = char::from_u32((97 + (v % 26)) as u32).unwrap_or('a');
        out.insert(0, ch);
        v = v / 26 - 1;
        if v < 0 {
            break;
        }
    }
    out
}

/// The lowercase roman-numeral counter (mirrors the JS `toRoman`).
fn to_roman(value: i64) -> String {
    if value <= 0 {
        return value.to_string();
    }
    const TABLE: [(i64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut v = value;
    let mut out = String::new();
    for (n, s) in TABLE {
        while v >= n {
            out.push_str(s);
            v -= n;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    //! Unit tests for the pure marker helpers (engine tests live in
    //! `tests/layout*.rs` and mirror the JS suite).
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

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
        assert_eq!(list_marker_text(list_style::LOWER_ROMAN, 49), "xlix.");
        assert_eq!(list_marker_text(list_style::UPPER_ROMAN, 49), "XLIX.");
        assert_eq!(list_marker_text(list_style::NONE, 1), "");
        assert_eq!(list_marker_text(255, 1), "");
    }

    #[test]
    fn alpha_counter_wraps_columns() {
        assert_eq!(to_alpha(1), "a");
        assert_eq!(to_alpha(26), "z");
        assert_eq!(to_alpha(27), "aa");
        assert_eq!(to_alpha(28), "ab");
        assert_eq!(to_alpha(703), "aaa");
    }

    #[test]
    fn roman_counter_uses_subtractive_forms() {
        assert_eq!(to_roman(4), "iv");
        assert_eq!(to_roman(9), "ix");
        assert_eq!(to_roman(49), "xlix");
        assert_eq!(to_roman(2026), "mmxxvi");
        assert_eq!(to_roman(0), "0");
        assert_eq!(to_roman(-3), "-3");
    }
}
