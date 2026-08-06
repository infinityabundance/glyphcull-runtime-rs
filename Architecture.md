# Architecture — glyphcull-runtime-rs

Status: Phases 4.1–4.2 landed (glyphcull-core reader + document model); living
document, updated as each crate lands. This runtime mirrors `glyphcull-runtime-js`
exactly in architecture — subsystem responsibilities, state machines, and data flow
are identical; only implementation differs.

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
- Viewport culling + semantic culling → visible set. Culling determines; it never
  materializes. Sequential frontier walk with cached layout records (identical discipline
  to the JS runtime).

### 3.4 materialization
- Priority queue (deterministic priorities: viewport distance + direction of travel),
  time/memory budgets, cooperative yields (in wasm: budget per call; in native: same
  discipline so behavior is identical across hosts).
- Eviction: Visible → Cooling → Evicted with a cooling period; LRU-with-age, deterministic.

### 3.5 lifecycle
- The chunk lifecycle state machine — identical states and transitions to the JS runtime:
  Compressed → Queued → Materializing → Visible → Cooling → Evicted. Transition log with
  injected clock for tests.

### 3.6 layout
- UAX #29 word boundaries; Knuth–Plass line breaking; block layout; tables (colspan/
  rowspan); images. Same layout scope boundary as the JS runtime (simple shaping +
  combining marks; complex shaping/bidi documented exclusions).
- Deterministic: same inputs ⇒ same layout records.

### 3.7 glyph cache
- Budgeted cache of glyph instances keyed by (atlas, codepoint, size, color); evicted with
  its owning chunk's lifecycle.

### 3.8 selection
- Logical ranges (chunk id + offsets); geometric projection for highlight rendering;
  plain-text copy with the documented policy (paragraph breaks, table cells → tabs, rows →
  newlines).

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
