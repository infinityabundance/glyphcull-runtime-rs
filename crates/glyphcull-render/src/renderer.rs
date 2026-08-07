//! The wgpu MSDF renderer (Architecture.md §4; the Rust counterpart of the
//! JS `src/render/gl.ts`): executes a [`RenderPlan`] — the compiled draw
//! list — against a render target, with texture uploads
//! (atlas pages + images, budgeted), premultiplied-alpha blending, and
//! device-loss recovery (the host re-creates the renderer and re-uploads from
//! the core model).
//!
//! wgpu with the WebGPU and GL (WebGL2 via EGL) backends; the MSDF program is
//! the single WGSL source in [`crate::shader`] translated to every backend by
//! naga. The renderer targets a non-sRGB texture format so the premultiplied
//! u32 colors and gamma-space blending match the JS WebGL renderer exactly
//! (DESIGN.md D28): the MSDF distance channels stay linear, and the shader
//! output is stored raw.
//!
//! The renderer owns the device/queue/pipeline/textures; the host owns the
//! surface (winit/wasm) and passes a target view per frame.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
// The synchronous readback map (native only): wasm maps asynchronously.
#[cfg(not(target_family = "wasm"))]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(not(target_family = "wasm"))]
use std::time::{Duration, Instant};

use crate::plan::{PlanOp, RenderPlan};
use crate::shader::MSDF_WGSL;

/// The readback row alignment wgpu requires for buffer→texture copies
/// (COPY_BYTES_PER_ROW_ALIGNMENT); rows are padded to it and compacted on
/// read.
const COPY_BYTES_PER_ROW_ALIGNMENT: u32 = 256;

/// Premultiply RGBA8 texels in place of a copy (alpha-scaled channels). The
/// MSDF program treats `texture.rgb` as a coverage field, so a transparent
/// texel must carry zero-valued channels; this matches the JS WebGL renderer's
/// `UNPACK_PREMULTIPLY_ALPHA_WEBGL` upload convention.
fn premultiply_rgba8(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for chunk in pixels.chunks_exact(4) {
        let a = chunk.get(3).copied().unwrap_or(0);
        let scale = u16::from(a);
        out.push(((u16::from(chunk.first().copied().unwrap_or(0)) * scale) / 255) as u8);
        out.push(((u16::from(chunk.get(1).copied().unwrap_or(0)) * scale) / 255) as u8);
        out.push(((u16::from(chunk.get(2).copied().unwrap_or(0)) * scale) / 255) as u8);
        out.push(a);
    }
    out
}

/// The copy row byte width for a texture of `width` pixels (4 bytes/pixel),
/// padded to wgpu's COPY_BYTES_PER_ROW_ALIGNMENT.
#[must_use]
pub fn align_row_bytes(width: u32) -> u32 {
    let bytes = width * 4;
    (bytes + COPY_BYTES_PER_ROW_ALIGNMENT - 1) & !(COPY_BYTES_PER_ROW_ALIGNMENT - 1)
}

/// Compact a padded, possibly BGRA8 readback into tightly packed RGBA8
/// (row-major, top-to-bottom): strips the per-row padding and swaps R/B when
/// the source format is BGRA. Pure (unit-tested); the GPU path only produces
/// the padded bytes.
#[must_use]
pub fn compact_rgba8(
    raw: &[u8],
    bytes_per_row: u32,
    width: u32,
    height: u32,
    bgra: bool,
) -> Vec<u8> {
    let bytes_per_row = bytes_per_row as usize;
    let width = width as usize;
    let height = height as usize;
    let mut out = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        let start = row * bytes_per_row;
        for px in 0..width {
            let i = start + px * 4;
            // Bounds-checked reads (the workspace denies indexing/slicing);
            // the slice is exactly `bytes_per_row * height` bytes by
            // construction, so these never fall back.
            let r = raw.get(i).copied().unwrap_or(0);
            let g = raw.get(i + 1).copied().unwrap_or(0);
            let b = raw.get(i + 2).copied().unwrap_or(0);
            let a = raw.get(i + 3).copied().unwrap_or(0);
            if bgra {
                out.push(b);
                out.push(g);
                out.push(r);
                out.push(a);
            } else {
                out.push(r);
                out.push(g);
                out.push(b);
                out.push(a);
            }
        }
    }
    out
}

