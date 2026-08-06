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

### 4.5 — glyphcull-core materialization (no benchmark: measured in 4.9)

The scheduler's hot path is the priority queue; its behavior (ordering, starvation
freedom, budget discipline) is covered by `tests/materialize.rs`, including a 100-case
no-starvation property. End-to-end materialization throughput is measured once the
renderer lands (4.9), where frames are real.

### 4.4 — glyphcull-core visibility (no benchmark: bounded by the walk)

Culling is one O(n) walk over the visible portion of the document; per-frame cost is
bounded by the walked set (100k-chunk cull stress in `tests/visibility_stress.rs`,
RSS gate at 8 × package). No criterion bench: the walk has no tunable constants to
regress; determinism and invariants are property-tested.

### 4.3 — glyphcull-core chunk lifecycle (no benchmark: trivially bounded)

State transitions are O(log n) map operations with no allocation-sensitive path; the
materializer (4.5) is where budget behavior is measured. Determinism and guard coverage
are enforced by `tests/lifecycle.rs` (exhaustive table + 300-case model-based property
run).

### 4.2 — glyphcull-core document model (committed benchmarks + memory gate)

Same environment as 4.1. The model borrows the package's decoded payloads, so build cost
is validation + style resolution, not data copying.

| Benchmark | Time |
|---|---|
| `document/build/golden-22-chunks` (parse + build + walks) | 748 µs |
| `document/build/wide-100k-chunks` (parse + build + walk) | 19.2 ms |

Memory: `tests/document_memory.rs` measures the RSS high-water delta over 25 parse+build
iterations of the 855 KiB golden: **peak ≈ 1.63 MiB ≈ 2 × package size** (gate: 8 ×
package size) — the borrowed-model design adds the resolved style table only.

Commands: `cargo bench -p glyphcull-core --bench document_build`,
`cargo test --test document_memory -- --nocapture`.

### 4.1 — glyphcull-core reader (committed benchmarks + memory gate)

Environment: AMD Ryzen 7 9800X3D (8 cores / 16 threads), Linux 7.1.4 (CachyOS), rustc
1.96.0, release profile, criterion 0.5. Absolute numbers; re-measure per machine.

| Benchmark | Time | Throughput (approx.) |
|---|---|---|
| `reader/parse/v1-minimal` (195 B) | 2.78 µs | — |
| `reader/parse/pipeline-golden-855k` (854,875 B file) | 733 µs | ≈ 1.2 GB/s file |
| `reader/parse/synthetic-1m-decoded` (1 MiB text, zlib) | 495 µs | ≈ 2.1 GB/s decoded |
| `reader/full-contract/golden…` (parse + all typed decoders + SEAL) | 1.45 ms | ≈ 590 MB/s |

Memory: `tests/reader_memory.rs` measures the RSS high-water delta over 25 parse+
full-contract iterations of the 855 KiB golden: **peak ≈ 1.09 MiB ≈ 1.3 × package size**
(gate: 8 × package size). The 10 MB `load()` budget (§2) is compatible with this
per-package footprint; re-measured at 4.2 (document model) and again when streaming
loads land.

Commands: `cargo bench --all` (criterion), `cargo test --test reader_memory -- --nocapture`.
