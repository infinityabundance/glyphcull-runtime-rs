# Security — glyphcull-runtime-rs

Status: Phase 0 (foundations). Threat model mirrors the JS runtime; Rust-specific controls
noted. Audited concretely in Phase 4.

## 1. Position

Identical to `glyphcull-runtime-js`: no claim of impossible-to-scrape; extraction-resistant
properties are architectural consequences. Technically accurate claims only.

## 2. Trust model

- **Packages are untrusted** (network, storage, archives, decades of rotation). The reader
  and pipeline must handle hostile packages without memory unsafety, unbounded resource
  use, or crashes.
- **Hosts are trusted**; the runtime provides no sandbox for the host.
- **Packages carry no code**: the strongest property is architectural — there is nothing in
  a package to execute.

## 3. Threat model

| # | Threat | Vector | Control |
|---|---|---|---|
| T1 | Memory unsafety / crash | Malformed package | Safe Rust reader; bounds-checked arithmetic; no `unsafe` in core (policy); fuzz corpus; `cargo miri` where applicable |
| T2 | DoS (unbounded resource use) | Huge counts, pathological structure | SPEC.md §1.3 limits at load; budgets at runtime; counting-allocator regression tests |
| T3 | Renderer DoS | Glyph density, huge images, many atlases | Texture/dimension caps; draw list caps; budget eviction |
| T4 | Data exfiltration | Malicious content | Packages are inert data; runtime never executes content, never performs network I/O, never resolves package URLs |
| T5 | Selection/copy confusion | Manipulated geometry | Logical selection verified against content length; copy extracts from content |
| T6 | Panic in wasm (host DoS) | Any input path | Typed errors only; `panic=abort` policy assessed for wasm; fuzz corpus |
| T7 | GPU driver issues | WebGPU/GL | Bounded resources; device-loss recovery; no extensions beyond core; validation layers in debug builds |
| T8 | Archival format rot | Version drift | Versioned format; two independent readers; loud rejection of unknown versions |

## 4. Hardening rules

1. No `unsafe` in `glyphcull-core`; any `unsafe` elsewhere requires written justification,
   safety comments, and review; audited module list maintained in this file.
2. Bounds-checked arithmetic on untrusted lengths (overflow-checked offset+len, caps before
   allocation).
3. No network access from the runtime.
4. SEAL verification mandatory when present; failure ⇒ load error.
5. `destroy()` releases GPU + heap resources in dependency order; budgets enforced.
6. MSRV pinned; dependencies audited (`cargo audit` in CI); lockfiles committed.

## 5. Reporting

Security issues: see CONTRIBUTING.md (coordinated disclosure). No secrets in this repo.
