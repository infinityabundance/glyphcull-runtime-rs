//! The render plan (mirrors the JS `WebGlRenderer.draw` batching in
//! `src/render/gl.ts`): a draw list is compiled into an ordered sequence of
//! GPU ops — glyph batches (consecutive commands sharing a texture, one draw
//! call each) and single quads (fills, rulers, images) — plus the per-frame
//! view uniform. The plan is pure data: the wgpu executor (`renderer.rs`)
//! plays it back, and tests can assert batching, z-order, vertex data, and
//! the premultiplied color math without a GPU.
//!
//! Sampling convention (DESIGN.md D28, corrected): wgpu samples at
//! `uv·size − 0.5` like GL/Vulkan, so the plan carries the stamp UVs as-is
//! and the renderer shifts glyph UVs by half a texel at flatten time
//! (`renderer::flatten_plan`), where the texture size is known — the same
//! compensation the JS WebGL renderer applies in its vertex shader (the JS
//! shifts by +0.5/size; the CPU reference and the Canvas 2D fallback sample
//! at pixel centers directly). Image quads are not shifted (the reference
//! image path uses the raw −0.5 convention).

use glyphcull_core::draw_list::{DrawCommand, DrawList};

/// The handle of the 1×1 white texture used for fills and rulers.
pub const WHITE_TEXTURE: u32 = 1;

/// The per-frame view transform (document space → NDC).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewUniform {
    /// `(dpr / canvas_w, dpr / canvas_h)`.
    pub scale: [f32; 2],
    /// `(-viewport.x · scaleX, -viewport.y · scaleY)`.
    pub offset: [f32; 2],
}

/// One vertex: pos/uv/premultiplied-color/pxRange (9 f32, matching the JS
/// stride and attribute offsets).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    /// Document-space position.
    pub pos: [f32; 2],
    /// Texture coordinates.
    pub uv: [f32; 2],
    /// Premultiplied RGBA.
    pub color: [f32; 4],
    /// The shader's px-range input (document px per texel × dpr).
    pub px_range: f32,
}

impl Vertex {
    /// The flat 9-float form the GPU vertex buffer expects.
    #[must_use]
    pub const fn as_floats(&self) -> [f32; 9] {
        [
            self.pos[0],
            self.pos[1],
            self.uv[0],
            self.uv[1],
            self.color[0],
            self.color[1],
            self.color[2],
            self.color[3],
            self.px_range,
        ]
    }
}

/// One GPU draw op, in z-order.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanOp {
    /// Consecutive glyph commands sharing a texture (one draw call).
    GlyphBatch {
        /// The texture handle.
        texture: u32,
        /// 6 vertices per glyph quad, concatenated.
        vertices: Vec<Vertex>,
    },
    /// A single quad (fill, ruler, or image).
    Quad {
        /// The texture handle (`WHITE_TEXTURE` for fills/rulers).
        texture: u32,
        /// Exactly 6 vertices.
        vertices: Vec<Vertex>,
    },
}

/// An ordered render plan for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderPlan {
    /// The draw ops in z-order (selection first, then blocks).
    pub ops: Vec<PlanOp>,
    /// The per-frame view uniform.
    pub view: ViewUniform,
}

/// The device viewport a frame is drawn into (mirrors the JS
/// `RendererViewport`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RendererViewport {
    /// The document-space viewport origin (scroll position).
    pub x: f32,
    /// The document-space viewport origin (scroll position).
    pub y: f32,
    /// The viewport width.
    pub w: f32,
    /// The viewport height.
    pub h: f32,
    /// The device pixel ratio.
    pub dpr: f32,
}

/// Convert an RGBA u32 to premultiplied float components (mirrors the JS
/// `rgbaComponents`).
#[must_use]
pub fn rgba_components(color: u32) -> [f32; 4] {
    let r = ((color >> 24) & 0xff) as f32 / 255.0;
    let g = ((color >> 16) & 0xff) as f32 / 255.0;
    let b = ((color >> 8) & 0xff) as f32 / 255.0;
    let a = (color & 0xff) as f32 / 255.0;
    // Premultiply (matches the renderer's compositing).
    [r * a, g * a, b * a, a]
}

