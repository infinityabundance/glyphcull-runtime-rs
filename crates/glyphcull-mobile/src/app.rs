//! The mobile application: the winit host over the shared [`DesktopSink`]
//! (Android; the same code also runs on desktop for tests and smoke runs).
//!
//! The Android lifecycle drives the document: `resumed` (foreground) builds a
//! window + sink + host from the packaged bytes; `suspended` (background)
//! drops them — the native window, and therefore the surface, is destroyed on
//! suspend, and rebuilding on resume is the canonical Android GPU lifecycle
//! (the wgpu examples do the same). Input: wheel (mice/trackpads), touch drag
//! (content follows the finger), fling/inertia on release (exponential
//! velocity decay), and pinch zoom (the viewport shrinks around the pinch
//! midpoint while the surface stays at the window size — the
//! `scroll_with_surface` seam); left-drag selects.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, Touch, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use glyphcull_core::materialize::Direction;
use glyphcull_core::reader::parse;
use glyphcull_core::selection::TextPosition;
use glyphcull_core::visibility::Viewport;
use glyphcull_desktop::input::{self, WheelDelta};
use glyphcull_desktop::DesktopSink;
use glyphcull_host::{HostDocument, HostError, HostOptions};

use crate::gesture::{self, Fling, FLING_SAMPLE_MS, FLING_STOP_EPSILON};

/// A typed mobile host error.
#[derive(Debug)]
pub enum MobileError {
    /// The event loop or window failed.
    Window(String),
    /// The wgpu renderer failed to initialize or draw.
    Renderer(String),
    /// The package failed to parse or validate.
    Load(String),
    /// The host rejected the operation.
    Host(HostError),
    /// The packaged asset could not be read.
    Asset(String),
}

impl fmt::Display for MobileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Window(message) => write!(f, "window: {message}"),
            Self::Renderer(message) => write!(f, "renderer: {message}"),
            Self::Load(message) => write!(f, "load: {message}"),
            Self::Host(error) => write!(f, "host: {error}"),
            Self::Asset(message) => write!(f, "asset: {message}"),
        }
    }
}

impl std::error::Error for MobileError {}

impl From<HostError> for MobileError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

/// The document-pixel vertical scroll for a touch move: the content follows
/// the finger, so swiping up (finger y decreases) scrolls down (positive).
/// Both positions are **document** px (touch locations are converted with
/// `input::cursor_to_doc`, so the dpr cancels — no extra division). Pure
/// (unit-tested).
#[must_use]
pub fn scroll_delta_for_touch(prev: (f32, f32), cur: (f32, f32)) -> f32 {
    prev.1 - cur.1
}

/// The travel direction implied by a scroll delta (negative = up; zero = Down,
/// direction is moot).
#[must_use]
pub fn direction_for_delta(dy: f32) -> Direction {
    if dy < 0.0 {
        Direction::Up
    } else {
        Direction::Down
    }
}

/// The mobile document: a window serving the six-operation runtime API
/// (`load`/`scroll`/`paint`/`select`/`copy`/`destroy`) over the shared sink,
/// plus the mobile gesture layer (drag/fling scroll, pinch zoom).
pub struct MobileDocument {
    window: Arc<Window>,
    host: HostDocument,
    /// The current viewport in document pixels (may be zoomed: w < canvas.w).
    viewport: Viewport,
    /// The device pixel ratio (physical → logical).
    dpr: f32,
    /// The drawing surface size in CSS px (the window ÷ dpr) — the host's
    /// `scroll_with_surface` canvas; the viewport can be smaller (zoom).
    canvas: (f32, f32),
    /// The document's fit-width content width (the zoom reference).
    content_width: f32,
    /// The drag anchor in document space while a drag is in progress.
    drag_anchor: Option<(f32, f32)>,
    /// The last cursor position in physical pixels.
    last_cursor: Option<(f64, f64)>,
    /// Active touches (touch id → document-space position).
    touches: BTreeMap<u64, (f32, f32)>,
    /// The previous pinch finger distance (doc px), while two touches are down.
    pinch_prev_dist: Option<f32>,
    /// The active fling, if any.
    fling: Option<Fling>,
    /// The last fling frame time (dt for the velocity decay).
    last_frame: Option<std::time::Instant>,
    /// Recent single-finger drag samples `(elapsed_ms, doc_y)` for the
    /// release-velocity estimate (newest last).
    touch_samples: VecDeque<(u64, f32)>,
    /// The monotonic clock for the sample timestamps.
    clock: std::time::Instant,
}

