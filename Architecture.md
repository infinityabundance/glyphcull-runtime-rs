# Architecture — glyphcull-runtime-rs

Status: Phases 4.1–4.7 landed (glyphcull-core reader + document model + lifecycle +
visibility + materialization + layout + glyph cache/selection); living document, updated
as each crate lands. This runtime mirrors `glyphcull-runtime-js` exactly in architecture
— subsystem responsibilities, state machines, and data flow are identical; only
implementation differs.

## 1. Position

A native compiled-document host. Consumes `.cull` packages; never sees HTML, Markdown, or
CSS. Same pipeline as the JS runtime:

```
Compiled Document (.cull)
        ↓
Visibility System
        ↓
Streaming Runtime
        ↓
GPU Draw List
        ↓
Pixels
```

## 2. Crate architecture

```
                 ┌─────────────────────┐
                 │  glyphcull-desktop  │  native host (winit/wgpu)
                 └──────────┬──────────┘
                            │
                 ┌──────────▼──────────┐
                 │  glyphcull-wasm     │  wasm32 bindings (same tiny API)
                 └──────────┬──────────┘
                            │
                 ┌──────────▼──────────┐
                 │  glyphcull-render   │  wgpu: WebGPU backend + GL(WebGL2) fallback
                 └──────────┬──────────┘
                            │  draw list (commands), textures, atlases
                 ┌──────────▼──────────┐
                 │  glyphcull-core     │  everything platform-agnostic
                 └─────────────────────┘
```

Dependency direction is strictly downward. `glyphcull-core` knows nothing about wgpu, winit,
or wasm-bindgen; `glyphcull-render` knows nothing about hosts. This is what makes the core
testable headlessly and portable to every target.

## 3. Subsystems (glyphcull-core)

### 3.1 reader

**Delivered (4.1).** `glyphcull-core::reader` — the second independent implementation of
SPEC.md, after the JS one. Contract-tested against the compiler's golden fixtures
(`crates/glyphcull-core/tests/fixtures/`, refreshed by `scripts/refresh-fixtures.sh`) with
`cull inspect` diagnostics pinned.

- Container: header + section table validation (overflow-checked arithmetic, reserved
  bits, `section_count` caps), per-section zlib decode with explicit header + Adler-32
  verification and bounded incremental accumulation, `decoded_len` exactness, per-section
  CRC-32, duplicate-kind rejection, unknown kinds addressable.
- Typed decoders: INFO (deterministic JSON subset, strict keys), CHNK (records + all four
  extra kinds), STYL (all 16 property tags), CONT (text + image refs), GLYF (atlases +
  sorted kerning + page texels), IMGS (RGBA8/RGB8), SEAL (hash tree). Lazy behind
  `OnceLock` caches; payloads are owned (input slice droppable after `parse`).
- SEAL verification at load: per-section SHA-256 over decoded payloads + overall hash
  over header bytes `0..12` and covered sections in canonical order.
- Bounds-checked (`checked_add` on all untrusted offsets/lengths), typed errors (SPEC.md
  §1.6), never panics on input.

### 3.2 document model

**Delivered (4.2).** `glyphcull-core::document` — the trusted runtime view of a
package, mirroring the JS `DocumentModel`. **No geometry lives here**.

- `build_document` validates at load: required sections (INFO/CHNK), chunk-graph
  invariants (single `document` root, dense ids/ordinals, sibling-ring consistency,
  parent/depth consistency, full reachability, no cycles), reference resolution
  (style ids, content indices, image refs against IMGS, resolved `font_id` against
  GLYF), and the five INFO count cross-checks.
- `ResolvedStyle`: all 16 SPEC §2.3 properties with defaults applied (`resolve_style`).
- Trusted views: pre-order `all_ids`, sibling `child_ids`, per-chunk `extras_for`,
  `direct_text`, `image_ref`, and iterative `plain_text` (document-order copy text;
  `br` → newline, code blocks contribute their direct text).
- The model borrows the package's decoded payloads — building copies only the
  resolved style table (no chunk/atlas duplication).

### 3.3 visibility

**Delivered (4.4).** `glyphcull-core::visibility` — viewport culling + semantic culling →
visible set. Culling determines; it never materializes.

