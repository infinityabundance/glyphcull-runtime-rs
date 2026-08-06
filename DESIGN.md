# Design — glyphcull-runtime-rs

Status: Phases 4.1–4.2 landed (glyphcull-core reader + document model). Decisions with
rationale, alternatives, tradeoffs. Decisions that mirror the JS runtime are marked
(mirrors JS Dn); decisions specific to Rust/wgpu are new.

## D1. One architecture, two implementations (mirrors JS D1)

- The Rust runtime is not a rewrite with drift: subsystem boundaries, state machines, and
  determinism policies are identical to the JS runtime. The draw list and visible set are
  defined as cross-runtime contracts (validated in Phase 5 cross-implementation tests).
- **Rationale**: the architecture is the product; implementations prove it portable.

## D2. wgpu as the single renderer stack

- **Rationale**: wgpu is the production-grade WebGPU implementation with backends for
  Vulkan/Metal/DX12 (native), WebGPU (browser), and GL (WebGL2 fallback). One codebase
  delivers "WebGPU with WebGL fallback" plus native, matching the charter precisely.
- **Alternatives**: raw WebGPU (no native), raw OpenGL (no WebGPU), separate backends
  (drift).
- **Tradeoffs**: wgpu's API is async-first (device requests) — handled with a documented
  initialization lifecycle and a synchronous-readiness facade for wasm hosts.

## D3. Core/render/wasm/desktop crate split (mirrors JS modularity)

- `glyphcull-core` is platform-agnostic and headless-testable; `glyphcull-render` depends
  only on core; hosts depend on both. Dependency direction is strictly downward.
- **Rationale**: core is where correctness lives; render is where drivers live; hosts are
  thin. Mobile targets then need only a host crate.
- **Tradeoffs**: a trait boundary between core and render (the draw list as data, not
  calls) — deliberate: the draw list is serializable and testable.

## D4. Reader safety: no panics on untrusted input, no unsafe

- Bounds-checked arithmetic on all untrusted lengths; typed errors; fuzz corpus; SEAL
  verification. `unsafe` is forbidden in core except in one audited module if ever
  required, with justification + safety comment (policy identical to the compiler).
- **Rationale**: packages are untrusted inputs with decades-long archival lifetimes.

## D5. Determinism (mirrors JS D10)

- Same policy: no wall-clock in decisions, no unordered iteration (BTreeMap/indexed
  vectors, never HashMap, in output paths), explicit ordering. The double-build draw list
  test is a core test.

## D6. Cooperative materialization with budgets (mirrors JS D3)

- wasm32 has no threads; native could parallelize, but behavior must be identical across
  hosts, so scheduling is cooperative and deterministic everywhere. Parallelism is a
  documented future option behind the same scheduler contract.

## D7. Chunk lifecycle (mirrors JS D4)

- Identical state machine with transition log and injected clock.

## D8. Layout scope (mirrors JS D7)

- UAX #29 word boundaries + Knuth–Plass + simple shaping (combining-mark attachment);
  complex shaping/bidi/hyphenation are documented exclusions for v1.

## D9. MSDF reconstruction (mirrors JS D8)

- Median-of-three with texel→screen conversion; WGSL and GLSL from one logical program;
  rendering validation vs the reference rasterizer (shared golden images).

## D10. Memory discipline

- Counting-allocator harness for deterministic memory regression tests; budgets for glyph
  cache, texture bytes, and layout records; `destroy()` releases in dependency order.

## D11. Toolchain policy

- Edition 2021; MSRV pinned in `rust-toolchain.toml`; `wasm32-unknown-unknown` and Android
  targets recorded; CI verifies all targets build (where toolchains permit) and runs the
  full test matrix on native.

## D12. Public API surface (mirrors JS D12)

- Exactly `load/scroll/paint/select/copy/destroy`, exposed over wasm-bindgen and as a
  native library API; everything else is crate-internal. Destroyed handles reject calls
  with typed errors.

## D13. Owned payloads, lazy typed decoders (reader)

- The JS `Package` retains the input buffer and views raw sections zero-copy; the Rust
  `Package` copies each decoded payload into an owned buffer and drops the input after
  `parse`. Typed decoders (INFO/CHNK/STYL/CONT/GLYF/IMGS/SEAL) run lazily behind
  `OnceLock` caches — the same lazy model as the JS runtime.
- **Rationale**: Rust ownership makes packages fully self-contained and `Send`-friendly;
  the input slice (possibly a network or file buffer) can be released immediately.
