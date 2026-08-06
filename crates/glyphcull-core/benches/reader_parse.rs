//! Parse-throughput benchmark for the `.cull` reader (criterion).
//!
//! Measures the container+section decode path (`parse`) and the full
//! contract path (typed decoders + SEAL) against the committed golden
//! fixtures and a synthetic ~1 MiB decoded package. Baselines are recorded
//! in `PERFORMANCE.md` §6 with the environment; this bench is also the CI
//! smoke gate (`cargo bench --all -- --test` runs each case once).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

#[path = "../tests/common/mod.rs"]
mod common;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use glyphcull_core::reader::parse;

/// The golden package bytes (855 KiB, embedded at compile time).
fn golden() -> &'static [u8] {
    common::pipeline_golden()
}

/// The INFO-only minimal package.
fn minimal() -> &'static [u8] {
    common::v1_minimal()
}

/// A synthetic package with ~1 MiB of decoded text payload.
fn synthetic_mib() -> Vec<u8> {
    let text: Vec<u8> = (0..(1 << 20))
        .map(|i| b'a' + ((i / 7) % 26) as u8)
        .collect();
    common::build_package(&[
        common::TestSection {
            kind: 1,
            compression: 1,
            payload: common::info_payload(),
        },
        common::TestSection {
            kind: 101,
            compression: 1,
            payload: text,
        },
    ])
}

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("reader/parse");

    group.bench_function("v1-minimal", |b| {
        let bytes = minimal();
        b.iter(|| parse(bytes).expect("parse"));
    });

    group.bench_function("pipeline-golden-855k", |b| {
        let bytes = golden();
        b.iter(|| parse(bytes).expect("parse"));
    });

    group.bench_function("synthetic-1m-decoded", |b| {
        b.iter_batched(
            synthetic_mib,
            |bytes| parse(&bytes).expect("parse"),
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_full_contract(c: &mut Criterion) {
    let mut group = c.benchmark_group("reader/full-contract");

    group.bench_function("golden-info-chunks-styles-content-atlases-seal", |b| {
        let bytes = golden();
        b.iter(|| {
            let pkg = parse(bytes).expect("parse");
            pkg.info().expect("info");
            pkg.chunks().expect("chunks");
            pkg.styles().expect("styles");
            pkg.content().expect("content");
            pkg.atlases().expect("atlases");
            pkg.verify_seal().expect("seal");
        });
    });

    group.finish();
}

criterion_group!(benches, bench_parse, bench_full_contract);
criterion_main!(benches);
