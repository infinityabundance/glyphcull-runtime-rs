# Contributing — glyphcull-runtime-rs

GlyphCull is decades-long infrastructure. Standards below are enforced by reviewers.

## 1. Getting started

- Rust stable (MSRV pinned in `rust-toolchain.toml`), `wasm32-unknown-unknown` target.
- Gate: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all`
  (plus `cargo bench -- --quick` smoke; wasm tests where changed).
- Read in order: `README.md` → `Architecture.md` → `DESIGN.md` →
  `glyphcull-compiler/docs/format/SPEC.md` → `glyphcull-compiler/docs/format/GLOSSARY.md` →
  `TESTING.md` → `PERFORMANCE.md` → `SECURITY.md`.

## 2. Standards

- **Terminology**: graphics-engine vocabulary (GLOSSARY.md); browser terms forbidden.
- **Architecture**: pipeline is absolute; culling never materializes; materialization never
  culls; dependency direction strictly downward (core ← render ← hosts).
- **Lifecycle**: chunk state changes only through the state machine; no hidden state.
- **Determinism**: no wall-clock decisions, no HashMap in output paths, explicit ordering;
  determinism tests must pass.
- **Safety**: no `unsafe` without written justification + safety comment + review; no
  panics on untrusted input; bounds-checked arithmetic.
- **No placeholders**: no `TODO`, `FIXME`, `todo!()`, `unimplemented!()`, dead code.
- **Small reviews**: one logical unit per change; tests and docs in the same commit.
- **Dependencies**: additions require DESIGN.md + SECURITY.md updates; lockfiles committed.

## 3. Workflow

1. Branch from `main` (e.g., `phase4/core-lifecycle`).
2. Tests-first where practical; golden fixtures for outputs; proptest for invariants.
3. Local gate (above); update docs in the same change.
4. PR with change description, evidence, doc delta.

## 4. Review requirements

- Behavior changes include before/after evidence (measurements, diffs of golden bytes).
- Golden fixture regeneration is deliberate (script refuses on dirty tree; diff reviewed).
- API surface changes require a DESIGN.md decision (API is intentionally tiny — D12).
- Renderer changes require rendering validation evidence (parity across WebGPU/GL).

## 5. Bug reports

- Include: repro (package or test), expected vs actual, target (native/wasm), versions.
- Every accepted bug gets a permanent regression test in the same PR.

## 6. Security

- No public issues for vulnerabilities; follow SECURITY.md.

## 7. License

Apache-2.0 (LICENSE). Contributions accepted under these terms.
