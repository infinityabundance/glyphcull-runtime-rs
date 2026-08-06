# Testing — glyphcull-runtime-rs

Status: Phases 4.1–4.10 landed (reader, document model, lifecycle, visibility,
materialization, layout, glyph cache, selection, draw list, render, wasm host). The
pyramid below is the target for Phase 4; those layers are delivered (see
`crates/*/tests/`).

## 1. Principles

- Deterministic tests: injected clock, no network, no wall-clock dependencies.
- Untrusted-input tests assert typed errors, never panics (malformed corpus wrapped in
  `catch_unwind` where panics are provably impossible is still asserted).
- Golden artifacts committed; regeneration via checked-in scripts with reviewed diffs.
- Core is headless-testable (no GPU, no host); render is tested natively and in wasm.

## 2. Layers

### Unit (per crate, per module)

- **core/reader**: every SPEC.md §1.6 rule; truncation corpus (every proper prefix of a
  golden package errors); CRC-32 known-answer vectors; zlib round-trip + size limits; SEAL
  pass/fail.
- **core/document**: chunk tree invariants; style defaults; content reference resolution.
  Delivered: `tests/document_model.rs` (golden contract + every invariant rejection),
  `tests/document_property.rs` (random-tree generator: every generated tree builds, walk is
  a bijection, depths consistent; golden byte-mutations never panic), `tests/document_stress.rs`
  (100k-chunk wide + 10k-deep documents, iterative traversals), `tests/document_memory.rs`
  (RSS gate: build peak ≈ 2 × package size vs 8 × budget).
- **core/lifecycle**: exhaustive transition table; guards both ways; transition log;
  injected-clock cooling; invalid-transition rejection. Delivered: `tests/lifecycle.rs`
  (6 × 9 exhaustive table vs a reference model, cooling-elapsed/selection/hidden guards,
  ordered log with injected timestamps, log determinism, state census, idempotent
  registration, and a 300-case model-based property test over random event sequences).
- **core/visibility**: culling decisions (margin, semantic flags, direction); culling does
  not mutate materialization state (boundary assertion). Delivered: `tests/visibility.rs`
  (geometric/semantic culling, margin expansion, edge-inclusive intersection, frontier,
  purity, read-only boundary with exact read counts, structural-context visibility,
  golden invariants), `tests/visibility_property.rs` (300-case invariants + purity over
  random documents/geometries/viewports), `tests/visibility_stress.rs` (100k-chunk culls,
  determinism, RSS memory gate).
- **core/materialize**: priority order; budget yield; eviction order; no starvation.
  Delivered: `tests/materialize.rs` (priority_key tiers/direction/tie-breaks, processing
  order, frame budget, cooperative yields with no starvation, visit-sequence
  determinism, reconcile cull/requeue paths, tick expiry + selection pins,
  evict_for_memory order, and a 100-case no-starvation proptest).
