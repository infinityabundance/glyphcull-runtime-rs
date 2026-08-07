# glyphcull-runtime-rs

[![CI](https://github.com/infinityabundance/glyphcull-runtime-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/infinityabundance/glyphcull-runtime-rs/actions/workflows/ci.yml)

**A compiled GPU document runtime.**

The native GlyphCull runtime in Rust. Architecture is identical to
`glyphcull-runtime-js`; only implementation changes. Compiles to:

> **Status: v0.1 experimental infrastructure prototype.** GlyphCull currently supports
> Latin-script, per-codepoint text rendering for MVP validation. It is not yet a production
> typography engine for complex shaping, bidirectional text, vertical text, Indic scripts,
> Arabic shaping, or full international publishing. GlyphCull is not DRM and does not make
> scraping impossible; it raises the cost of ordinary DOM-based extraction by replacing HTML
> text nodes with a compiled, streamed, GPU-rendered document runtime (see SECURITY.md).

- `wasm32-unknown-unknown` (browser host)
- desktop (Linux/macOS/Windows)
- mobile (`aarch64-linux-android`, `x86_64-linux-android`)

Renderer: **WebGPU** (via wgpu) with **WebGL** fallback.

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

## Crate layout

```
crates/
  glyphcull-core/     — reader, Document model, visibility, materialization, lifecycle,
                        layout, glyph cache, selection, draw list. Platform-agnostic.
  glyphcull-render/   — wgpu renderer: WebGPU backend, GL (WebGL2) fallback; MSDF shaders;
                        texture management; draw list executor.
  glyphcull-host/     — the six-operation document host (load/scroll/paint/select/copy/
                        destroy) shared by the wasm and desktop bindings.
  glyphcull-wasm/     — wasm32 bindings exposing the identical tiny API.
  glyphcull-desktop/  — native host application (winit + wgpu); `glyphcull-desktop <file.cull>`
                        opens a compiled document in a window; the re-attachable
                        `DesktopSink` (attach/detach) is shared with mobile; GL adapters
                        get downlevel limits and a `GLYPHCULL_WGPU_BACKEND` override.
  glyphcull-mobile/   — Android host (winit + wgpu, Phase 4.13): loads the packaged `.cull`
                        from APK assets; same six-op API; drag/fling scroll + pinch zoom
                        (pure `gesture` module); APK assembled by the demo's Docker
                        pipeline and pixel-validated on an emulator (D31 capture, PASS).
  glyphcull-ios/      — iOS host (Phase 4.13 follow-up): the same winit app + shared sink;
                        only the asset read differs (the bundle resource); CI type-checks
                        all three iOS targets.
```

## Rendering convention (normative)

MSDF decoding follows the canonical sign convention (SPEC.md §2.5, GLOSSARY:
"MSDF sign convention"), identical in the wgpu/GL shader (`glyphcull-render`), the
CPU reference reconstruction (`glyphcull-render::msdf`), and the demo reference
compositor:

```text
MSDF channel value < 0.5  = outside glyph
MSDF channel value == 0.5 = glyph edge
MSDF channel value > 0.5  = inside glyph
```

Coverage = smoothstep of `(median(r, g, b) − 0.5) × pxRange` (positive = inside,
1 device-px edge). The renderer never inverts this to compensate for an atlas;
a non-conforming atlas is a bug in the atlas, not a case for a renderer workaround.

## The runtime is not a browser

The runtime knows nothing about HTML, Markdown, or CSS. The compiler owns translation; the
runtime owns execution. The package format (`glyphcull-compiler/docs/format/SPEC.md`) is the
only contract. The Rust reader is an *independent* implementation of that spec — one of two
independent readers (JS being the other), which is how the contract is validated: the
canonical cross-reader **conformance suite** (`glyphcull-demo/conformance/`) proves all three
readers agree on every valid fixture and reject every hostile entry consistently.

## Principles

- **Culling never materializes; materialization never culls.**
- **Chunk lifecycle is explicit**: Compressed → Queued → Materializing → Visible → Cooling →
  Evicted. No hidden state.
- **Deterministic architecture**: same package + same viewport → same draw list.
- **Memory discipline**: no `unsafe` except an audited single module, if ever; bounds-checked
  reader; resource budgets enforced.
- **Terminology**: graphics-engine vocabulary (compiler repo `docs/format/GLOSSARY.md`).

## Repository documents

`Architecture.md` · `DESIGN.md` · `ROADMAP.md` · `TESTING.md` · `PERFORMANCE.md` ·
`SECURITY.md` · `CONTRIBUTING.md` · `CHANGELOG.md`

## License

Apache-2.0. See [`LICENSE`](LICENSE).
