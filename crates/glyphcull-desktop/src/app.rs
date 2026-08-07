//! The winit host: the event loop, window, input wiring, and the
//! [`DesktopDocument`] — the desktop face of the six-operation runtime API.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

use glyphcull_core::materialize::Direction;
use glyphcull_core::reader::parse;
use glyphcull_core::selection::TextPosition;
use glyphcull_core::visibility::Viewport;
use glyphcull_host::{HostDocument, HostError, HostOptions};
use glyphcull_render::renderer::straighten_premultiplied_rgba8;

use crate::input::{self, WheelDelta};
use crate::sink::DesktopSink;

/// A typed desktop host error.
#[derive(Debug)]
pub enum DesktopError {
    /// The event loop or window failed.
    Window(String),
    /// The wgpu renderer failed to initialize or draw.
    Renderer(String),
    /// The package failed to parse or validate.
    Load(String),
    /// The host rejected the operation.
    Host(HostError),
}

impl fmt::Display for DesktopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Window(message) => write!(f, "window: {message}"),
            Self::Renderer(message) => write!(f, "renderer: {message}"),
            Self::Load(message) => write!(f, "load: {message}"),
            Self::Host(error) => write!(f, "host: {error}"),
        }
    }
}

impl std::error::Error for DesktopError {}

impl From<HostError> for DesktopError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

/// The desktop document: a winit window serving the six-operation runtime API
/// (`load`/`scroll`/`paint`/`select`/`copy`/`destroy`) over a wgpu surface.
pub struct DesktopDocument {
    /// The window (also kept alive by the sink's surface).
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
}

impl DesktopDocument {
    /// Load a `.cull` package into a new window-backed document.
    ///
    /// The window is created by the caller (the event loop's `resumed`); this
    /// blocks on wgpu initialization (adapter/device) once.
    pub fn load(
        window: Arc<Window>,
        bytes: &[u8],
        options: HostOptions,
    ) -> Result<Self, DesktopError> {
        let pkg = parse(bytes).map_err(|e| DesktopError::Load(e.to_string()))?;
        let sink =
            pollster::block_on(DesktopSink::new(window.clone())).map_err(DesktopError::Renderer)?;
        let host = HostDocument::load(pkg, Box::new(sink), options)?;
        let dpr = window.scale_factor() as f32;
        let size = window.inner_size();
        let viewport = input::viewport_for_size(size.width, size.height, dpr);
        let mut document = Self {
            window,
            host,
            viewport,
            dpr,
            drag_anchor: None,
            last_cursor: None,
        };
        // Size the surface to the real window (the host's load used the
        // option defaults).
        document.scroll(document.viewport, Direction::Down)?;
        Ok(document)
    }

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

    /// Release every resource; idempotent.
    pub fn destroy(&mut self) {
        self.host.destroy();
    }

