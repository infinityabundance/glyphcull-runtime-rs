//! The platform-agnostic document host (mirrors the JS `DocumentHost` in
//! `src/api/runtime.ts`): wires the core pipeline — layout, glyph cache,
//! lifecycle, scheduler, draw list, selection — behind the tiny six-operation
//! contract, against a pluggable [`FrameSink`] (the wgpu sink on wasm and
//! desktop; a recording stub in native tests).
//!
//! Ownership (DESIGN.md D29): the model borrows the package and the layout
//! engine borrows the model, so the host is a self-referential struct
//! (`ouroboros`): package → model → layout, plus the free-standing
//! lifecycle/scheduler (they borrow the `'static` real clock, mirroring the
//! JS passing the clock by reference). The cooperative budget is inherent:
//! `scroll` runs one bounded `run_frame` per call (no threads in wasm32).
//!
//! Everything here is native-testable — no wasm-bindgen, no winit, no GPU.

// The ouroboros macro generates a constructor (and builder) mirroring every
// field; with sixteen fields it exceeds clippy's style threshold. The lint is
// not part of the workspace's denied set — it only bites under `-D warnings` —
// and the generated constructor is not our API surface.
#![allow(clippy::too_many_arguments)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;

use glyphcull_core::clock::{Clock, RealClock};
use glyphcull_core::document::{build_document, DocumentModel};
use glyphcull_core::draw_list::{DrawListBuilder, TextureResolver};
use glyphcull_core::glyphs::{prepare_glyph, GlyphCache, GlyphKey};
use glyphcull_core::layout::layout::{BlockLayout, LayoutEngine, LayoutOptions};
use glyphcull_core::lifecycle::{ChunkLifecycleConfig, LifecycleManager, LifecycleOptions};
use glyphcull_core::materialize::{
    Direction, MaterializationScheduler, MaterializeWorker, SchedulerOptions, WorkResult,
};
use glyphcull_core::reader::chunk::flags;
use glyphcull_core::reader::glyph::Atlas;
use glyphcull_core::reader::Package;
use glyphcull_core::selection::{
    copy_text, covered_chunk_ids, hit_test_point, normalize_selection, range_quads, Selection,
    SelectionQuad, TextPosition,
};
use glyphcull_core::visibility::{compute_visible_set, Viewport};
use glyphcull_render::plan::{build_plan, RenderPlan, RendererViewport};

/// The default visibility margin in CSS px (mirrors the JS `DEFAULT_MARGIN`).
pub const DEFAULT_MARGIN: f32 = 120.0;
/// The default glyph cache budget in bytes (mirrors the JS).
pub const DEFAULT_GLYPH_BUDGET: u64 = 8 * 1024 * 1024;
/// The default per-frame materialization budget in ms (mirrors the JS).
pub const DEFAULT_FRAME_BUDGET_MS: u64 = 8;
/// The default cooling period before eviction in ms (mirrors the JS).
pub const DEFAULT_COOLING_MS: u64 = 1500;

/// The real clock as a `'static` (zero-sized), so the lifecycle and the
/// scheduler can borrow it without a self-reference.
static REAL_CLOCK: RealClock = RealClock;

/// A typed host error (mirrors the JS `RuntimeError` kinds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// Invalid load options or an invalid viewport.
    InvalidOptions(String),
    /// The document handle was destroyed.
    Destroyed,
    /// Package parse or document validation failed.
    Load(String),
    /// The render sink failed.
    Renderer(String),
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostError::InvalidOptions(msg) => write!(f, "invalid-options: {msg}"),
            HostError::Destroyed => write!(f, "destroyed"),
            HostError::Load(msg) => write!(f, "load: {msg}"),
            HostError::Renderer(msg) => write!(f, "renderer: {msg}"),
        }
    }
}

impl std::error::Error for HostError {}

