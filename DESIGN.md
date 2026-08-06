# Design — glyphcull-runtime-rs

Status: Phases 4.1–4.9 landed (glyphcull-core complete; glyphcull-render with the wgpu
MSDF renderer). Decisions with rationale, alternatives, tradeoffs. Decisions that mirror
the JS runtime are marked (mirrors JS Dn); decisions specific to Rust/wgpu are new.

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

## D17. Iterative document traversals (document model)

- `all_ids` and `plain_text` use an explicit stack instead of recursion (the JS
  `plainText` recurses). Output order is identical (pre-order, direct text before
  children, `br` → newline).
- **Rationale**: the SPEC caps chunk depth at 2^16 and documents may be adversarial;
  native stack frames are larger than JS's, so recursion would overflow on deep chains.
  Stress-tested at 10k depth; the walk stays iterative by design.

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

## D22. The scheduler owns its lifecycle (materialization)

- `MaterializationScheduler` holds its `LifecycleManager` by value and exposes
  `lifecycle()`/`lifecycle_mut()` accessors, instead of borrowing `&mut` (which would
  freeze all other access for the scheduler's lifetime — Rust's exclusivity where the
  JS shares references freely).
- **Rationale**: composition keeps the lifecycle's invariants intact (every transition
  still goes through the manager) while letting the runtime and tests read state
  freely. The runtime composes one scheduler per document.

## D23. Exact f64 priority arithmetic (materialization)

- `priority_key` mirrors the JS `number` arithmetic (`tier × 2³² + ahead-bit + ordinal`)
  in `f64`, so ordering is identical across runtimes; the heap comparator reproduces the
  JS `!==`/`<` semantics exactly, including NaN behavior (never less) — keys are finite
  for realistic documents.

## D24. Layout precision and record ownership (layout)

- **Precision**: text measurement and Knuth–Plass run in `f64` (the JS `number`):
  `measure_run` widens the atlas's `f32` metrics exactly (`f64::from(f32)`), so a run's
  advances and widths match the JS runtime at f64 precision. Geometry (block boxes,
  line positions, glyph origins) is `f32` at the record boundary, matching `Rect` and
  the renderer contract. The two meet at explicit `as f32` casts (see R1 for the one
  measurement divergence).
- **Ownership**: a nested block exists exactly once — `records` (`BTreeMap<u32,
  Rc<BlockLayout>>`) and a parent's `children` hold the same `Rc`, exactly like the JS
  object sharing. Without this, an n-deep tree would be duplicated at every ancestor
  (O(n²) memory). Records are immutable after construction, so `Rc` (not
  `Rc<RefCell<…>>`) is safe. `run_rects` gives visibility/hit-testing the per-run
  rectangles, mirroring the JS `runRects`.
- **Records are deterministic**: `BTreeMap` keyed by chunk id and `Rc` give stable
  identity; the double-build equality test (`tests/layout.rs::is_deterministic…`)
  compares two full engines' record maps.
- **KP active-list exhaustion is JS parity**: when every active node is infeasible at a
  breakpoint (a narrow column whose lines each fit one word), the active list dies and
  both runtimes fall back to a single line covering the paragraph (pinned in
  `tests/layout_kp.rs::narrow_columns_fall_back…`). The paper's deactivation rule is a
  documented future improvement; for v1 the fallback is deterministic and identical
  across runtimes.

## D25. The sequential frontier (layout)

- `extend_to`/`materialize`/`next_frontier_block` mirror the JS exactly: top-level
  blocks are laid out in document order; `materialize` is idempotent (the same `Rc` is
  returned for a repeated call) and advances the cursor by the laid block's height;
  `extend_to(f64::INFINITY)` lays out the whole document. Streaming is the point: chunks
  beyond the viewport are never laid out.
- Layout of a table cell measures it twice (height pass at the natural width, then the
  final placement) exactly like the JS; the transient record is overwritten by the
  final one, and the rowspan growth rule (`rowHeights[last spanned row] += h -
  current`) is mirrored line-for-line.

