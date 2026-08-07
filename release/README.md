# Release receipts — glyphcull-runtime-rs

Every published crate must be traceable to its **source commit, version, package
hash, build/test commands, dry-run result, and release timestamp**. This directory
is that evidence: committed receipts, the schema, and the scripts that generate
and validate them. (The canonical conformance suite lives in `glyphcull-demo`;
the receipts reference it as the conformance gate.)

## Layout

```
release/
  README.md                     this document
  templates/
    receipt.template.json       the receipt schema (placeholder values)
  scripts/
    generate-release-receipt.sh   write release/receipts/<package>-<version>.json
    check-release-receipts.sh     validate the committed receipts (--fast CI gate,
                                  --full release procedure)
    release-dry-run.sh            assemble every crate in release order
  receipts/                     the committed receipts (one per published crate)
```

## The receipt contract

See `glyphcull-compiler/release/README.md` for the field-by-field contract — the
schema is identical (project `glyphcull`, source-tree hash over `git ls-tree -r`,
package hash over the real `cargo package` tarball, toolchain, commands, results,
UTC release timestamp). Everything except `release_timestamp`, `toolchain`,
`git_commit`, and the two hashes is schema-fixed and validated by the check
script. Timestamps never enter `.cull` output.

## Release order (enforced)

Publish crates in dependency order — each crate's manifest depends only on
earlier crates, so crates.io resolves at publish time:

```text
glyphcull-core → glyphcull-render → glyphcull-host → glyphcull-wasm
→ glyphcull-desktop → glyphcull-mobile → glyphcull-ios
```

`release-dry-run.sh` iterates this exact order (it fails on the first crate that
does not assemble), and `check-release-receipts.sh` requires every crate in the
order to have a receipt.

## Usage

```sh
# Assemble every crate in release order (the dry run; nothing is published).
release/scripts/release-dry-run.sh

# Generate a receipt for one crate (dirty-tree-refusing), with the full gates:
GLYPHCULL_RECEIPT_FULL=1 release/scripts/generate-release-receipt.sh glyphcull-core

# Validate the committed receipts.
release/scripts/check-release-receipts.sh --fast   # CI gate
release/scripts/check-release-receipts.sh --full   # release procedure (recomputes
                                                   # every package hash from a git
                                                   # worktree of its commit)
```

CI runs `check-release-receipts.sh --fast` on every push/PR: schema, filename,
commit existence, source-tree-hash honesty, clean-tree claim, and completeness.
The `--full` mode is the manual release gate — it re-derives every package
archive hash from a real checkout of the recorded commit, proving reproducibility
(cargo embeds `.cargo_vcs_info.json` only when packaging from an actual git
checkout, which is why the check uses a worktree, not a plain archive).

## Workflow

1. Finish the change; commit + push; the tree must be clean.
2. `release/scripts/release-dry-run.sh` — every crate assembles.
3. `GLYPHCULL_RECEIPT_FULL=1 release/scripts/generate-release-receipt.sh <crate>`
   for each crate in release order.
4. `release/scripts/check-release-receipts.sh --full` — all receipts valid,
   hashes reproduce.
5. Commit the receipts, push, then `cargo publish -p <crate>` in release order.
