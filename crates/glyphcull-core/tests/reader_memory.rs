//! Memory regression test: peak allocation while parsing must stay within
//! the committed budget (PERFORMANCE.md §2: steady-state memory overhead
//! < 6 × package size for load-time work; this gate is deliberately looser —
//! 8 × the golden's file size — because parse peak includes allocator arena
//! growth, a conservative overestimate).
//!
//! Measurement: the process RSS high-water mark (`VmHWM` from
//! `/proc/self/status`, Linux) before and after the loop; the delta is the
//! parse peak footprint. Deterministic and noise-free for the same binary +
//! input, mirroring the compiler's memory gate.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

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
fn parse_peak_memory_within_budget() {
    let golden = common::pipeline_golden();
    // Warm up the allocator and page the fixture in before the baseline.
    parse(golden).expect("warm parse");

    let baseline = vmhwm_bytes();
    for _ in 0..25 {
        let pkg = parse(golden).expect("parse");
        // Force the typed decoders so every section's owned buffers are
        // allocated inside the measured window.
        pkg.info().expect("info");
        pkg.chunks().expect("chunks");
        pkg.styles().expect("styles");
        pkg.content().expect("content");
        pkg.atlases().expect("atlases");
        pkg.verify_seal().expect("seal");
    }
    let peak = vmhwm_bytes() - baseline;
    let budget = golden.len() * PEAK_MULTIPLIER;
    eprintln!(
        "reader parse peak: {peak} bytes over 25 iterations of a {} byte package (budget {budget})",
        golden.len()
    );
    assert!(
        peak <= budget,
        "parse peak {peak} bytes exceeds budget {budget} ({PEAK_MULTIPLIER} × package)",
    );
}