## D26. Layout recursion is bounded by document depth (layout)

- Container layout (`layout_quote`, `layout_list`, `layout_cell`, `layout_table`) is
  recursive — it mirrors the JS engine, which is recursive too — so the layout stack
  depth is the document depth. The SPEC caps depth at 2^16; hosts must provide a
  commensurate native stack (the wasm host sets `--stack-size`; the desktop host's main
  thread default suffices).
- **Rationale**: converting container layout to an explicit work stack would diverge
  structurally from the JS algorithm for no behavioral gain; the bound is documented
  and stress-tested (a 5000-deep quote chain lays out on a 64 MiB-stack thread in
  `tests/layout_stress.rs`). Document walks (`all_ids`, `plain_text`) remain iterative
  (D17) — the common adversarial path.

## D27. Deterministic LRU without a linked list (glyph cache)

- The JS `GlyphCache` keeps its stamps in a `Map` whose insertion order *is* the LRU
  order (`get`/`put` delete + re-insert to touch). Rust's `BTreeMap` has no insertion
  order, so the LRU chain is a monotonic touch counter: every `get`/`put` takes the next
  `u64`, and eviction pops the smallest counter. The eviction *set* is identical to the
  JS (least recently used first); the order map's key tie-break never fires because
  counters are unique.
- **Rationale**: `indexmap` would add a dependency; a linked list would need unsafe or
  `Rc<RefCell>`; the counter is deterministic, allocation-free after setup, and O(log n)
  per operation like the JS `Map`.

## D28. Sampling convention and target formats (render)

- **Pixel-center sampling (DESIGN.md D9)**: the CPU reference samples texel centers at
  `(i + 0.5)` in texel space. wgpu normalizes the half-texel phase across backends
  (Vulkan-style), so the WGSL shader samples texel centers directly from the stamp UVs
  — no shift. The JS WebGL renderer applies a `+0.5/size` UV shift because GLSL's
  `texture2D` samples at `uv·size − 0.5`; the Canvas 2D fallback is phase-exact. All
  three agree with the reference within tolerance; pinned by
  `tests/validation.rs::stamps_and_plan_agree_on_the_sampling_convention`.
- **Non-sRGB targets**: the pipeline requires a non-sRGB target format so the
  premultiplied u32 colors and the MSDF distance channels are stored raw (an sRGB
  target would re-encode the shader output — double-encoding our already-sRGB colors,
  and gamma-correcting the distance field). This matches the JS WebGL1 renderer's
  gamma-space blending exactly.
- **One logical program**: wgpu compiles the single WGSL source to GLSL (gles backend)
  and SPIR-V (Vulkan) via naga; the tests validate both translations headlessly. The
  JS's hand-written GLSL expresses the same math.

## D29. The host is a self-referential document pipeline (wasm host)

- **One owned object, not glued lifetimes**: the host pins the borrow chain
  `Package → DocumentModel → LayoutEngine` with `ouroboros` instead of joining three
  separately-owned objects with raw pointers. `doc` and `layout` are declared
  `#[covariant] #[borrows(...)]`; the host is constructed once (the package is
  validated by an independent build first, so the pinned re-build's `expect` is
  provably safe) and destroyed as a whole.
- **One lifecycle**: `MaterializationScheduler` owns the `LifecycleManager` (D22); the
  host reaches it through `scheduler.lifecycle_mut()` — there is exactly one manager
  per document, and every transition still goes through the scheduler's guard.
- **`with_mut` field discipline**: ouroboros hands tail fields to the closure as
  `&mut T` and borrowed fields as `&T`. The scheduler's geometry callback borrows only
  the layout engine (`let layout = &*fields.layout`), and stamp preparation is a free
  function (`prepare_stamps(cache, atlases, record)`) so the borrow checker splits the
  worker's `layout` read from its `cache` write — no interior mutability needed.