/// The resolved host options (all budgets configurable, per DESIGN.md D12).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostOptions {
    /// Device pixel ratio.
    pub dpr: f32,
    /// The page content width in CSS pixels.
    pub content_width: f32,
    /// The initial viewport.
    pub viewport: Viewport,
    /// The visibility margin in CSS pixels.
    pub margin: f32,
    /// The glyph cache budget in bytes.
    pub glyph_budget_bytes: u64,
    /// The materialization frame budget in ms.
    pub frame_budget_ms: u64,
    /// The cooling period before eviction in ms.
    pub cooling_period_ms: u64,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            dpr: 1.0,
            content_width: 800.0,
            viewport: Viewport {
                x: 0.0,
                y: 0.0,
                w: 800.0,
                h: 600.0,
            },
            margin: DEFAULT_MARGIN,
            glyph_budget_bytes: DEFAULT_GLYPH_BUDGET,
            frame_budget_ms: DEFAULT_FRAME_BUDGET_MS,
            cooling_period_ms: DEFAULT_COOLING_MS,
        }
    }
}

/// Validate load options; reject nonsense with a typed error (mirrors the JS
/// `validateOptions`).
pub fn validate_options(options: &HostOptions) -> Result<(), HostError> {
    let reject = |message: String| Err(HostError::InvalidOptions(message));
    if options.dpr <= 0.0 || !options.dpr.is_finite() {
        return reject(format!(
            "dpr must be a positive number, got {}",
            options.dpr
        ));
    }
    if options.margin < 0.0 || !options.margin.is_finite() {
        return reject(format!(
            "margin must be non-negative, got {}",
            options.margin
        ));
    }
    if !options.viewport.w.is_finite()
        || !options.viewport.h.is_finite()
        || options.viewport.w <= 0.0
        || options.viewport.h <= 0.0
    {
        return reject(format!(
            "viewport size must be positive and finite, got {}×{}",
            options.viewport.w, options.viewport.h
        ));
    }
    if !options.viewport.x.is_finite() || !options.viewport.y.is_finite() {
        return reject(format!(
            "viewport origin must be finite, got {},{}",
            options.viewport.x, options.viewport.y
        ));
    }
    if options.frame_budget_ms == 0 {
        return reject("frameBudgetMs must be positive".to_string());
    }
    Ok(())
}

/// The render sink seam: the host's renderer (uploads + draw).
pub trait FrameSink {
    /// Upload (or refresh) an atlas page; returns the handle the texture
    /// resolver maps back to.
    fn upload_atlas_page(
        &mut self,
        atlas_id: u32,
        page_index: u16,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> u32;
    /// Upload (or refresh) an image; `rgb` selects RGB8 (padded to RGBA) vs
    /// RGBA8.
    fn upload_image(
        &mut self,
        image_id: u32,
        pixels: &[u8],
        width: u32,
        height: u32,
        rgb: bool,
    ) -> u32;
    /// Draw a prepared plan into the current frame.
    fn draw(
        &mut self,
        plan: &RenderPlan,
        viewport: RendererViewport,
        surface_w: f32,
        surface_h: f32,
    ) -> Result<(), String>;
    /// Resize the drawing surface (device pixels).
    fn resize(&mut self, width: u32, height: u32);
    /// Optionally capture the last presented frame as tightly packed
    /// premultiplied RGBA8 (device pixels, top-left origin). Hosts without a
    /// readable GPU surface return `None` (the default); the desktop host
    /// re-renders the last plan offscreen for `--screenshot` (DESIGN.md D31).
    fn capture(&mut self) -> Option<Vec<u8>> {
        None
    }
    /// Release GPU resources.
    fn destroy(&mut self);
}

/// The draw list's texture resolver: page/image → sink handle (0 = missing).
struct HandleResolver<'a> {
    pages: &'a BTreeMap<(u32, u16), u32>,
    images: &'a BTreeMap<u32, u32>,
}

impl TextureResolver for HandleResolver<'_> {
    fn atlas_page(&self, atlas_id: u32, page_index: u16) -> u32 {
        self.pages
            .get(&(atlas_id, page_index))
            .copied()
            .unwrap_or(0)
    }
    fn image(&self, image_id: u32) -> u32 {
        self.images.get(&image_id).copied().unwrap_or(0)
    }
}

