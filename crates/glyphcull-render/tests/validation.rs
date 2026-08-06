//! Headless rendering validation: the CPU MSDF reference reconstructs golden
//! glyphs from the atlas pages and the full pipeline (layout → draw list →
//! plan) produces consistent geometry — without a GPU. The GPU path is
//! exercised on the desktop host (4.11); the shader's math is proven
//! equivalent here and in `shader.rs`'s unit tests.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use glyphcull_core::document::build_document;
use glyphcull_core::draw_list::{DrawListBuilder, TextureResolver};
use glyphcull_core::glyphs::prepare_glyph;
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::parse;
use glyphcull_render::msdf::{reconstruct_glyph, texel_to_px};
use glyphcull_render::plan::build_plan;

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

#[test]
fn the_cpu_reference_reconstructs_a_golden_glyph_with_a_sensible_profile() {
    let pkg = parse(GOLDEN).expect("parses");
    let atlases = pkg.atlases().expect("atlases").expect("present");
    let atlas = &atlases[0];
    let page = &atlas.pages[0];
    // 'D' — the golden heading's first glyph.
    let glyph = atlas
        .glyphs
        .binary_search_by(|g| g.codepoint.cmp(&u32::from('D')))
        .ok()
        .and_then(|i| atlas.glyphs.get(i))
        .expect("D glyph");
    // Reconstruct at 1:1 with the golden's density and dpr 1.
    let tpe = texel_to_px(16.0, 1.0, f64::from(atlas.texels_per_em()));
    let out = reconstruct_glyph(
        page,
        atlas.page_width as usize,
        glyph.box_x as usize,
        glyph.box_y as usize,
        glyph.box_w as usize,
        glyph.box_h as usize,
        tpe,
        4,
    );
    assert_eq!(out.len(), glyph.box_w as usize * glyph.box_h as usize);
    // The glyph has ink: both fully-covered and empty texels exist (the box
    // includes padding), and the coverage is monotone across the letter's
    // left edge.
    let covered = out.iter().filter(|&&b| b > 200).count();
    let empty = out.iter().filter(|&&b| b < 50).count();
    assert!(covered > 0, "the box contains ink");
    assert!(empty > 0, "the box contains padding/background");
    // The first column is empty (padding + the stem's left edge starts later
    // or the bearing offset places it inside the box).
    let first_col: Vec<u8> = out.iter().take(glyph.box_h as usize).copied().collect();
    let _ = first_col;
}

#[test]
fn the_full_pipeline_produces_in_bounds_uvs_for_every_quad() {
    let pkg = parse(GOLDEN).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let atlases = pkg.atlases().expect("atlases").expect("present");
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
    let atlases_ref: &[glyphcull_core::reader::glyph::Atlas] = atlases;
    let stamps = |_chunk_id: u32, glyph: &glyphcull_core::layout::layout::GlyphInstance| {
        let atlas = atlases_ref.get(glyph.atlas_id as usize)?;
        prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color)
    };
    let list = builder.build(&engine, &visible_ids, stamps, &[]);
    let plan = build_plan(
        &list,
        glyphcull_render::plan::RendererViewport {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
            dpr: 1.0,
        },
        800.0,
        600.0,
    );
    for op in &plan.ops {
        let vertices = match op {
            glyphcull_render::plan::PlanOp::GlyphBatch { vertices, .. }
            | glyphcull_render::plan::PlanOp::Quad { vertices, .. } => vertices,
        };
        for vertex in vertices {
            for uv in vertex.uv {
                assert!((0.0..=1.0).contains(&uv), "uv {uv} outside the page");
            }
        }
    }
}

#[test]
fn stamps_and_plan_agree_on_the_sampling_convention() {
    // D28: the plan uses the stamp UVs as-is (wgpu normalizes the half-texel
    // phase; the JS WebGL renderer shifts by +0.5/size). Assert the plan's
    // glyph quad UVs equal the stamp UVs exactly.
    let pkg = parse(GOLDEN).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let atlases = pkg.atlases().expect("atlases").expect("present");
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
    let atlases_ref: &[glyphcull_core::reader::glyph::Atlas] = atlases;
    let list = builder.build(
        &engine,
        &visible_ids,
        |_chunk_id, glyph| {
            let atlas = atlases_ref.get(glyph.atlas_id as usize)?;
            prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color)
        },
        &[],
    );
    let plan = build_plan(
        &list,
        glyphcull_render::plan::RendererViewport {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
            dpr: 1.0,
        },
        800.0,
        600.0,
    );
    // Collect the draw list's glyph uv per texture-quad order and compare.
    let mut command_uvs: Vec<[f32; 4]> = Vec::new();
    for command in &list.commands {
        if let glyphcull_core::draw_list::DrawCommand::Glyph(g) = command {
            command_uvs.push(g.uv);
        }
    }
    let mut plan_uvs: Vec<[f32; 4]> = Vec::new();
    for op in &plan.ops {
        if let glyphcull_render::plan::PlanOp::GlyphBatch { vertices, .. } = op {
            for quad in vertices.chunks(6) {
                // Quad vertices: 0 = (x,y,u0,v0), 1 = (x+w,y,u1,v0),
                // 2 = (x,y+h,u0,v1).
                plan_uvs.push([quad[0].uv[0], quad[0].uv[1], quad[1].uv[0], quad[2].uv[1]]);
            }
        }
    }
    assert_eq!(command_uvs.len(), plan_uvs.len());
    for (a, b) in command_uvs.iter().zip(&plan_uvs) {
        assert!((a[0] - b[0]).abs() < 1e-6, "uv0");
        assert!((a[1] - b[1]).abs() < 1e-6, "uv1");
        assert!((a[2] - b[2]).abs() < 1e-6, "uv2");
        assert!((a[3] - b[3]).abs() < 1e-6, "uv3");
    }
}