/// Compile a draw list into an ordered render plan. `canvas_w`/`canvas_h`
/// are the drawing surface's CSS-pixel dimensions.
#[must_use]
pub fn build_plan(
    list: &DrawList,
    viewport: RendererViewport,
    canvas_w: f32,
    canvas_h: f32,
) -> RenderPlan {
    let scale_x = viewport.dpr / canvas_w;
    let scale_y = viewport.dpr / canvas_h;
    let view = ViewUniform {
        scale: [scale_x, scale_y],
        offset: [-viewport.x * scale_x, -viewport.y * scale_y],
    };
    let mut ops: Vec<PlanOp> = Vec::new();
    let mut batch: Vec<Vertex> = Vec::new();
    let mut batch_texture: Option<u32> = None;
    let flush = |ops: &mut Vec<PlanOp>, batch: &mut Vec<Vertex>, texture: Option<u32>| {
        if let Some(texture) = texture {
            if !batch.is_empty() {
                ops.push(PlanOp::GlyphBatch {
                    texture,
                    vertices: std::mem::take(batch),
                });
            }
        }
    };
    for command in &list.commands {
        match command {
            DrawCommand::Glyph(g) => {
                if batch_texture.is_some() && batch_texture != Some(g.texture) {
                    flush(&mut ops, &mut batch, batch_texture);
                }
                batch_texture = Some(g.texture);
                let color = rgba_components(g.color);
                let px_range = g.px_per_texel * viewport.dpr;
                push_quad(&mut batch, g.x, g.y, g.w, g.h, g.uv, color, px_range);
            }
            DrawCommand::Image(i) => {
                flush(&mut ops, &mut batch, batch_texture);
                batch_texture = None;
                let white = [1.0, 1.0, 1.0, 1.0];
                ops.push(PlanOp::Quad {
                    texture: i.texture,
                    vertices: quad_vertices(i.x, i.y, i.w, i.h, [0.0, 0.0, 1.0, 1.0], white, 1.0),
                });
            }
            DrawCommand::Fill(f) => {
                flush(&mut ops, &mut batch, batch_texture);
                batch_texture = None;
                let color = rgba_components(f.color);
                ops.push(PlanOp::Quad {
                    texture: WHITE_TEXTURE,
                    vertices: quad_vertices(f.x, f.y, f.w, f.h, [0.0, 0.0, 1.0, 1.0], color, 1.0),
                });
            }
            DrawCommand::Ruler(r) => {
                flush(&mut ops, &mut batch, batch_texture);
                batch_texture = None;
                let color = rgba_components(r.color);
                // Rulers draw as 1-device-px strips (mirrors the JS).
                ops.push(PlanOp::Quad {
                    texture: WHITE_TEXTURE,
                    vertices: quad_vertices(r.x, r.y, r.w, 1.0, [0.0, 0.0, 1.0, 1.0], color, 1.0),
                });
            }
        }
    }
    flush(&mut ops, &mut batch, batch_texture);
    RenderPlan { ops, view }
}

/// Push one textured quad (6 vertices) into `vertices`.
///
/// The argument count mirrors the JS `pushQuad` (scoped allow).
#[allow(clippy::too_many_arguments)]
fn push_quad(
    vertices: &mut Vec<Vertex>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv: [f32; 4],
    color: [f32; 4],
    px_range: f32,
) {
    vertices.extend(quad_vertices(x, y, w, h, uv, color, px_range));
}

/// The 6 vertices of one textured quad (mirrors the JS `pushQuad` order).
fn quad_vertices(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    uv: [f32; 4],
    color: [f32; 4],
    px_range: f32,
) -> Vec<Vertex> {
    let [u0, v0, u1, v1] = uv;
    let v = |px: f32, py: f32, u: f32, vv: f32| Vertex {
        pos: [px, py],
        uv: [u, vv],
        color,
        px_range,
    };
    vec![
        v(x, y, u0, v0),
        v(x + w, y, u1, v0),
        v(x, y + h, u0, v1),
        v(x, y + h, u0, v1),
        v(x + w, y, u1, v0),
        v(x + w, y + h, u1, v1),
    ]
}

#[cfg(test)]
mod tests {
    //! Unit tests for the pure plan builder (golden-based mirror vectors live
    //! in `tests/plan.rs`).
    #![allow(
        clippy::expect_used,
        clippy::unwrap_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;
    use glyphcull_core::draw_list::{
        DrawCommand, DrawList, FillCommand, GlyphCommand, ImageCommand, RulerCommand,
    };

    #[test]
    fn view_uniform_maps_document_space_to_ndc() {
        let plan = build_plan(
            &DrawList {
                commands: Vec::new(),
            },
            RendererViewport {
                x: 40.0,
                y: 60.0,
                w: 400.0,
                h: 300.0,
                dpr: 2.0,
            },
            800.0,
            600.0,
        );
        // scale = dpr / canvas; offset = -viewport * scale (f32, so compare
        // with tolerance).
        assert!((plan.view.scale[0] - 2.0 / 800.0).abs() < 1e-6);
        assert!((plan.view.scale[1] - 2.0 / 600.0).abs() < 1e-6);
        assert!((plan.view.offset[0] - (-40.0 * 2.0 / 800.0)).abs() < 1e-6);
        assert!((plan.view.offset[1] - (-60.0 * 2.0 / 600.0)).abs() < 1e-6);
        assert!(plan.ops.is_empty());
    }

    #[test]
    fn rgba_components_premultiplies() {
        // 0x80ff0040: premultiplied by the exact f32 8-bit fractions.
        let c = rgba_components(0x80ff_0040);
        let expect = |v: u32| v as f32 / 255.0;
        assert!((c[0] - (expect(0x80) * expect(0x40))).abs() < 1e-6);
        assert!((c[1] - (expect(0xff) * expect(0x40))).abs() < 1e-6);
        assert_eq!(c[2], 0.0);
        assert!((c[3] - expect(0x40)).abs() < 1e-6);
    }

