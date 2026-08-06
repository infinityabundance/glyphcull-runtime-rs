//! Memory regression test: peak allocation while building the document model
//! must stay within the committed budget (PERFORMANCE.md §2). The model
//! borrows the package's decoded payloads, so building copies only the
//! resolved style table — the gate is accordingly tight.
//!
//! Measurement: the process RSS high-water mark (`VmHWM` from
//! `/proc/self/status`, Linux) before and after the loop; the delta is the
//! build peak footprint (including allocator arena growth — a conservative
//! overestimate). Mirrors the reader's memory gate.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use glyphcull_core::document::build_document;
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
fn document_build_peak_memory_within_budget() {
    let golden = common::pipeline_golden();
    // Warm up the allocator and page the fixture in before the baseline.
    let warm = parse(golden).expect("warm parse");
    let _ = build_document(&warm).expect("warm build");

    let baseline = vmhwm_bytes();
    for _ in 0..25 {
        let pkg = parse(golden).expect("parse");
        let doc = build_document(&pkg).expect("build");
        // Touch the derived views so everything allocated in a build is
        // inside the measured window.
        let ids = doc.all_ids();
        assert!(!ids.is_empty());
        let _ = doc.plain_text(1);
    }
    let peak = vmhwm_bytes() - baseline;
    let budget = golden.len() * PEAK_MULTIPLIER;
    eprintln!(
        "document build peak: {peak} bytes over 25 builds of a {} byte package (budget {budget})",
        golden.len()
    );
    assert!(
        peak <= budget,
        "document build peak {peak} bytes exceeds budget {budget} ({PEAK_MULTIPLIER} × package)",
    );
}
