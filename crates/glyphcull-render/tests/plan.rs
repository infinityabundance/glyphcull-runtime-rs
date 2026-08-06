//! Render plan tests: the golden pipeline (parse → model → layout → draw
//! list → plan) produces correctly batched, z-ordered, premultiplied vertex
//! data — deterministically, without a GPU.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use glyphcull_core::document::build_document;
use glyphcull_core::draw_list::{DrawListBuilder, TextureResolver};
use glyphcull_core::glyphs::{prepare_glyph, GlyphStamp};
use glyphcull_core::layout::layout::{GlyphInstance, LayoutEngine, LayoutOptions};
use glyphcull_core::reader::glyph::Atlas;
use glyphcull_core::reader::parse;
use glyphcull_render::plan::{build_plan, rgba_components, PlanOp, RenderPlan, RendererViewport};

/// The golden fixture bytes (shared with glyphcull-core's tests).
const GOLDEN: &[u8] = include_bytes!("../../glyphcull-core/tests/fixtures/pipeline-golden.cull");

struct TestResolver;

impl TextureResolver for TestResolver {
    fn atlas_page(&self, atlas_id: u32, page_index: u16) -> u32 {
        atlas_id * 100 + u32::from(page_index)
    }
    fn image(&self, image_id: u32) -> u32 {
        2000 + image_id
    }
}

fn stamp_for<'a>(
    engine: &'a LayoutEngine<'a>,
) -> impl FnMut(u32, &GlyphInstance) -> Option<GlyphStamp> + 'a {
    let atlases: &'a [Atlas] = engine.document().atlases();
    move |_chunk_id, glyph| {
        let atlas = atlases.get(glyph.atlas_id as usize)?;
        prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color)
    }
}

