#!/usr/bin/env bash
# The package dry-run gate (scripts/ci-package.sh): every workspace crate must
# package cleanly — `cargo package -p <crate>` assembles the crate from its
# manifest and verifies it builds from the packaged form (this IS the dry run:
# nothing is published; the tarballs land in target/package, gitignored). The
# release gate: a crate is published only from a tree where this passes.
#
# Usage: scripts/ci-package.sh   (from the repository root)

set -euo pipefail
cd "$(dirname "$0")/.."

cargo metadata --no-deps --format-version 1 >/dev/null # fail fast on manifest errors

for pkg in $(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name'); do
  echo "== packaging $pkg"
  cargo package -p "$pkg"
done

echo "package dry-run OK: all workspace crates package cleanly"
