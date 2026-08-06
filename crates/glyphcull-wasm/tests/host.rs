//! Native host tests: the six-operation contract against a recording
//! [`FrameSink`] — no wasm-bindgen, no GPU (mirrors the JS `runtime.test.ts`
//! public-surface suite: load, scroll, paint, select, copy, destroy).
//!
//! The wgpu sink itself (`WgpuSink`) is exercised on the web/desktop hosts;
//! here the host logic — package validation, lifecycle registration, texture
//! upload scheduling, materialization, selection pinning, destruction — is
//! proven against a sink that records every call.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::cell::RefCell;
use std::rc::Rc;

use glyphcull_core::document::build_document;
use glyphcull_core::layout::layout::{LayoutEngine, LayoutOptions};
use glyphcull_core::materialize::Direction;
use glyphcull_core::reader::chunk::ChunkKind;
use glyphcull_core::reader::parse;
use glyphcull_core::selection::TextPosition;
use glyphcull_core::visibility::Viewport;
use glyphcull_render::plan::{PlanOp, RenderPlan, RendererViewport};
use glyphcull_wasm::{FrameSink, HostDocument, HostError, HostOptions};

const GOLDEN: &[u8] = include_bytes!("../../glyphcull-core/tests/fixtures/pipeline-golden.cull");
const V1_MINIMAL: &[u8] = include_bytes!("../../glyphcull-core/tests/fixtures/v1-minimal.cull");

/// The recorded call log (shared so tests can inspect it after the host owns
/// the sink).
#[derive(Debug, Default)]
struct SinkLog {
    atlas_uploads: u32,
    image_uploads: u32,
    draws: u32,
    glyph_batches: u32,
    resizes: Vec<(u32, u32)>,
    destroyed: bool,
}

/// A sink that records every call the host makes (handles start at 1; 0 is
/// reserved for "missing", mirroring the real resolver's convention).
#[derive(Clone, Default)]
struct RecordingSink {
    log: Rc<RefCell<SinkLog>>,
}

impl FrameSink for RecordingSink {
    fn upload_atlas_page(
        &mut self,
        _atlas_id: u32,
        _page_index: u16,
        _pixels: &[u8],
        _width: u32,
        _height: u32,
    ) -> u32 {
        let mut log = self.log.borrow_mut();
        log.atlas_uploads += 1;
        log.atlas_uploads
    }

    fn upload_image(
        &mut self,
        _image_id: u32,
        _pixels: &[u8],
        _width: u32,
        _height: u32,
        _rgb: bool,
    ) -> u32 {
        let mut log = self.log.borrow_mut();
        log.image_uploads += 1;
        1000 + log.image_uploads
    }

    fn draw(
        &mut self,
        plan: &RenderPlan,
        _viewport: RendererViewport,
        _surface_w: f32,
        _surface_h: f32,
    ) -> Result<(), String> {
        let mut log = self.log.borrow_mut();
        log.draws += 1;
        if plan
            .ops
            .iter()
            .any(|op| matches!(op, PlanOp::GlyphBatch { .. }))
        {
            log.glyph_batches += 1;
        }
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.log.borrow_mut().resizes.push((width, height));
    }

    fn destroy(&mut self) {
        self.log.borrow_mut().destroyed = true;
    }
}

/// The default host options (valid; mirrors the JS defaults).
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

/// A golden host over a recording sink (the sink stays inspectable).
fn host_with_sink() -> (HostDocument, RecordingSink) {
    let sink = RecordingSink::default();
    let pkg = parse(GOLDEN).expect("golden parses");
    let host = HostDocument::load(pkg, Box::new(sink.clone()), options()).expect("host loads");
    (host, sink)
}

/// The golden heading block's id, its run's id, and the run's text.
fn heading_chunks() -> (u32, u32, String) {
    let pkg = parse(GOLDEN).expect("golden parses");
    let doc = build_document(&pkg).expect("golden model");
    let heading = doc
        .all_ids()
        .into_iter()
        .find(|&id| doc.chunk(id).is_some_and(|c| c.kind == ChunkKind::Heading1))
        .expect("heading chunk");
    let run = doc
        .child_ids(heading)
        .into_iter()
        .find(|&id| doc.chunk(id).is_some_and(|c| c.kind == ChunkKind::Run))
        .expect("heading run");
    let text = doc.direct_text(run).expect("heading text").to_string();
    (heading, run, text)
}

/// The viewport every interaction test scrolls to.
fn viewport() -> Viewport {
    Viewport {
        x: 0.0,
        y: 0.0,
        w: 800.0,
        h: 600.0,
    }
}

#[test]
fn load_rejects_malformed_bytes_at_the_reader_boundary() {
    // The host takes a parsed package, so the byte-level rejection lives at
    // the reader boundary (typed error, never a panic).
    assert!(parse(b"not a cull package at all").is_err());
}

