#!/usr/bin/env bash
# The unsafe audit (scripts/ci-audit.sh): `unsafe_code = deny` is the
# workspace lint, with exactly ONE documented allow — the `android_main`
# symbol-export shim in glyphcull-mobile (a `#[no_mangle]` export override,
# not memory unsafety; see crates/glyphcull-mobile/src/lib.rs). Any other
# allow, any `unsafe` block, `unsafe fn`/`impl`/`trait`, or the `unsafe`
# operator fails the gate.
#
# Usage: scripts/ci-audit.sh   (from the repository root)

set -euo pipefail
cd "$(dirname "$0")/.."

ALLOWS="$(grep -rln '#\[allow(unsafe_code)\]' crates || true)"
COUNT="$(printf '%s\n' "$ALLOWS" | sed '/^$/d' | wc -l)"
if [ "$COUNT" -ne 1 ]; then
  echo "error: expected exactly one allow(unsafe_code) (the mobile android_main shim), found:" >&2
  printf '%s\n' "$ALLOWS" >&2
  exit 1
fi
case "$ALLOWS" in
  crates/glyphcull-mobile/src/lib.rs) ;;
  *)
    echo "error: the sole allow(unsafe_code) must be the documented android_main shim in glyphcull-mobile/src/lib.rs" >&2
    echo "found: $ALLOWS" >&2
    exit 1
    ;;
esac

# No `unsafe` blocks/expressions/signatures anywhere (comments may mention the
# word; code may not use it).
UNSAFE="$(grep -rnE 'unsafe\s*(\{|fn|impl|trait|static|const|extern)' crates --include='*.rs' || true)"
if [ -n "$UNSAFE" ]; then
  echo "error: unsafe code found (only the documented allow is permitted):" >&2
  printf '%s\n' "$UNSAFE" >&2
  exit 1
fi

echo "unsafe audit OK: exactly one documented allow (android_main shim); no unsafe blocks"
