# Changelog

All notable changes, reverse chronological. Keep a Changelog format; Semantic Versioning.

## [Unreleased]

### Added (Phase 4.9 — glyphcull-render)

- `glyphcull-render` workspace member: the wgpu MSDF renderer (WebGPU + GL/WebGL2
  backends; naga translates one WGSL program to GLSL and SPIR-V).
  - `msdf` — the normative CPU reconstruction (median, smoothstep, texel→px, bilinear
    coverage, supersampled glyph reconstruction, edge clamping) mirroring the JS
    `src/render/msdf.ts`; the single source of truth for the shader and the
    validation.
  - `shader` — one logical WGSL program (median-of-three, 1-device-px edge,
    premultiplied alpha, document-y-down), validated and translated headlessly via
    naga (GLSL for the GL backend, SPIR-V for Vulkan).
  - `plan` — the pure render plan: draw list → batched GPU ops (glyph batches by
    texture, single quads for fills/rulers/images) + the view uniform; premultiplied
    colors, dpr-scaled px-range, z-order, and determinism are testable without a GPU.
  - `renderer` — the wgpu executor: async init lifecycle, budgeted textures with
    deterministic eviction (DEFAULT_TEXTURE_BUDGET), in-place page refreshes,
    premultiplied-alpha blending against non-sRGB targets (D28), device-loss
    recovery via `on_device_lost`, and per-frame plan playback.
- Test suites: `tests/msdf.rs` (JS-mirror vectors incl. the pinned vertical-edge
  profile), `tests/plan.rs` (golden pipeline → plan batching/z-order/premultiplied
  data/determinism), `tests/validation.rs` (headless CPU rendering validation incl.
  the D28 sampling-convention pin), in-crate shader/plan unit tests, and the
  `plan_golden` criterion benchmark (baselines in PERFORMANCE.md §6).

### Added (Phase 4.8 — glyphcull-core draw list)

- `glyphcull-core::draw_list` — the ordered draw command sequence mirroring the JS
  `src/render/drawlist.ts`: `GlyphCommand` (texture, UV, quad, color, px-per-texel for
  the shader's px range), `ImageCommand`, `FillCommand` (selection + backgrounds),
  `RulerCommand`, the `DrawCommand` enum, `DrawList`, the `TextureResolver` trait, and
  `DrawListBuilder` (selection fills first beneath content; per block background →
  line glyphs → list marker → children; the visible-set `emitted` guard emits every
  block exactly once; missing stamps are skipped mid-flight). Markers render as glyph
  commands from the disc/alpha/roman stamp.
- The builder is a pure function of its inputs; double-build equality is pinned by
  tests, as is the R5 divergence (container-nested images and rules render — the JS
  children path drops them).
- Test suites: `tests/draw_list.rs` (JS-mirror vectors, R5 pins, repeated-build
  stability, 100-case subset/selection proptest) and the criterion benchmark
  `draw_list` (baselines in PERFORMANCE.md §6).

### Added (Phase 4.7 — glyphcull-core glyph cache + selection)

- `glyphcull-core::glyphs` — the glyph cache mirroring the JS `src/glyphs/cache.ts`:
  `prepare_glyph` (size-specific stamps: quad + UV + f64 advance + flags per
  (atlas, codepoint, size, color), from the SPEC §2.5 placement convention) and
  `GlyphCache` (byte budget, deterministic LRU eviction via a monotonic touch counter
  — DESIGN.md D27 — chunk-owning `release_chunk` for lifecycle coupling, `clear`). The
  budget is `u64`, so invalid budgets are unrepresentable (R4).
- `glyphcull-core::selection` — logical selection mirroring the JS
  `src/selection/selection.ts`: char-based `TextPosition` (R3), normalized
  `Selection`, `compare_positions`/`normalize_selection`/`is_collapsed`,
  `hit_test_point` (nearest line, then nearest glyph center), `range_quads` (per-line
  highlight quads with contiguous merging and a proportional fallback for glyph-less
  runs), `covered_chunk_ids`, and `copy_text` with the documented boundary policy
  (runs re-join, blocks → `\n`, cells of one row → `\t`, rows → `\n`, `br` → `\n`).
- Test suites: `tests/glyph_cache.rs` (JS-mirror vectors incl. LRU budgeting and
  release-chunk sharing), `tests/selection.rs` (JS-mirror vectors incl. the synthetic
  table/br copy policy), `tests/selection_property.rs` (300-case proptests), and the
  criterion benchmark `glyph_selection` (baselines in PERFORMANCE.md §6).
- The visibility RSS memory gate moved to its own binary (`tests/visibility_memory.rs`)
  so parallel test threads cannot inflate `VmHWM` mid-measurement — the same
  single-test-file convention as the other memory gates.

### Added (Phase 4.6 — glyphcull-core layout)

- `glyphcull-core::layout` — materialization turns semantic chunks into renderable
  geometry (mirrors the JS `src/layout/*`):
  - `breaks` — UAX #29-grounded tokenization (word runs, closing/opening punctuation
    binding, CJK between-ideograph breaks, forced newlines) with the exact ECMAScript
    `\s` and `\p{L}\p{N}` classes so whitespace/word runs match the JS runtime.
  - `kp` — the Knuth–Plass dynamic program (boxes/glue/penalties, prefix sums, fitness
    classes, twice-around fitness pass, active-list dedup by fitness class, the
    single-line fallback when the active list exhausts).
  - `measure` — per-codepoint glyph measurement: advances, kerning (binary search over
    the SPEC-sorted pair table), combining-mark attachment, letter-spacing, and the
    half-em tofu fallback; f64 arithmetic on f32 atlas metrics.
  - `layout` — the `LayoutEngine`: sequential frontier (`extend_to`/`materialize`/
    `next_frontier_block`), text blocks (KP breaking + first-line indent), preformatted
    code blocks, quotes, lists with markers (disc/circle/square/decimal/alpha/roman),
    images (dpr-aware aspect), `hr`, and the deterministic table auto layout
    (colspan/rowspan with the last-spanned-row growth rule). Records are `Rc`-shared
    between the records index and parent children (each block exists exactly once); the
    engine implements the visibility `GeometrySource` contract.
