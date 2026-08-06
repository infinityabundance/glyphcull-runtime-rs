//! Memory regression test: peak allocation while culling a 100k-chunk
//! document must stay within the committed budget (PERFORMANCE.md §2).
//! Culling allocates per walked chunk (visible/not-yet-visible vectors), so
//! the gate uses the wide-document package.
//!
//! Measurement: the process RSS high-water mark (`VmHWM` from
//! `/proc/self/status`, Linux) before and after the loop; the delta is the
//! cull peak footprint. The gate lives in its own test binary so no
//! concurrently running test thread can inflate `VmHWM` mid-measurement (the
//! reason it was split out of `visibility_stress.rs`; the other memory gates
//! follow the same single-test-file convention).

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::collections::HashMap;

use glyphcull_core::document::build_document;
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;
use glyphcull_core::visibility::{compute_visible_set, GeometrySource, Rect, Viewport};

/// The committed peak-memory multiplier over the input package size.
const PEAK_MULTIPLIER: usize = 8;

struct MapGeometry(HashMap<u32, Rect>);

impl GeometrySource for MapGeometry {
    fn rect(&self, chunk_id: u32) -> Option<Rect> {
        self.0.get(&chunk_id).copied()
    }
}

/// A 100k-chunk document: root with 99,999 paragraph children.
fn wide_document_bytes() -> Vec<u8> {
    let count = 100_000_u32;
    let mut chunks = Vec::with_capacity(count as usize + 1);
    chunks.push(common::TestChunk {
        id: 1,
        kind: ChunkKind::Document as u8,
        flags: 0x10,
        first_child_id: 2,
        last_child_id: count + 1,
        ..Default::default()
    });
    for i in 0..count {
        let id = i + 2;
        chunks.push(common::TestChunk {
            id,
            kind: ChunkKind::Paragraph as u8,
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

/// Every paragraph at `y = 30 × index`, height 25 — a tall document.
fn tall_geometry(count: u32) -> HashMap<u32, Rect> {
    let mut rects = HashMap::with_capacity(count as usize);
    for i in 0..count {
        rects.insert(
            i + 2,
            Rect {
                x: 0.0,
                y: (i * 30) as f32,
                w: 400.0,
                h: 25.0,
            },
        );
    }
    rects
}

/// The process RSS high-water mark in bytes (Linux).
fn vmhwm_bytes() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").expect("read /proc/self/status");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: usize = rest
                .trim()
                .trim_end_matches(" kB")
                .trim()
                .parse()
                .expect("parse VmHWM");
            return kb * 1024;
        }
    }
    panic!("VmHWM not found in /proc/self/status");
}

#[test]
fn culling_peak_memory_within_budget() {
    // The committed budget is 8 × package size (PERFORMANCE.md §2); culling
    // allocations are proportional to the walked document, so the gate uses
    // the wide-document package.
    let bytes = wide_document_bytes();
    let pkg = parse(&bytes).expect("parses");
    let doc = build_document(&pkg).expect("builds");
    let geometry = MapGeometry(tall_geometry(100_000));

    // Warm up.
    let _ = compute_visible_set(
        &doc,
        &geometry,
        Viewport {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 400.0,
        },
        0.0,
    );

    let baseline = vmhwm_bytes();
    for i in 0..25 {
        let y = (i * 400) as f32;
        let viewport = Viewport {
            x: 0.0,
            y,
            w: 400.0,
            h: 400.0,
        };
        let result = compute_visible_set(&doc, &geometry, viewport, 50.0);
        assert!(!result.visible.is_empty());
    }
    let peak = vmhwm_bytes() - baseline;
    let budget = bytes.len() * PEAK_MULTIPLIER;
    eprintln!(
        "visibility cull peak: {peak} bytes over 25 culls of a {} byte package (budget {budget})",
        bytes.len()
    );
    assert!(
        peak <= budget,
        "cull peak {peak} exceeds budget {budget} ({PEAK_MULTIPLIER} × package)",
    );
}