#[test]
fn load_rejects_a_package_that_fails_document_validation() {
    // The v1-minimal fixture is INFO-only: it parses, but the document model
    // requires a CHNK section — a typed load error, not a panic.
    let pkg = parse(V1_MINIMAL).expect("parses");
    let result = HostDocument::load(pkg, Box::new(RecordingSink::default()), options());
    assert!(
        matches!(result, Err(HostError::Load(_))),
        "must reject without a chunk graph"
    );
}

#[test]
fn load_rejects_invalid_options_before_any_work() {
    let valid = options();
    assert!(HostDocument::load(
        parse(GOLDEN).expect("golden parses"),
        Box::new(RecordingSink::default()),
        valid,
    )
    .is_ok());

    let mut bad = options();
    bad.dpr = 0.0;
    assert_rejects(bad, "dpr zero");

    let mut bad = options();
    bad.dpr = f32::NAN;
    assert_rejects(bad, "dpr NaN");

    let mut bad = options();
    bad.margin = -1.0;
    assert_rejects(bad, "negative margin");

    let mut bad = options();
    bad.viewport.w = 0.0;
    assert_rejects(bad, "zero-width viewport");

    let mut bad = options();
    bad.viewport.x = f32::NAN;
    assert_rejects(bad, "NaN viewport origin");

    let mut bad = options();
    bad.frame_budget_ms = 0;
    assert_rejects(bad, "zero frame budget");
}

fn assert_rejects(options: HostOptions, what: &str) {
    let result = HostDocument::load(
        parse(GOLDEN).expect("golden parses"),
        Box::new(RecordingSink::default()),
        options,
    );
    assert!(
        matches!(result, Err(HostError::InvalidOptions(_))),
        "{what} must be rejected"
    );
}

#[test]
fn load_uploads_every_atlas_page_and_no_images() {
    let (_host, sink) = host_with_sink();
    let log = sink.log.borrow();
    // Golden diagnostics: three atlases with 2+1+1 pages, zero images. Load
    // also runs the initial scroll, which resizes the surface once.
    assert_eq!(log.atlas_uploads, 4);
    assert_eq!(log.image_uploads, 0);
    assert_eq!(log.resizes, vec![(800, 600)]);
    assert_eq!(log.draws, 0);
    assert!(!log.destroyed);
}

#[test]
fn scroll_materializes_visible_text_and_paint_emits_glyph_batches() {
    let (mut host, sink) = host_with_sink();
    host.scroll(viewport(), Direction::Down).expect("scrolls");
    {
        let log = sink.log.borrow();
        // First scroll resizes the surface to the viewport size.
        assert_eq!(log.resizes, vec![(800, 600)]);
        assert_eq!(log.draws, 0);
    }
    host.paint().expect("paints");
    {
        let log = sink.log.borrow();
        assert_eq!(log.draws, 1);
        // The scheduler materialized the visible text: the plan carries at
        // least one glyph batch (the heading), not an empty draw.
        assert!(log.glyph_batches >= 1, "no glyph batches in the plan");
    }
}

#[test]
fn paint_before_any_scroll_uses_the_initial_viewport() {
    let (mut host, sink) = host_with_sink();
    host.paint().expect("paints");
    assert_eq!(sink.log.borrow().draws, 1);
}

#[test]
fn scroll_resizes_only_when_the_surface_size_changes() {
    let (mut host, sink) = host_with_sink();
    host.scroll(viewport(), Direction::Down).expect("scrolls");
    host.scroll(viewport(), Direction::Down).expect("scrolls");
    {
        let log = sink.log.borrow();
        assert_eq!(log.resizes, vec![(800, 600)]);
    }
    let wide = Viewport {
        x: 0.0,
        y: 0.0,
        w: 1024.0,
        h: 768.0,
    };
    host.scroll(wide, Direction::Down).expect("scrolls");
    {
        let log = sink.log.borrow();
        assert_eq!(log.resizes, vec![(800, 600), (1024, 768)]);
    }
}

#[test]
fn scroll_rejects_non_finite_and_non_positive_viewports() {
    let (mut host, _sink) = host_with_sink();
    let cases = [
        Viewport {
            x: f32::NAN,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        },
        Viewport {
            x: 0.0,
            y: f32::INFINITY,
            w: 800.0,
            h: 600.0,
        },
        Viewport {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 600.0,
        },
        Viewport {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: f32::NEG_INFINITY,
        },
    ];
    for case in cases {
        assert!(
            matches!(
                host.scroll(case, Direction::Down),
                Err(HostError::InvalidOptions(_))
            ),
            "viewport {case:?} must be rejected"
        );
    }
    // A rejected scroll leaves the host usable.
    host.scroll(viewport(), Direction::Down)
        .expect("still scrolls");
}