impl MobileDocument {
    /// The current viewport in document pixels.
    #[must_use]
    pub fn viewport(&self) -> Viewport {
        self.viewport
    }

    /// Whether the document has been destroyed.
    #[must_use]
    pub fn destroyed(&self) -> bool {
        self.host.destroyed()
    }

    // --- the six operations (delegating to the shared host) ---

    /// Move the viewport and run one materialization cycle. The drawing
    /// surface stays at the window size (the `scroll_with_surface` seam), so
    /// a zoomed viewport renders into the full surface — a crisp GPU zoom.
    pub fn scroll(&mut self, viewport: Viewport, direction: Direction) -> Result<(), HostError> {
        self.viewport = viewport;
        self.host
            .scroll_with_surface(viewport, direction, self.canvas)
    }

    /// Render the current viewport + selection into the window.
    pub fn paint(&mut self) -> Result<(), HostError> {
        self.host.paint()
    }

    /// Select the text at a document point (collapsed).
    pub fn select_point(&mut self, x: f32, y: f32) -> Result<(), HostError> {
        self.host.select_point(x, y)
    }

    /// Select between two document points (drag).
    pub fn select_between(&mut self, ax: f32, ay: f32, bx: f32, by: f32) -> Result<(), HostError> {
        self.host.select_between(ax, ay, bx, by)
    }

    /// Select a logical range directly (normalized to document order).
    pub fn select_range(
        &mut self,
        start: TextPosition,
        end: TextPosition,
    ) -> Result<(), HostError> {
        self.host.select_range(start, end)
    }

    /// The plain text of the current selection ('' when none/collapsed).
    pub fn copy(&self) -> Result<String, HostError> {
        self.host.copy()
    }

    /// Release every resource; idempotent. Also runs on drop (suspend).
    pub fn destroy(&mut self) {
        self.host.destroy();
    }

    /// Capture the last presented frame as tightly packed premultiplied RGBA8
    /// (device pixels) — the host diagnostic (DESIGN.md D31). Returns `None`
    /// if no frame has been drawn yet or the capture fails.
    #[must_use]
    pub fn capture_last_frame(&mut self) -> Option<Vec<u8>> {
        self.host.capture_last_frame()
    }

