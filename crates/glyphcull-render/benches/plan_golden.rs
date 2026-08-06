//! Render plan benchmark (criterion).
//!
//! Measures the headless hot path: full golden pipeline (parse → model →
//! layout → draw list → render plan) and the pure MSDF reference
//! reconstruction. Baselines are recorded in `PERFORMANCE.md` §6.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use criterion::{criterion_group, criterion_main, Criterion};

use glyphcull_core::document::build_document;
use glyphcull_core::draw_list::{DrawListBuilder, TextureResolver};
use glyphcull_core::glyphs::prepare_glyph;
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::parse;
use glyphcull_render::msdf::reconstruct_glyph;
use glyphcull_render::plan::{build_plan, RendererViewport};

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

fn bench_plan(c: &mut Criterion) {
    let mut group = c.benchmark_group("render-plan");

    group.bench_function("golden-full-pipeline", |b| {
        b.iter(|| {
            let pkg = parse(GOLDEN).expect("parse");
            let doc = build_document(&pkg).expect("build");
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
            let atlases = doc.atlases();
            let list = builder.build(
                &engine,
                &visible_ids,
                |_chunk_id, glyph| {
                    let atlas = atlases.get(glyph.atlas_id as usize)?;
                    prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color)
                },
                &[],
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
            std::hint::black_box(plan.ops.len());
        });
    });

    // The CPU MSDF reference: reconstruct a golden glyph at 1:1.
    let pkg = parse(GOLDEN).expect("parse");
    let atlas = &pkg.atlases().expect("atlases").expect("present")[0];
    let glyph = atlas
        .glyphs
        .binary_search_by(|g| g.codepoint.cmp(&u32::from('D')))
        .ok()
        .and_then(|i| atlas.glyphs.get(i))
        .expect("D glyph");
    let page = &atlas.pages[0];
    group.bench_function("msdf-reconstruct-glyph-1x1", |b| {
        b.iter(|| {
            let out = reconstruct_glyph(
                page,
                atlas.page_width as usize,
                glyph.box_x as usize,
                glyph.box_y as usize,
                glyph.box_w as usize,
                glyph.box_h as usize,
                0.5,
                4,
            );
            std::hint::black_box(out.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_plan);
criterion_main!(benches);
