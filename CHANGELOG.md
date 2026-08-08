# Changelog

All notable changes, reverse chronological. Keep a Changelog format; Semantic Versioning.

## [Unreleased]

### Added (iOS app bundle pipeline — the .ipa by CI)

- `crates/glyphcull-ios/src/main.rs` — the app bundle's binary: loads the
  bundled `doc.cull` resource and runs `run_with` (winit's iOS backend calls
  `UIApplicationMain` from there — no Objective-C shell); an inert stub on
  other targets so the workspace keeps building.
- `crates/glyphcull-ios/app/` — the bundle manifest (`Info.plist`, iOS 13.0+)
  and the `aarch64-apple-ios` linker wrapper (clang with the iPhoneOS sysroot).
- `crates/glyphcull-ios/scripts/build-ipa.sh` — cargo build + app assembly
  (executable, Info.plist, PkgInfo, the bundled `doc.cull`), ad-hoc codesign,
  `.ipa` packaging, and verification.
- `.github/workflows/ios-ipa.yml` — the `.ipa` build on a macOS runner (real
  Xcode): the ad-hoc signed `GlyphCull-0.1.0.ipa` (a genuine
  `aarch64-apple-ios` Mach-O linking UIKit) is uploaded as a workflow artifact.
  The docker-osx local-VM path and its recorded blocker (the recovery license
  panel fails to render in QEMU; the Xcode download is Apple-ID-gated) are in
  the demo's `docs/ios-build.md`.

### Added (release receipts, hardening pass H7)

- `release/` — the receipt system: schema template, `generate-release-receipt.sh`
  (records commit, deterministic source-tree hash, the real `cargo package` tarball
  hash, toolchain, commands, results, UTC timestamp), `check-release-receipts.sh`
  (`--fast` CI gate; `--full` recomputes every package hash from a git worktree of
  its recorded commit), and `release-dry-run.sh` (all seven crates assemble in
  release order: core → render → host → wasm → desktop → mobile → ios).
- `release/receipts/` — committed receipts for all seven published crates with
  build/test/conformance/package gates recorded as pass (full gates re-run at
  generation).
- CI: the `gate` job runs `release/scripts/check-release-receipts.sh --fast`.

### Fixed (core reader tests vs the v1 INFO-required rule)

- The v1 compatibility rules (SPEC.md §1.6 rule 9, enforced in `ade4e85`) made INFO
  the container-required section; `reader_sections.rs` and `reader_stress.rs` still
  built INFO-less single-purpose containers, so 13 tests were red since that commit.
  The containers now carry a minimal INFO section first (canonical order), preserving
  each test's section-decoder / stress intent. Found by the H7 receipt full gate.
- `reader_compat.rs` was missing the project's test-file clippy allow header that
  every sibling test file carries, so `cargo clippy --all-targets` was red since the
  same commit; the header is now present. Clippy re-verified clean.

### Changed (README status correction, hardening pass H4)

- The README now leads with the tagline **"A compiled GPU document runtime."** and a
  status block: v0.1 experimental infrastructure prototype; Latin-script per-codepoint
  rendering only (complex shaping, bidi, vertical text, Indic/Arabic scripts, and full
  international publishing are documented exclusions); not DRM; does not make scraping
  impossible. Added the CI badge and the conformance-suite link.

### Added (CI hardening — the release gate)

- The workflow now also runs `cargo build --workspace --all-targets`, the explicit
  desktop build, `scripts/ci-audit.sh` (exactly one documented
  `allow(unsafe_code)` — the mobile `android_main` shim — and no `unsafe` blocks
  anywhere), and `scripts/ci-package.sh` (every crate packages cleanly). README CI
  badge; CONTRIBUTING documents the workflow.

### Added (0.2.3 — the surface-free WebGPU load: pixel-exact browser validation)

