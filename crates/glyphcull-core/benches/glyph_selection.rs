//! Glyph cache + selection benchmark (criterion).
//!
//! Measures the cache's put/get throughput against the golden atlas and the
//! selection hot paths (hit testing, range→quad projection, and plain-text
//! copy) over the fully laid-out golden document. Baselines are recorded in
//! `PERFORMANCE.md` §6.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

#[path = "../tests/common/mod.rs"]
mod common;

use criterion::{criterion_group, criterion_main, Criterion};

use glyphcull_core::document::build_document;
use glyphcull_core::glyphs::{prepare_glyph, GlyphCache};
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::parse;
use glyphcull_core::selection::{
    copy_text, hit_test_point, range_quads, Point, Selection, TextPosition,
};

fn bench_glyph_selection(c: &mut Criterion) {
    // The parsed package, its model, and the fully laid-out engine all live
    // for the whole bench group (the engine borrows the model, which borrows
    // the package).
    let pkg = parse(common::pipeline_golden()).expect("parse");
    let atlas = pkg.atlases().expect("atlases").expect("present")[0].clone();
    let doc = build_document(&pkg).expect("build");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    engine.extend_to(f64::INFINITY);
    let stamp = prepare_glyph(&atlas, 'D' as u32, 16.0, 0x0000_00ff).expect("stamp");

    let mut group = c.benchmark_group("glyphs");

    group.bench_function("cache-put-64k-unlimited", |b| {
        b.iter(|| {
            let mut cache = GlyphCache::new(u64::MAX);
            for i in 0..64_000_u32 {
                let mut key = stamp.key;
                key.codepoint = i % 4096;
                cache.put(key, stamp, i % 1000);
            }
            std::hint::black_box(cache.size());
        });
    });

    group.bench_function("cache-get-64k-unlimited", |b| {
        let mut cache = GlyphCache::new(u64::MAX);
        for i in 0..64_000_u32 {
            let mut key = stamp.key;
            key.codepoint = i % 4096;
            cache.put(key, stamp, i % 1000);
        }
        let mut key = stamp.key;
        b.iter(|| {
            for i in 0..64_000_u32 {
                key.codepoint = i % 4096;
                let _ = cache.get(key);
            }
            std::hint::black_box(cache.size());
        });
    });

    // Selection over the fully laid-out golden document.
    // The whole-document selection (heading start → quote end).
    let selection = Selection {
        start: TextPosition {
            chunk_id: 3,
            offset: 0,
        },
        end: TextPosition {
            chunk_id: 22,
            offset: 5,
        },
    };

    group.bench_function("hit-test-point", |b| {
        b.iter(|| {
            let _ = hit_test_point(&engine, Point { x: 100.0, y: 100.0 });
        });
    });

    group.bench_function("range-quads-full-doc", |b| {
        b.iter(|| {
            std::hint::black_box(range_quads(&engine, selection));
        });
    });

    group.bench_function("copy-text-full-doc", |b| {
        b.iter(|| {
            std::hint::black_box(copy_text(engine.document(), selection));
        });
    });

    group.finish();
}

criterion_group!(benches, bench_glyph_selection);
criterion_main!(benches);