/// The scheduler's worker: lay out a block and prepare its cached stamps.
struct HostWorker<'x, 'this> {
    top_level_ids: &'x BTreeSet<u32>,
    doc: &'x DocumentModel<'this>,
    layout: &'x mut LayoutEngine<'this>,
    atlases: &'x [Atlas],
    cache: &'x mut GlyphCache,
}

impl HostWorker<'_, '_> {
    /// The top-level block ancestor of a chunk (itself included), or `None`.
    fn top_level_of(&self, chunk_id: u32) -> Option<u32> {
        let mut id = chunk_id;
        loop {
            if self.top_level_ids.contains(&id) {
                return Some(id);
            }
            let chunk = self.doc.chunk(id)?;
            if chunk.parent_id == 0 {
                return None;
            }
            id = chunk.parent_id;
        }
    }
}

/// Prepare and cache the stamps of a block's laid-out glyphs.
///
/// A free function (not a `&mut self` method) so the borrow checker can split
/// the worker's field borrows: `record` borrows `layout` while `cache` is
/// written, and the two places are disjoint.
fn prepare_stamps(cache: &mut GlyphCache, atlases: &[Atlas], record: &BlockLayout) {
    for line in &record.lines {
        for glyph in &line.glyphs {
            if glyph.mark_of.is_some() || !glyph.has_outline {
                continue;
            }
            let Some(atlas) = atlases.get(glyph.atlas_id as usize) else {
                continue;
            };
            let Some(stamp) =
                prepare_glyph(atlas, glyph.codepoint, glyph.font_size_px, glyph.color)
            else {
                continue;
            };
            // Stamps are owned by the run chunk carrying the glyph:
            // eviction is per chunk, so a culled run releases exactly its
            // own stamps.
            cache.put(stamp.key, stamp, glyph.run_chunk_id);
        }
    }
}

impl MaterializeWorker for HostWorker<'_, '_> {
    fn work(&mut self, chunk_id: u32, _budget_ms: u64, _elapsed_ms: u64) -> WorkResult {
        // Nested blocks exist only once their top-level ancestor is laid out;
        // materializing the ancestor is idempotent, so this also covers runs
        // and nested chunks whose parent is already materialized.
        let Some(top) = self.top_level_of(chunk_id) else {
            return WorkResult::Complete;
        };
        let _ = self.layout.materialize(top);
        let Some(record) = self.layout.record(chunk_id) else {
            return WorkResult::Complete;
        };
        prepare_stamps(self.cache, self.atlases, record);
        WorkResult::Complete
    }

    fn release(&mut self, chunk_id: u32) {
        self.cache.release_chunk(chunk_id);
    }
}

/// The document host: exactly the six runtime operations, platform-agnostic.
///
/// Self-referential ownership (DESIGN.md D29): the package is the root of
/// the borrow graph (`pkg → doc → layout`); the lifecycle and the scheduler
/// borrow the `'static` clock.
#[ouroboros::self_referencing]
pub struct HostDocument {
    pkg: Package,
    // `DocumentModel`/`LayoutEngine` hold only covariant `&'a` references.
    #[covariant]
    #[borrows(pkg)]
    doc: DocumentModel<'this>,
    #[covariant]
    #[borrows(doc)]
    layout: LayoutEngine<'this>,
    glyph_cache: GlyphCache,
    // The lifecycle is owned by the scheduler (the scheduler pins cooling
    // chunks against eviction); the host reaches it via `lifecycle_mut()`.
    // The clock is a `'static` trait object so hosts can inject a platform
    // clock (wasm has no std time; the binding provides performance.now).
    scheduler: MaterializationScheduler<'static, dyn Clock + 'static>,
    #[borrows(doc)]
    top_level_ids: BTreeSet<u32>,
    page_handles: BTreeMap<(u32, u16), u32>,
    image_handles: BTreeMap<u32, u32>,
    sink: Box<dyn FrameSink>,
    margin: f32,
    dpr: f32,
    viewport: Viewport,
    selection: Option<Selection>,
    selected_chunks: BTreeSet<u32>,
    surface_size: Option<(f32, f32)>,
    destroyed: bool,
}

