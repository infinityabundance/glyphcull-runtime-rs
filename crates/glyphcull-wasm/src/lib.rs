//! glyphcull-wasm: the tiny wasm API — exactly `load`, `scroll`, `paint`,
//! `select`, `copy`, `destroy` (mirrors the JS `src/api/runtime.ts`).
//!
//! `load(bytes, canvas, options)` parses and validates the package, requests
//! a wgpu WebGPU adapter for the canvas, and returns a `GlyphCullDocument`
//! handle. `scroll` runs one bounded materialization cycle (the cooperative
//! per-call budget — no threads in wasm32); `paint` builds and plays the draw
//! list; `select` is hit-test based (point, two points, or a logical range);
//! `copy` extracts the selection's plain text; `destroy` releases everything
//! (idempotent; all other calls then throw a typed `destroyed` error).
//!
//! The host logic is platform-agnostic and native-tested (`host.rs`); this
//! module is a thin wasm-bindgen layer.

use js_sys::Reflect;
use wasm_bindgen::prelude::*;
#[cfg(web)]
use wasm_bindgen_futures::future_to_promise;
#[cfg(web)]
use web_sys::HtmlCanvasElement;

#[cfg(web)]
use glyphcull_core::document::build_document;
use glyphcull_core::materialize::Direction;
#[cfg(web)]
use glyphcull_core::reader::parse;
use glyphcull_core::selection::TextPosition;
use glyphcull_core::visibility::Viewport;

mod sink;

pub use glyphcull_host::{
    validate_options, FrameSink, HostDocument, HostError, HostOptions, DEFAULT_COOLING_MS,
    DEFAULT_FRAME_BUDGET_MS, DEFAULT_GLYPH_BUDGET, DEFAULT_MARGIN,
};
pub use sink::WgpuSink;

/// The wasm document handle: exactly the six runtime operations.
#[wasm_bindgen]
pub struct GlyphCullDocument {
    host: Option<HostDocument>,
}

#[wasm_bindgen]
impl GlyphCullDocument {
    /// Whether the handle has been destroyed.
    #[wasm_bindgen(getter)]
    pub fn destroyed(&self) -> bool {
        self.host.as_ref().is_none_or(|host| host.destroyed())
    }

    /// Move the viewport (document CSS pixels) and run one materialization
    /// cycle. `direction` (< 0 = up) is the travel direction for scheduling.
    pub fn scroll(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        direction: i32,
    ) -> Result<(), JsValue> {
        let host = self.require_alive()?;
        host.scroll(
            Viewport { x, y, w, h },
            if direction < 0 {
                Direction::Up
            } else {
                Direction::Down
            },
        )
        .map_err(js_err)
    }

    /// Render the current viewport + selection into the canvas.
    pub fn paint(&mut self) -> Result<(), JsValue> {
        self.require_alive()?.paint().map_err(js_err)
    }

    /// Select the text at a point (collapsed selection).
    pub fn select_point(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.require_alive()?.select_point(x, y).map_err(js_err)
    }

    /// Select between two document points (drag).
    pub fn select_between(&mut self, ax: f32, ay: f32, bx: f32, by: f32) -> Result<(), JsValue> {
        self.require_alive()?
            .select_between(ax, ay, bx, by)
            .map_err(js_err)
    }

    /// Select a logical range directly (normalized to document order).
    #[wasm_bindgen(js_name = selectRange)]
    pub fn select_range(
        &mut self,
        start_chunk: u32,
        start_offset: u32,
        end_chunk: u32,
        end_offset: u32,
    ) -> Result<(), JsValue> {
        self.require_alive()?
            .select_range(
                TextPosition {
                    chunk_id: start_chunk,
                    offset: start_offset as usize,
                },
                TextPosition {
                    chunk_id: end_chunk,
                    offset: end_offset as usize,
                },
            )
            .map_err(js_err)
    }

    /// The plain text of the current selection ('' when none/collapsed).
    pub fn copy(&self) -> Result<String, JsValue> {
        match self.host.as_ref() {
            Some(host) if !host.destroyed() => host.copy().map_err(js_err),
            _ => Err(js_err(HostError::Destroyed)),
        }
    }

    /// Release every resource; idempotent. All other calls then reject.
    pub fn destroy(&mut self) {
        if let Some(mut host) = self.host.take() {
            host.destroy();
        }
    }
}

impl GlyphCullDocument {
    fn require_alive(&mut self) -> Result<&mut HostDocument, JsValue> {
        match self.host.as_mut() {
            Some(host) if !host.destroyed() => Ok(host),
            _ => Err(js_err(HostError::Destroyed)),
        }
    }
}

