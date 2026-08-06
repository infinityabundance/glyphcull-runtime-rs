//! The draw list (Architecture.md §3.9; mirrors the JS `src/render/drawlist.ts`):
//! an ordered sequence of draw commands produced deterministically from the
//! visible set + layout records + glyph stamps. Glyph commands carry their
//! texture (atlas page), UV rect, quad, and color; the renderer batches
//! consecutive commands with the same texture into one draw call.
//!
//! Builders never read the wall clock and never iterate unordered maps in the
//! output path: the draw list is a pure function of (layout, visible set,
//! glyph stamps, selection).
//!
//! Z-order is the command order: selection fills first (beneath everything),
//! then per block background fill → line glyphs → list marker → children.
//! Ruler and image blocks emit a single command and return.
//!
//! Correctness divergence (DESIGN.md R5): the JS `emitBlockLayout` (the
//! children path) omits the ruler/image branches, so images and `hr`s nested
//! inside containers (quotes, lists, table cells) emit no quad. The Rust
//! builder shares one emission body between both paths, so nested images and
//! rules render. The visible-set `emitted` guard prevents double emission
//! either way.

use std::collections::BTreeSet;

use crate::glyphs::{prepare_glyph, GlyphStamp};
use crate::layout::layout::{BlockLayout, GlyphInstance, LayoutEngine, LineLayout};
use crate::layout::measure::measure_run;
use crate::reader::chunk::ChunkKind;
use crate::selection::SelectionQuad;

/// The selection highlight color the builder emits (fill under content;
/// mirrors the JS `SELECTION_COLOR`).
pub const SELECTION_COLOR: u32 = 0x3399_ff66;

/// A textured glyph quad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphCommand {
    /// The texture handle of the atlas page (assigned by the renderer).
    pub texture: u32,
    /// UV rect in texture space: `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
    /// Quad position and size in document pixels.
    pub x: f32,
    /// Quad position and size in document pixels.
    pub y: f32,
    /// Quad width.
    pub w: f32,
    /// Quad height.
    pub h: f32,
    /// The text color as u32 RGBA.
    pub color: u32,
    /// Document pixels per atlas texel at this glyph's size
    /// (fontSize/texelsPerEm) — the shader's px-range input.
    pub px_per_texel: f32,
}

/// A raster image quad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageCommand {
    /// The texture handle of the image (assigned by the renderer).
    pub texture: u32,
    /// Quad position and size in document pixels.
    pub x: f32,
    /// Quad position and size in document pixels.
    pub y: f32,
    /// Quad width.
    pub w: f32,
    /// Quad height.
    pub h: f32,
}

/// A solid fill (backgrounds, selection).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FillCommand {
    /// Fill rect in document pixels.
    pub x: f32,
    /// Fill rect in document pixels.
    pub y: f32,
    /// Fill rect width.
    pub w: f32,
    /// Fill rect height.
    pub h: f32,
    /// The fill color as u32 RGBA.
    pub color: u32,
}

/// A horizontal rule.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RulerCommand {
    /// The ruler's left edge.
    pub x: f32,
    /// The ruler's vertical center.
    pub y: f32,
    /// The ruler's width.
    pub w: f32,
    /// The ruler color as u32 RGBA.
    pub color: u32,
}

/// One draw command.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DrawCommand {
    /// A textured glyph quad.
    Glyph(GlyphCommand),
    /// A raster image quad.
    Image(ImageCommand),
    /// A solid fill.
    Fill(FillCommand),
    /// A horizontal rule.
    Ruler(RulerCommand),
}

/// An ordered sequence of draw commands.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawList {
    /// The commands in z-order.
    pub commands: Vec<DrawCommand>,
}

/// A texture resolver: glyph stamp page / image id → renderer texture handle.
pub trait TextureResolver {
    /// Resolve an atlas page to a renderer texture handle.
    fn atlas_page(&self, atlas_id: u32, page_index: u16) -> u32;
    /// Resolve an image id to a renderer texture handle.
    fn image(&self, image_id: u32) -> u32;
}

/// The draw list builder.
pub struct DrawListBuilder<T: TextureResolver> {
    texture: T,
}

impl<T: TextureResolver> DrawListBuilder<T> {
    /// Create a builder with the given texture resolver.
    #[must_use]
    pub fn new(texture: T) -> Self {
        Self { texture }
    }

    /// Build the draw list for the visible chunk ids. Glyphs are looked up in
    /// `stamps` (chunk id → per-glyph stamp); missing stamps are skipped
    /// (their chunk is not materialized yet — the scheduler is mid-flight).
    /// Markers, backgrounds, rulers, and images are block geometry and render
    /// regardless: only text-run glyphs are gated by the cache.
    pub fn build<F>(
        &self,
        layout: &LayoutEngine<'_>,
        visible_ids: &[u32],
        mut stamps: F,
        selection: &[SelectionQuad],
    ) -> DrawList
    where
        F: FnMut(u32, &GlyphInstance) -> Option<GlyphStamp>,
    {
        let mut commands: Vec<DrawCommand> = Vec::new();
        // Selection fills first, beneath all content (z-order).
        for quad in selection {
            commands.push(DrawCommand::Fill(FillCommand {
                x: quad.x,
                y: quad.y,
                w: quad.w,
                h: quad.h,
                color: SELECTION_COLOR,
            }));
        }
        // The visible set contains every visible chunk (nested included),
        // while a block's emission recurses into its children. Emit each block
        // once: when a parent is emitted its whole subtree is marked, so a
        // nested id appearing later in the list is skipped; a nested id whose
        // ancestors are absent still emits its own subtree.
        let mut emitted: BTreeSet<u32> = BTreeSet::new();
        for &chunk_id in visible_ids {
            if emitted.contains(&chunk_id) {
                continue;
            }
            self.emit_block(layout, chunk_id, &mut commands, &mut stamps, &mut emitted);
        }
        DrawList { commands }
    }