- Layout test suite: JS-mirror vectors (breaks/kp/measure/layout), synthetic table/
  image/hr packages, token-partition guarantee, 300-case proptests (KP invariants,
  determinism, geometry sanity), stress (5000-deep quote chain on a 64 MiB-stack
  thread, 2000-block streaming, 3000-word KP), RSS memory gate (full golden layout ≈
  2.3 × package size vs 8 × budget), and the criterion benchmark `layout` (baselines
  in PERFORMANCE.md §6).
- Deliberate correctness divergences from the JS runtime, pinned by tests and
  documented in DESIGN.md (R-series): the token→line index mapping (R2) and
  measurement hardening for astral codepoints (R1).

### Added (Phase 4.5 — glyphcull-core materialization)

- `glyphcull-core::materialize` — the streaming materialization scheduler mirroring the
  JS runtime: a deterministic binary min-heap; `priority_key` (pure function of
  geometry/viewport/direction: intersecting chunks first, 1024 px distance tiers,
  ahead-of-travel preferred, document-order tie-breaks, sequential frontier for
  geometry-less chunks); `reconcile` (visible set → lifecycle transitions incl.
  requeue of cooling chunks and dequeue/cancel of departed ones); `run_frame`
  (per-frame budget, cooperative yields with an anti-starvation penalty, measured
  elapsed never affecting decisions); `tick` (cooling expiry through the worker,
  selection-respecting); `evict_for_memory` (furthest visible chunks released first,
  deterministic). The scheduler owns its `LifecycleManager` and borrows the clock.
- Materialization test suite: priority ordering (intersect/distance/direction/
  tie-breaks), processing order, frame budget, no-starvation cooperative yielding,
  visit-sequence determinism, reconcile (cull + cancel + dequeue + requeue paths),
  tick (period, selection pins, release order), evict_for_memory (furthest-first,
  byte targets), and a 100-case no-starvation proptest.

### Added (Phase 4.4 — glyphcull-core visibility)

- `glyphcull-core::visibility` — the visibility system mirroring the JS runtime:
  `Rect`/`Viewport`, the `GeometrySource` trait (implemented by layout), semantic
  culling (hidden flag excludes a whole subtree), geometric culling against the
  margin-expanded viewport (inclusive of edges), the materialization frontier
  (`not_yet_visible`), and structural-context visibility (a structural chunk is visible
  iff a descendant is). Iterative pre-order walk with exactly one geometry read per
  non-hidden chunk; the visible set is a pure function of its inputs.
- Visibility test suite: geometric/semantic culling, margin expansion, edge-inclusive
  intersection, frontier semantics, purity, the read-only boundary (document and
  geometry never mutate; exact read counts), structural-context visibility, golden
  invariants, a 300-case proptest over random documents/geometries/viewports
  (invariants + purity), and 100k-chunk stress with an RSS memory gate.

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

- glyphcull-wasm (tiny API bindings); glyphcull-desktop (native
  host); mobile targets.
