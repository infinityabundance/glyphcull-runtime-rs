//! GlyphCull core: the platform-agnostic compiled-document runtime.
//!
//! `glyphcull-core` is the heart of the Rust runtime (`glyphcull-runtime-rs`).
//! It consumes `.cull` packages — compiled documents produced by
//! `glyphcull-compiler` — and executes the document pipeline
//! (visibility → materialization → layout → glyph cache → draw list) without
//! ever seeing HTML, Markdown, or CSS. The compiler owns translation; the
//! runtime owns execution; the package format is the only contract
//! (`glyphcull-compiler/docs/format/SPEC.md`).
//!
//! The architecture mirrors `glyphcull-runtime-js` exactly — subsystem
//! responsibilities, state machines, and data flow are identical; only the
//! implementation language differs. This crate is deliberately free of wgpu,
//! winit, and wasm-bindgen: `glyphcull-render` drives rendering from the
//! draw list this crate produces, and the hosts (`glyphcull-wasm`,
//! `glyphcull-desktop`) own windowing and input.
//!
//! # Modules
//!
//! - [`error`] — typed reader/runtime errors (mirrors the JS `CullError`).
//! - [`limits`] — the SPEC §1.3 caps enforced before allocating on untrusted
//!   lengths.
//! - [`reader`] — the independent `.cull` container reader: header and section
//!   table validation, zlib with explicit header/Adler-32 verification,
//!   per-section CRC-32, typed section decoders (INFO/CHNK/STYL/CONT/GLYF/
//!   IMGS/SEAL), and mandatory SEAL verification.
//!
//! Subsequent phases add the document model, chunk lifecycle, visibility,
//! materialization, layout, glyph cache, selection, and the draw list as
//! sibling modules (see `ROADMAP.md` 4.2–4.8).
//!
//! # Guarantees
//!
//! - **Never panics on input.** Every untrusted byte path returns a typed
//!   [`error::Error`]; direct indexing is confined to the bounds-checked
//!   reader cursor.
//! - **Deterministic.** No wall-clock in decisions, no unordered iteration in
//!   output paths; identical input ⇒ identical behavior.
//! - **Memory-safe.** `unsafe` is forbidden workspace-wide.

pub mod error;
pub mod limits;
pub mod reader;
