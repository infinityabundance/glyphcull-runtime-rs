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
- [x] **4.4 glyphcull-core/visibility**: viewport + semantic culling; determinism tests.
- [x] **4.5 glyphcull-core/materialize**: priority queue; budgets; cooperative scheduling;
      eviction; starvation tests.
- [x] **4.6 glyphcull-core/layout**: UAX #29 word boundaries; Knuth–Plass; block layout;
      tables; images; golden layout fixtures (shared with JS runtime).
- [x] **4.7 glyphcull-core/glyph cache + selection**: budgets; logical selection; copy policy.
- [x] **4.8 glyphcull-core/draw list**: deterministic batching; serialization for tests.
- [x] **4.9 glyphcull-render**: wgpu init lifecycle; WebGPU backend; GL fallback; MSDF
      shaders (WGSL); texture management; device-loss recovery.
- [x] **4.10 glyphcull-wasm**: wasm32 bindings; tiny API; memory budget reporting.
- [x] **4.11 glyphcull-desktop**: winit host; input wiring; windowing. **Visually verified**
      on a real display (2026-08-07): the binary captures the first painted frame
      (`--screenshot`/`--size`/`--scroll`, DESIGN.md D31) and the demo's
      `scripts/desktop-smoke.sh` validates every fixture's capture against the CPU
      reference compositor under the D3 tolerance policy — all seven cases pass. This
      first native pixel validation found and fixed two renderer defects (glyph
      half-texel phase, D28 corrected; the never-written white texture), each with
      regression coverage.
- [x] **4.12 Mobile targets**: Android target build verification (CI builds the library
      targets for both ABIs); core/render/host/desktop readiness demonstrated (winit
      compiles for Android via the pure-Rust `android-native-activity` backend; iOS is
      target-ready in the same split).
- [x] **4.13 glyphcull-mobile**: the Android host crate — the winit application over the
      shared, re-attachable `DesktopSink` (0.1.2: `new_unsurfaced`/`attach`/`detach`;
      render 0.1.4 `retarget_target_format`), loading the packaged `.cull` from APK
      assets, with the rebuild-on-resume lifecycle (DESIGN.md D32) and wheel + touch
      drag-scroll. **APK delivered** (2026-08-07): the demo's Docker pipeline
      (docker/Dockerfile android stage + scripts/android-build.sh) links both ABIs
      (`arm64-v8a`, `x86_64`) with the NDK and assembles a signed, zipaligned APK over
      the system's NativeActivity (`com.glyphcull.host`, `assets/doc.cull`, verified by
      apksigner + aapt badging). CI type-checks the crate for both ABIs (the NDK link
      lives in the Docker pipeline).
- [x] **4.13 follow-up: the emulator visual run** (2026-08-07) — the first on-device
      pixel validation: `scripts/android-emulator-smoke.sh` (KVM + SwiftShader +
      Xvfb) validated the app's D31 offscreen capture against the CPU reference under
      the D3 policy (**PASS**, mean 0.0069 vs 1/64; evidence in
      `glyphcull-demo/docs/android-emulator-smoke.md`). The hunt surfaced two real
      fixes (GL downlevel limits; `GLYPHCULL_WGPU_BACKEND`) and documented the
      emulator's broken present path as a platform defect. A physical-device visual
      run remains the only unexecuted host check.
- [x] **4.13 follow-up: the mobile gesture layer** (2026-08-07) — fling/inertia +
      pinch zoom on `glyphcull-mobile` 0.1.2 (host 0.1.4's `scroll_with_surface`
      decouples the viewport from the drawing surface — a crisp GPU zoom; the pure
      `gesture` module is headless-tested; the drag delta is fixed to doc px).
- [x] **4.13 follow-up: the iOS host crate** (2026-08-07) — `glyphcull-ios` 0.1.0: the
      same crate split as Android (entry + asset read only) over the shared winit
      app; CI type-checks it for all three iOS targets. The Xcode app-bundle build
      (docker-osx pipeline) is recorded in the demo roadmap — this environment's
      Docker storage cannot run the macOS VM to completion (see the demo's
      docker-osx note).
- [x] **Test pyramid complete**: unit/integration/property/stress/memory/performance layers
      per subsystem (see TESTING.md); rendering validation vs a committed golden image
      (`crates/glyphcull-render/tests/fixtures/golden-document.png`, headless CPU reference
      rasterizer); CI green across native + wasm32 + Android.
- [x] **Documentation complete**: Architecture.md/DESIGN.md updated with the crate diagram
      and D29/D30/D32; ROADMAP/TESTING/PERFORMANCE/CHANGELOG in lockstep; API docs (the
      workspace denies `missing_docs`, doc build green under `-D warnings`).

## Definition of done (every phase)

- All tests green; clippy `-D warnings`; fmt check; no `TODO`/`FIXME`/`todo!()`/
  `unimplemented!()`; docs in lockstep; memory + performance baselines recorded; every bug
  gets a regression test.

## Future phase candidates (recorded, not scheduled)

- Threaded materialization behind the scheduler contract.
- Complex-script shaping / bidi (with compiler scope extension).
- Android/iOS host crates (core/render/host already target-ready: Android ABIs build in
  CI; iOS shares the same crate split and winit/wgpu support).
