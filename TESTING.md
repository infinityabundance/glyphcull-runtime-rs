# Testing — glyphcull-runtime-rs

Status: Phase 0 (foundations). The pyramid below is the target for Phase 4.

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
- **core/lifecycle**: exhaustive transition table; guards both ways; transition log;
  injected-clock cooling; invalid-transition rejection.
- **core/visibility**: culling decisions (margin, semantic flags, direction); culling does
  not mutate materialization state (boundary assertion).
- **core/materialize**: priority order; budget yield; eviction order; no starvation.
- **core/layout**: UAX #29 sample vectors; KP quality on known texts; block/table/image
  layout golden fixtures (shared vectors with the JS runtime).
- **core/glyph cache**: budget enforcement; eviction coupling with lifecycle.
- **core/selection**: hit testing at boundaries; range merging; copy policy.
- **core/draw list**: batching, z-order, determinism (double-build equality).
- **render**: shader math unit tests (median-of-three coverage vs analytic), DPR mapping,
  texture budget, device-loss recovery path, WebGPU/GL parity fixtures.
- **wasm**: binding contract tests; budget reporting; destroyed-handle errors.
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