    /// Emit a block looked up by chunk id (the top-level path).
    fn emit_block<F>(
        &self,
        layout: &LayoutEngine<'_>,
        chunk_id: u32,
        commands: &mut Vec<DrawCommand>,
        stamps: &mut F,
        emitted: &mut BTreeSet<u32>,
    ) where
        F: FnMut(u32, &GlyphInstance) -> Option<GlyphStamp>,
    {
        emitted.insert(chunk_id);
        let Some(record) = layout.record(chunk_id) else {
            return;
        };
        self.emit_block_commands(layout, record, commands, stamps, emitted);
    }

    /// Emit one block's commands. Both the top-level and the children paths
    /// share this body (the JS duplicates it in `emitBlockLayout` and drops
    /// the ruler/image branches there — DESIGN.md R5).
    fn emit_block_commands<F>(
        &self,
        layout: &LayoutEngine<'_>,
        block: &BlockLayout,
        commands: &mut Vec<DrawCommand>,
        stamps: &mut F,
        emitted: &mut BTreeSet<u32>,
    ) where
        F: FnMut(u32, &GlyphInstance) -> Option<GlyphStamp>,
    {
        emitted.insert(block.chunk_id);
        if block.kind == ChunkKind::Hr {
            if let Some(ruler) = block.ruler {
                commands.push(DrawCommand::Ruler(RulerCommand {
                    x: ruler.x,
                    y: ruler.y,
                    w: ruler.w,
                    color: block.style.color,
                }));
            }
            return;
        }
        if block.kind == ChunkKind::Image {
            if let Some(image) = block.image {
                commands.push(DrawCommand::Image(ImageCommand {
                    texture: self.texture.image(image.image_id),
                    x: image.x,
                    y: image.y,
                    w: image.w,
                    h: image.h,
                }));
            }
            return;
        }
        // Backgrounds first (beneath the content).
        let bg = block.style.background_color;
        if bg != 0 && (block.w > 0.0 || block.h > 0.0) {
            commands.push(DrawCommand::Fill(FillCommand {
                x: block.x,
                y: block.y,
                w: block.w,
                h: block.h,
                color: bg,
            }));
        }
        for line in &block.lines {
            self.emit_line(line, commands, stamps);
        }
        self.emit_marker(layout, block, commands);
        for child in &block.children {
            self.emit_block_commands(layout, child, commands, stamps, emitted);
        }
    }

    /// Emit a list marker as glyphs at its baseline. Markers are block
    /// geometry (never gated by the cache), so no stamps callback is needed.
    fn emit_marker(
        &self,
        layout: &LayoutEngine<'_>,
        block: &BlockLayout,
        commands: &mut Vec<DrawCommand>,
    ) {
        let Some(marker) = &block.marker else {
            return;
        };
        if marker.text.is_empty() {
            return;
        }
        let Some(atlas) = layout
            .document()
            .atlases()
            .get(block.style.font_id as usize)
        else {
            return;
        };
        let font_size = block.style.font_size_px;
        let measured = measure_run(atlas, &marker.text, font_size, 0.0);
        let mut x = marker.x;
        for metric in &measured.glyphs {
            if let Some(stamp) =
                prepare_glyph(atlas, metric.codepoint, font_size, block.style.color)
            {
                if !stamp.no_outline {
                    commands.push(DrawCommand::Glyph(GlyphCommand {
                        texture: self
                            .texture
                            .atlas_page(stamp.key.atlas_id, stamp.page_index),
                        uv: stamp.uv,
                        x: x + stamp.offset_x,
                        y: marker.y - stamp.offset_y,
                        w: stamp.quad_w,
                        h: stamp.quad_h,
                        color: block.style.color,
                        px_per_texel: if stamp.texels_per_em > 0.0 {
                            font_size / stamp.texels_per_em
                        } else {
                            1.0
                        },
                    }));
                }
            }
            x += metric.advance_px as f32;
        }
    }

    /// Emit one line's outlined glyphs (marks ride with their base and never
    /// emit their own quad).
    fn emit_line<F>(&self, line: &LineLayout, commands: &mut Vec<DrawCommand>, stamps: &mut F)
    where
        F: FnMut(u32, &GlyphInstance) -> Option<GlyphStamp>,
    {
        for glyph in &line.glyphs {
            if glyph.mark_of.is_some() {
                continue; // marks ride with the base
            }
            let Some(stamp) = stamps(glyph.run_chunk_id, glyph) else {
                continue;
            };
            if stamp.no_outline || stamp.quad_w == 0.0 || stamp.quad_h == 0.0 {
                continue; // spaces and not-yet-cached stamps
            }
            commands.push(DrawCommand::Glyph(GlyphCommand {
                texture: self
                    .texture
                    .atlas_page(stamp.key.atlas_id, stamp.page_index),
                uv: stamp.uv,
                x: glyph.x + stamp.offset_x,
                y: glyph.y - stamp.offset_y,
                w: stamp.quad_w,
                h: stamp.quad_h,
                color: glyph.color,
                px_per_texel: if stamp.texels_per_em > 0.0 {
                    glyph.font_size_px / stamp.texels_per_em
                } else {
                    1.0
                },
            }));
        }
    }
}
