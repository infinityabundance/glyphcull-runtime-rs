//! glyphcull-desktop: the winit host (Phase 4.11).
//!
//! A native window that loads a compiled `.cull` package and serves the
//! identical six-operation runtime API — `load`, `scroll`, `paint`, `select`,
//! `copy`, `destroy` — over a wgpu surface (WebGPU or GL), mirroring the wasm
//! binding exactly. The runtime logic itself lives in `glyphcull-host`; this
//! crate is the winit face of it:
//!
//! - `DesktopDocument` — a window + the shared host, with the six operations
//!   and the input wiring (wheel → scroll, drag → select, Esc/close → exit).
//! - `input` — pure, headless-testable input translation (winit events →
//!   document-space operations).
//! - `sink` — the native wgpu `FrameSink` (`Surface<'static>` via the
//!   Arc-backed window handle, non-sRGB target, per-frame present).
//! - the `glyphcull-desktop` binary — open a `.cull` file in a window.

mod app;
pub mod input;
mod sink;

pub use app::{run, DesktopDocument, DesktopError};