- **The sink is a trait seam**: `FrameSink` (uploads, draw, resize, destroy) is a
  `Box<dyn FrameSink>`; the wgpu implementation lives in `sink.rs`, and native tests
  inject a recording sink — the entire host is tested with no wasm and no GPU.
- **The surface is `'static` by ownership**: `create_surface(SurfaceTarget::Canvas(canvas))`
  passes the canvas by value — the surface keeps its own copy of the handle, so the
  returned `Surface<'static>` matches the sink's field. Borrowing the canvas would
  yield a frame-bound surface that cannot be stored.
- **`#[cfg(web)]` boundary**: `load` (and its `load_inner`/`parse_options` helpers and
  the canvas/future imports) are gated on `web`; `host.rs` is platform-agnostic and
  compiles on native and Android. CI builds native (tests), wasm32, and both Android
  ABIs from the same workspace member.

## R-series: deliberate divergences from the JS runtime (correctness)

The Rust runtime is a faithful mirror; these are the places it deliberately does not
reproduce a JS defect, each pinned by a test.

### R1. Measurement hardening (measure)

- `sum_advances` clamps its end bound, and measurement is per codepoint, so astral text
  (surrogate pairs) measures one glyph per codepoint; the JS `laidToken` sums
  `glyphs[0..text.length]` in UTF-16 units, which reads past the array and throws on
  astral text. For BMP text (all fixtures) the results are identical.

### R2. Token→line index mapping (layout)

- A KP item is a box (even index) plus a break item (odd index) per token, so a line
  spanning items `[start..end]` owns tokens `[start/2 .. (end-1)/2]`. The JS
  `buildStyledLine` filters tokens by `tokenIndex ∈ [br.start, br.end]` — comparing
  token positions to item indices. That is correct only for one-word lines; for
  multi-word lines it over-fills early lines and starves later ones (empty lines),
  verified against the JS runtime with a probe (golden paragraph at 220px breaks
  `[0..1],[2..22]`; the JS assigns "Deterministic " / "golden fixture with a link.",
  the Rust assigns "Deterministic" / " golden fixture with a link.").
- The Rust runtime implements the correct mapping; break geometry (line count, y
  positions, heights, ratios) is identical to the JS. Pinned by
  `tests/layout.rs::wrapped_paragraph_preserves_every_token_exactly_once` and the
  long-paragraph partition check in `tests/layout_stress.rs`.

### R3. Selection offsets are char-based, not UTF-16 (selection)

- `TextPosition.offset` and the run `start`/`end` offsets count Unicode scalar values;
  the JS counts UTF-16 code units. For BMP text (all fixtures) the two are identical;
  for astral text the JS can land mid-surrogate (a selection or slice that corrupts
  text), while the Rust offsets are always well-formed. `copy_text` slices by char
  offsets accordingly.

### R4. Cache budgets are u64 (glyph cache)

- `GlyphCache::new(budget_bytes: u64)` cannot express a negative or NaN budget, so the
  JS `RangeError` on invalid budgets is unrepresentable; a zero budget is valid and
  evicts every stamp on insert (pinned by the in-crate unit test).

### R5. Container-nested images and rules render (draw list)

- The JS `emitBlockLayout` (the children path of `DrawListBuilder`) duplicates
  `emitBlock`'s body but drops the ruler/image branches, so an image or `hr` nested
  inside a quote, list item, or table cell emits no quad (images inside paragraphs
  still render, because text blocks keep no children). The Rust builder shares one
  emission body between both paths, so nested images and rules emit exactly once
  (the visible-set `emitted` guard prevents doubles). Pinned by
  `tests/draw_list.rs::nested_image_inside_a_quote_renders…` and
  `nested_hr_inside_a_list_item_renders…`.

### R6. Zero reconstruction sizes are unrepresentable (render/msdf)

- The JS `reconstruct` throws a `RangeError` for a zero or fractional output size.
  Rust's `usize` makes fractions unrepresentable, and a zero dimension yields an
  empty buffer defensively (the JS contract becomes an early return; pinned by
  `tests/msdf.rs::a_zero_output_size_is_defensive_empty`).