- `glyphcull-wasm` 0.2.3: `loadHeadless(bytes, options)` — the same document
  host over a wgpu WebGPU device with **no canvas surface** (a host diagnostic,
  not one of the six operations). The device is requested with
  `compatible_surface: None`, so it is never `present()`-ed and
  `GPUBuffer.mapAsync` works even in the headless SwiftShader builds where a
  surface-configured device rejects maps (DESIGN.md D10). Rendering happens
  offscreen: `paint` keeps the plan and `captureLastFrame` re-renders it and
  maps the readback — byte-deterministic, pixel-exact GPU output without the
  DOM canvas. `WgpuSink::draw` with no surface now stores the plan and returns
  `Ok(())` (previously an error) — the surface-less sink is a first-class
  mode. The canvas-bound `load` is unchanged and remains `#[cfg(web)]`-gated;
  `loadHeadless` shares the same options object.

### Added (0.1.0 — the iOS host crate, Phase 4.13 follow-up)

- `glyphcull-ios` 0.1.0: the iOS face of the six-operation API — the **same
  crate split as the Android host (entry + asset read only)**: the app is
  `glyphcull-mobile`'s winit `MobileApp`/`run_with` verbatim (winit's iOS
  backend calls `UIApplicationMain`), and this crate adds the bundle-resource
  read (`doc.cull` beside the executable, `std::env::current_exe` — no FFI;
  pure path logic, unit-tested). CI type-checks it for
  `aarch64-apple-ios`/`aarch64-apple-ios-sim`/`x86_64-apple-ios`; the `.ipa`
  app bundle is built by the `ios-ipa.yml` workflow (real Xcode on a macOS
  runner — ad-hoc signed; the docker-osx local-VM path and its recorded
  blocker are in the demo's `docs/ios-build.md`).

### Added (0.1.4 / 0.1.2 — the mobile gesture layer: fling/inertia + pinch zoom)

- `glyphcull-host` 0.1.4: `HostDocument::scroll_with_surface` — the viewport and the
  drawing surface decouple (the mobile host's pinch zoom keeps the surface at the
  window size while the zoomed viewport shrinks — the renderer maps the viewport into
  the full surface, a crisp GPU zoom). The sink resize now uses **device** pixels
  (canvas × dpr, the `FrameSink::resize` contract) on every scroll — fixing the
  latent dpr≠1 path that configured the surface at CSS pixels (smoke-neutral: all
  validation runs at dpr 1).
- `glyphcull-mobile` 0.1.2: the gesture layer in a new `gesture` module (pure,
  headless-tested) — fling/inertia (release-velocity estimation from recent touch
  samples, exponential velocity decay driven per redraw, `Fling`), pinch zoom
  (viewport shrink around the pinch midpoint, zoom 0.5×–4× of fit-width, anchored),
  and the single-finger drag delta fixed to document px (the dpr no longer divides
  twice).

### Fixed + Added (0.1.3 / 0.1.1 — the Android emulator's first on-device pixel validation)

- `glyphcull-desktop` 0.1.3: **GL adapters now request `downlevel_webgl2_defaults()`
  limits** — SwiftShader's GLES reports no compute support, so `Limits::default()`
  failed device creation on the Android GL path (parity with the wasm binding's
  `backend: webgl`); Vulkan is unchanged. New `GLYPHCULL_WGPU_BACKEND`
  (auto|vulkan|gl) backend override for device diagnostics; the attach path logs the
  chosen backend + surface formats once.
- `glyphcull-mobile` 0.1.1: the on-device D31 capture dump (Android-only, once per
  process — the emulator smoke's authoritative pixel check, pulled via `run-as`) and
  resumed/resize diagnostics.
- The demo's `scripts/android-emulator-smoke.sh` validated the Android host's D31
  capture against the CPU reference under the D3 policy: **PASS** (mean 0.0069 vs
  1/64). The emulator present path is a recorded platform defect (gfxstream/
  SwiftShader; offset composition on google_apis, black on aosp_atd) — root cause and
  evidence in `glyphcull-demo/docs/android-emulator-smoke.md`.

### Added (0.1.4 / 0.1.2 / 0.1.0 — re-attachable sink + the mobile host, Phase 4.13)

- `glyphcull-render` 0.1.4: `Renderer::retarget_target_format` — rebuild the MSDF
  pipeline for a different non-sRGB target format without losing the
  device/buffers/textures (a sink attaching to a different surface; DESIGN.md
  D29). Pipeline creation is extracted into the shared `create_msdf_pipeline`.
- `glyphcull-desktop` 0.1.2: the sink is public and re-attachable (DESIGN.md
  D29) — `DesktopSink::new(window)` keeps the surface-first desktop path;
  `new_unsurfaced()`/`attach(window)`/`detach()` serve the Android
  suspend/resume lifecycle (the native window, and with it the surface, dies
  on suspend); `draw` reports "no surface" while detached; the attach
  format-selection is pure and unit-tested. New `capture_readback` override
  (all backends).
- `glyphcull-mobile` 0.1.0: the Android host — a winit application that loads
  a packaged `.cull` from the APK assets (`assets/doc.cull`, validated by the
  pure `asset::validate_asset_name`) and serves the identical six-operation
  runtime API over the shared sink. Lifecycle: rebuild the document on
  `resumed`, drop on `suspended` (the canonical Android GPU pattern). Input:
  wheel + touch-drag scroll (content follows the finger), drag select. The
  `android_main` entry is the workspace's only `#[no_mangle]` (unsafe_code is
  deny, not forbid, for that documented shim).

### Fixed (pre-existing, found by the 0.1.4 sink work)

- `glyphcull-desktop` on wasm32: the sink's `capture` override called the
  native-only `render_to_rgba`, so the crate (and the whole `--all` wasm32
  build) has not compiled since the 0.1.3 readback split. The override is now
  `cfg(not(target_family = "wasm"))`; the all-backends `capture_readback`
  keeps the last-plan seam live on every target.

### Added (0.1.3 / 0.1.3 / 0.2.2 — async frame capture on the wasm binding)

- `glyphcull-render` 0.1.3: the readback split for wasm — `FrameReadback` +
  `render_to_readback` (render + copy, all backends) and the native-only
  `readback_to_rgba`/`render_to_rgba` (sync map; `PollType::Wait` panics on wasm).
- `glyphcull-host` 0.1.3: `FrameSink::capture_readback` (default `None`) and
  `HostDocument::capture_last_frame_readback` — the un-mapped readback seam.
- `glyphcull-wasm` 0.2.2: `captureLastFrame()` — the wasm face of the D31 capture
  diagnostic; the binding maps the readback asynchronously (`map_async` + a JS
  promise). In headless Chromium + SwiftShader the WebGPU readback is
  platform-blocked (DESIGN.md D10: `present()` poisons `mapAsync`); the WebGL2
  backend's canvas is read via `readPixels` instead, which the demo now
  pixel-validates (all fixtures within tolerance).

### Fixed (0.1.2 / 0.1.2 / 0.1.1 — desktop visual smoke, 2026-08-07; two renderer defects found by native pixel validation)

- **Glyph sampling phase (D28, corrected)**: wgpu samples at `uv·size − 0.5`; the
  renderer now shifts glyph UVs by half a texel at plan flattening
  (`renderer::flatten_plan` + `glyph_half_texel_shift`, where the texture size is
  known), matching the CPU reference and the JS renderer's vertex-stage shift.
  Image quads are deliberately unshifted (the reference image path uses the raw
  `−0.5` convention). Previously every glyph edge was half a texel off — invisible to
  all functional validation because headless Chromium cannot read back wgpu canvases
  (D10).
- **White texture never written**: fills and rulers sample the renderer's 1×1 white
  texture as a coverage field; `ensure_white` created the texture but never uploaded
  the texel, so it read as zeros on real GPUs and every fill/ruler rendered fully
  transparent. The white texel is now uploaded; the section ruler in the flagship
  fixture is the pixel regression.

### Added (Phase 4.11 follow-up — headless frame capture + desktop smoke)

- `glyphcull-render`: `Renderer::render_to_rgba` — offscreen render + readback of a
  plan as tightly packed premultiplied RGBA8 (row padding stripped, BGRA swapped),
  plus the pure `compact_rgba8`/`straighten_premultiplied_rgba8`/`flatten_plan`
  helpers (unit-tested).
- `glyphcull-host`: `FrameSink::capture` (default `None`) and
  `HostDocument::capture_last_frame` — a host diagnostic, not part of the six-op API.
- `glyphcull-desktop`: `--screenshot <path> [--size WxH] [--scroll y]` — capture the
  first painted frame to a PNG and exit (DESIGN.md D31); `HostLaunch`;
  `parse_args` (unit-tested).
- The demo's `scripts/desktop-smoke.sh` builds the binary, runs it on a real display
  (or Xvfb) against every fixture, and validates each capture against the CPU
  reference compositor under the D3 tolerance policy — the first native pixel
  validation of the Rust renderer (all seven cases pass, mean 0.0002–0.0045 vs the
  1/64 limit). Evidence: `glyphcull-demo/docs/desktop-smoke.md`.

### Fixed (patch releases 0.1.1 / 0.2.1 — crates.io parity with HEAD)

- `glyphcull-core` 0.1.1: the clock generics accept `?Sized` clocks
  (`LifecycleManager<'_, dyn Clock>` / `MaterializationScheduler<'_, dyn Clock>`),
  enabling the host's injectable-clock API.
- `glyphcull-render` 0.1.1: image uploads are premultiplied (transparent texels
  with non-zero RGB must never read as ink in the MSDF program) — matches the JS
  WebGL `UNPACK_PREMULTIPLY_ALPHA_WEBGL` convention so the two runtimes render
  images identically; `chunk.get(0)` → `chunk.first()` (clippy `get_first`).
- `glyphcull-host` 0.1.1: `load_with_clock` — the default `load` keeps the real
  clock; hosts inject a platform clock (`performance.now` on wasm, where
  `SystemTime::now` panics).
- `glyphcull-wasm` 0.2.1: the canvas-bound `load`/`load_inner`/`parse_options`
  and the wasm-only clock are behind the `web` cfg (set on wasm32 by the build
  script; declared via check-cfg), so native builds — test, clippy, doc —
  compile without a DOM canvas; the binding ships as a cdylib; the optional
  `backend` option (`webgpu`/`webgl`/`auto`); wgpu's `webgl` feature for the
  WebGL2 fallback with downlevel limits; the injected `performance.now` clock.
  The crate is warning-free on both native and wasm32 targets.
- `glyphcull-desktop` remains 0.1.0 (unchanged since its publish).

### Added (Phase 4.12 — Mobile targets + final test-pyramid layers)

- **Rendering validation vs a golden image** (`glyphcull-render/tests/golden_image.rs`):
  a CPU reference rasterizer (MSDF reconstruction + premultiplied-over compositing, the
  shader's math) plays the full pipeline — parse → model → layout → draw list → pixels —
  and compares against the committed `tests/fixtures/golden-document.png` (800×262,
  deterministic; regeneration via `scripts/regen-golden-image.sh`, deliberate).
- **Host memory regression** (`glyphcull-host/tests/host_memory.rs`): RSS gate over 25
  full host lifecycles (load → scroll → paint → select → copy → destroy) of the golden
  package — measured ≈ 0.3 MiB peak against the committed 8 MiB budget.
- **Mobile targets**: Android library-target builds for both ABIs verified in CI for
  core, render, host, and desktop (winit `android-native-activity`, no NDK); the mobile
  host crate is recorded as a future candidate with readiness demonstrated. iOS shares
  the same crate split.
- Docs finalized: ROADMAP Phase 4 fully checked off (4.12, test pyramid, documentation);
  Architecture.md (renderer golden-image layer, mobile evidence), PERFORMANCE.md §6
  evidence, TESTING.md (delivered layers, host suite path, CI wording), README
  (terminology link).

### Added (Phase 4.11 — glyphcull-desktop, winit host)

- `glyphcull-host` workspace member: the platform-agnostic six-operation document host,
  extracted from the wasm binding and shared by both hosts (mirrors the JS
  `src/api/runtime.ts`; DESIGN.md D29): `HostDocument` (ouroboros-pinned
  `Package → model → layout`), `FrameSink` seam, typed `HostError`s, option validation,
  the RGB8→RGBA8 padding helper, and the recording-sink test suite (moved with it).
- `glyphcull-wasm` 0.2.0: now a thin wasm-bindgen layer over `glyphcull-host` — the
  `WgpuSink` (canvas surface) and the JS handle; the public names are unchanged
  (re-exported).
- `glyphcull-desktop` 0.1.0: the winit host — a window serving the identical six-operation
  API over a wgpu surface.
  - `DesktopDocument` — window + shared host; the six operations plus the input wiring
    (wheel → scroll, drag → select, resize → viewport + surface, Esc/close → exit,
    `RedrawRequested` → paint); blocking wgpu init once in the event loop's `resumed`.
  - `sink` — the native wgpu `FrameSink`: `Surface<'static>` via the `Arc<Window>`
    handle (DESIGN.md D30), non-sRGB target (D28), RGB8 padding, per-frame present.
  - `input` — pure, headless-tested input translation (line/pixel wheel → doc px,
    physical cursor → doc point, viewport advance with top clamping).
  - The `glyphcull-desktop` binary — open a `.cull` file in a window (manual harness).
- CI: Android steps verify library targets only (a bin would need the NDK linker; the
  mobile host is 4.12).
- Test suites: `glyphcull-host/tests/host.rs` (the six-operation contract against a
  recording sink — moved from wasm), host lib unit tests (padding helpers),
  `glyphcull-desktop/tests/desktop.rs` (typed errors + composed input scenarios) and
  `input` unit tests.

### Added (Phase 4.10 — glyphcull-wasm)

- `glyphcull-wasm` workspace member: the wasm host — wasm-bindgen bindings for exactly
  `load/scroll/paint/select/copy/destroy` (mirrors the JS `src/api/runtime.ts`), typed
  errors with the JS `RuntimeError` kinds.
  - `host` — the platform-agnostic document host: a self-referential ouroboros pipeline
    (`Package → DocumentModel → LayoutEngine`, DESIGN.md D29), the scheduler owning the
    lifecycle (D22), visibility + reconciliation + one bounded cooperative cycle per
    `scroll`, draw-list + render-plan playback per `paint`, hit-test selection with
    lifecycle pinning, `copy`, and idempotent `destroy`.
  - `sink` — the wgpu `FrameSink`: atlas-page and image uploads (RGB8 padded to RGBA8),
    surface configuration against a non-sRGB target (D28), per-frame present.
  - `lib` — the `#[wasm_bindgen]` layer: `load(bytes, canvas, options)` takes the canvas
    by value so the surface is `'static` (`SurfaceTarget::Canvas`), and the entire
    canvas-dependent path is `#[cfg(web)]` — the crate compiles on native, wasm32, and
    both Android ABIs from one workspace member.
  - `Renderer::device()`/`queue()` accessors so the host can configure its surface.
- Test suite: `tests/host.rs` — the six-operation contract against a recording sink
  (load validation, upload census, scroll/paint/resize behavior, select-and-copy
  round-trips incl. glyph-position hit tests, destroy semantics, multi-document
  isolation, selection pinning); no wasm, no GPU.
- Workspace lint config: `unexpected_cfgs` declares `cfg(web)` (a built-in rustc target
  cfg that is not in check-cfg's default set).

### Added (Phase 4.9 — glyphcull-render)

- `glyphcull-render` workspace member: the wgpu MSDF renderer (WebGPU + GL/WebGL2
  backends; naga translates one WGSL program to GLSL and SPIR-V).
  - `msdf` — the normative CPU reconstruction (median, smoothstep, texel→px, bilinear
    coverage, supersampled glyph reconstruction, edge clamping) mirroring the JS
    `src/render/msdf.ts`; the single source of truth for the shader and the
    validation.
  - `shader` — one logical WGSL program (median-of-three, 1-device-px edge,
    premultiplied alpha, document-y-down), validated and translated headlessly via
    naga (GLSL for the GL backend, SPIR-V for Vulkan).
  - `plan` — the pure render plan: draw list → batched GPU ops (glyph batches by
    texture, single quads for fills/rulers/images) + the view uniform; premultiplied
    colors, dpr-scaled px-range, z-order, and determinism are testable without a GPU.
  - `renderer` — the wgpu executor: async init lifecycle, budgeted textures with
    deterministic eviction (DEFAULT_TEXTURE_BUDGET), in-place page refreshes,
    premultiplied-alpha blending against non-sRGB targets (D28), device-loss
    recovery via `on_device_lost`, and per-frame plan playback.
- Test suites: `tests/msdf.rs` (JS-mirror vectors incl. the pinned vertical-edge
  profile), `tests/plan.rs` (golden pipeline → plan batching/z-order/premultiplied
  data/determinism), `tests/validation.rs` (headless CPU rendering validation incl.
  the D28 sampling-convention pin), in-crate shader/plan unit tests, and the
  `plan_golden` criterion benchmark (baselines in PERFORMANCE.md §6).

### Added (Phase 4.8 — glyphcull-core draw list)

- `glyphcull-core::draw_list` — the ordered draw command sequence mirroring the JS
  `src/render/drawlist.ts`: `GlyphCommand` (texture, UV, quad, color, px-per-texel for
  the shader's px range), `ImageCommand`, `FillCommand` (selection + backgrounds),
  `RulerCommand`, the `DrawCommand` enum, `DrawList`, the `TextureResolver` trait, and
  `DrawListBuilder` (selection fills first beneath content; per block background →
  line glyphs → list marker → children; the visible-set `emitted` guard emits every
  block exactly once; missing stamps are skipped mid-flight). Markers render as glyph
  commands from the disc/alpha/roman stamp.
- The builder is a pure function of its inputs; double-build equality is pinned by
  tests, as is the R5 divergence (container-nested images and rules render — the JS
  children path drops them).
- Test suites: `tests/draw_list.rs` (JS-mirror vectors, R5 pins, repeated-build
  stability, 100-case subset/selection proptest) and the criterion benchmark
  `draw_list` (baselines in PERFORMANCE.md §6).

### Added (Phase 4.7 — glyphcull-core glyph cache + selection)

- `glyphcull-core::glyphs` — the glyph cache mirroring the JS `src/glyphs/cache.ts`:
  `prepare_glyph` (size-specific stamps: quad + UV + f64 advance + flags per
  (atlas, codepoint, size, color), from the SPEC §2.5 placement convention) and
  `GlyphCache` (byte budget, deterministic LRU eviction via a monotonic touch counter
  — DESIGN.md D27 — chunk-owning `release_chunk` for lifecycle coupling, `clear`). The
  budget is `u64`, so invalid budgets are unrepresentable (R4).
- `glyphcull-core::selection` — logical selection mirroring the JS
  `src/selection/selection.ts`: char-based `TextPosition` (R3), normalized
  `Selection`, `compare_positions`/`normalize_selection`/`is_collapsed`,
  `hit_test_point` (nearest line, then nearest glyph center), `range_quads` (per-line
  highlight quads with contiguous merging and a proportional fallback for glyph-less
  runs), `covered_chunk_ids`, and `copy_text` with the documented boundary policy
  (runs re-join, blocks → `\n`, cells of one row → `\t`, rows → `\n`, `br` → `\n`).
- Test suites: `tests/glyph_cache.rs` (JS-mirror vectors incl. LRU budgeting and
  release-chunk sharing), `tests/selection.rs` (JS-mirror vectors incl. the synthetic
  table/br copy policy), `tests/selection_property.rs` (300-case proptests), and the
  criterion benchmark `glyph_selection` (baselines in PERFORMANCE.md §6).
- The visibility RSS memory gate moved to its own binary (`tests/visibility_memory.rs`)
  so parallel test threads cannot inflate `VmHWM` mid-measurement — the same
  single-test-file convention as the other memory gates.

### Added (Phase 4.6 — glyphcull-core layout)

- `glyphcull-core::layout` — materialization turns semantic chunks into renderable
  geometry (mirrors the JS `src/layout/*`):
  - `breaks` — UAX #29-grounded tokenization (word runs, closing/opening punctuation
    binding, CJK between-ideograph breaks, forced newlines) with the exact ECMAScript
    `\s` and `\p{L}\p{N}` classes so whitespace/word runs match the JS runtime.
  - `kp` — the Knuth–Plass dynamic program (boxes/glue/penalties, prefix sums, fitness
    classes, twice-around fitness pass, active-list dedup by fitness class, the
    single-line fallback when the active list exhausts).
  - `measure` — per-codepoint glyph measurement: advances, kerning (binary search over
    the SPEC-sorted pair table), combining-mark attachment, letter-spacing, and the
    half-em tofu fallback; f64 arithmetic on f32 atlas metrics.
  - `layout` — the `LayoutEngine`: sequential frontier (`extend_to`/`materialize`/
    `next_frontier_block`), text blocks (KP breaking + first-line indent), preformatted
    code blocks, quotes, lists with markers (disc/circle/square/decimal/alpha/roman),
    images (dpr-aware aspect), `hr`, and the deterministic table auto layout
    (colspan/rowspan with the last-spanned-row growth rule). Records are `Rc`-shared
    between the records index and parent children (each block exists exactly once); the
    engine implements the visibility `GeometrySource` contract.
- Layout test suite: JS-mirror vectors (breaks/kp/measure/layout), synthetic table/
  image/hr packages, token-partition guarantee, 300-case proptests (KP invariants,
  determinism, geometry sanity), stress (5000-deep quote chain on a 64 MiB-stack
  thread, 2000-block streaming, 3000-word KP), RSS memory gate (full golden layout ≈
  2.3 × package size vs 8 × budget), and the criterion benchmark `layout` (baselines
  in PERFORMANCE.md §6).
- Deliberate correctness divergences from the JS runtime, pinned by tests and
  documented in DESIGN.md (R-series): the token→line index mapping (R2) and
  measurement hardening for astral codepoints (R1).

### Added (Phase 4.5 — glyphcull-core materialization)

- `glyphcull-core::materialize` — the streaming materialization scheduler mirroring the
  JS runtime: a deterministic binary min-heap; `priority_key` (pure function of
  geometry/viewport/direction: intersecting chunks first, 1024 px distance tiers,
  ahead-of-travel preferred, document-order tie-breaks, sequential frontier for
  geometry-less chunks); `reconcile` (visible set → lifecycle transitions incl.
  requeue of cooling chunks and dequeue/cancel of departed ones); `run_frame`
  (per-frame budget, cooperative yields with an anti-starvation penalty, measured
  elapsed never affecting decisions); `tick` (cooling expiry through the worker,
  selection-respecting); `evict_for_memory` (furthest visible chunks released first,
  deterministic). The scheduler owns its `LifecycleManager` and borrows the clock.
- Materialization test suite: priority ordering (intersect/distance/direction/
  tie-breaks), processing order, frame budget, no-starvation cooperative yielding,
  visit-sequence determinism, reconcile (cull + cancel + dequeue + requeue paths),
  tick (period, selection pins, release order), evict_for_memory (furthest-first,
  byte targets), and a 100-case no-starvation proptest.

### Added (Phase 4.4 — glyphcull-core visibility)

- `glyphcull-core::visibility` — the visibility system mirroring the JS runtime:
  `Rect`/`Viewport`, the `GeometrySource` trait (implemented by layout), semantic
  culling (hidden flag excludes a whole subtree), geometric culling against the
  margin-expanded viewport (inclusive of edges), the materialization frontier
  (`not_yet_visible`), and structural-context visibility (a structural chunk is visible
  iff a descendant is). Iterative pre-order walk with exactly one geometry read per
  non-hidden chunk; the visible set is a pure function of its inputs.
- Visibility test suite: geometric/semantic culling, margin expansion, edge-inclusive
  intersection, frontier semantics, purity, the read-only boundary (document and
  geometry never mutate; exact read counts), structural-context visibility, golden
  invariants, a 300-case proptest over random documents/geometries/viewports
  (invariants + purity), and 100k-chunk stress with an RSS memory gate.

### Added (Phase 4.3 — glyphcull-core chunk lifecycle)

- `glyphcull-core::clock` — the injected clock: `Clock` trait, `RealClock` (wall),
  `FakeClock` (deterministic, interior-mutable so the manager can share it by reference).
- `glyphcull-core::lifecycle` — the chunk lifecycle state machine mirroring the JS
  runtime: six states, nine guarded events (hidden chunks never enqueue; expire needs the
  cooling period elapsed and no selection pin), `BTreeMap`-backed per-chunk tables
  (deterministic enumeration), and an ordered transition log stamped by the injected
  clock.
- Lifecycle test suite: exhaustive 6×9 transition table against a reference model, guard
  tests (cooling period, selection pins, hidden chunks), log order/determinism, state
  census, idempotent registration, and a 300-case model-based proptest over random event
  sequences.

### Added (Phase 4.2 — glyphcull-core document model)

- `glyphcull-core::document` — the trusted runtime view of a package (mirrors the JS
  `DocumentModel`): `build_document` load-time validation of chunk-graph invariants
  (single document root, dense ids/ordinals, sibling-ring consistency, parent/depth
  consistency, full reachability, no cycles), reference resolution (style ids, content
  indices, image refs against IMGS, resolved font_id against GLYF), and the five INFO
  count cross-checks.
- `ResolvedStyle` + `resolve_style`: all 16 SPEC §2.3 properties with defaults applied.
- Trusted views: `all_ids` (pre-order), `child_ids`, `extras_for`, `direct_text`,
  `image_ref`, and iterative `plain_text` (document-order copy text; `br` → newline;
  code blocks contribute direct text). The model borrows the package's payloads — no
  data duplication at build.
- Reader completion: INFO `format_version` must equal the header version
  (`UnsupportedVersion`), and `source_digest`/`document_id` must be lowercase hex of the
  SPEC lengths (`InvalidValue`) — mirroring the JS reader.
- Document test suite (30 tests + 300-case proptest properties): golden contract, every
  invariant rejection (cycle, unreachable chunk, broken sibling ring, dangling
  style/content/image refs, count mismatches, wrong payload kinds), style resolution,
  direct-text/image-ref resolution, plain-text order (br/code/hr), isolation, random-tree
  generator property, golden byte-mutation never-panic property, stress (100k-wide and
  10k-deep documents, iterative traversals), RSS memory gate (build peak ≈ 2 × package
  size vs 8 × budget).
- Criterion benchmark `document/build` (baselines in PERFORMANCE.md §6).

### Added (Phase 4.1 — glyphcull-core reader)

- `glyphcull-core` workspace member: the platform-agnostic compiled-document runtime
  crate (reader layer first, per ROADMAP).
- `glyphcull-core::reader` — the independent `.cull` container reader (SPEC.md §1):
  header + section table validation with overflow-checked arithmetic; per-section zlib
  decode with explicit header and Adler-32 verification and bounded incremental
  accumulation; `decoded_len` exactness; per-section CRC-32; duplicate-kind rejection;
  reserved kinds addressable without interpretation.
- Typed section decoders (SPEC.md §2): INFO (deterministic JSON subset, strict keys,
  duplicate-key rejection), CHNK (records + all four extra kinds), STYL (all 16 property
  tags), CONT (text + image refs), GLYF (atlases, sorted kerning, page texels), IMGS
  (RGBA8/RGB8), SEAL (hash tree). Lazy behind `OnceLock` caches; owned payloads.
- SEAL verification at load (per-section SHA-256 + overall hash over header `0..12` and
  covered sections in canonical order), mirroring the JS runtime's `verifySeal`.
- `glyphcull-core::error` — typed `Error`/`ErrorKind` (mirrors JS `CullError`);
  `glyphcull-core::limits` — the SPEC §1.3 caps.
- Reader test suite (56 tests + 500-case proptest properties): golden contract tests
  with `cull inspect` diagnostics pinned, truncation corpus (every proper prefix),
  single-byte mutation corpus, zlib strictness, section-decoder edge coverage, unknown/
  duplicate/reserved-kind handling, determinism, stress (64-section cap, 1 MiB decoded,
  200-iteration stability), and the RSS-based memory regression gate.
- `scripts/refresh-fixtures.sh` + committed compiler fixtures
  (`crates/glyphcull-core/tests/fixtures/`).
- Criterion benchmark `reader/parse` + `reader/full-contract` (baselines in
  PERFORMANCE.md §6).

### Added (Phase 0 — Foundations)

- Repository scaffolding: README, Architecture.md, DESIGN.md, ROADMAP.md, TESTING.md,
  PERFORMANCE.md, SECURITY.md, CONTRIBUTING.md, LICENSE (Apache-2.0), CHANGELOG,
  .gitignore, .editorconfig.

### Planned (Phase 4 — per master plan)

- glyphcull-wasm (tiny API bindings); glyphcull-desktop (native
  host); mobile targets.