/// Convert premultiplied RGBA8 (the framebuffer/capture convention) to
/// straight RGBA8 (the PNG storage convention): divide the channels by alpha
/// where drawn, zero elsewhere. Uses the reference compositor's exact formula
/// (`round(channel · 255 / alpha)`, clamped) so a capture's PNG bytes match
/// what the reference would encode from the same premultiplied value.
#[must_use]
pub fn straighten_premultiplied_rgba8(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for chunk in pixels.chunks_exact(4) {
        let a = chunk.get(3).copied().unwrap_or(0);
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let un = |channel: u8| {
            let value = (f32::from(channel) * 255.0) / f32::from(a);
            value.round().clamp(0.0, 255.0) as u8
        };
        out.push(un(chunk.first().copied().unwrap_or(0)));
        out.push(un(chunk.get(1).copied().unwrap_or(0)));
        out.push(un(chunk.get(2).copied().unwrap_or(0)));
        out.push(a);
    }
    out
}

/// The half-texel UV shift for glyph sampling (DESIGN.md D28, corrected):
/// wgpu — like GL/Vulkan — samples at `uv·size − 0.5` (texel centers at
/// integers), while the CPU reference and the Canvas 2D fallback sample at
/// pixel centers (`uv·size`). The JS WebGL renderer compensates in its
/// vertex stage; the Rust renderer applies the same `+0.5/size` shift at plan
/// flattening so all agree. Images are deliberately **not** shifted: the
/// reference image path uses the raw `−0.5` convention explicitly.
#[must_use]
pub fn glyph_half_texel_shift(width: u32, height: u32) -> [f32; 2] {
    [0.5 / width.max(1) as f32, 0.5 / height.max(1) as f32]
}