    /// Dump the offscreen D31 capture to the app's data dir (Android-only):
    /// the on-device pixel truth. The emulator's present path can fail to
    /// composite (gfxstream/SwiftShader), while this capture re-renders the
    /// last plan offscreen — the same mechanism the desktop `--screenshot`
    /// smoke validates. The smoke harness pulls it for the D3 comparison.
    /// Dumped once per process (the first painted frame is the smoke's
    /// target; later paints skip the write).
    #[cfg(target_os = "android")]
    fn dump_capture(&mut self) {
        let path = std::path::Path::new("/data/data/com.glyphcull.host/frame.rgba");
        if path.exists() {
            return;
        }
        let Some(rgba) = self.capture_last_frame() else {
            return;
        };
        let (width, height) = {
            let viewport = self.viewport;
            (viewport.w.max(1.0) as u32, viewport.h.max(1.0) as u32)
        };
        let mut bytes = Vec::with_capacity(8 + rgba.len());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&rgba);
        if std::fs::write(path, &bytes).is_err() {
            eprintln!("glyphcull-mobile: capture dump failed");
        }
    }

    // --- input wiring (winit events → the operations) ---

    fn on_resize(&mut self, width: u32, height: u32) {
        self.dpr = self.window.scale_factor() as f32;
        // The surface follows the window; the viewport resets to it (a
        // rotation resets the zoom — recorded behavior).
        self.canvas = ((width as f32) / self.dpr, (height as f32) / self.dpr);
        let viewport = input::viewport_for_size(width, height, self.dpr);
        eprintln!(
            "glyphcull-mobile: resize {}x{} dpr={} viewport={}x{}",
            width, height, self.dpr, viewport.w, viewport.h
        );
        if self.scroll(viewport, Direction::Down).is_ok() {
            self.window.request_redraw();
        }
    }

    fn on_wheel(&mut self, delta: MouseScrollDelta) {
        let delta = WheelDelta::from(delta);
        let dy = input::scroll_for_wheel(delta, self.dpr, 40.0);
        if dy == 0.0 {
            return;
        }
        let direction = input::direction_for_wheel(delta);
        let viewport = input::scrolled_viewport(self.viewport, dy);
        if self.scroll(viewport, direction).is_ok() {
            self.window.request_redraw();
        }
    }

    fn on_touch(&mut self, touch: Touch) {
        let position = input::cursor_to_doc((touch.location.x, touch.location.y), self.dpr);
        match touch.phase {
            TouchPhase::Started => {
                self.touches.insert(touch.id, position);
                if self.touches.len() == 2 {
                    // A pinch begins: stop any fling and clear the drag
                    // samples (a two-finger gesture is never a fling).
                    self.fling = None;
                    self.touch_samples.clear();
                    self.pinch_prev_dist = Some(self.pinch_distance());
                }
            }
            TouchPhase::Moved => {
                self.touches.insert(touch.id, position);
                if self.touches.len() >= 2 {
                    // Pinch: zoom around the midpoint (no scrolling, no
                    // drag samples).
                    self.touch_samples.clear();
                    let Some(prev_dist) = self.pinch_prev_dist else {
                        self.pinch_prev_dist = Some(self.pinch_distance());
                        return;
                    };
                    let dist = self.pinch_distance();
                    let factor = gesture::pinch_factor(prev_dist, dist);
                    self.pinch_prev_dist = Some(dist);
                    if (factor - 1.0).abs() < 1e-4 {
                        return;
                    }
                    let midpoint = self.pinch_midpoint();
                    let viewport =
                        gesture::zoom_viewport(self.viewport, factor, midpoint, self.content_width);
                    if self.scroll(viewport, Direction::Down).is_ok() {
                        self.window.request_redraw();
                    }
                } else {
                    // Single-finger drag: content follows the finger; record
                    // the sample for the release-velocity estimate.
                    let previous = self
                        .touches
                        .iter()
                        .next()
                        .map(|(_, p)| *p)
                        .unwrap_or(position);
                    let dy = scroll_delta_for_touch(previous, position);
                    let now = self.now_ms();
                    self.touch_samples.push_back((now, position.1));
                    let cutoff = now.saturating_sub(FLING_SAMPLE_MS);
                    while self.touch_samples.front().is_some_and(|(t, _)| *t < cutoff) {
                        self.touch_samples.pop_front();
                    }
                    if dy == 0.0 {
                        return;
                    }
                    let viewport = input::scrolled_viewport(self.viewport, dy);
                    if self.scroll(viewport, direction_for_delta(dy)).is_ok() {
                        self.window.request_redraw();
                    }
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.touches.remove(&touch.id);
                match self.touches.len() {
                    0 => {
                        // Release: fling when the drag was fast (a pinch never
                        // flings — `pinch_prev_dist` is set during one).
                        if self.pinch_prev_dist.is_none() {
                            let samples: Vec<(u64, f32)> =
                                self.touch_samples.iter().copied().collect();
                            let velocity = gesture::scroll_release_velocity(&samples);
                            if velocity.abs() > FLING_STOP_EPSILON {
                                self.fling = Some(Fling::new(velocity));
                                self.last_frame = Some(std::time::Instant::now());
                                self.window.request_redraw();
                            }
                        }
                        self.pinch_prev_dist = None;
                        self.touch_samples.clear();
                    }
                    1 => {
                        // A pinch ended with one finger still down: it is a
                        // plain drag now.
                        self.pinch_prev_dist = None;
                        self.touch_samples.clear();
                    }
                    _ => {}
                }
            }
        }
    }

    /// Advance the fling by the time since the last frame; returns whether it
    /// is still running (the caller keeps requesting redraws while so).
    fn drive_fling(&mut self) {
        let Some(mut fling) = self.fling else {
            return;
        };
        let now = std::time::Instant::now();
        let dt = self
            .last_frame
            .map_or(1.0 / 60.0, |last| now.duration_since(last).as_secs_f32())
            .clamp(0.0, 1.0 / 20.0); // cap: no giant jumps after a stall
        self.last_frame = Some(now);
        let dy = fling.step(dt);
        if fling.stopped() {
            self.fling = None;
        } else {
            self.fling = Some(fling);
        }
        if dy == 0.0 {
            return;
        }
        let viewport = input::scrolled_viewport(self.viewport, dy);
        if self.scroll(viewport, direction_for_delta(dy)).is_ok() {
            self.window.request_redraw();
        }
    }

    /// The distance between the two active touches (doc px).
    fn pinch_distance(&self) -> f32 {
        let mut positions = self.touches.values();
        let Some((x0, y0)) = positions.next().copied() else {
            return 0.0;
        };
        let Some((x1, y1)) = positions.next().copied() else {
            return 0.0;
        };
        ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt()
    }

    /// The midpoint of the two active touches (doc px) — the zoom anchor.
    fn pinch_midpoint(&self) -> (f32, f32) {
        let mut positions = self.touches.values();
        let Some((x0, y0)) = positions.next().copied() else {
            return (0.0, 0.0);
        };
        let Some((x1, y1)) = positions.next().copied() else {
            return (x0, y0);
        };
        ((x0 + x1) / 2.0, (y0 + y1) / 2.0)
    }

    /// The monotonic clock in ms (for the release-velocity samples).
    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    fn on_cursor_moved(&mut self, position: (f64, f64)) {
        self.last_cursor = Some(position);
        let Some(anchor) = self.drag_anchor else {
            return;
        };
        let (x, y) = input::cursor_to_doc(position, self.dpr);
        if self.select_between(anchor.0, anchor.1, x, y).is_ok() {
            self.window.request_redraw();
        }
    }

    fn on_drag_start(&mut self) {
        let Some((x, y)) = self.last_cursor else {
            return;
        };
        let (x, y) = input::cursor_to_doc((x, y), self.dpr);
        self.drag_anchor = Some((x, y));
        if self.select_point(x, y).is_ok() {
            self.window.request_redraw();
        }
    }

    fn on_drag_end(&mut self) {
        self.drag_anchor = None;
    }
}

