# Design — glyphcull-runtime-rs

Status: Phase 0 (foundations). Decisions with rationale, alternatives, tradeoffs. Decisions
that mirror the JS runtime are marked (mirrors JS Dn); decisions specific to Rust/wgpu are
new.

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
