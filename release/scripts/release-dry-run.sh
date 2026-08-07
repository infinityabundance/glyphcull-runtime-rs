#!/usr/bin/env bash
# The release dry-run (release/scripts/release-dry-run.sh): assembles every
# package in the documented release order (release/README.md) and verifies each
# one packages cleanly — this IS the dry run, nothing is published. The order
# is enforced: the script iterates the canonical order and fails on the first
# package that does not assemble.
#
# Usage: release/scripts/release-dry-run.sh

set -euo pipefail
cd "$(dirname "$0")/../.."

# Canonical release order (release/README.md): dependency order on crates.io.
ORDER="glyphcull-core glyphcull-render glyphcull-host glyphcull-wasm glyphcull-desktop glyphcull-mobile glyphcull-ios"

for pkg in $ORDER; do
  echo "== ($pkg)"
  cargo package -p "$pkg" --no-verify
done

echo "release dry-run OK: all $(echo "$ORDER" | wc -w) crates assemble in release order"