impl Drop for MobileDocument {
    fn drop(&mut self) {
        // Explicit release on suspend/exit: the surface and the device are
        // torn down promptly (the host's destroy is idempotent).
        self.host.destroy();
    }
}

/// The winit application: owns the packaged bytes until a window exists, then
/// the document. `resumed` (re)builds the document; `suspended` drops it.
pub struct MobileApp {
    bytes: Vec<u8>,
    options: HostOptions,
    document: Option<MobileDocument>,
}

impl MobileApp {
    /// Create the app for a packaged document (the bytes are parsed per
    /// resume; parsing is deterministic and validated).
    #[must_use]
    pub fn new(bytes: Vec<u8>, options: HostOptions) -> Self {
        Self {
            bytes,
            options,
            document: None,
        }
    }

    /// Report a build failure: log it, and exit the loop on desktop (on
    /// Android there is nothing to exit — the next resume retries).
    fn fail(&mut self, event_loop: &ActiveEventLoop, error: MobileError) {
        eprintln!("glyphcull-mobile: {error}");
        if !cfg!(target_os = "android") {
            event_loop.exit();
        }
    }
}

impl ApplicationHandler for MobileApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.document.is_some() {
            return; // Android re-fires resumed on re-show
        }
        let window =
            match event_loop.create_window(Window::default_attributes().with_title("GlyphCull")) {
                Ok(window) => Arc::new(window),
                Err(error) => {
                    self.fail(event_loop, MobileError::Window(error.to_string()));
                    return;
                }
            };
        let sink = match pollster::block_on(DesktopSink::new(window.clone())) {
            Ok(sink) => sink,
            Err(error) => {
                self.fail(event_loop, MobileError::Renderer(error));
                return;
            }
        };
        let pkg = match parse(&self.bytes) {
            Ok(pkg) => pkg,
            Err(error) => {
                self.fail(event_loop, MobileError::Load(error.to_string()));
                return;
            }
        };
        let host = match HostDocument::load(pkg, Box::new(sink), self.options) {
            Ok(host) => host,
            Err(error) => {
                self.fail(event_loop, MobileError::Host(error));
                return;
            }
        };
        let dpr = window.scale_factor() as f32;
        let size = window.inner_size();
        let viewport = input::viewport_for_size(size.width, size.height, dpr);
        eprintln!(
            "glyphcull-mobile: resumed window={}x{} dpr={} viewport={}x{}",
            size.width, size.height, dpr, viewport.w, viewport.h
        );
        let mut document = MobileDocument {
            window,
            host,
            viewport,
            dpr,
            canvas: (viewport.w, viewport.h),
            content_width: self.options.content_width,
            drag_anchor: None,
            last_cursor: None,
            touches: BTreeMap::new(),
            pinch_prev_dist: None,
            fling: None,
            last_frame: None,
            touch_samples: VecDeque::new(),
            clock: std::time::Instant::now(),
        };
        // Size the surface to the real window (the host's load used the
        // option defaults).
        if let Err(error) = document.scroll(viewport, Direction::Down) {
            self.fail(event_loop, MobileError::Host(error));
            return;
        }
        document.window.request_redraw();
        self.document = Some(document);
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // The native window (and with it the surface) is destroyed; the
        // document is rebuilt on the next resume.
        self.document = None;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(document) = self.document.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => document.on_resize(size.width, size.height),
            WindowEvent::MouseWheel { delta, .. } => document.on_wheel(delta),
            WindowEvent::Touch(touch) => document.on_touch(touch),
            WindowEvent::CursorMoved { position, .. } => {
                document.on_cursor_moved((position.x, position.y));
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => match state {
                ElementState::Pressed => document.on_drag_start(),
                ElementState::Released => document.on_drag_end(),
            },
            WindowEvent::KeyboardInput { event, .. } => {
                if event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                    && event.state == ElementState::Pressed
                {
                    event_loop.exit();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(error) = document.paint() {
                    eprintln!("glyphcull-mobile: paint: {error}");
                }
                #[cfg(target_os = "android")]
                document.dump_capture();
                document.drive_fling();
            }
            _ => {}
        }
    }
}

