//! The wgpu [`FrameSink`] for the wasm host: uploads atlas pages and images
//! to the renderer and plays render plans back into the canvas surface.
//!
//! RGB8 images are padded to RGBA8 at upload (wgpu has no RGB8 texture
//! format; the shared `glyphcull_host::pad_rgb8_to_rgba8` does the padding);
//! the surface is configured with a non-sRGB format (DESIGN.md D28) matching
//! the renderer's pipeline.

use glyphcull_render::plan::{RenderPlan, RendererViewport};
use glyphcull_render::renderer::{Renderer, DEFAULT_TEXTURE_BUDGET};

use glyphcull_host::FrameSink;

/// The wgpu host sink (WebGPU backend in the browser; GL on native).
pub struct WgpuSink {
    renderer: Renderer,
    surface: Option<wgpu::Surface<'static>>,
    format: wgpu::TextureFormat,
    /// The surface size in device pixels (set by the host's resize calls).
    size: (u32, u32),
    /// The last drawn plan, re-rendered offscreen for capture (DESIGN.md D31)
    /// — rendering is deterministic, so the capture equals the presented
    /// frame without touching the DOM canvas (which headless Chromium cannot
    /// read back; D10).
    last_plan: Option<RenderPlan>,
}

impl WgpuSink {
    /// Create a sink over a configured renderer + surface.
    #[must_use]
    pub fn new(
        renderer: Renderer,
        surface: Option<wgpu::Surface<'static>>,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self {
            renderer,
            surface,
            format,
            size: (0, 0),
            last_plan: None,
        }
    }

    /// Build a sink from an existing device/queue and surface (hosts that
    /// request the adapter themselves for the canvas).
    #[must_use]
    pub fn from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        surface: Option<wgpu::Surface<'static>>,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self::new(
            Renderer::from_device(device, queue, format, DEFAULT_TEXTURE_BUDGET),
            surface,
            format,
        )
    }
}

impl FrameSink for WgpuSink {
    fn upload_atlas_page(
        &mut self,
        atlas_id: u32,
        page_index: u16,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> u32 {
        self.renderer
            .upload_atlas_page(atlas_id, page_index, pixels, width, height)
    }

    fn upload_image(
        &mut self,
        image_id: u32,
        pixels: &[u8],
        width: u32,
        height: u32,
        rgb: bool,
    ) -> u32 {
        if rgb {
            // RGB8 → RGBA8 (alpha 255): wgpu has no RGB8 texture format.
            let rgba = glyphcull_host::pad_rgb8_to_rgba8(pixels);
            self.renderer.upload_image(image_id, &rgba, width, height)
        } else {
            self.renderer.upload_image(image_id, pixels, width, height)
        }
    }

    fn draw(
        &mut self,
        plan: &RenderPlan,
        _viewport: RendererViewport,
        _surface_w: f32,
        _surface_h: f32,
    ) -> Result<(), String> {
        // Keep the plan for capture (DESIGN.md D31): the capture re-renders
        // it offscreen, which is byte-deterministic and independent of the
        // DOM canvas.
        self.last_plan = Some(plan.clone());
        let Some(surface) = &self.surface else {
            // Headless mode (the `loadHeadless` binding path): there is no
            // canvas to present — the frame exists only as the stored plan,
            // which the offscreen capture reads. Never touching a surface
            // also keeps the device surface-free, so `mapAsync` works even
            // on the SwiftShader builds where a presented device rejects it
            // (DESIGN.md D10).
            return Ok(());
        };
        let frame = surface.get_current_texture().map_err(|e| e.to_string())?;
        let view = frame.texture.create_view(&Default::default());
        self.renderer.draw(plan, &view);
        frame.present();
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
        let Some(surface) = &self.surface else {
            return;
        };
        surface.configure(
            self.renderer.device(),
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: self.format,
                width: width.max(1),
                height: height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Auto,
                view_formats: Vec::new(),
                desired_maximum_frame_latency: 2,
            },
        );
    }

    fn capture(&mut self) -> Option<Vec<u8>> {
        // The web cannot block on `device.poll(Wait)`; the binding maps the
        // readback asynchronously via `capture_readback`.
        None
    }

    fn capture_readback(&mut self) -> Option<glyphcull_render::renderer::FrameReadback> {
        let plan = self.last_plan.as_ref()?;
        let (width, height) = self.size;
        if width == 0 || height == 0 {
            return None;
        }
        self.renderer
            .render_to_readback(plan, width, height)
            .map_err(|e| {
                eprintln!("glyphcull-wasm: capture: {e}");
                e
            })
            .ok()
    }

    fn destroy(&mut self) {
        self.renderer.destroy();
    }
}
