//! The native wgpu [`FrameSink`] for the desktop host: uploads atlas pages
//! and images to the renderer and plays render plans back into the window's
//! surface.
//!
//! The window is passed by value at creation (via `Arc<Window>`): wgpu's
//! `SurfaceTarget::Window` keeps its own copy of the handle alive, so the
//! surface is `'static` — the same ownership pattern as the wasm canvas
//! (DESIGN.md D29). RGB8 images are padded to RGBA8 at upload (wgpu has no
//! RGB8 texture format); the surface is configured with a non-sRGB format
//! (DESIGN.md D28) matching the renderer's pipeline.

use std::sync::Arc;

use winit::window::Window;

use glyphcull_host::FrameSink;
use glyphcull_render::plan::{RenderPlan, RendererViewport};
use glyphcull_render::renderer::{Renderer, DEFAULT_TEXTURE_BUDGET};

/// The wgpu host sink over a winit window (WebGPU/GL on desktop).
pub(crate) struct DesktopSink {
    renderer: Renderer,
    surface: wgpu::Surface<'static>,
    format: wgpu::TextureFormat,
}

impl DesktopSink {
    /// Create the sink: instance → surface (from the window) → adapter →
    /// device. Call from a context that can block (the event loop's
    /// `resumed`); the caller owns the block via `pollster`.
    pub(crate) async fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY | wgpu::Backends::GL,
            ..Default::default()
        });
        let surface = instance
            .create_surface(window)
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: Some(&surface),
            })
            .await
            .map_err(|e| format!("adapter: {e}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("glyphcull-desktop"),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("device: {e}"))?;
        // The pipeline requires a non-sRGB target (DESIGN.md D28) so the
        // premultiplied colors and the MSDF distance channels stay raw.
        let format = surface
            .get_capabilities(&adapter)
            .formats
            .iter()
            .copied()
            .find(|format| !format.is_srgb())
            .unwrap_or(wgpu::TextureFormat::Rgba8Unorm);
        Ok(Self {
            renderer: Renderer::from_device(device, queue, format, DEFAULT_TEXTURE_BUDGET),
            surface,
            format,
        })
    }
}

impl FrameSink for DesktopSink {
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
        let frame = self
            .surface
            .get_current_texture()
            .map_err(|e| e.to_string())?;
        let view = frame.texture.create_view(&Default::default());
        self.renderer.draw(plan, &view);
        frame.present();
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.surface.configure(
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

    fn destroy(&mut self) {
        self.renderer.destroy();
    }
}