/// Run the mobile host over a packaged document (any platform: Android runs
/// it from `android_main` with the packaged bytes; desktop runs it for smoke
/// validation). Blocks until the window closes.
pub fn run_with(bytes: &[u8], options: HostOptions) -> Result<(), MobileError> {
    let event_loop = EventLoop::new().map_err(|e| MobileError::Window(e.to_string()))?;
    let mut app = MobileApp::new(bytes.to_vec(), options);
    event_loop
        .run_app(&mut app)
        .map_err(|e| MobileError::Window(e.to_string()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn touch_scroll_follows_the_finger() {
        // Finger moves up (y decreases): content follows → scroll down
        // (positive dy, in document px — the dpr already cancelled in the
        // cursor conversion).
        assert_eq!(scroll_delta_for_touch((0.0, 100.0), (0.0, 50.0)), 50.0);
        // Finger moves down: scroll up (negative).
        assert_eq!(scroll_delta_for_touch((0.0, 50.0), (0.0, 100.0)), -50.0);
        // Horizontal motion alone does not scroll vertically.
        assert_eq!(scroll_delta_for_touch((10.0, 50.0), (90.0, 50.0)), 0.0);
    }

    #[test]
    fn direction_follows_the_sign() {
        assert_eq!(direction_for_delta(1.0), Direction::Down);
        assert_eq!(direction_for_delta(-1.0), Direction::Up);
        // Zero delta defaults to Down (no motion — direction is moot).
        assert_eq!(direction_for_delta(0.0), Direction::Down);
    }
}
