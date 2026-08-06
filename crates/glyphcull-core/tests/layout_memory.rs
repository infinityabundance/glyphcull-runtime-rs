//! Memory regression test: peak allocation while laying out the golden
//! document must stay within the committed budget (PERFORMANCE.md §2). The
//! layout engine borrows the parsed package and the model — it copies only
//! the layout records (blocks, lines, glyphs), which are small relative to
//! the package's atlas pages.
//!
//! Measurement: the process RSS high-water mark (`VmHWM` from
//! `/proc/self/status`, Linux) before and after the loop; the delta is the
//! layout peak footprint (including allocator arena growth — a conservative
//! overestimate). Mirrors the reader's and document's memory gates.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use glyphcull_core::document::build_document;
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::reader::parse;

/// The committed peak-memory multiplier over the input package size.
const PEAK_MULTIPLIER: usize = 8;

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
fn layout_peak_memory_within_budget() {
    let golden = common::pipeline_golden();
    // Warm up the allocator and page the fixture in before the baseline.
    let warm = parse(golden).expect("warm parse");
    let warm_doc = build_document(&warm).expect("warm build");
    let mut warm_engine = LayoutEngine::new(
        &warm_doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    warm_engine.extend_to(f64::INFINITY);

    let baseline = vmhwm_bytes();
    for _ in 0..25 {
        let pkg = parse(golden).expect("parse");
        let doc = build_document(&pkg).expect("build");
        let mut engine = LayoutEngine::new(
            &doc,
            LayoutOptions {
                dpr: 1.0,
                content_width: 800.0,
            },
        );
        engine.extend_to(f64::INFINITY);
        // Touch the laid-out records so everything a layout allocates is
        // inside the measured window.
        let count = engine.records_all().len();
        assert!(count > 0);
        let _ = engine.frontier_y();
    }
    let peak = vmhwm_bytes() - baseline;
    let budget = golden.len() * PEAK_MULTIPLIER;
    eprintln!(
        "layout peak: {peak} bytes over 25 layouts of a {} byte package (budget {budget})",
        golden.len()
    );
    assert!(
        peak <= budget,
        "layout peak {peak} bytes exceeds budget {budget} ({PEAK_MULTIPLIER} × package)",
    );
}