/// Load a `.cull` package and construct its document handle.
///
/// `options` is a plain object with optional numeric fields: `dpr`,
/// `contentWidth`, `width`, `height`, `margin`, `glyphBudgetBytes`,
/// `frameBudgetMs`, `coolingPeriodMs`.
///
/// Web only: the canvas is the DOM canvas the surface binds to.
#[cfg(web)]
#[wasm_bindgen]
pub fn load(
    bytes: &[u8],
    canvas: HtmlCanvasElement,
    options: JsValue,
) -> Result<js_sys::Promise, JsValue> {
    let options = parse_options(&options)?;
    let bytes = bytes.to_vec();
    Ok(future_to_promise(async move {
        load_inner(&bytes, canvas, options).await
    }))
}

#[cfg(web)]
async fn load_inner(
    bytes: &[u8],
    canvas: HtmlCanvasElement,
    options: HostOptions,
) -> Result<JsValue, JsValue> {
    let pkg = parse(bytes).map_err(|e| js_err(HostError::Load(e.to_string())))?;
    // Typed validation (also guards the host's internal re-build).
    {
        let doc = build_document(&pkg).map_err(|e| js_err(HostError::Load(e.to_string())))?;
        let _ = doc.all_ids().len();
    }

    // The wgpu renderer: instance → surface → adapter → device (WebGPU in
    // the browser; GL on native).
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
        ..Default::default()
    });
    // The canvas is passed by value: wgpu's `SurfaceTarget::Canvas` keeps its
    // own copy of the handle alive, so the surface is `'static` (which is what
    // the sink stores). Borrowing the canvas here would produce a surface tied
    // to this function's frame and could not be stored.
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| js_err(HostError::Renderer(e.to_string())))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        })
        .await
        .map_err(|e| js_err(HostError::Renderer(format!("renderer-unavailable: {e}"))))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("glyphcull-wasm"),
            ..Default::default()
        })
        .await
        .map_err(|e| js_err(HostError::Renderer(e.to_string())))?;
    let format = surface
        .get_capabilities(&adapter)
        .formats
        .iter()
        .copied()
        .find(|format| !format.is_srgb())
        .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);
    let sink = WgpuSink::from_device(device, queue, Some(surface), format);
    let host = HostDocument::load(pkg, Box::new(sink), options).map_err(js_err)?;
    Ok(JsValue::from(GlyphCullDocument { host: Some(host) }))
}

/// Read the load options object (all fields optional numbers).
#[cfg(web)]
fn parse_options(options: &JsValue) -> Result<HostOptions, JsValue> {
    let get = |name: &str| -> Result<Option<f64>, JsValue> {
        if options.is_undefined() || options.is_null() {
            return Ok(None);
        }
        let value = Reflect::get(options, &JsValue::from_str(name)).map_err(|_| {
            js_err(HostError::InvalidOptions(format!(
                "{name} could not be read"
            )))
        })?;
        if value.is_undefined() || value.is_null() {
            Ok(None)
        } else {
            value
                .as_f64()
                .ok_or_else(|| {
                    js_err(HostError::InvalidOptions(format!(
                        "{name} must be a number"
                    )))
                })
                .map(Some)
        }
    };
    let dpr = get("dpr")?.unwrap_or(1.0) as f32;
    let width = get("width")?.unwrap_or(800.0);
    let height = get("height")?.unwrap_or(600.0);
    let content_width = get("contentWidth")?.unwrap_or(800.0);
    let margin = get("margin")?.unwrap_or(DEFAULT_MARGIN as f64);
    let glyph_budget_bytes = get("glyphBudgetBytes")?.unwrap_or(DEFAULT_GLYPH_BUDGET as f64);
    let frame_budget_ms = get("frameBudgetMs")?.unwrap_or(DEFAULT_FRAME_BUDGET_MS as f64);
    let cooling_period_ms = get("coolingPeriodMs")?.unwrap_or(DEFAULT_COOLING_MS as f64);
    let options = HostOptions {
        dpr,
        content_width: content_width as f32,
        viewport: Viewport {
            x: 0.0,
            y: 0.0,
            w: width as f32,
            h: height as f32,
        },
        margin: margin as f32,
        glyph_budget_bytes: glyph_budget_bytes as u64,
        frame_budget_ms: frame_budget_ms as u64,
        cooling_period_ms: cooling_period_ms as u64,
    };
    validate_options(&options).map_err(js_err)?;
    Ok(options)
}

/// A JS `Error` with a `.kind` property (mirrors the JS typed errors).
fn js_err(error: HostError) -> JsValue {
    let kind = match &error {
        HostError::InvalidOptions(_) => "invalid-options",
        HostError::Destroyed => "destroyed",
        HostError::Load(_) => "load",
        HostError::Renderer(_) => "renderer",
    };
    let error_object = js_sys::Error::new(&error.to_string());
    let _ = Reflect::set(
        &error_object,
        &JsValue::from_str("kind"),
        &JsValue::from_str(kind),
    );
    error_object.into()
}
