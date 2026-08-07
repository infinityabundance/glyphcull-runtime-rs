//! The mobile application: the winit host over the shared [`DesktopSink`]
//! (Android; the same code also runs on desktop for tests and smoke runs).
//!
//! The Android lifecycle drives the document: `resumed` (foreground) builds a
//! window + sink + host from the packaged bytes; `suspended` (background)
//! drops them — the native window, and therefore the surface, is destroyed on
//! suspend, and rebuilding on resume is the canonical Android GPU lifecycle
//! (the wgpu examples do the same). Input: wheel (mice/trackpads) and touch
//! drag (content follows the finger) scroll; left-drag selects.

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
/// Document px = physical px ÷ dpr. Pure (unit-tested).
#[must_use]
pub fn scroll_delta_for_touch(prev: (f32, f32), cur: (f32, f32), dpr: f32) -> f32 {
    (prev.1 - cur.1) / dpr
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
/// (`load`/`scroll`/`paint`/`select`/`copy`/`destroy`) over the shared sink.
pub struct MobileDocument {
    window: Arc<Window>,
    host: HostDocument,
    /// The current viewport in document pixels.
    viewport: Viewport,
    /// The device pixel ratio (physical → logical).
    dpr: f32,
    /// The drag anchor in document space while a drag is in progress.
    drag_anchor: Option<(f32, f32)>,
    /// The last cursor position in physical pixels.
    last_cursor: Option<(f64, f64)>,
    /// The active touch's id + last document-space position (touch drag
    /// scrolls; only the first touch is tracked).
    touch_anchor: Option<(u64, (f32, f32))>,
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

    /// Move the viewport and run one materialization cycle.
    pub fn scroll(&mut self, viewport: Viewport, direction: Direction) -> Result<(), HostError> {
        self.viewport = viewport;
        self.host.scroll(viewport, direction)
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

    // --- input wiring (winit events → the operations) ---

    fn on_resize(&mut self, width: u32, height: u32) {
        self.dpr = self.window.scale_factor() as f32;
        let mut viewport = self.viewport;
        viewport.w = (width as f32) / self.dpr;
        viewport.h = (height as f32) / self.dpr;
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
                if self.touch_anchor.is_none() {
                    self.touch_anchor = Some((touch.id, position));
                }
            }
            TouchPhase::Moved => {
                let Some((id, previous)) = self.touch_anchor else {
                    return;
                };
                if id != touch.id {
                    return;
                }
                let dy = scroll_delta_for_touch(previous, position, self.dpr);
                self.touch_anchor = Some((touch.id, position));
                if dy == 0.0 {
                    return;
                }
                let viewport = input::scrolled_viewport(self.viewport, dy);
                if self.scroll(viewport, direction_for_delta(dy)).is_ok() {
                    self.window.request_redraw();
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                if self.touch_anchor.is_some_and(|(id, _)| id == touch.id) {
                    self.touch_anchor = None;
                }
            }
        }
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
        let mut document = MobileDocument {
            window,
            host,
            viewport,
            dpr,
            drag_anchor: None,
            last_cursor: None,
            touch_anchor: None,
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
        // (positive dy, in document px after the dpr division).
        assert_eq!(scroll_delta_for_touch((0.0, 100.0), (0.0, 50.0), 2.0), 25.0);
        // Finger moves down: scroll up (negative).
        assert_eq!(
            scroll_delta_for_touch((0.0, 50.0), (0.0, 100.0), 2.0),
            -25.0
        );
        // Horizontal motion alone does not scroll vertically.
        assert_eq!(scroll_delta_for_touch((10.0, 50.0), (90.0, 50.0), 1.0), 0.0);
    }

    #[test]
    fn direction_follows_the_sign() {
        assert_eq!(direction_for_delta(1.0), Direction::Down);
        assert_eq!(direction_for_delta(-1.0), Direction::Up);
        // Zero delta defaults to Down (no motion — direction is moot).
        assert_eq!(direction_for_delta(0.0), Direction::Down);
    }
}
