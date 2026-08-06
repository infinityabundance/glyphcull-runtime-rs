# Roadmap — glyphcull-runtime-rs

Execution order is mandatory and matches the root `PLAN.md`. This runtime starts only after
the JS runtime phase completes (cross-runtime contract validation requires both).

## Phase 4 — Rust runtime (as sequenced in the master plan)

- [x] **4.1 glyphcull-core/reader**: independent `.cull` reader per SPEC.md; validation;
      SEAL; malformed corpus + truncation tests; contract tests against compiler golden
      fixtures.
- [x] **4.2 glyphcull-core/document**: Document model; load-time validation.
- [x] **4.3 glyphcull-core/lifecycle**: chunk lifecycle state machine; transition log;
      property tests; injected clock.
- [x] **4.4 glyphcull-core/visibility**: viewport + semantic culling; determinism tests.
- [x] **4.5 glyphcull-core/materialize**: priority queue; budgets; cooperative scheduling;
      eviction; starvation tests.
- [x] **4.6 glyphcull-core/layout**: UAX #29 word boundaries; Knuth–Plass; block layout;
      tables; images; golden layout fixtures (shared with JS runtime).
- [x] **4.7 glyphcull-core/glyph cache + selection**: budgets; logical selection; copy policy.
- [x] **4.8 glyphcull-core/draw list**: deterministic batching; serialization for tests.
- [x] **4.9 glyphcull-render**: wgpu init lifecycle; WebGPU backend; GL fallback; MSDF
      shaders (WGSL); texture management; device-loss recovery.
- [x] **4.10 glyphcull-wasm**: wasm32 bindings; tiny API; memory budget reporting.
- [x] **4.11 glyphcull-desktop**: winit host; input wiring; windowing.
- [x] **4.12 Mobile targets**: Android target build verification (CI builds the library
      targets for both ABIs); the mobile host crate is recorded as a future candidate with
      core/render/host readiness demonstrated (winit compiles for Android via the pure-Rust
      `android-native-activity` backend; iOS is target-ready in the same split).
- [x] **Test pyramid complete**: unit/integration/property/stress/memory/performance layers
      per subsystem (see TESTING.md); rendering validation vs a committed golden image
      (`crates/glyphcull-render/tests/fixtures/golden-document.png`, headless CPU reference
      rasterizer); CI green across native + wasm32 + Android.
- [x] **Documentation complete**: Architecture.md/DESIGN.md updated with the crate diagram
      and D29/D30; ROADMAP/TESTING/PERFORMANCE/CHANGELOG in lockstep; API docs (the
      workspace denies `missing_docs`, doc build green under `-D warnings`).

## Definition of done (every phase)

- All tests green; clippy `-D warnings`; fmt check; no `TODO`/`FIXME`/`todo!()`/
  `unimplemented!()`; docs in lockstep; memory + performance baselines recorded; every bug
  gets a regression test.

## Future phase candidates (recorded, not scheduled)

- Threaded materialization behind the scheduler contract.
- Complex-script shaping / bidi (with compiler scope extension).
- Android/iOS host crates (core/render/host already target-ready: Android ABIs build in
  CI; iOS shares the same crate split and winit/wgpu support).