    #[test]
    fn glyph_batches_split_on_texture_change() {
        let uv = [0.0, 0.0, 1.0, 1.0];
        let glyph = |texture: u32| {
            DrawCommand::Glyph(GlyphCommand {
                texture,
                uv,
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                color: 0x0000_00ff,
                px_per_texel: 1.0,
            })
        };
        let list = DrawList {
            commands: vec![glyph(2), glyph(2), glyph(3), glyph(2)],
        };
        let plan = build_plan(
            &list,
            RendererViewport {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
                dpr: 1.0,
            },
            100.0,
            100.0,
        );
        // Three batches: (2,2), (3), (2) — consecutive same-texture runs.
        assert_eq!(plan.ops.len(), 3);
        match &plan.ops[0] {
            PlanOp::GlyphBatch { texture, vertices } => {
                assert_eq!(*texture, 2);
                assert_eq!(vertices.len(), 12, "two quads → 12 vertices");
            }
            other => panic!("unexpected op {other:?}"),
        }
        match &plan.ops[1] {
            PlanOp::GlyphBatch { texture, vertices } => {
                assert_eq!(*texture, 3);
                assert_eq!(vertices.len(), 6);
            }
            other => panic!("unexpected op {other:?}"),
        }
        match &plan.ops[2] {
            PlanOp::GlyphBatch { texture, vertices } => {
                assert_eq!(*texture, 2);
                assert_eq!(vertices.len(), 6);
            }
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn non_glyph_commands_flush_the_batch_and_emit_single_quads() {
        let glyph = DrawCommand::Glyph(GlyphCommand {
            texture: 2,
            uv: [0.0, 0.0, 1.0, 1.0],
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            color: 0x0000_00ff,
            px_per_texel: 1.0,
        });
        let fill = DrawCommand::Fill(FillCommand {
            x: 0.0,
            y: 0.0,
            w: 50.0,
            h: 20.0,
            color: 0x80ff_0040,
        });
        let ruler = DrawCommand::Ruler(RulerCommand {
            x: 0.0,
            y: 10.0,
            w: 100.0,
            color: 0x0000_00ff,
        });
        let image = DrawCommand::Image(ImageCommand {
            texture: 2000,
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 20.0,
        });
        let list = DrawList {
            commands: vec![glyph, fill, ruler, image, glyph],
        };
        let plan = build_plan(
            &list,
            RendererViewport {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
                dpr: 1.0,
            },
            100.0,
            100.0,
        );
        assert_eq!(plan.ops.len(), 5);
        // fill uses the white texture; ruler draws a 1-px strip; image uses
        // its own texture with a full white tint.
        match &plan.ops[1] {
            PlanOp::Quad { texture, vertices } => {
                assert_eq!(*texture, WHITE_TEXTURE);
                assert_eq!(vertices.len(), 6);
                assert!((vertices[0].pos[1] - 0.0).abs() < 1e-6);
            }
            other => panic!("unexpected op {other:?}"),
        }
        match &plan.ops[2] {
            PlanOp::Quad { texture, vertices } => {
                assert_eq!(*texture, WHITE_TEXTURE);
                // The ruler is at y=10 and its height is clamped to 1 px:
                // the bottom edge is 11.
                assert!((vertices[2].pos[1] - 11.0).abs() < 1e-6);
            }
            other => panic!("unexpected op {other:?}"),
        }
        match &plan.ops[3] {
            PlanOp::Quad { texture, vertices } => {
                assert_eq!(*texture, 2000);
                assert_eq!(vertices[0].color, [1.0, 1.0, 1.0, 1.0]);
            }
            other => panic!("unexpected op {other:?}"),
        }
        // The trailing glyph starts a fresh batch.
        match &plan.ops[4] {
            PlanOp::GlyphBatch { texture, vertices } => {
                assert_eq!(*texture, 2);
                assert_eq!(vertices.len(), 6);
            }
            other => panic!("unexpected op {other:?}"),
        }
    }

    #[test]
    fn glyph_px_range_scales_with_dpr() {
        let glyph = DrawCommand::Glyph(GlyphCommand {
            texture: 2,
            uv: [0.0, 0.0, 1.0, 1.0],
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            color: 0x0000_00ff,
            px_per_texel: 2.0,
        });
        let list = DrawList {
            commands: vec![glyph],
        };
        let plan = build_plan(
            &list,
            RendererViewport {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
                dpr: 2.0,
            },
            100.0,
            100.0,
        );
        match &plan.ops[0] {
            PlanOp::GlyphBatch { vertices, .. } => {
                assert!((vertices[0].px_range - 4.0).abs() < 1e-6, "2 · 2 dpr");
            }
            other => panic!("unexpected op {other:?}"),
        }
    }
}
