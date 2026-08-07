//! The native wgpu [`FrameSink`] for the winit host (desktop **and** Android
//! — the mobile crate reuses it): uploads atlas pages and images to the
//! renderer and plays render plans back into the window's surface.
//!
//! The surface is **re-attachable** (DESIGN.md D29): it is created from the
//! window (wgpu's `SurfaceTarget::Window` keeps its own copy of the handle
//! alive, so the surface is `'static`) but stored in an `Option`, so a host
//! can `detach()` it when the native window is destroyed (Android
//! `suspended`) and `attach()` a fresh one when a new window arrives
//! (`resumed`). A detached sink still serves offscreen capture (DESIGN.md
//! D31); `draw` reports "no surface" instead of touching a stale handle.
//! RGB8 images are padded to RGBA8 at upload (wgpu has no RGB8 texture
//! format); the surface is configured with a non-sRGB format (DESIGN.md D28)
//! matching the renderer's pipeline.

use std::sync::Arc;

use winit::window::Window;

use glyphcull_host::FrameSink;
use glyphcull_render::plan::{RenderPlan, RendererViewport};
use glyphcull_render::renderer::{Renderer, DEFAULT_TEXTURE_BUDGET};

/// The wgpu backend mask: `GLYPHCULL_WGPU_BACKEND` (auto|vulkan|gl) overrides
/// the default (Vulkan + GL fallback). A device diagnostic — e.g. the Android
/// emulator's SwiftShader Vulkan present path can render black while the GLES
/// path works; the wasm binding has the equivalent `backend` load option.
fn backend_mask() -> wgpu::Backends {
    match std::env::var("GLYPHCULL_WGPU_BACKEND").as_deref() {
        Ok("vulkan") => wgpu::Backends::VULKAN,
        Ok("gl") => wgpu::Backends::GL,
        _ => wgpu::Backends::PRIMARY | wgpu::Backends::GL,
    }
}

/// The wgpu host sink over a winit window (WebGPU/GL on desktop and Android).
pub struct DesktopSink {
    /// The wgpu instance, kept for `attach` (a surface is created from it).
    instance: wgpu::Instance,
    /// The chosen adapter, kept for `attach` (surface capabilities).
    adapter: wgpu::Adapter,
    renderer: Renderer,
    /// The surface, when a window is attached. `None` after `detach` (Android
    /// suspend) or before the first `attach` (unsurfaced init).
    surface: Option<wgpu::Surface<'static>>,
    /// The surface format the pipeline writes (updated by `attach` when the
    /// attached surface prefers a different non-sRGB format).
    format: wgpu::TextureFormat,
    /// The surface size in device pixels (updated on resize).
    size: (u32, u32),
    /// The last drawn plan, re-rendered offscreen for capture (DESIGN.md D31)
    /// — rendering is deterministic, so the capture equals the presented
    /// frame, and it works while the sink is detached.
    last_plan: Option<RenderPlan>,
}