/// Flatten a plan into interleaved vertex data (9 f32 per vertex) and draw
/// ranges `(start vertex, count, texture handle)`, applying the glyph
/// half-texel UV shift to every `GlyphBatch` op (the shift needs the texture
/// pixel size, which the pure plan builder does not know). Pure and
/// deterministic (unit-tested); `draw` only uploads the result.
fn flatten_plan(
    plan: &RenderPlan,
    texture_size: impl Fn(u32) -> Option<(u32, u32)>,
) -> (Vec<f32>, Vec<(u32, u32, u32)>) {
    let mut vertex_data: Vec<f32> = Vec::new();
    let mut ops: Vec<(u32, u32, u32)> = Vec::new(); // (start vertex, count, texture)
    for op in &plan.ops {
        let (vertices, texture, half_texel) = match op {
            PlanOp::GlyphBatch { texture, vertices } => {
                let shift = texture_size(*texture).map(|(w, h)| glyph_half_texel_shift(w, h));
                (vertices, *texture, shift)
            }
            PlanOp::Quad { texture, vertices } => (vertices, *texture, None),
        };
        let start = (vertex_data.len() / 9) as u32;
        for vertex in vertices {
            if let Some([hu, hv]) = half_texel {
                let mut shifted = *vertex;
                shifted.uv = [shifted.uv[0] + hu, shifted.uv[1] + hv];
                vertex_data.extend_from_slice(&shifted.as_floats());
            } else {
                vertex_data.extend_from_slice(&vertex.as_floats());
            }
        }
        ops.push((start, vertices.len() as u32, texture));
    }
    (vertex_data, ops)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::{
        align_row_bytes, compact_rgba8, flatten_plan, glyph_half_texel_shift, premultiply_rgba8,
        straighten_premultiplied_rgba8,
    };
    use crate::plan::{PlanOp, RenderPlan, Vertex, ViewUniform};

    /// One vertex with the given UV.
    fn vertex(uv: [f32; 2]) -> Vertex {
        Vertex {
            pos: [0.0, 0.0],
            uv,
            color: [1.0, 1.0, 1.0, 1.0],
            px_range: 4.0,
        }
    }

    /// Read a vertex back out of flattened data (9 f32 per vertex).
    fn read_vertex(data: &[f32], index: usize) -> Vertex {
        let base = index * 9;
        Vertex {
            pos: [data[base], data[base + 1]],
            uv: [data[base + 2], data[base + 3]],
            color: [
                data[base + 4],
                data[base + 5],
                data[base + 6],
                data[base + 7],
            ],
            px_range: data[base + 8],
        }
    }

    #[test]
    fn half_texel_shift_is_per_texel() {
        assert_eq!(
            glyph_half_texel_shift(1024, 512),
            [0.5 / 1024.0, 0.5 / 512.0]
        );
        assert_eq!(glyph_half_texel_shift(0, 0), [0.5, 0.5]); // degenerate: clamped to 1
    }

    #[test]
    fn flatten_shifts_glyph_uvs_but_not_quads() {
        // A glyph batch over a 1024×512 texture and a fill quad (white
        // texture, UV-irrelevant): the glyph UVs move by half a texel, the
        // quad vertices pass through untouched.
        let plan = RenderPlan {
            ops: vec![
                PlanOp::GlyphBatch {
                    texture: 100,
                    vertices: vec![vertex([0.1, 0.2]), vertex([0.3, 0.4])],
                },
                PlanOp::Quad {
                    texture: 1,
                    vertices: vec![vertex([0.0, 0.0]), vertex([1.0, 1.0])],
                },
            ],
            view: ViewUniform {
                scale: [0.0, 0.0],
                offset: [0.0, 0.0],
            },
        };
        let sizes = |handle: u32| match handle {
            100 => Some((1024, 512)),
            _ => None,
        };
        let (data, ops) = flatten_plan(&plan, sizes);
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0], (0, 2, 100));
        assert_eq!(ops[1], (2, 2, 1));
        let hu = 0.5 / 1024.0;
        let hv = 0.5 / 512.0;
        assert_eq!(read_vertex(&data, 0).uv, [0.1 + hu, 0.2 + hv]);
        assert_eq!(read_vertex(&data, 1).uv, [0.3 + hu, 0.4 + hv]);
        // Quads: untouched (images use the raw uv·size − 0.5 convention).
        assert_eq!(read_vertex(&data, 2).uv, [0.0, 0.0]);
        assert_eq!(read_vertex(&data, 3).uv, [1.0, 1.0]);
    }

    #[test]
    fn flatten_passes_through_when_the_texture_is_missing() {
        // An evicted handle: no size lookup, so no shift — the draw skips the
        // batch anyway (the host re-uploads next frame).
        let plan = RenderPlan {
            ops: vec![PlanOp::GlyphBatch {
                texture: 7,
                vertices: vec![vertex([0.5, 0.5])],
            }],
            view: ViewUniform {
                scale: [0.0, 0.0],
                offset: [0.0, 0.0],
            },
        };
        let (data, _ops) = flatten_plan(&plan, |_| None);
        assert_eq!(read_vertex(&data, 0).uv, [0.5, 0.5]);
    }

    #[test]
    fn premultiply_scales_channels_by_alpha() {
        // Opaque: identity. Transparent white: zero channels (no phantom ink).
        assert_eq!(
            premultiply_rgba8(&[255, 255, 255, 255]),
            vec![255, 255, 255, 255]
        );
        assert_eq!(premultiply_rgba8(&[255, 255, 255, 0]), vec![0, 0, 0, 0]);
        assert_eq!(
            premultiply_rgba8(&[255, 255, 255, 128]),
            vec![128, 128, 128, 128]
        );
        assert_eq!(
            premultiply_rgba8(&[100, 200, 50, 100]),
            vec![39, 78, 19, 100]
        );
    }

    #[test]
    fn row_alignment_pads_to_the_copy_constant() {
        assert_eq!(align_row_bytes(1), 256);
        assert_eq!(align_row_bytes(64), 256);
        // 800 px × 4 = 3200 bytes → padded to 3328 (13 rows of 256).
        assert_eq!(align_row_bytes(800), 3328);
        assert_eq!(align_row_bytes(256), 1024);
    }

    #[test]
    fn compact_strips_row_padding_and_swaps_bgra() {
        // Two rows of 2 px, padded to 256-byte rows; RGBA source.
        let mut raw = vec![0u8; 256 * 2];
        raw[0..8].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        raw[256..264].copy_from_slice(&[9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(
            compact_rgba8(&raw, 256, 2, 2, false),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]
        );
        // BGRA source: channels are stored B,G,R,A and must be swapped back.
        assert_eq!(
            compact_rgba8(&raw, 256, 2, 2, true),
            vec![3, 2, 1, 4, 7, 6, 5, 8, 11, 10, 9, 12, 15, 14, 13, 16]
        );
    }

    #[test]
    fn straighten_matches_the_reference_formula() {
        // The contract: a capture's PNG bytes must equal what the reference
        // compositor encodes from the same premultiplied value —
        // `round(channel · 255 / alpha)`, clamped. Assert per-channel.
        let reference = |channel: u8, a: u8| {
            let value = (f32::from(channel) * 255.0) / f32::from(a);
            value.round().clamp(0.0, 255.0) as u8
        };
        for pixels in [
            &[255, 255, 255, 255][..],
            &[100, 200, 50, 100][..],
            &[200, 30, 180, 64][..],
            &[39, 78, 19, 100][..],
            &[7, 28, 45, 64][..],
            &[1, 2, 3, 1][..],
        ] {
            let straight = straighten_premultiplied_rgba8(pixels);
            for (i, (got, expected)) in straight.iter().zip(pixels).enumerate() {
                if i == 3 {
                    assert_eq!(*got, *expected, "alpha passes through");
                } else {
                    assert_eq!(*got, reference(*expected, pixels[3]), "{got} vs {expected}");
                }
            }
        }
        // Fully transparent premultiplied pixels zero out (nothing drawn).
        assert_eq!(
            straighten_premultiplied_rgba8(&[0, 0, 0, 0]),
            vec![0, 0, 0, 0]
        );
        // Saturated premultiplied white un-premultiplies to opaque white.
        assert_eq!(
            straighten_premultiplied_rgba8(&[120, 120, 120, 120]),
            vec![255, 255, 255, 120]
        );
    }
}

