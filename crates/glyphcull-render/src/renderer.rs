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

use crate::plan::{PlanOp, RenderPlan};
use crate::shader::MSDF_WGSL;

/// Premultiply RGBA8 texels in place of a copy (alpha-scaled channels). The
/// MSDF program treats `texture.rgb` as a coverage field, so a transparent
/// texel must carry zero-valued channels; this matches the JS WebGL renderer's
/// `UNPACK_PREMULTIPLY_ALPHA_WEBGL` upload convention.
fn premultiply_rgba8(pixels: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len());
    for chunk in pixels.chunks_exact(4) {
        let a = chunk.get(3).copied().unwrap_or(0);
        let scale = u16::from(a);
        out.push(((u16::from(chunk.get(0).copied().unwrap_or(0)) * scale) / 255) as u8);
        out.push(((u16::from(chunk.get(1).copied().unwrap_or(0)) * scale) / 255) as u8);
        out.push(((u16::from(chunk.get(2).copied().unwrap_or(0)) * scale) / 255) as u8);
        out.push(a);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::premultiply_rgba8;

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
}

/// The budget (in bytes) of GPU texture memory the renderer keeps. When an
/// upload would exceed it, the oldest-uploaded entries are evicted
/// deterministically (ascending handle = upload order); a draw referencing an
/// evicted handle is skipped, and the host re-uploads before the next frame
/// (the texture resolver maps (atlas, page)/image → the current handle).
pub const DEFAULT_TEXTURE_BUDGET: u64 = 128 * 1024 * 1024;

/// A typed renderer initialization error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenderError {
    /// No adapter matched the requested backends.
    NoAdapter(String),
    /// The device request failed.
    Device(String),
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::NoAdapter(msg) => write!(f, "no adapter: {msg}"),
            RenderError::Device(msg) => write!(f, "device request failed: {msg}"),
        }
    }
}

impl std::error::Error for RenderError {}

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
        // Flatten the plan's vertex data and record draw ranges.
        let mut vertex_data: Vec<f32> = Vec::new();
        let mut ops: Vec<(u32, u32, u32)> = Vec::new(); // (start vertex, count, texture)
        for op in &plan.ops {
            let (vertices, texture) = match op {
                PlanOp::GlyphBatch { texture, vertices } => (vertices, *texture),
                PlanOp::Quad { texture, vertices } => (vertices, *texture),
            };
            let start = (vertex_data.len() / 9) as u32;
            for vertex in vertices {
                vertex_data.extend_from_slice(&vertex.as_floats());
            }
            ops.push((start, vertices.len() as u32, texture));
        }
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