impl HostDocument {
    /// Load a parsed package into a document handle (mirrors the JS
    /// `DocumentHost` constructor: register chunks, upload textures, initial
    /// scroll).
    ///
    /// The package is first validated by an independent build (typed errors);
    /// the self-referential construction then re-runs the same deterministic
    /// build inside the pinned struct — the `expect` is provably safe.
    #[allow(clippy::expect_used)] // the build was validated above; identical input
    pub fn load(
        pkg: Package,
        sink: Box<dyn FrameSink>,
        options: HostOptions,
    ) -> Result<Self, HostError> {
        Self::load_with_clock(pkg, sink, options, &REAL_CLOCK)
    }

    /// Load a parsed package with an explicit `'static` clock. The native
    /// default is the real clock; the wasm binding injects a
    /// performance.now-based clock (wasm32-unknown-unknown has no std time,
    /// and `SystemTime::now` panics there).
    #[allow(clippy::expect_used)] // the build was validated above; identical input
    pub fn load_with_clock(
        pkg: Package,
        sink: Box<dyn FrameSink>,
        options: HostOptions,
        clock: &'static dyn Clock,
    ) -> Result<Self, HostError> {
        validate_options(&options)?;
        {
            let doc = build_document(&pkg).map_err(|e| HostError::Load(e.to_string()))?;
            let _ = doc.all_ids().len();
        }
        let lifecycle = LifecycleManager::new(
            clock,
            LifecycleOptions {
                default_cooling_period_ms: options.cooling_period_ms,
            },
        );
        let scheduler = MaterializationScheduler::new(
            clock,
            lifecycle,
            SchedulerOptions {
                frame_budget_ms: options.frame_budget_ms,
                yield_penalty: 1,
            },
        );
        let mut host = HostDocument::new(
            pkg,
            |pkg| build_document(pkg).expect("document validated above"),
            |doc| {
                LayoutEngine::new(
                    doc,
                    LayoutOptions {
                        dpr: options.dpr,
                        content_width: options.content_width,
                    },
                )
            },
            GlyphCache::new(options.glyph_budget_bytes),
            scheduler,
            |doc| doc.child_ids(doc.root().id).into_iter().collect(),
            BTreeMap::new(),
            BTreeMap::new(),
            sink,
            options.margin,
            options.dpr,
            options.viewport,
            None,
            BTreeSet::new(),
            None,
            false,
        );
        host.with_mut(|fields| {
            // Lifecycle registration: every chunk, hidden per its flag.
            for id in fields.doc.all_ids() {
                if let Some(chunk) = fields.doc.chunk(id) {
                    let _ = fields.scheduler.lifecycle_mut().register(
                        id,
                        ChunkLifecycleConfig {
                            hidden: chunk.flags & flags::HIDDEN != 0,
                            cooling_period_ms: options.cooling_period_ms,
                        },
                    );
                }
            }
            // Texture uploads (once per page/image).
            for atlas in fields.doc.atlases() {
                for (page_index, page) in atlas.pages.iter().enumerate() {
                    let handle = fields.sink.upload_atlas_page(
                        atlas.font_id,
                        page_index as u16,
                        page,
                        atlas.page_width,
                        atlas.page_height,
                    );
                    fields
                        .page_handles
                        .insert((atlas.font_id, page_index as u16), handle);
                }
            }
            for image in fields.doc.images() {
                let handle = fields.sink.upload_image(
                    image.id,
                    &image.data,
                    u32::from(image.width),
                    u32::from(image.height),
                    crate::image_is_rgb(image.format),
                );
                fields.image_handles.insert(image.id, handle);
            }
        });
        host.scroll(options.viewport, Direction::Down)?;
        Ok(host)
    }

    /// Whether the handle has been destroyed.
    #[must_use]
    pub fn destroyed(&self) -> bool {
        self.with(|fields| *fields.destroyed)
    }