impl DesktopSink {
    /// Create the sink for a window: instance → surface (from the window) →
    /// adapter (compatible with the surface) → device, with the surface
    /// attached. Call from a context that can block (the event loop's
    /// `resumed`); the caller owns the block via `pollster`.
    pub async fn new(window: Arc<Window>) -> Result<Self, String> {
        // Read the size first: `create_surface(window)` moves the Arc into
        // the `'static` surface (the window handle keeps it alive).
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: backend_mask(),
            ..Default::default()
        });
        let surface = instance
            .create_surface(window)
            .map_err(|e| format!("surface: {e}"))?;
        let (adapter, format) = Self::request_adapter(&instance, Some(&surface)).await?;
        let capabilities = surface.get_capabilities(&adapter);
        eprintln!(
            "glyphcull-desktop: backend={:?} surface formats={:?} present={:?}",
            adapter.get_info().backend,
            capabilities.formats,
            capabilities.present_modes
        );
        let (device, queue) = Self::request_device(&adapter).await?;
        let renderer = Renderer::from_device(device, queue, format, DEFAULT_TEXTURE_BUDGET);
        Ok(Self {
            instance,
            adapter,
            renderer,
            surface: Some(surface),
            format,
            size: (size.width, size.height),
            last_plan: None,
        })
    }

    /// Create the sink without a surface: instance → adapter → device only.
    /// The pipeline is built against a provisional non-sRGB format; the first
    /// [`Self::attach`] retargets it to the surface's real format when they
    /// differ. This is the mobile path — on Android the window (and its
    /// surface) only exist between `resumed` and `suspended`, so the sink is
    /// initialized once and the surface is attached per resume.
    pub async fn new_unsurfaced() -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY | wgpu::Backends::GL,
            ..Default::default()
        });
        let (adapter, _) = Self::request_adapter(&instance, None).await?;
        let (device, queue) = Self::request_device(&adapter).await?;
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let renderer = Renderer::from_device(device, queue, format, DEFAULT_TEXTURE_BUDGET);
        Ok(Self {
            instance,
            adapter,
            renderer,
            surface: None,
            format,
            size: (0, 0),
            last_plan: None,
        })
    }

    /// Attach (or re-attach) the surface for `window`: create the surface from
    /// the window, match the pipeline to the surface's preferred non-sRGB
    /// format (retargeting when it differs), and configure it at the window's
    /// current size. Idempotent per window: repeated attaches replace the
    /// previous surface (Android resume after suspend).
    pub fn attach(&mut self, window: Arc<Window>) -> Result<(), String> {
        let size = window.inner_size();
        let surface = self
            .instance
            .create_surface(window)
            .map_err(|e| format!("surface: {e}"))?;
        let capabilities = surface.get_capabilities(&self.adapter);
        // Prefer the pipeline's current target format when the surface
        // supports it (the common case: the same window class reports the
        // same format, so no pipeline rebuild); otherwise pick the first
        // non-sRGB format and retarget the pipeline — a genuinely different
        // surface (another display, or a first attach after unsurfaced init).
        let format = choose_format(&capabilities.formats, self.renderer.target_format)
            .ok_or_else(|| "surface offers no non-sRGB format".to_string())?;
        self.renderer.retarget_target_format(format);
        self.format = format;
        self.size = (size.width, size.height);
        self.surface = Some(surface);
        self.configure();
        eprintln!(
            "glyphcull-desktop: attach window={}x{} format={:?} offered={:?}",
            size.width, size.height, format, capabilities.formats
        );
        Ok(())
    }

    /// Drop the surface (Android `suspended`: the native window is destroyed,
    /// so the handle must not be touched again). The renderer, textures, and
    /// the last plan survive; offscreen capture still works. The next
    /// [`Self::attach`] re-creates and re-configures the surface.
    pub fn detach(&mut self) {
        self.surface = None;
    }

    /// The surface size in device pixels of the last attach/resize, if a
    /// surface is attached.
    #[must_use]
    pub const fn surface_size(&self) -> Option<(u32, u32)> {
        match &self.surface {
            Some(_) => Some(self.size),
            None => None,
        }
    }

    /// Request the adapter: with a surface when one exists (the compatible
    /// adapter — the desktop path), by power preference otherwise (the
    /// unsurfaced path; the first attach then picks the format).
    async fn request_adapter(
        instance: &wgpu::Instance,
        surface: Option<&wgpu::Surface<'static>>,
    ) -> Result<(wgpu::Adapter, wgpu::TextureFormat), String> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: surface,
            })
            .await
            .map_err(|e| format!("adapter: {e}"))?;
        let format = match surface {
            // The pipeline requires a non-sRGB target (DESIGN.md D28) so the
            // premultiplied colors and the MSDF distance channels stay raw.
            Some(surface) => surface
                .get_capabilities(&adapter)
                .formats
                .iter()
                .copied()
                .find(|format| !format.is_srgb())
                .unwrap_or(wgpu::TextureFormat::Rgba8Unorm),
            None => wgpu::TextureFormat::Rgba8Unorm,
        };
        Ok((adapter, format))
    }

    async fn request_device(
        adapter: &wgpu::Adapter,
    ) -> Result<(wgpu::Device, wgpu::Queue), String> {
        // GL adapters expose downlevel limits — SwiftShader reports no compute
        // support, so `Limits::default()` makes device creation fail (the
        // wasm binding hits the same wall and uses the downlevel defaults;
        // this is the parity fix). WebGPU keeps the full limits.
        let limits = if adapter.get_info().backend == wgpu::Backend::Gl {
            wgpu::Limits::downlevel_webgl2_defaults()
        } else {
            wgpu::Limits::default()
        };
        adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("glyphcull-desktop"),
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("device: {e}"))
    }

    /// (Re)configure the attached surface at the current size. No-op while
    /// detached (the next attach configures).
    fn configure(&mut self) {
        let Some(surface) = &self.surface else {
            return;
        };
        let (width, height) = self.size;
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
}

