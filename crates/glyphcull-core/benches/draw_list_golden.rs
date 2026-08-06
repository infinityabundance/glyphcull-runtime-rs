//! Draw list benchmark (criterion).
//!
//! Measures full-document draw list construction over the golden package —
//! the per-frame hot path (visible set + layout records + glyph stamps →
//! commands) — with and without selection quads. Baselines are recorded in
//! `PERFORMANCE.md` §6.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

#[path = "../tests/common/mod.rs"]
mod common;

use criterion::{criterion_group, criterion_main, Criterion};

use glyphcull_core::document::build_document;
use glyphcull_core::draw_list::{DrawListBuilder, TextureResolver};
use glyphcull_core::glyphs::{prepare_glyph, GlyphStamp};
use glyphcull_core::layout::layout::{GlyphInstance, LayoutEngine, LayoutOptions};
use glyphcull_core::reader::glyph::Atlas;
use glyphcull_core::reader::parse;

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

fn bench_draw_list(c: &mut Criterion) {
    let pkg = parse(common::pipeline_golden()).expect("parse");
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

    let mut group = c.benchmark_group("draw-list");

    group.bench_function("full-doc-no-selection", |b| {
        b.iter(|| {
            let list = builder.build(&engine, &visible_ids, stamp_for(&engine), &[]);
            std::hint::black_box(list.commands.len());
        });
    });

    group.bench_function("full-doc-with-selection", |b| {
        let selection = vec![
            glyphcull_core::selection::SelectionQuad {
                x: 10.0,
                y: 20.0,
                w: 100.0,
                h: 16.0,
            },
            glyphcull_core::selection::SelectionQuad {
                x: 10.0,
                y: 60.0,
                w: 40.0,
                h: 16.0,
            },
        ];
        b.iter(|| {
            let list = builder.build(&engine, &visible_ids, stamp_for(&engine), &selection);
            std::hint::black_box(list.commands.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_draw_list);
criterion_main!(benches);
