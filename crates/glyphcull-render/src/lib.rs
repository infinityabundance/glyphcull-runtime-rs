//! glyphcull-render: the wgpu MSDF renderer (Architecture.md §4).
//!
//! Consumes the draw list produced by `glyphcull-core` and nothing else —
//! the renderer never sees the document model. wgpu with the WebGPU and GL
//! (WebGL2) backends; the MSDF program is a single WGSL source translated to
//! every backend by naga; textures are budgeted; device loss is recovered by
//! re-creating the renderer and re-uploading from the core model.
//!
//! # Modules
//!
//! - [`msdf`] — the normative MSDF reconstruction (CPU reference; the single
//!   source of truth shared by the shader and the validation).
//! - [`shader`] — the one logical WGSL program (median-of-three, 1-device-px
//!   edge, premultiplied alpha), translated to GLSL/SPIR-V by naga.
//! - [`plan`] — the pure render plan: a draw list compiled into batched GPU
//!   ops + the view uniform (testable without a GPU).
//! - [`renderer`] — the wgpu renderer: init lifecycle, budgeted textures,
//!   plan playback, device-loss recovery.

pub mod msdf;
pub mod plan;
pub mod renderer;
pub mod shader;
