# Changelog

All notable changes, reverse chronological. Keep a Changelog format; Semantic Versioning.

## [Unreleased]

### Added (Phase 4.1 — glyphcull-core reader)

- `glyphcull-core` workspace member: the platform-agnostic compiled-document runtime
  crate (reader layer first, per ROADMAP).
- `glyphcull-core::reader` — the independent `.cull` container reader (SPEC.md §1):
  header + section table validation with overflow-checked arithmetic; per-section zlib
  decode with explicit header and Adler-32 verification and bounded incremental
  accumulation; `decoded_len` exactness; per-section CRC-32; duplicate-kind rejection;
  reserved kinds addressable without interpretation.
- Typed section decoders (SPEC.md §2): INFO (deterministic JSON subset, strict keys,
  duplicate-key rejection), CHNK (records + all four extra kinds), STYL (all 16 property
  tags), CONT (text + image refs), GLYF (atlases, sorted kerning, page texels), IMGS
  (RGBA8/RGB8), SEAL (hash tree). Lazy behind `OnceLock` caches; owned payloads.
- SEAL verification at load (per-section SHA-256 + overall hash over header `0..12` and
  covered sections in canonical order), mirroring the JS runtime's `verifySeal`.
- `glyphcull-core::error` — typed `Error`/`ErrorKind` (mirrors JS `CullError`);
  `glyphcull-core::limits` — the SPEC §1.3 caps.
- Reader test suite (56 tests + 500-case proptest properties): golden contract tests
  with `cull inspect` diagnostics pinned, truncation corpus (every proper prefix),
  single-byte mutation corpus, zlib strictness, section-decoder edge coverage, unknown/
  duplicate/reserved-kind handling, determinism, stress (64-section cap, 1 MiB decoded,
  200-iteration stability), and the RSS-based memory regression gate.
- `scripts/refresh-fixtures.sh` + committed compiler fixtures
  (`crates/glyphcull-core/tests/fixtures/`).
- Criterion benchmark `reader/parse` + `reader/full-contract` (baselines in
  PERFORMANCE.md §6).

### Added (Phase 0 — Foundations)

- Repository scaffolding: README, Architecture.md, DESIGN.md, ROADMAP.md, TESTING.md,
  PERFORMANCE.md, SECURITY.md, CONTRIBUTING.md, LICENSE (Apache-2.0), CHANGELOG,
  .gitignore, .editorconfig.

### Planned (Phase 4 — per master plan)

- glyphcull-core (document model, lifecycle, visibility, materialization, layout, glyph
  cache, selection, draw list); glyphcull-render (wgpu: WebGPU + GL fallback, MSDF);
  glyphcull-wasm (tiny API bindings); glyphcull-desktop (native host); mobile targets.
