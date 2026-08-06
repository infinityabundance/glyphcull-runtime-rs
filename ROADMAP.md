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
- [ ] **4.4 glyphcull-core/visibility**: viewport + semantic culling; determinism tests.
- [ ] **4.5 glyphcull-core/materialize**: priority queue; budgets; cooperative scheduling;
      eviction; starvation tests.
- [ ] **4.6 glyphcull-core/layout**: UAX #29 word boundaries; Knuth–Plass; block layout;
      tables; images; golden layout fixtures (shared with JS runtime).
- [ ] **4.7 glyphcull-core/glyph cache + selection**: budgets; logical selection; copy policy.
- [ ] **4.8 glyphcull-core/draw list**: deterministic batching; serialization for tests.
- [ ] **4.9 glyphcull-render**: wgpu init lifecycle; WebGPU backend; GL fallback; MSDF
      shaders (WGSL + GLSL); texture management; device-loss recovery.
- [ ] **4.10 glyphcull-wasm**: wasm32 bindings; tiny API; memory budget reporting.
- [ ] **4.11 glyphcull-desktop**: winit host; input wiring; windowing.
- [ ] **4.12 Mobile targets**: Android target build verification; mobile host crate recorded
      as future candidate with core/render readiness demonstrated.
- [ ] **Test pyramid complete**; rendering validation vs golden images; CI green across
      native + wasm.
- [ ] **Documentation complete**: Architecture.md/DESIGN.md updated; diagrams; API docs
      (rustdoc with `#![deny(missing_docs)]`).

## Definition of done (every phase)

- All tests green; clippy `-D warnings`; fmt check; no `TODO`/`FIXME`/`todo!()`/
  `unimplemented!()`; docs in lockstep; memory + performance baselines recorded; every bug
  gets a regression test.

## Future phase candidates (recorded, not scheduled)

- Threaded materialization behind the scheduler contract.
- Complex-script shaping / bidi (with compiler scope extension).
- Android/iOS host crates (core/render already target-ready).