- Semantic culling: the `hidden` flag excludes a chunk and its whole subtree.
- Geometric culling: a renderable chunk is visible iff its rect (from the
  `GeometrySource`, implemented by layout) intersects the viewport expanded by a margin
  (inclusive of edges). Geometry-less renderable chunks are `not_yet_visible` (the
  materialization frontier), never absent; structural chunks are visible iff a descendant
  is visible.
- Iterative pre-order walk (explicit stack); the geometry source is consulted exactly
  once per non-hidden chunk. The visible set is a pure function of (document, geometry,
  viewport, margin).

### 3.4 materialization

**Delivered (4.5).** `glyphcull-core::materialize` — the streaming scheduler.

- Deterministic binary min-heap (mirrors the JS `PriorityQueue`): `priority_key` is a
  pure function of (geometry, viewport, direction of travel) — intersecting chunks
  first in document order, then 1024 px distance tiers with chunks ahead of travel
  preferred, then document order; geometry-less chunks sort by document order (the
  sequential frontier).
- `reconcile` drives the visible set into the lifecycle (enqueue/requeue/dequeue/cancel/
  cull); `run_frame` executes work within the per-frame budget with cooperative yields
  (a yielding chunk pauses back to Queued and re-queues behind an anti-starvation
  penalty); `tick` expires cooling chunks through the worker; `evict_for_memory`
  releases the furthest visible chunks first under memory pressure (never failing).
- The scheduler owns its `LifecycleManager` (composition) and borrows the clock; every
  state change goes through the lifecycle — the scheduler never mutates chunk state
  directly.

### 3.5 lifecycle

**Delivered (4.3).** `glyphcull-core::lifecycle` — the chunk lifecycle state machine,
identical states and transitions to the JS runtime: Compressed → Queued → Materializing →
Visible → Cooling → Evicted.

- Guarded transitions (`hidden` never enqueues; `expire` needs the cooling period elapsed
  and no selection pin), transition log with the injected clock
  (`glyphcull-core::clock`: `RealClock`/`FakeClock`), selection pins, per-chunk cooling
  periods.
- All per-chunk tables are `BTreeMap`s — enumeration is deterministic (ascending chunk
  id). Model-based property tests drive random event sequences against a reference
  transition table.

### 3.6 layout

**Delivered (4.6).** `glyphcull-core::layout` — materialization turns semantic chunks into
renderable geometry (mirrors the JS `src/layout/*`). **No CSS cascade lives here** — styles
are resolved in the package.

- `breaks` — UAX #29-grounded tokenization (word boundaries, closing/opening punctuation
  binding, CJK between-ideograph breaks, forced newlines). Exact ECMAScript `\s` and
  `\p{L}\p{N}` classes so whitespace runs and word runs match the JS runtime byte-for-
  byte.
- `kp` — the Knuth–Plass dynamic program over boxes/glue/penalties, mirrored operation for
  operation (prefix sums, fitness classes, the twice-around fitness pass, the active-list
  dedup by fitness class, and the single-line fallback when the active list exhausts on
  pathological narrow columns — JS parity, DESIGN.md D24).
- `measure` — per-codepoint advances, kerning (binary search over the SPEC-sorted pair
  table), combining-mark attachment (advance 0), letter-spacing, and the half-em tofu
  fallback for missing codepoints. f64 arithmetic on f32 atlas metrics (DESIGN.md D24).
- `layout` — the `LayoutEngine`: sequential frontier (`extend_to`/`materialize`/
  `next_frontier_block`), text blocks (KP breaking with first-line indent), preformatted
  code blocks, quotes (24px indent), lists (markers: disc/circle/square/decimal/alpha/
  roman), images (intrinsic aspect ratio, dpr-aware), horizontal rules, and the
  deterministic table auto layout (column counts, natural widths, scaling, row heights,
  colspan/rowspan with the last-spanned-row growth rule). Every record is absolute
  geometry in document pixels; records are `Rc`-shared between the `records` index and
  parent `children` (a block exists exactly once; DESIGN.md D24).
- The engine implements the visibility `GeometrySource` contract — `rect` answers block
  and run geometry for culling and hit testing.
- **Deliberate divergence**: the token→line index mapping corrects a JS defect that
  mispartitions multi-word lines (DESIGN.md R2); break geometry is identical to the JS.
- Deterministic: same inputs ⇒ same layout records (double-build equality test);
  streaming: chunks beyond the viewport are never laid out.