    /// Move the viewport (document CSS pixels) and run one materialization
    /// cycle. `direction` is the travel direction for scheduling priorities.
    pub fn scroll(&mut self, viewport: Viewport, direction: Direction) -> Result<(), HostError> {
        self.assert_alive()?;
        if !viewport.x.is_finite() || !viewport.y.is_finite() {
            return Err(HostError::InvalidOptions(format!(
                "viewport origin must be finite, got {},{}",
                viewport.x, viewport.y
            )));
        }
        if !viewport.w.is_finite()
            || !viewport.h.is_finite()
            || viewport.w <= 0.0
            || viewport.h <= 0.0
        {
            return Err(HostError::InvalidOptions(format!(
                "viewport size must be positive, got {}×{}",
                viewport.w, viewport.h
            )));
        }
        self.with_mut(|fields| {
            *fields.viewport = viewport;
            // Lay out the geometry the viewport needs (the sequential
            // frontier), then reconcile the visible set with the scheduler
            // and run one bounded cycle.
            fields
                .layout
                .extend_to(f64::from(viewport.y + viewport.h + *fields.margin));
            let result = compute_visible_set(fields.doc, fields.layout, viewport, *fields.margin);
            // The geometry callback borrows only the layout engine (not the
            // whole fields struct), so the scheduler can be mutated alongside.
            let layout = &*fields.layout;
            let geometry = |id: u32| layout.rect(id);
            fields
                .scheduler
                .reconcile(&result.visible, viewport, direction, &geometry)
                .map_err(|e| HostError::Load(e.to_string()))?;
            let mut worker = HostWorker {
                top_level_ids: fields.top_level_ids,
                doc: fields.doc,
                layout: fields.layout,
                atlases: fields.doc.atlases(),
                cache: fields.glyph_cache,
            };
            fields
                .scheduler
                .run_frame(&mut worker)
                .map_err(|e| HostError::Load(e.to_string()))?;
            fields
                .scheduler
                .tick(&mut worker)
                .map_err(|e| HostError::Load(e.to_string()))?;
            let needs_resize = fields
                .surface_size
                .is_none_or(|(w, h)| w != viewport.w || h != viewport.h);
            if needs_resize {
                *fields.surface_size = Some((viewport.w, viewport.h));
                fields
                    .sink
                    .resize(viewport.w.max(1.0) as u32, viewport.h.max(1.0) as u32);
            }
            Ok(())
        })
    }

    /// Render the current viewport + selection into the sink.
    pub fn paint(&mut self) -> Result<(), HostError> {
        self.assert_alive()?;
        self.with_mut(|fields| {
            let result =
                compute_visible_set(fields.doc, fields.layout, *fields.viewport, *fields.margin);
            let selection: Vec<SelectionQuad> = match *fields.selection {
                Some(selection) => range_quads(fields.layout, selection),
                None => Vec::new(),
            };
            let resolver = HandleResolver {
                pages: fields.page_handles,
                images: fields.image_handles,
            };
            let builder = DrawListBuilder::new(resolver);
            let list = builder.build(
                fields.layout,
                &result.visible,
                |_chunk_id, glyph| {
                    fields.glyph_cache.get(GlyphKey {
                        atlas_id: glyph.atlas_id,
                        codepoint: glyph.codepoint,
                        font_size_px: glyph.font_size_px,
                        color: glyph.color,
                    })
                },
                &selection,
            );
            let (surface_w, surface_h) = fields
                .surface_size
                .unwrap_or((fields.viewport.w, fields.viewport.h));
            let viewport = RendererViewport {
                x: fields.viewport.x,
                y: fields.viewport.y,
                w: fields.viewport.w,
                h: fields.viewport.h,
                dpr: *fields.dpr,
            };
            let plan = build_plan(&list, viewport, surface_w, surface_h);
            fields
                .sink
                .draw(&plan, viewport, surface_w, surface_h)
                .map_err(HostError::Renderer)?;
            Ok(())
        })
    }

