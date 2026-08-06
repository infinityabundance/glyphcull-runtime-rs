#!/usr/bin/env sh
# Refreshes the committed contract fixtures from the glyphcull-compiler repo.
#
# The fixtures are compiler output, so they live in the compiler repository;
# this runtime repo commits copies for independent reader contract tests.
# Refresh deliberately: review the diff before committing (the reader tests
# pin the exact bytes).
set -eu

COMPILER="${GLYPHCULL_COMPILER:-$(cd "$(dirname "$0")/../.." && pwd)/glyphcull-compiler}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$HERE/crates/glyphcull-core/tests/fixtures"

if [ ! -d "$COMPILER" ]; then
    echo "error: compiler repo not found at $COMPILER (set GLYPHCULL_COMPILER)" >&2
    exit 1
fi

mkdir -p "$DEST"

cp "$COMPILER/tests/fixtures/v1-minimal.cull" "$DEST/v1-minimal.cull"
cp "$COMPILER/crates/glyphcull-pipeline/tests/fixtures/golden.cull" "$DEST/pipeline-golden.cull"
cp "$COMPILER/crates/glyphcull-pipeline/tests/fixtures/golden.md" "$DEST/golden.md"

echo "fixtures refreshed from $COMPILER; review the diff before committing"
sha256sum "$DEST"/* | sed "s|$DEST/||"
