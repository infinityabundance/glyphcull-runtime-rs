# Performance — glyphcull-runtime-rs

Status: Phases 4.1–4.9 landed (reader, document model, lifecycle, visibility,
materialization, layout, glyph cache, selection, draw list, render). Budgets declared;
baselines measured per phase. No premature optimization: deterministic architecture,
then profile, measure, optimize on evidence.

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

### 4.9 — glyphcull-render (committed benchmarks, headless)

Same environment as 4.1. The plan benchmark measures the headless hot path (the GPU
frame cost adds the wgpu submission on the desktop host, measured at 4.11); the MSDF
benchmark measures the CPU reference reconstruction of a golden glyph.

| Benchmark | Time |
|---|---|
| `render-plan/golden-full-pipeline` (parse → layout → draw list → plan) | 759 µs |
| `render-plan/msdf-reconstruct-glyph-1x1` | 322 µs |

GPU memory: the texture manager is self-limiting by construction (the byte budget,
`DEFAULT_TEXTURE_BUDGET` 128 MiB, evicts oldest-uploaded textures deterministically).
The renderer itself cannot run headless in CI — its logic (plan batching, premultiplied
vertex data, shader translation) is validated by the pure-module suites; GPU execution
is measured on the desktop host (4.11).

Commands: `cargo bench -p glyphcull-render --bench plan_golden`.

### 4.8 — glyphcull-core draw list (committed benchmarks)

Same environment as 4.1. The benchmark builds the full-document draw list over the
laid-out golden (the per-frame hot path), with and without selection quads.

| Benchmark | Time |
|---|---|
| `draw-list/full-doc-no-selection` | 845 ns |
| `draw-list/full-doc-with-selection` | 876 ns |

The draw list is bounded by the visible content (never the document size); the
repeated-build stability test and the random-subset proptest pin determinism.

Commands: `cargo bench -p glyphcull-core --bench draw_list_golden`.

### 4.7 — glyphcull-core glyph cache + selection (committed benchmarks)

Same environment as 4.1. The cache benchmark measures the deterministic LRU (monotonic
counter; DESIGN.md D27) at 64k stamps with an unlimited budget; the selection benchmarks
run against the fully laid-out golden document (22 chunks, ~40 lines).

| Benchmark | Time |
|---|---|
| `glyphs/cache-put-64k-unlimited` | 9.17 ms (≈ 7.0 M puts/s) |
| `glyphs/cache-get-64k-unlimited` | 6.65 ms (≈ 9.6 M gets/s) |
| `glyphs/hit-test-point` | 13.9 ns |
| `glyphs/range-quads-full-doc` | 687 ns |
| `glyphs/copy-text-full-doc` | 811 ns |

Selection is proportional to the covered text (not the document); `copy_text` on a
100k-chunk span is linear in the covered ids. The glyph cache is self-limiting by
construction — the byte budget caps retained bytes, so no separate memory gate is needed
(the LRU eviction is covered by `tests/glyph_cache.rs`).

Commands: `cargo bench -p glyphcull-core --bench glyph_selection`.

### 4.6 — glyphcull-core layout (committed benchmarks + memory gate)

Same environment as 4.1 (AMD Ryzen 7 9800X3D, Linux, rustc 1.96.0, release, criterion 0.5).
The golden full-document benchmark includes parse + build + layout of the whole document;
the paragraph benchmark isolates the Knuth–Plass dynamic program on realistic prose (the
active list is deduplicated per fitness class, so it stays small on long paragraphs).

| Benchmark | Time |
|---|---|
| `layout/golden-full-document` (22-chunk golden, whole doc) | 760 µs |
| `layout/paragraph-2000-words` (2000-word paragraph, KP) | 2.06 ms |

Memory: `tests/layout_memory.rs` measures the RSS high-water delta over 25 parse+build+
full-layout iterations of the 855 KiB golden: **peak ≈ 1.88 MiB ≈ 2.3 × package size**
(gate: 8 × package size) — records, lines, and glyphs are small relative to the atlas
pages the reader owns.

Layout correctness notes: the 3000-word paragraph stress (KP active-list growth) and the
5000-deep quote chain (layout recursion depth, 64 MiB-stack thread) both complete fast;
the 2000-block streaming test materializes block-by-block with no backtracking.

Commands: `cargo bench -p glyphcull-core --bench layout_golden`,
`cargo test --test layout_memory -- --nocapture`.

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