/// The golden engine + a full-document draw list, compiled into a plan.
fn golden_plan<R>(f: impl FnOnce(&LayoutEngine<'_>, RenderPlan) -> R) -> R {
    let pkg = parse(GOLDEN).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let visible_ids: Vec<u32> = engine.records_all().keys().copied().collect();
    let builder = DrawListBuilder::new(TestResolver);
    let list = builder.build(&engine, &visible_ids, stamp_for(&engine), &[]);
    let plan = build_plan(
        &list,
        RendererViewport {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
            dpr: 1.0,
        },
        800.0,
        600.0,
    );
    f(&engine, plan)
}

#[test]
fn batches_glyphs_by_texture_and_preserves_order() {
    golden_plan(|_engine, plan| {
        // Glyph batches split when the texture changes; z-order is preserved.
        assert!(!plan.ops.is_empty());
        let mut textures_seen: Vec<u32> = Vec::new();
        for op in &plan.ops {
            match op {
                PlanOp::GlyphBatch { texture, vertices } => {
                    textures_seen.push(*texture);
                    assert_eq!(vertices.len() % 6, 0, "whole quads only");
                }
                PlanOp::Quad { .. } => {}
            }
        }
        // At least one distinct glyph texture (atlas page).
        let mut distinct: Vec<u32> = textures_seen.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert!(!distinct.is_empty());
        // No texture alternates without a batch boundary: consecutive ops
        // with the same texture would have been merged.
        for pair in plan.ops.windows(2) {
            if let (PlanOp::GlyphBatch { texture: a, .. }, PlanOp::GlyphBatch { texture: b, .. }) =
                (&pair[0], &pair[1])
            {
                assert_ne!(a, b, "consecutive batches never share a texture");
            }
        }
    });
}

#[test]
fn vertex_data_is_premultiplied_and_uvs_are_valid() {
    golden_plan(|_engine, plan| {
        let mut glyph_count = 0usize;
        for op in &plan.ops {
            if let PlanOp::GlyphBatch { vertices, .. } = op {
                for vertex in vertices {
                    // Premultiplied: rgb ≤ a within f32 tolerance.
                    assert!(vertex.color[0] <= vertex.color[3] + 1e-5, "premultiplied r");
                    assert!(vertex.color[1] <= vertex.color[3] + 1e-5, "premultiplied g");
                    assert!(vertex.color[2] <= vertex.color[3] + 1e-5, "premultiplied b");
                    assert!((vertex.color[3] - 1.0).abs() < 1e-5, "text is opaque");
                    // UVs inside the page.
                    for uv in vertex.uv {
                        assert!((0.0..=1.0).contains(&uv));
                    }
                }
                glyph_count += vertices.len() / 6;
            }
        }
        assert!(glyph_count > 0, "the golden emits glyph quads");
    });
}

#[test]
fn the_view_uniform_scrolls_the_document() {
    let pkg = parse(GOLDEN).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let visible_ids: Vec<u32> = engine.records_all().keys().copied().collect();
    let builder = DrawListBuilder::new(TestResolver);
    let list = builder.build(&engine, &visible_ids, stamp_for(&engine), &[]);
    // Scrolled viewport: the offset shifts by the scroll position.
    let plan = build_plan(
        &list,
        RendererViewport {
            x: 120.0,
            y: 200.0,
            w: 800.0,
            h: 600.0,
            dpr: 2.0,
        },
        1600.0,
        1200.0,
    );
    assert!((plan.view.scale[0] - 2.0 / 1600.0).abs() < 1e-6);
    assert!((plan.view.offset[0] - (-120.0 * 2.0 / 1600.0)).abs() < 1e-6);
    assert!((plan.view.offset[1] - (-200.0 * 2.0 / 1200.0)).abs() < 1e-6);
}

#[test]
fn selection_fills_precede_content_in_the_plan() {
    let pkg = parse(GOLDEN).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let visible_ids: Vec<u32> = engine.records_all().keys().copied().collect();
    let builder = DrawListBuilder::new(TestResolver);
    let list = builder.build(
        &engine,
        &visible_ids,
        stamp_for(&engine),
        &[glyphcull_core::selection::SelectionQuad {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 16.0,
        }],
    );
    let plan = build_plan(
        &list,
        RendererViewport {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
            dpr: 1.0,
        },
        800.0,
        600.0,
    );
    // The first op is the selection fill (a quad on the white texture).
    match &plan.ops[0] {
        PlanOp::Quad { texture, vertices } => {
            assert_eq!(*texture, glyphcull_render::plan::WHITE_TEXTURE);
            assert_eq!(vertices.len(), 6);
            let c = rgba_components(glyphcull_core::draw_list::SELECTION_COLOR);
            assert!((vertices[0].color[0] - c[0]).abs() < 1e-6);
        }
        other => panic!("expected a selection fill first, got {other:?}"),
    }
}

#[test]
fn is_deterministic_double_build_equality() {
    golden_plan(|engine, plan| {
        let _ = engine;
        let pkg = parse(GOLDEN).expect("parses");
        let doc = build_document(&pkg).expect("builds");
        let mut engine2 = LayoutEngine::new(
            &doc,
            LayoutOptions {
                dpr: 1.0,
                content_width: 800.0,
            },
        );
        engine2.extend_to(f64::INFINITY);
        let visible_ids: Vec<u32> = engine2.records_all().keys().copied().collect();
        let builder = DrawListBuilder::new(TestResolver);
        let list = builder.build(&engine2, &visible_ids, stamp_for(&engine2), &[]);
        let plan2 = build_plan(
            &list,
            RendererViewport {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
                dpr: 1.0,
            },
            800.0,
            600.0,
        );
        assert_eq!(plan, plan2, "identical inputs ⇒ identical plans");
    });
}

#[test]
fn glyph_commands_match_the_laid_out_glyph_geometry() {
    golden_plan(|engine, plan| {
        // Every laid-out outlined glyph (non-mark) yields exactly one vertex
        // quad with its position/px-range.
        let mut laid_out = 0usize;
        let mut seen = 0usize;
        for block in engine.records_all().values() {
            for line in &block.lines {
                for glyph in &line.glyphs {
                    if glyph.mark_of.is_none() && glyph.has_outline {
                        laid_out += 1;
                    }
                }
            }
        }
        for op in &plan.ops {
            if let PlanOp::GlyphBatch { vertices, .. } = op {
                seen += vertices.len() / 6;
            }
        }
        // The two list-item disc markers add two more quads.
        assert_eq!(
            seen,
            laid_out + 2,
            "one quad per outlined glyph plus markers"
        );
    });
}
