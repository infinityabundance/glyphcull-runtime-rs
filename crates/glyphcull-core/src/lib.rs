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
//! - [`document`] — the trusted document model: load-time chunk-graph
//!   invariant validation, style resolution with SPEC §2.3 defaults, and
//!   content/reference resolution.
//! - [`clock`] — the injected clock: the determinism seam for all time-based
//!   decisions (`RealClock` in production, `FakeClock` in tests).
//! - [`lifecycle`] — the chunk lifecycle state machine: six explicit states,
//!   guarded transitions, transition log, selection pins, hidden-chunk and
//!   cooling-period guards.
//! - [`visibility`] — the visibility system: semantic + geometric culling
//!   producing the visible set, the hidden set, and the materialization
//!   frontier — a pure function of (document, geometry, viewport, margin).
//! - [`materialize`] — the streaming materialization scheduler: a
//!   deterministic priority queue (geometry, viewport, direction of travel),
//!   cooperative yields within a per-frame time budget, and deterministic
//!   eviction (cooling expiry + memory-pressure eviction of the furthest
//!   visible chunks).
//! - [`layout`] — the layout engine: breaks, Knuth–Plass line breaking,
//!   glyph measurement, and the block/layout engine producing absolute
//!   document geometry (the visibility `GeometrySource`).
//! - [`glyphs`] — the glyph cache: size-specific prepared stamps (quad + UV +
//!   metrics per (atlas, codepoint, size, color)), a byte budget with
//!   deterministic LRU eviction, and chunk-owning release for lifecycle
//!   coupling.
//! - [`selection`] — logical selection: document-order positions, hit
//!   testing, range→quad projection, and plain-text copy with the documented
//!   boundary policy — pure functions of (document, layout, selection).
//!
//! Subsequent phases add the draw list as a sibling module (see `ROADMAP.md`
//! 4.8).
//!
//! # Guarantees
//!
//! - **Never panics on input.** Every untrusted byte path returns a typed
//!   [`error::Error`]; direct indexing is confined to the bounds-checked
//!   reader cursor.
//! - **Deterministic.** No wall-clock in decisions, no unordered iteration in
//!   output paths; identical input ⇒ identical behavior.
//! - **Memory-safe.** `unsafe` is forbidden workspace-wide.

pub mod clock;
pub mod document;
pub mod error;
pub mod glyphs;
pub mod layout;
pub mod lifecycle;
pub mod limits;
pub mod materialize;
pub mod reader;
pub mod selection;
pub mod visibility;