- **core/layout**: UAX #29 sample vectors; KP quality on known texts; block/table/image
  layout golden fixtures (shared vectors with the JS runtime). Delivered: `tests/layout_breaks.rs`
  (JS mirror vectors: word runs, punctuation binding, contractions, CJK, forced breaks,
  whitespace-run newline absorption — including a cross-check of the exact `\s` class),
  `tests/layout_kp.rs` (JS mirror vectors incl. forbidden/forced penalties, tolerance,
  fitness classes, emergency-stretch ratio, active-list-exhaustion fallback parity, and a
  reconstruction check that every line's ratio is consistent with its item span),
  `tests/layout_measure.rs` (golden-atlas advances, kerning, marks, letter-spacing, tofu,
  astral codepoints), `tests/layout.rs` (golden full-document layout: increasing y, heading
  text, frontier streaming, materialize idempotence, run geometry, list markers, code
  blocks, quote indents, double-build determinism — plus synthetic packages for tables
  (columns/colspan/rowspan growth), images (dpr + aspect), `hr`, and the token-partition
  guarantee), `tests/layout_property.rs` (300-case KP invariants + determinism + geometry
  sanity over random texts/widths), `tests/layout_stress.rs` (5000-deep quote chain on a
  64 MiB-stack thread, 2000-block streaming, 3000-word paragraph KP), `tests/layout_memory.rs`
  (RSS gate: full golden layout within the 8 × package budget).
- **core/glyph cache**: budget enforcement; eviction coupling with lifecycle. Delivered:
  `tests/glyph_cache.rs` (JS-mirror vectors: stamp placement convention vs the golden
  atlas, missing-codepoint tofu, size scaling, color keying, LRU byte-budget eviction,
  unlimited budget, `release_chunk` sharing/freeing, `clear`, key round-trip) plus
  in-crate unit tests (placement convention on a hand-built atlas, missing codepoints,
  zero-budget eviction).
- **core/selection**: hit testing at boundaries; range merging; copy policy. Delivered:
  `tests/selection.rs` (JS-mirror vectors: position ordering/normalization, glyph-center
  hit testing with boundary/clamp cases, range→quad projection with per-line merging
  and the proportional fallback, copy policy over the golden — heading/partial/styled
  runs/block separators — plus synthetic tables for the cell→tab/row→newline policy and
  a `br` document; block-span copy equivalence property), `tests/selection_property.rs`
  (300-case proptests: comparison antisymmetry, normalization ordering, copyText
  determinism, covered-id slice equality).
- **core/draw list**: batching, z-order, determinism (double-build equality). Delivered:
  `tests/draw_list.rs` (JS-mirror vectors: double-build byte equality, selection
  z-order beneath content, glyph command geometry + px-range input, one command per
  laid-out outlined glyph plus list markers, marker emission from the bullet stamp,
  ruler pass-through, missing-stamp resilience incl. partial coverage, repeated-build
  stability — plus the R5 divergence pins for container-nested images and rules — and a
  100-case proptest over random visible subsets and selection quads).
- **render**: shader math unit tests (median-of-three coverage vs analytic), DPR mapping,
  texture budget, device-loss recovery path, WebGPU/GL parity fixtures. Delivered:
  `crates/glyphcull-render/tests/msdf.rs` (JS-mirror vectors: median/smoothstep/texel
  scaling/coverage incl. the median guard and AA width, the synthetic vertical-edge
  profile with pinned values, downsampling, page-edge clamping, box-larger-than-page),
  `tests/plan.rs` (golden pipeline → plan: batching by texture, z-order, premultiplied
  vertex data, scroll uniform, determinism, one quad per outlined glyph plus markers),
  `tests/validation.rs` (headless CPU rendering validation: golden glyph reconstruction
  profile, in-bounds UVs through the full pipeline, and the D28 sampling-convention
  pin), plus in-crate tests (the WGSL parses/validates and translates to GLSL and SPIR-V
  via naga; shader math equals the CPU reference; plan unit tests). GPU execution is
  exercised on the desktop host (4.11); the renderer compiles for native, wasm32, and
  Android in CI.
- **wasm**: the six-operation contract over the compiler golden package against a
  recording sink — `tests/host.rs` (load validation, upload census, scroll/paint/resize,
  select-and-copy round-trips, destroy semantics, multi-document isolation, selection
  pinning); no wasm, no GPU.
- **desktop**: input→API wiring tests (with a headless harness where the host allows).

### Integration

- load → scroll → paint cycles over compiler golden packages; visible set and draw list
  compared against committed fixtures (shared with JS runtime — cross-runtime contract).
- select → copy over mixed content.
- Renderer: native WebGPU output vs golden reference images (tolerance policy per
  TESTING.md of the JS runtime); GL fallback parity (same draw list, tolerance).

### Property (proptest)

- Reader: arbitrary bytes never panic; decode∘encode round-trips on writer output.
- Lifecycle: model-based testing — random valid transition sequences never violate guards
  (reference state machine).
- Draw list: same inputs ⇒ identical serialized draw list.
- Layout: KP output invariants (no overfull lines beyond tolerance, deterministic).

### Stress

- 100k+ chunk documents; full-document scroll; oscillation; whole-document selection;
  multiple Documents. Assert bounded time and memory.

### Memory regression

- Counting-allocator harness (test-only global allocator): load/paint/scroll/destroy
  cycles; retained allocation growth below committed baselines; budget eviction releases
  allocations.
- RSS gates, one per binary so parallel test threads cannot inflate `VmHWM`
  mid-measurement: `reader_memory.rs`, `document_memory.rs`, `layout_memory.rs`,
  `visibility_memory.rs` (moved out of `visibility_stress.rs` in 4.7 for this reason).

### Performance regression

- Criterion benches with committed baselines (load, first paint, scroll frame, materialize
  throughput, memory); ratio-based regression checks.

### Rendering validation

- Offscreen native render (wgpu) of golden packages compared to reference rasterizer
  images; wasm variant in browser harness (Playwright, in demo repo).

### Package validation

- `load()` rejects malformed packages with typed errors; every rejection has a test.

## 3. Tooling

- cargo test, cargo clippy `-D warnings`, cargo fmt --check, proptest, criterion,
  cargo-fuzz (reader, scheduled), wasm-bindgen-test (wasm target), CI matrix: native +
  wasm32 + Android build targets.

## 4. CI

- fmt → clippy → test (native) → wasm tests → bench smoke → doc build (missing_docs deny)
  → Android build targets.