- **Tradeoffs**: one copy of decoded section bytes at load; measured parse peak ≈ 1.3 ×
  package size (gate at 8 × in `tests/reader_memory.rs`). The SEAL overall hash covers
  header bytes `0..12`, so the package retains a 12-byte header prefix after the input
  is dropped.

## D14. Bounded incremental zlib decode (reader)

- `inflate_verified` never pre-allocates `decoded_len` — a hostile table can claim the
  full 2 GiB section cap with a tiny stored stream. It accumulates in 8 KiB reads, stops
  as soon as the output would exceed the authoritative length, and verifies the zlib
  header, `decoded_len` exactness, and the trailing Adler-32 explicitly (SPEC.md §1.5).
  Mirrors the JS runtime's bounded accumulation; `decompress-mismatch` is the typed
  surface for platform decode failures, exactly as in JS.
- **Rationale**: allocation is bounded by actual decoded content, never by an untrusted
  claim; the explicit wrapper checks are kept even though flate2 also validates the
  stream, because the SPEC mandates them and they pin the failure kinds.

## D15. Reader lint discipline (reader)

- `unsafe_code = forbid` workspace-wide; direct indexing is confined to the
  bounds-checked `Cursor` (a documented `#[allow(clippy::indexing_slicing)]`, mirroring
  `glyphcull-format::util`); `unwrap/expect/panic` are denied in production paths.
- Test code asserts via `expect` under explicit allows — the integration suites carry
  `#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]` at
  their crate roots, and the in-crate unit-test module scopes the same allows. Same
  policy as the compiler workspace.

## D16. The document model borrows the package (document model)

- `DocumentModel<'a>` holds shared references into the package's decoded payloads
  (chunk records, extras, content, atlases, images) and owns only the derived data: the
  cloned `Info` and the resolved style table. The JS model holds the same relationship
  (it keeps the `Package` and the decoded arrays by reference).
- **Rationale**: building a model must not duplicate the atlas pages or chunk graph;
  measured build peak ≈ 2 × package size vs the reader's 1.3 × (the delta is the
  resolved styles).
- **Tradeoffs**: the model's lifetime is tied to the package — the runtime owns both,
  and `build_document(package)` returns a model that borrows it, which is exactly the
  JS ownership shape.

## D18. The clock is borrowed, not owned (lifecycle)

- `LifecycleManager<'a, C: Clock>` holds `&'a C`, and `FakeClock` uses `Cell<u64>` so
  tests can advance time through the same shared reference the manager reads — the exact
  shape of the JS runtime, which passes the clock object by reference.
- **Rationale**: the clock is the determinism seam; owning it would fork time between the
  manager and its caller.

## D19. Deterministic enumeration in the lifecycle (lifecycle)

- All per-chunk tables are `BTreeMap<u32, _>` (ascending id), so `chunks_in_state` and
  `cooling_remaining` are deterministic — the JS `Map` gives insertion order, which is
  deterministic too, but id order is stronger (independent of call history).
- `cooling_remaining` treats a backwards wall clock as zero elapsed time
  (`saturating_sub`), which is strictly safer for eviction than the JS arithmetic
  artifact.

## D20. Visibility is a pure function with one read per chunk (visibility)

- `compute_visible_set(doc, geometry, viewport, margin)` never mutates either input; the
  geometry source is consulted exactly once per non-hidden chunk (asserted by a read
  count in tests). The visible set is a pure function, which is what makes determinism
  and cross-runtime contract tests possible.
- The walk is iterative (explicit stack, DESIGN.md D17) and two-phase: a pre-order pass
  collects semantic/geometric decisions, then a reverse pre-order pass aggregates
  structural visibility bottom-up — mirroring the JS recursion's post-order
  aggregation without the stack risk.

## D21. f32 document coordinates (visibility)

- `Rect`/`Viewport` use `f32` (the SPEC's glyph metrics are f32 and the renderer
  consumes f32), where the JS runtime uses `number` (f64). The inclusive-of-edges
  intersection test is exact for both at the values documents produce; cross-runtime
  rendering validation compares pixels with tolerance, not culling bits.

## D17. Iterative document traversals (document model)

- `all_ids` and `plain_text` use an explicit stack instead of recursion (the JS
  `plainText` recurses). Output order is identical (pre-order, direct text before
  children, `br` → newline).
- **Rationale**: the SPEC caps chunk depth at 2^16 and documents may be adversarial;
  native stack frames are larger than JS's, so recursion would overflow on deep chains.
  Stress-tested at 10k depth; the walk stays iterative by design.