#[test]
fn select_range_and_copy_reproduce_the_document_text() {
    let (_heading, run, text) = heading_chunks();
    assert_eq!(text, "Golden");
    let (mut host, _sink) = host_with_sink();
    host.scroll(viewport(), Direction::Down).expect("scrolls");
    // No selection yet: copy is empty.
    assert_eq!(host.copy().expect("empty copy"), "");
    host.select_range(
        TextPosition {
            chunk_id: run,
            offset: 0,
        },
        TextPosition {
            chunk_id: run,
            offset: text.len(),
        },
    )
    .expect("selects");
    assert_eq!(host.copy().expect("copies"), text);
}

#[test]
fn select_between_hits_glyphs_and_select_point_collapses() {
    let (heading, _run, text) = heading_chunks();
    // Compute the heading line's glyph positions with a replica engine
    // (identical options ⇒ identical layout, determinism D5).
    let pkg = parse(GOLDEN).expect("golden parses");
    let doc = build_document(&pkg).expect("golden model");
    let mut engine = LayoutEngine::new(
        &doc,
        LayoutOptions {
            dpr: 1.0,
            content_width: 800.0,
        },
    );
    let _ = engine.materialize(heading);
    let record = engine.record(heading).expect("heading layout");
    let line = &record.lines[0];
    let first = line.glyphs.first().expect("first glyph");
    let last = line.glyphs.last().expect("last glyph");

    let (mut host, _sink) = host_with_sink();
    host.scroll(viewport(), Direction::Down).expect("scrolls");

    // Anchor at the first glyph's origin, focus beyond the last glyph's
    // center: the selection spans the whole run.
    host.select_between(first.x, first.y, last.x + last.advance_px, last.y)
        .expect("selects");
    assert_eq!(host.copy().expect("copies"), text);

    // A point selection collapses: copy yields nothing.
    host.select_point(first.x + first.advance_px / 2.0, first.y)
        .expect("selects point");
    assert_eq!(host.copy().expect("collapsed copy"), "");
}

#[test]
fn destroy_is_idempotent_and_every_operation_after_it_rejects() {
    let (mut host, sink) = host_with_sink();
    assert!(!host.destroyed());
    host.destroy();
    assert!(host.destroyed());
    host.destroy(); // idempotent
    assert!(host.destroyed());
    assert!(sink.log.borrow().destroyed, "sink must be destroyed once");

    assert!(matches!(
        host.scroll(viewport(), Direction::Down),
        Err(HostError::Destroyed)
    ));
    assert!(matches!(host.paint(), Err(HostError::Destroyed)));
    assert!(matches!(
        host.select_point(0.0, 0.0),
        Err(HostError::Destroyed)
    ));
    assert!(matches!(
        host.select_between(0.0, 0.0, 10.0, 10.0),
        Err(HostError::Destroyed)
    ));
    assert!(matches!(
        host.select_range(
            TextPosition {
                chunk_id: 1,
                offset: 0
            },
            TextPosition {
                chunk_id: 1,
                offset: 1
            },
        ),
        Err(HostError::Destroyed)
    ));
    assert!(matches!(host.copy(), Err(HostError::Destroyed)));
}

#[test]
fn documents_are_fully_isolated() {
    let (_heading, run, text) = heading_chunks();
    let (mut a, _sa) = host_with_sink();
    let (mut b, _sb) = host_with_sink();

    a.scroll(viewport(), Direction::Down).expect("a scrolls");
    b.scroll(viewport(), Direction::Down).expect("b scrolls");
    a.select_range(
        TextPosition {
            chunk_id: run,
            offset: 0,
        },
        TextPosition {
            chunk_id: run,
            offset: 1,
        },
    )
    .expect("a selects");
    assert_eq!(a.copy().expect("a copies"), "G");
    assert_eq!(b.copy().expect("b has no selection"), "");

    // Destroying a leaves b untouched.
    a.destroy();
    b.select_range(
        TextPosition {
            chunk_id: run,
            offset: 0,
        },
        TextPosition {
            chunk_id: run,
            offset: text.len(),
        },
    )
    .expect("b selects");
    assert_eq!(b.copy().expect("b copies"), text);
}

#[test]
fn select_pins_chunks_and_destroy_unpins_them() {
    // The selection pin path (lifecycle select/unselect) runs without error
    // and destroy releases everything — the pinned chunks never block teardown.
    let (_heading, run, text) = heading_chunks();
    let (mut host, sink) = host_with_sink();
    host.scroll(viewport(), Direction::Down).expect("scrolls");
    host.select_range(
        TextPosition {
            chunk_id: run,
            offset: 0,
        },
        TextPosition {
            chunk_id: run,
            offset: text.len(),
        },
    )
    .expect("selects");
    assert_eq!(host.copy().expect("copies"), text);
    host.destroy();
    assert!(sink.log.borrow().destroyed);
}