/// The budget (in bytes) of GPU texture memory the renderer keeps. When an
/// upload would exceed it, the oldest-uploaded entries are evicted
/// deterministically (ascending handle = upload order); a draw referencing an
/// evicted handle is skipped, and the host re-uploads before the next frame
/// (the texture resolver maps (atlas, page)/image → the current handle).
pub const DEFAULT_TEXTURE_BUDGET: u64 = 128 * 1024 * 1024;

/// A typed renderer error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// No adapter matched the requested backends.
    NoAdapter(String),
    /// The device request failed.
    Device(String),
    /// A frame readback (offscreen capture) failed.
    Readback(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::NoAdapter(msg) => write!(f, "no adapter: {msg}"),
            RenderError::Device(msg) => write!(f, "device request failed: {msg}"),
            RenderError::Readback(msg) => write!(f, "frame readback failed: {msg}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// An **un-mapped** offscreen frame readback (DESIGN.md D31): the renderer
/// renders the plan into an offscreen texture and copies it into `buffer`;
/// hosts map it synchronously (native, via `Renderer::readback_to_rgba`) or
/// asynchronously (wasm, where `device.poll(Wait)` is unsupported).
#[derive(Debug)]
pub struct FrameReadback {
    /// The mapped-on-demand buffer (COPY_DST | MAP_READ).
    pub buffer: wgpu::Buffer,
    /// The padded row byte width (wgpu's copy alignment).
    pub bytes_per_row: u32,
    /// The frame width in device pixels.
    pub width: u32,
    /// The frame height in device pixels.
    pub height: u32,
    /// Whether the source format is BGRA (channels must be swapped to RGBA).
    pub bgra: bool,
}

/// One managed texture (page or image) with its view, sampler, and bind group.
struct TextureEntry {
    texture: wgpu::Texture,
    _view: wgpu::TextureView,
    _sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    _width: u32,
    _height: u32,
    bytes: u64,
}

/// The texture manager: atlas pages + images as GPU textures, deduplicated by
/// (atlas, page)/image id, byte-budgeted with deterministic eviction.
struct TextureManager {
    device: wgpu::Device,
    queue: wgpu::Queue,
    bind_group_layout: wgpu::BindGroupLayout,
    entries: BTreeMap<u32, TextureEntry>,
    page_handles: BTreeMap<(u32, u16), u32>,
    image_handles: BTreeMap<u32, u32>,
    next_handle: u32,
    budget_bytes: u64,
    used_bytes: u64,
}

impl TextureManager {
    fn new(device: wgpu::Device, queue: wgpu::Queue, budget_bytes: u64) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyphcull-texture-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let mut manager = Self {
            device,
            queue,
            bind_group_layout,
            entries: BTreeMap::new(),
            page_handles: BTreeMap::new(),
            image_handles: BTreeMap::new(),
            next_handle: 2,
            budget_bytes,
            used_bytes: 0,
        };
        manager.ensure_white();
        manager
    }

    /// The 1×1 white texture (handle 1) used by fills and rulers.
    fn ensure_white(&mut self) {
        if self.entries.contains_key(&crate::plan::WHITE_TEXTURE) {
            return;
        }
        let handle = crate::plan::WHITE_TEXTURE;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyphcull-white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        // NEAREST, mirroring the JS white texture (fills cover the full UV).
        let sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyphcull-white-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // The white texel must be written: a freshly-created texture is not
        // guaranteed to read as white (on most backends it reads as zeros), and
        // the MSDF program treats `texture.rgb` as a coverage field — a zero
        // median would render every fill and ruler fully transparent. This
        // mirrors the JS renderer's 1×1 white texture upload; the desktop
        // smoke (D31) pins it pixel-exactly.
        self.write_texture(&texture, &[255, 255, 255, 255], 1, 1);
        self.insert_entry(handle, texture, view, sampler, 1, 1, 4);
    }

    fn linear_sampler() -> wgpu::SamplerDescriptor<'static> {
        wgpu::SamplerDescriptor {
            label: Some("glyphcull-msdf-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        }
    }

    /// Insert a fully-built texture into the entries table.
    #[allow(clippy::too_many_arguments)]
    fn insert_entry(
        &mut self,
        handle: u32,
        texture: wgpu::Texture,
        view: wgpu::TextureView,
        sampler: wgpu::Sampler,
        width: u32,
        height: u32,
        bytes: u64,
    ) {
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyphcull-texture-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        self.entries.insert(
            handle,
            TextureEntry {
                texture,
                _view: view,
                _sampler: sampler,
                bind_group,
                _width: width,
                _height: height,
                bytes,
            },
        );
        self.used_bytes += bytes;
    }

    /// Create (or refresh) a texture from RGBA8 pixels.
    #[allow(clippy::too_many_arguments)]
    fn create_or_refresh(&mut self, handle: u32, pixels: &[u8], width: u32, height: u32) {
        if let Some(texture) = self.entries.get(&handle).map(|e| &e.texture) {
            // In-place refresh (the JS texImage2D update).
            self.write_texture(texture, pixels, width, height);
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyphcull-texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.write_texture(&texture, pixels, width, height);
        let view = texture.create_view(&Default::default());
        let sampler = self.device.create_sampler(&Self::linear_sampler());
        self.insert_entry(
            handle,
            texture,
            view,
            sampler,
            width,
            height,
            u64::from(width) * u64::from(height) * 4,
        );
    }

    fn write_texture(&self, texture: &wgpu::Texture, pixels: &[u8], width: u32, height: u32) {
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Upload (or refresh) an atlas page texture. Returns a handle the draw
    /// list's texture resolver maps back to.
    fn upload_atlas_page(
        &mut self,
        atlas_id: u32,
        page_index: u16,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> u32 {
        let key = (atlas_id, page_index);
        let handle = match self.page_handles.get(&key) {
            Some(handle) => *handle,
            None => {
                let handle = self.next_handle;
                self.next_handle += 1;
                self.page_handles.insert(key, handle);
                handle
            }
        };
        self.create_or_refresh(handle, pixels, width, height);
        self.evict_to_budget();
        handle
    }

    /// Upload (or refresh) an image texture. Returns a handle.
    ///
    /// Image texels are premultiplied before upload: the MSDF program samples
    /// `texture.rgb` as a coverage field (median), and the JS WebGL renderer
    /// uploads with `UNPACK_PREMULTIPLY_ALPHA_WEBGL` — premultiplying here
    /// keeps the two runtimes' image rendering identical (a transparent texel
    /// with non-zero RGB must not read as ink). Atlas pages stay raw: their
    /// distance channels are already the coverage field.
    fn upload_image(&mut self, image_id: u32, pixels: &[u8], width: u32, height: u32) -> u32 {
        let handle = match self.image_handles.get(&image_id) {
            Some(handle) => *handle,
            None => {
                let handle = self.next_handle;
                self.next_handle += 1;
                self.image_handles.insert(image_id, handle);
                handle
            }
        };
        let premultiplied = premultiply_rgba8(pixels);
        self.create_or_refresh(handle, &premultiplied, width, height);
        self.evict_to_budget();
        handle
    }

    /// Evict oldest-uploaded textures (ascending handle) until the budget is
    /// satisfied. Deterministic. A draw referencing an evicted handle is
    /// skipped; the host re-uploads before the next frame.
    fn evict_to_budget(&mut self) {
        let victims: Vec<u32> = self
            .entries
            .keys()
            .copied()
            .filter(|h| *h != crate::plan::WHITE_TEXTURE)
            .take_while(|_| self.used_bytes > self.budget_bytes)
            .collect();
        for handle in victims {
            if self.used_bytes <= self.budget_bytes {
                break;
            }
            if let Some(entry) = self.entries.remove(&handle) {
                self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
            }
            self.page_handles.retain(|_, h| *h != handle);
            self.image_handles.retain(|_, h| *h != handle);
        }
    }

    /// The bind group of a texture handle (white included), or `None` when
    /// the handle was evicted.
    fn bind_group(&self, handle: u32) -> Option<&wgpu::BindGroup> {
        self.entries.get(&handle).map(|e| &e.bind_group)
    }

    /// The pixel size of a texture handle (needed for the glyph half-texel
    /// UV shift at plan flattening), or `None` when evicted.
    fn texture_size(&self, handle: u32) -> Option<(u32, u32)> {
        self.entries.get(&handle).map(|e| (e._width, e._height))
    }

    /// The current texture byte usage.
    fn used_bytes(&self) -> u64 {
        self.used_bytes
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.page_handles.clear();
        self.image_handles.clear();
        self.used_bytes = 0;
        self.next_handle = 2;
        self.ensure_white();
    }
}

/// The wgpu MSDF renderer.
pub struct Renderer {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity_bytes: u64,
    textures: TextureManager,
    /// The non-sRGB target format the pipeline writes (DESIGN.md D28).
    pub target_format: wgpu::TextureFormat,
}

impl Renderer {
    /// Create the instance, adapter, and device for the given backends.
    ///
    /// Async (wgpu device requests are async-first); the desktop host wraps
    /// this in `pollster::block_on`, the wasm host awaits it directly.
    pub async fn init(
        backends: wgpu::Backends,
        target_format: wgpu::TextureFormat,
        texture_budget_bytes: u64,
    ) -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .map_err(|err| RenderError::NoAdapter(err.to_string()))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("glyphcull-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .map_err(|err| RenderError::Device(err.to_string()))?;
        Ok(Self::from_device(
            device,
            queue,
            target_format,
            texture_budget_bytes,
        ))
    }

    /// Build the renderer from an existing device/queue (hosts that configure
    /// the adapter themselves, e.g. for a specific surface).
    #[must_use]
    pub fn from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
        texture_budget_bytes: u64,
    ) -> Self {
        // The MSDF distance channels must stay linear and the premultiplied
        // u32 colors must be stored raw (DESIGN.md D28): an sRGB target would
        // re-encode the shader output.
        debug_assert!(
            !target_format.is_srgb(),
            "the MSDF pipeline targets non-sRGB formats only"
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("glyphcull-msdf"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(MSDF_WGSL)),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyphcull-view-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("glyphcull-texture-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glyphcull-msdf-pl"),
            bind_group_layouts: &[&uniform_bgl, &texture_bgl],
            push_constant_ranges: &[],
        });

        let vertex_buffer_layout = wgpu::VertexBufferLayout {
            array_stride: crate::shader::VERTEX_STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 8,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 2,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 32,
                    shader_location: 3,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyphcull-msdf-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[vertex_buffer_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyphcull-view-ub"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyphcull-view-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyphcull-vertices"),
            size: 0,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // `Device`/`Queue` are `Arc`-backed handles; the texture manager
        // shares them with the renderer.
        let textures = TextureManager::new(device.clone(), queue.clone(), texture_budget_bytes);
        Self {
            device,
            queue,
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            vertex_buffer,
            vertex_capacity_bytes: 0,
            textures,
            target_format,
        }
    }

    /// The device (the host needs it to configure its surface).
    #[must_use]
    pub const fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue (for direct uploads by the host, if needed).
    #[must_use]
    pub const fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Register a device-loss callback (the host re-creates the renderer and
    /// re-uploads textures from the core model; mirrors the JS context-restore
    /// flow).
    pub fn on_device_lost(
        &self,
        callback: impl Fn(wgpu::DeviceLostReason, String) + Send + 'static,
    ) {
        self.device.set_device_lost_callback(callback);
    }

    /// Upload (or refresh) an atlas page texture; returns the handle the
    /// texture resolver should map `(atlas_id, page_index)` to.
    pub fn upload_atlas_page(
        &mut self,
        atlas_id: u32,
        page_index: u16,
        pixels: &[u8],
        width: u32,
        height: u32,
    ) -> u32 {
        self.textures
            .upload_atlas_page(atlas_id, page_index, pixels, width, height)
    }

    /// Upload (or refresh) an image texture; returns the handle the texture
    /// resolver should map `image_id` to.
    pub fn upload_image(&mut self, image_id: u32, pixels: &[u8], width: u32, height: u32) -> u32 {
        self.textures.upload_image(image_id, pixels, width, height)
    }

    /// The current GPU texture byte usage.
    #[must_use]
    pub fn texture_bytes(&self) -> u64 {
        self.textures.used_bytes()
    }

    /// Draw a render plan into `target` (the full target is the viewport).
    pub fn draw(&mut self, plan: &RenderPlan, target: &wgpu::TextureView) {
        // Flatten the plan (glyph UVs get the half-texel shift) and record
        // draw ranges.
        let (vertex_data, ops) = flatten_plan(plan, |handle| self.textures.texture_size(handle));
        let byte_len = vertex_data.len() as u64 * 4;
        self.ensure_vertex_buffer(byte_len);
        if !vertex_data.is_empty() {
            self.queue
                .write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&vertex_data));
        }
        let view: [f32; 4] = [
            plan.view.scale[0],
            plan.view.scale[1],
            plan.view.offset[0],
            plan.view.offset[1],
        ];
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&view));

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("glyphcull-frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("glyphcull-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            for (start, count, texture) in ops {
                let Some(bind_group) = self.textures.bind_group(texture) else {
                    continue; // evicted handle: the host re-uploads next frame
                };
                pass.set_bind_group(1, bind_group, &[]);
                let byte_start = u64::from(start) * crate::shader::VERTEX_STRIDE;
                let byte_end = byte_start + u64::from(count) * crate::shader::VERTEX_STRIDE;
                pass.set_vertex_buffer(0, self.vertex_buffer.slice(byte_start..byte_end));
                pass.draw(0..count, 0..1);
            }
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Wait for the GPU to finish the submitted work (headless validation and
    /// teardown).
    pub fn poll(&self, wait: bool) -> Result<wgpu::PollStatus, wgpu::PollError> {
        self.device.poll(if wait {
            wgpu::PollType::Wait
        } else {
            wgpu::PollType::Poll
        })
    }

    /// Render a plan into an offscreen texture and copy it into an **un-mapped**
    /// readback buffer (DESIGN.md D31): the offscreen target uses the
    /// pipeline's non-sRGB format and the same clear + premultiplied blend as
    /// the surface, so the bytes equal what the surface presents. Works on
    /// every backend — including wasm, where `device.poll(Wait)` is
    /// unsupported, so hosts map the buffer instead (synchronously on native
    /// via [`Self::readback_to_rgba`], asynchronously on wasm).
    pub fn render_to_readback(
        &mut self,
        plan: &RenderPlan,
        width: u32,
        height: u32,
    ) -> Result<FrameReadback, RenderError> {
        let width = width.max(1);
        let height = height.max(1);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyphcull-capture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        self.draw(plan, &view);

        let bytes_per_row = align_row_bytes(width);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyphcull-capture-readback"),
            size: u64::from(bytes_per_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("glyphcull-capture-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);
        Ok(FrameReadback {
            buffer,
            bytes_per_row,
            width,
            height,
            bgra: self.target_format == wgpu::TextureFormat::Bgra8Unorm,
        })
    }

    /// Synchronously map a readback into tightly packed **premultiplied**
    /// RGBA8 (device pixels, top-left origin; row padding removed, BGRA
    /// swapped). Native only: `PollType::Wait` is unsupported on wasm, where
    /// the binding maps the buffer asynchronously instead.
    #[cfg(not(target_family = "wasm"))]
    pub fn readback_to_rgba(&self, readback: FrameReadback) -> Result<Vec<u8>, RenderError> {
        let FrameReadback {
            buffer,
            bytes_per_row,
            width,
            height,
            bgra,
        } = readback;
        // Map synchronously: `map_async` then keep polling until the callback
        // fires (the callback is driven by the device's poll loop).
        let slice = buffer.slice(..);
        let mapped = AtomicBool::new(false);
        let notifier = std::sync::Arc::new(mapped);
        let flag = std::sync::Arc::clone(&notifier);
        slice.map_async(wgpu::MapMode::Read, move |_| {
            flag.store(true, Ordering::Release);
        });
        let deadline = Instant::now() + Duration::from_secs(30);
        while !notifier.load(Ordering::Acquire) {
            self.poll(true)
                .map_err(|e| RenderError::Readback(e.to_string()))?;
            if Instant::now() >= deadline {
                return Err(RenderError::Readback(
                    "buffer map timed out after 30s".to_string(),
                ));
            }
        }
        let raw = slice.get_mapped_range();
        let rgba = compact_rgba8(&raw, bytes_per_row, width, height, bgra);
        drop(raw);
        buffer.unmap();
        Ok(rgba)
    }

    /// Render a plan into an offscreen texture and read it back as tightly
    /// packed **premultiplied** RGBA8 (device pixels, top-left origin) — the
    /// native synchronous convenience over [`Self::render_to_readback`] +
    /// [`Self::readback_to_rgba`] (hosts use this for the desktop
    /// `--screenshot` mode; wasm maps asynchronously).
    #[cfg(not(target_family = "wasm"))]
    pub fn render_to_rgba(
        &mut self,
        plan: &RenderPlan,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, RenderError> {
        let readback = self.render_to_readback(plan, width, height)?;
        self.readback_to_rgba(readback)
    }

    /// Release every GPU texture (the JS `reuploadAll` counterpart after
    /// device recreation: drop textures, keep the pipeline).
    pub fn clear_textures(&mut self) {
        self.textures.clear();
    }

    /// Release GPU resources.
    pub fn destroy(&self) {
        self.device.destroy();
    }

    fn ensure_vertex_buffer(&mut self, needed_bytes: u64) {
        if needed_bytes <= self.vertex_capacity_bytes {
            return;
        }
        let size = needed_bytes.max(4096);
        self.vertex_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyphcull-vertices"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertex_capacity_bytes = size;
    }
}
