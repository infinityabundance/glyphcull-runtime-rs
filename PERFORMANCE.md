# Performance — glyphcull-runtime-rs

Status: Phase 0 (foundations). Budgets declared; measurement lands in Phase 4. No premature
optimization: deterministic architecture, then profile, measure, optimize on evidence.

## 1. Objectives

Same streaming objectives as the JS runtime, with the expectation of lower constant factors
and bounded memory from the start:

- Arbitrarily large packages remain interactive; viewport-bound work, document-independent
  steady-state memory.
- Native and wasm share the same scheduling discipline (deterministic behavior across
  hosts), so budgets are expressed in the same units.

## 2. Budgets (v1, to be confirmed by measurement)

| Metric | Budget (reference: 2020-era laptop) |
|---|---|
| `load()` for 10 MB package | < 60 ms native, < 150 ms wasm |
| First `paint()` after load | < 50 ms native (progressive) |
| Sustained scroll frame time | < 16 ms p95 (paint); materialization scheduled around it |
| Materialization throughput | ≥ 10 MB text/s native, ≥ 5 MB text/s wasm |
| Glyph cache budget | default 64 MB, configurable; eviction, never failure |
| Materialization budget/frame | default 8 ms (same semantics as JS runtime) |
| Steady-state memory overhead | < 6 × package size native (layouts + caches) |
| Draw list build | < 2 ms per viewport |

## 3. Methodology

- Criterion benches committed; baselines machine-relative (ratios) + absolute with
  environment recorded.
- Memory via counting-allocator harness (deterministic bytes, not RSS) + wasm memory
  growth measurement.
- Every optimization: profile-motivated, behavior-preserving (determinism tests pass),
  regression-covered.

## 4. Known hot paths (to profile, not optimize blind)

- Reader: CRC-32 (SIMD-ready), zlib (flate2), one-pass decode.
- Layout: KP line breaking (per-paragraph cache).
- Glyph generation: quad emission; batching by atlas page; zero-copy draw list assembly.
- Render: texture upload budget; shader constant binding; draw call batching.

## 5. Streaming invariants (tested)

- Visible set bounded by viewport, not document.
- Queue bounded by budget; priorities deterministic.
- Eviction releases glyph cache + layout records; steady-state memory independent of
  document size.

## 6. Evidence log

To be appended as Phase 4 lands: environment, commands, measurements, decisions.