/// Choose the surface format for an attach: the pipeline's current target
/// format when the surface supports it (no pipeline rebuild), else the first
/// non-sRGB format (the pipeline retargets), else `None` (no compatible
/// format — the MSDF pipeline is non-sRGB only, DESIGN.md D28). Pure
/// (unit-tested); the GPU path only provides the capability list.
fn choose_format(
    formats: &[wgpu::TextureFormat],
    current: wgpu::TextureFormat,
) -> Option<wgpu::TextureFormat> {
    if formats.contains(&current) {
        return Some(current);
    }
    formats.iter().copied().find(|format| !format.is_srgb())
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
        // Keep the plan for capture (DESIGN.md D31): the capture re-renders
        // it offscreen, which is byte-deterministic.
        self.last_plan = Some(plan.clone());
        let Some(surface) = &self.surface else {
            return Err("no surface attached (suspended)".to_string());
        };
        let frame = surface.get_current_texture().map_err(|e| e.to_string())?;
        let view = frame.texture.create_view(&Default::default());
        self.renderer.draw(plan, &view);
        frame.present();
        Ok(())
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.size = (width.max(1), height.max(1));
        self.configure();
    }

    /// Synchronous capture: the renderer's `render_to_rgba` (native-only —
    /// wasm cannot block on `device.poll(Wait)`; the wasm binding maps the
    /// readback asynchronously via `capture_readback` instead).
    #[cfg(not(target_family = "wasm"))]
    fn capture(&mut self) -> Option<Vec<u8>> {
        let plan = self.last_plan.as_ref()?;
        let (width, height) = self.size;
        self.renderer
            .render_to_rgba(plan, width, height)
            .map_err(|e| {
                eprintln!("glyphcull-desktop: capture: {e}");
                e
            })
            .ok()
    }

    /// The un-mapped offscreen readback of the last plan (all backends):
    /// native hosts could map it synchronously; the wasm binding maps it
    /// asynchronously. Reading `last_plan` here also keeps the plan alive on
    /// wasm builds (where `capture` is native-only).
    fn capture_readback(&mut self) -> Option<glyphcull_render::renderer::FrameReadback> {
        let plan = self.last_plan.as_ref()?;
        let (width, height) = self.size;
        if width == 0 || height == 0 {
            return None;
        }
        self.renderer
            .render_to_readback(plan, width, height)
            .map_err(|e| {
                eprintln!("glyphcull-desktop: capture_readback: {e}");
                e
            })
            .ok()
    }

    fn destroy(&mut self) {
        self.renderer.destroy();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn prefers_the_pipeline_target_format_when_supported() {
        let formats = [
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Bgra8Unorm,
        ];
        // The pipeline already targets the non-sRGB surface format: attach
        // keeps it (no pipeline rebuild).
        assert_eq!(
            choose_format(&formats, wgpu::TextureFormat::Bgra8Unorm),
            Some(wgpu::TextureFormat::Bgra8Unorm)
        );
    }

    #[test]
    fn falls_back_to_the_first_non_srgb_format() {
        // The pipeline targets a format this surface does not offer (e.g. an
        // unsurfaced init's provisional format): pick the first non-sRGB and
        // retarget.
        let formats = [
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm,
        ];
        assert_eq!(
            choose_format(&formats, wgpu::TextureFormat::Bgra8Unorm),
            Some(wgpu::TextureFormat::Rgba8Unorm)
        );
    }

    #[test]
    fn no_non_srgb_format_is_an_error() {
        let formats = [wgpu::TextureFormat::Rgba8UnormSrgb];
        assert_eq!(
            choose_format(&formats, wgpu::TextureFormat::Bgra8Unorm),
            None
        );
    }

    #[test]
    fn empty_capabilities_are_an_error() {
        assert_eq!(choose_format(&[], wgpu::TextureFormat::Bgra8Unorm), None);
    }
}