    /// Select the text at a point (collapsed).
    pub fn select_point(&mut self, x: f32, y: f32) -> Result<(), HostError> {
        self.assert_alive()?;
        self.with_mut(|fields| {
            let hit = hit_test_point(fields.layout, glyphcull_core::selection::Point { x, y });
            set_selection(
                fields.scheduler.lifecycle_mut(),
                fields.doc,
                fields.selected_chunks,
                fields.selection,
                hit.map(|hit| Selection {
                    start: hit,
                    end: hit,
                }),
            );
            Ok(())
        })
    }

    /// Select between two document points (drag).
    pub fn select_between(&mut self, ax: f32, ay: f32, bx: f32, by: f32) -> Result<(), HostError> {
        self.assert_alive()?;
        self.with_mut(|fields| {
            let anchor = hit_test_point(
                fields.layout,
                glyphcull_core::selection::Point { x: ax, y: ay },
            );
            let focus = hit_test_point(
                fields.layout,
                glyphcull_core::selection::Point { x: bx, y: by },
            );
            set_selection(
                fields.scheduler.lifecycle_mut(),
                fields.doc,
                fields.selected_chunks,
                fields.selection,
                match (anchor, focus) {
                    (Some(anchor), Some(focus)) => {
                        Some(normalize_selection(fields.doc, anchor, focus))
                    }
                    _ => None,
                },
            );
            Ok(())
        })
    }

    /// Select a logical range directly (normalized to document order).
    pub fn select_range(
        &mut self,
        start: TextPosition,
        end: TextPosition,
    ) -> Result<(), HostError> {
        self.assert_alive()?;
        self.with_mut(|fields| {
            set_selection(
                fields.scheduler.lifecycle_mut(),
                fields.doc,
                fields.selected_chunks,
                fields.selection,
                Some(normalize_selection(fields.doc, start, end)),
            );
            Ok(())
        })
    }

    /// The plain text of the current selection ('' when none/collapsed).
    pub fn copy(&self) -> Result<String, HostError> {
        self.assert_alive()?;
        self.with(|fields| {
            Ok(match *fields.selection {
                Some(selection) => copy_text(fields.doc, selection),
                None => String::new(),
            })
        })
    }

    /// Capture the last presented frame as tightly packed premultiplied RGBA8
    /// (device pixels, top-left origin) — a host diagnostic for visual
    /// verification, not part of the six-operation runtime API. Returns `None`
    /// when the sink cannot capture (wasm, recording sinks); the desktop sink
    /// re-renders the last plan offscreen (DESIGN.md D31).
    pub fn capture_last_frame(&mut self) -> Option<Vec<u8>> {
        self.with_mut(|fields| fields.sink.capture())
    }

    /// Release every resource; idempotent. All other calls then reject.
    pub fn destroy(&mut self) {
        if self.destroyed() {
            return;
        }
        self.with_mut(|fields| {
            for &id in fields.selected_chunks.iter() {
                fields.scheduler.lifecycle_mut().unselect(id);
            }
            fields.selected_chunks.clear();
            *fields.selection = None;
            fields.sink.destroy();
            *fields.destroyed = true;
        });
    }

    fn assert_alive(&self) -> Result<(), HostError> {
        if self.destroyed() {
            Err(HostError::Destroyed)
        } else {
            Ok(())
        }
    }
}

/// The selection-pinning helper (mirrors the JS `setSelection`): pins the
/// covered chunks against eviction and stores the selection. The pieces are
/// passed explicitly because ouroboros's generated `BorrowedMutFields` type is
/// internal to the macro and cannot be named here.
fn set_selection(
    lifecycle: &mut LifecycleManager<'static, dyn Clock + 'static>,
    doc: &DocumentModel<'_>,
    selected_chunks: &mut BTreeSet<u32>,
    selection: &mut Option<Selection>,
    next: Option<Selection>,
) {
    let covered: BTreeSet<u32> = match next {
        Some(selection) => covered_chunk_ids(doc, selection).into_iter().collect(),
        None => BTreeSet::new(),
    };
    for &id in selected_chunks.iter() {
        if !covered.contains(&id) {
            lifecycle.unselect(id);
        }
    }
    for &id in &covered {
        if !selected_chunks.contains(&id) {
            lifecycle.select(id);
        }
    }
    *selected_chunks = covered;
    *selection = next;
}
