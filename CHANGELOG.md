# Changelog

All notable changes, reverse chronological. Keep a Changelog format; Semantic Versioning.

## [Unreleased]

### Added (Phase 4.3 — glyphcull-core chunk lifecycle)

- `glyphcull-core::clock` — the injected clock: `Clock` trait, `RealClock` (wall),
  `FakeClock` (deterministic, interior-mutable so the manager can share it by reference).
- `glyphcull-core::lifecycle` — the chunk lifecycle state machine mirroring the JS
  runtime: six states, nine guarded events (hidden chunks never enqueue; expire needs the
  cooling period elapsed and no selection pin), `BTreeMap`-backed per-chunk tables
  (deterministic enumeration), and an ordered transition log stamped by the injected
  clock.
- Lifecycle test suite: exhaustive 6×9 transition table against a reference model, guard
  tests (cooling period, selection pins, hidden chunks), log order/determinism, state
  census, idempotent registration, and a 300-case model-based proptest over random event
  sequences.

### Added (Phase 4.2 — glyphcull-core document model)

- `glyphcull-core::document` — the trusted runtime view of a package (mirrors the JS
  `DocumentModel`): `build_document` load-time validation of chunk-graph invariants
  (single document root, dense ids/ordinals, sibling-ring consistency, parent/depth
  consistency, full reachability, no cycles), reference resolution (style ids, content
  indices, image refs against IMGS, resolved font_id against GLYF), and the five INFO
  count cross-checks.
- `ResolvedStyle` + `resolve_style`: all 16 SPEC §2.3 properties with defaults applied.
- Trusted views: `all_ids` (pre-order), `child_ids`, `extras_for`, `direct_text`,
  `image_ref`, and iterative `plain_text` (document-order copy text; `br` → newline;
  code blocks contribute direct text). The model borrows the package's payloads — no
  data duplication at build.
- Reader completion: INFO `format_version` must equal the header version
  (`UnsupportedVersion`), and `source_digest`/`document_id` must be lowercase hex of the
  SPEC lengths (`InvalidValue`) — mirroring the JS reader.
- Document test suite (30 tests + 300-case proptest properties): golden contract, every
  invariant rejection (cycle, unreachable chunk, broken sibling ring, dangling
  style/content/image refs, count mismatches, wrong payload kinds), style resolution,
  direct-text/image-ref resolution, plain-text order (br/code/hr), isolation, random-tree
  generator property, golden byte-mutation never-panic property, stress (100k-wide and
  10k-deep documents, iterative traversals), RSS memory gate (build peak ≈ 2 × package
  size vs 8 × budget).
- Criterion benchmark `document/build` (baselines in PERFORMANCE.md §6).

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
