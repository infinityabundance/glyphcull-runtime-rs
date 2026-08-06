//! Block layout (Architecture.md §3.5; mirrors the JS
//! `src/layout/*`): materialization turns semantic chunks into renderable
//! geometry.
//!
//! Text blocks (paragraphs, headings, captions, quotes) break lines with the
//! Knuth–Plass algorithm (UAX #29-grounded tokenization, [`breaks`], [`kp`]);
//! code blocks are preformatted; lists emit markers; tables use a
//! deterministic auto layout honoring colspan/rowspan; images keep their
//! intrinsic aspect ratio. Every record is absolute geometry in document
//! pixels — the runtime owns no CSS cascade (styles are resolved in the
//! package).
//!
//! The engine drives a **sequential frontier**: top-level blocks are laid
//! out in document order, so chunks beyond the viewport are never laid out
//! (streaming). `extend_to` advances the frontier until the viewport is
//! covered; `materialize` lays out one top-level block (idempotent).
//!
//! Subsystem order in this phase:
//!
//! 1. [`breaks`] — tokenization into breakable units (word boundaries,
//!    punctuation binding, CJK, forced breaks).
//! 2. [`kp`] — the Knuth–Plass dynamic program over boxes/glue/penalties.
//! 3. [`measure`] — per-glyph advances, kerning, and mark attachment from
//!    the MSDF atlas.
//! 4. [`layout`] — the block/layout engine implementing the visibility
//!    `GeometrySource` contract.

pub mod breaks;
pub mod kp;
// The module mirrors the JS file name (`layout.ts`); the repetition of the
// parent module's name is intentional (clippy::module_inception is scoped).
#[allow(clippy::module_inception)]
pub mod layout;
pub mod measure;