    /// Capture the last presented frame as tightly packed premultiplied RGBA8
    /// (device pixels) — the host diagnostic behind the binary's
    /// `--screenshot` mode (DESIGN.md D31). Returns `None` if no frame has
    /// been drawn yet or the capture fails.
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

/// The winit application: owns the bytes until the window exists, then the
/// document.
struct DesktopApp {
    bytes: Vec<u8>,
    options: HostOptions,
    window_size: Option<(u32, u32)>,
    scroll_y: Option<f32>,
    screenshot: Option<PathBuf>,
    screenshot_taken: bool,
    document: Option<DesktopDocument>,
}

impl DesktopApp {
    /// Write the current frame as a straight-RGBA8 PNG (the premultiplied
    /// capture is un-premultiplied to the PNG storage convention).
    fn write_screenshot(&mut self) -> Result<(), DesktopError> {
        let document = self.document.as_mut().ok_or_else(|| {
            DesktopError::Renderer("no frame to capture (no document)".to_string())
        })?;
        let rgba = document.capture_last_frame().ok_or_else(|| {
            DesktopError::Renderer("frame capture failed or no frame drawn".to_string())
        })?;
        let size = document.window.inner_size();
        let straight = straighten_premultiplied_rgba8(&rgba);
        let path = self
            .screenshot
            .clone()
            .ok_or_else(|| DesktopError::Renderer("screenshot path missing".to_string()))?;
        encode_png(&straight, size.width, size.height, &path)?;
        self.screenshot_taken = true;
        eprintln!(
            "glyphcull-desktop: wrote {} ({}×{} device px)",
            path.display(),
            size.width,
            size.height
        );
        Ok(())
    }
}

impl ApplicationHandler for DesktopApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.document.is_some() {
            return;
        }
        let mut attributes = Window::default_attributes().with_title("GlyphCull");
        if let Some((width, height)) = self.window_size {
            // `--size` pins the physical window size; the smoke harness pairs
            // this with a forced dpr of 1 so the capture is 1:1 with the
            // golden geometry.
            attributes = attributes.with_inner_size(PhysicalSize::new(width, height));
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("glyphcull-desktop: window: {error}");
                event_loop.exit();
                return;
            }
        };
        match DesktopDocument::load(window, &self.bytes, self.options) {
            Ok(mut document) => {
                // `--scroll`: move the viewport before the first paint so the
                // capture shows a scrolled frame (smoke validation of the
                // streamed/imagery content).
                if let Some(y) = self.scroll_y {
                    let viewport = Viewport {
                        y,
                        ..document.viewport()
                    };
                    if let Err(error) = document.scroll(viewport, Direction::Down) {
                        eprintln!("glyphcull-desktop: scroll: {error}");
                        event_loop.exit();
                        return;
                    }
                }
                document.window.request_redraw();
                self.document = Some(document);
            }
            Err(error) => {
                eprintln!("glyphcull-desktop: {error}");
                event_loop.exit();
            }
        }
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
                    eprintln!("glyphcull-desktop: paint: {error}");
                }
                // `--screenshot`: capture the frame just drawn, then exit.
                if self.screenshot.is_some() && !self.screenshot_taken {
                    match self.write_screenshot() {
                        Ok(()) => event_loop.exit(),
                        Err(error) => {
                            eprintln!("glyphcull-desktop: {error}");
                            event_loop.exit();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// How the desktop host launches: the optional window size, the optional
/// `--screenshot` target, and the optional initial scroll (the smoke/validation
/// mode).
#[derive(Debug, Clone, Default)]
pub struct HostLaunch {
    /// Force the physical window inner size (device pixels).
    pub window_size: Option<(u32, u32)>,
    /// Capture the first painted frame to this PNG path, then exit.
    pub screenshot: Option<PathBuf>,
    /// Scroll the viewport to this document y before the first paint.
    pub scroll_y: Option<f32>,
}

/// Open a window for a `.cull` package and run the event loop until the
/// window closes. `options` seeds the host (the viewport is then taken from
/// the real window size).
pub fn run(bytes: &[u8], options: HostOptions) -> Result<(), DesktopError> {
    run_with(bytes, options, HostLaunch::default())
}

/// [`run`] with an explicit launch configuration (window size, screenshot).
pub fn run_with(
    bytes: &[u8],
    options: HostOptions,
    launch: HostLaunch,
) -> Result<(), DesktopError> {
    let event_loop = EventLoop::new().map_err(|e| DesktopError::Window(e.to_string()))?;
    let mut app = DesktopApp {
        bytes: bytes.to_vec(),
        options,
        window_size: launch.window_size,
        scroll_y: launch.scroll_y,
        screenshot: launch.screenshot,
        screenshot_taken: false,
        document: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| DesktopError::Window(e.to_string()))
}

/// Encode straight RGBA8 as a PNG at `path` (the `png` crate's fast path;
/// deterministic encoder settings).
fn encode_png(
    rgba: &[u8],
    width: u32,
    height: u32,
    path: &std::path::Path,
) -> Result<(), DesktopError> {
    let file = std::fs::File::create(path)
        .map_err(|e| DesktopError::Renderer(format!("create {path:?}: {e}")))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|e| DesktopError::Renderer(format!("png header: {e}")))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| DesktopError::Renderer(format!("png data: {e}")))?;
    Ok(())
}