### 3.7 glyph cache

**Delivered (4.7).** `glyphcull-core::glyphs` — budgeted cache of prepared glyph stamps
(mirrors the JS `src/glyphs/cache.ts`).

- `prepare_glyph` derives a size-specific stamp per (atlas, codepoint, size, color): the
  pixel-space quad (bearing × size ± padding·scale), the UV rect (box/page texels), the
  f64 advance, and the flags — the SPEC §2.5 placement convention, exactly as the JS.
- `GlyphCache`: a byte budget with deterministic LRU eviction (a monotonic touch counter
  reproduces the JS `Map` re-insertion order exactly — DESIGN.md D27), chunk-owning
  stamps with `release_chunk` (Evicted ⇒ entries released; shared stamps survive), and
  `clear` for `destroy()`. The budget is `u64`, so invalid budgets are unrepresentable
  (DESIGN.md R4).

### 3.8 selection

**Delivered (4.7).** `glyphcull-core::selection` — logical selection, geometric rendering
(mirrors the JS `src/selection/selection.ts`). Every function is a pure function of
(document, layout, point/selection): no state, no wall clock.

- `TextPosition` (run chunk + char offset) and normalized `Selection` (start ≤ end in
  document order), ordered via the dense pre-order index.
- `hit_test_point` — nearest line (smallest vertical distance) then nearest glyph center;
  `range_quads` — selection → per-line highlight quads with contiguous-piece merging
  (proportional fallback for glyph-less runs); `covered_chunk_ids`.
- `copy_text` — plain text with the documented boundary policy: runs of one block re-join
  (''), block boundaries → `\n`, cells of one table row → `\t`, rows → `\n`, `br` → `\n`.
- Offsets are char-based, not UTF-16 (DESIGN.md R3).

### 3.9 draw list
- Ordered, batched command sequence (glyph runs, image quads, selection quads,
  backgrounds), produced deterministically from the visible set.

## 4. Renderer (glyphcull-render)

- **wgpu** with two backends: WebGPU (native + WebGPU browsers) and GL (WebGL2 fallback;
  WebGL1-capable fallback where feasible).
- MSDF shader: median-of-three, texel→screen distance conversion via (size × dpr) /
  texels_per_em, screen-space derivative antialiasing; GLSL + WGSL sources generated from a
  single logical program (documented mapping).
- Texture management: atlas pages uploaded once; image payloads uploaded as RGBA/RGB
  textures; budgeted (max texture bytes) with eviction.
- Context/device loss: recreate device, re-upload textures, re-paint — transparent to the
  caller.
- The draw list is the renderer's only input; the renderer never sees the document model.

## 5. Hosts

- **wasm** (`glyphcull-wasm`): wasm-bindgen bindings for exactly
  `load/scroll/paint/select/copy/destroy`; canvas ownership per host convention;
  cooperative budget per call (no threads in wasm32).
- **desktop** (`glyphcull-desktop`): winit window + wgpu surface; input (scroll/select)
  wired to the same API.
- **mobile**: Android targets (`aarch64-linux-android`, `x86_64-linux-android`) recorded in
  the workspace; same core, host crate for Android is a future phase candidate (recorded in
  ROADMAP.md) — the core/render split is what makes this a compile-time story.

## 6. Chunk lifecycle (identical to JS runtime)

```
Compressed ──▶ Queued ──▶ Materializing ──▶ Visible
    ▲            │              │              │
    │            └──────────────┘              ▼
    └───────────────────(re-queue)── Cooling ◀──┘
                                        │
                                        ▼
                                     Evicted
```

Guarded transitions, transition log, injected clock. No hidden state.

## 7. Determinism and memory

- Same policies as the JS runtime: no wall-clock in decisions, no unordered iteration in
  output paths, explicit ordering everywhere.
- Memory: bounds-checked reader; counting-allocator test harness for regression testing;
  budgets enforced (glyph cache bytes, texture bytes, layout records).

## 8. Related documents

[`DESIGN.md`](DESIGN.md) · [`TESTING.md`](TESTING.md) · [`PERFORMANCE.md`](PERFORMANCE.md) ·
[`SECURITY.md`](SECURITY.md) · contract: `glyphcull-compiler/docs/format/SPEC.md` ·
terminology: `glyphcull-compiler/docs/format/GLOSSARY.md`.
