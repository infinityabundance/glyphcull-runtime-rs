//! Document-build benchmark (criterion).
//!
//! Measures `build_document` — graph validation, style resolution, and the
//! trusted model view — against the golden fixture and a synthetic 100k-chunk
//! document. Baselines are recorded in `PERFORMANCE.md` §6.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

#[path = "../tests/common/mod.rs"]
mod common;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use glyphcull_core::document::build_document;
use glyphcull_core::reader::parse;

fn golden() -> &'static [u8] {
    common::pipeline_golden()
}

/// A synthetic 100k-chunk wide document (root with 99,999 paragraphs).
fn wide_document() -> Vec<u8> {
    let count = 100_000_u32;
    let mut chunks = Vec::with_capacity(count as usize + 1);
    chunks.push(common::TestChunk {
        id: 1,
        kind: 1,
        flags: 0x10,
        first_child_id: 2,
        last_child_id: count + 1,
        ..Default::default()
    });
    for i in 0..count {
        let id = i + 2;
        chunks.push(common::TestChunk {
            id,
            kind: 8,
            parent_id: 1,
            prev_id: if i == 0 { 0 } else { id - 1 },
            next_id: if i + 1 == count { 0 } else { id + 1 },
            ordinal: id - 1,
            depth: 1,
            ..Default::default()
        });
    }
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload_counts(count + 1, 1, 0, 0, 0),
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
    ])
}

fn bench_document_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("document/build");

    group.bench_function("golden-22-chunks", |b| {
        let bytes = golden();
        b.iter(|| {
            let pkg = parse(bytes).expect("parse");
            let doc = build_document(&pkg).expect("build");
            // Touch the trusted views the runtime uses.
            let ids = doc.all_ids();
            let _ = ids.len();
            let _ = doc.plain_text(1);
        });
    });

    group.bench_function("wide-100k-chunks", |b| {
        b.iter_batched(
            wide_document,
            |bytes| {
                let pkg = parse(&bytes).expect("parse");
                let doc = build_document(&pkg).expect("build");
                let ids = doc.all_ids();
                assert_eq!(ids.len(), 100_001);
            },
            BatchSize::LargeInput,
        )
    });

    group.finish();
}

criterion_group!(benches, bench_document_build);
criterion_main!(benches);
