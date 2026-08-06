//! Layout benchmark (criterion).
//!
//! Measures the full-document layout pipeline (parse → build → `extend_to`)
//! against the golden fixture, plus a 2000-word paragraph to baseline the
//! Knuth–Plass dynamic program on realistic prose. Baselines are recorded in
//! `PERFORMANCE.md` §6.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

#[path = "../tests/common/mod.rs"]
mod common;

use criterion::{criterion_group, criterion_main, Criterion};

use glyphcull_core::document::build_document;
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::parse;

/// A package with a single paragraph whose run carries `text`.
fn paragraph_package(text: &str) -> Vec<u8> {
    let chunks = vec![
        common::TestChunk {
            id: 1,
            kind: 1,
            flags: 0x10,
            first_child_id: 2,
            last_child_id: 2,
            ..Default::default()
        },
        common::TestChunk {
            id: 2,
            kind: 8,
            parent_id: 1,
            first_child_id: 3,
            last_child_id: 3,
            ordinal: 1,
            depth: 1,
            ..Default::default()
        },
        common::TestChunk {
            id: 3,
            kind: 18,
            parent_id: 2,
            content_index: 1,
            ordinal: 2,
            depth: 2,
            ..Default::default()
        },
    ];
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(3, 1, 1, 0, 0),
        },
        common::TestSection {
            kind: 2,
            compression: 1,
            payload: common::chnk_payload(&chunks, &[]),
        },
        common::TestSection {
            kind: 3,
            compression: 1,
            payload: common::styl_payload(&[(0, vec![])]),
        },
        common::TestSection {
            kind: 4,
            compression: 1,
            payload: common::cont_payload(&[text], &[]),
        },
    ])
}

fn bench_layout(c: &mut Criterion) {
    let mut group = c.benchmark_group("layout");

    group.bench_function("golden-full-document", |b| {
        b.iter(|| {
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
            std::hint::black_box(engine.records_all().len());
        });
    });

    // A 2000-word paragraph baselines the Knuth–Plass dynamic program on
    // realistic prose (the active list is deduplicated per fitness class).
    let text: String = (0..2000)
        .map(|i| format!("word{}", i % 97))
        .collect::<Vec<_>>()
        .join(" ");
    let bytes = paragraph_package(&text);
    group.bench_function("paragraph-2000-words", |b| {
        b.iter(|| {
            let pkg = parse(&bytes).expect("parse");
            let doc = build_document(&pkg).expect("build");
            let mut engine = LayoutEngine::new(
                &doc,
                LayoutOptions {
                    dpr: 1.0,
                    content_width: 400.0,
                },
            );
            engine.extend_to(f64::INFINITY);
            std::hint::black_box(engine.records_all().len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_layout);
criterion_main!(benches);
