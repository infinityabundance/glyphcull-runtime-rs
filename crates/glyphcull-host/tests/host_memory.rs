//! Memory regression test: peak allocation across full host lifecycles —
//! load → scroll → paint → select → copy → destroy — must stay within the
//! committed budget (PERFORMANCE.md §2). Each cycle drops the whole document
//! (package → model → layout → glyph cache → handles), so a leak anywhere in
//! the host shows up as unbounded peak growth across cycles.
//!
//! Measurement: the process RSS high-water mark (`VmHWM` from
//! `/proc/self/status`, Linux) before and after the loop; the delta is the
//! peak footprint (including allocator arena growth — a conservative
//! overestimate). Same method as the reader/document/layout gates; the
//! counting-allocator variant is not available because the workspace forbids
//! `unsafe` (a `GlobalAlloc` impl), so this is the repo's established RSS
//! gate pattern.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use glyphcull_core::materialize::Direction;
use glyphcull_core::reader::parse;
use glyphcull_core::selection::TextPosition;
use glyphcull_core::visibility::Viewport;
use glyphcull_host::{FrameSink, HostDocument, HostOptions};
use glyphcull_render::plan::{RenderPlan, RendererViewport};

const GOLDEN: &[u8] = include_bytes!("../../glyphcull-core/tests/fixtures/pipeline-golden.cull");

/// The committed peak-memory budget for one host lifecycle over the pinned
/// golden fixture. Evidence (PERFORMANCE.md §6 4.12): the measured peak over
/// 25 cycles is ≈ 0.3 MiB; 8 MiB leaves ≈ 25× headroom while still catching
/// any per-cycle leak (a 0.33 MiB/cycle leak would hit the budget).
const PEAK_BUDGET_BYTES: usize = 8 * 1024 * 1024;

/// A sink that does nothing (no GPU): the host's CPU footprint only.
#[derive(Default)]
struct NoopSink;

impl FrameSink for NoopSink {
    fn upload_atlas_page(
        &mut self,
        _atlas_id: u32,
        _page_index: u16,
        _pixels: &[u8],
        _width: u32,
        _height: u32,
    ) -> u32 {
        1
    }
    fn upload_image(
        &mut self,
        _image_id: u32,
        _pixels: &[u8],
        _width: u32,
        _height: u32,
        _rgb: bool,
    ) -> u32 {
        1
    }
    fn draw(
        &mut self,
        _plan: &RenderPlan,
        _viewport: RendererViewport,
        _surface_w: f32,
        _surface_h: f32,
    ) -> Result<(), String> {
        Ok(())
    }
    fn resize(&mut self, _width: u32, _height: u32) {}
    fn destroy(&mut self) {}
}

fn options() -> HostOptions {
    HostOptions {
        dpr: 1.0,
        content_width: 800.0,
        viewport: Viewport {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        },
        margin: 120.0,
        glyph_budget_bytes: 8 * 1024 * 1024,
        frame_budget_ms: 8,
        cooling_period_ms: 1500,
    }
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

/// One full host lifecycle over the golden package.
fn host_cycle() {
    let pkg = parse(GOLDEN).expect("parse");
    let mut host = HostDocument::load(pkg, Box::new(NoopSink), options()).expect("load");
    // Scroll through the document (materializes everything), paint, select,
    // copy, then release.
    for y in [0.0_f32, 200.0, 400.0] {
        host.scroll(
            Viewport {
                x: 0.0,
                y,
                w: 800.0,
                h: 600.0,
            },
            Direction::Down,
        )
        .expect("scroll");
    }
    host.paint().expect("paint");
    host.select_range(
        TextPosition {
            chunk_id: 2,
            offset: 0,
        },
        TextPosition {
            chunk_id: 2,
            offset: 6,
        },
    )
    .expect("select");
    let _ = host.copy().expect("copy");
    host.destroy();
}

#[test]
fn host_cycles_stay_within_the_committed_memory_budget() {
    // Warm up the allocator and page the fixture in before the baseline.
    host_cycle();

    let baseline = vmhwm_bytes();
    for _ in 0..25 {
        host_cycle();
    }
    let peak = vmhwm_bytes() - baseline;
    eprintln!("host lifecycle peak: {peak} bytes over 25 cycles (budget {PEAK_BUDGET_BYTES})");
    assert!(
        peak <= PEAK_BUDGET_BYTES,
        "host lifecycle peak {peak} bytes exceeds budget {PEAK_BUDGET_BYTES}"
    );
}
