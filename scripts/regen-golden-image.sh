#!/usr/bin/env sh
# Regenerates the committed golden rendering fixture
# (crates/glyphcull-render/tests/fixtures/golden-document.png).
#
# The fixture is the CPU reference rasterization of the compiler's golden
# package; the test compares every render against it. Regenerate deliberately:
# review the diff before committing (the rasterizer is deterministic, so an
# image diff means the pipeline's visual output changed).
set -eu

HERE="$(cd "$(dirname "$0")/.." && pwd)"

GLYPHCULL_REGEN_GOLDEN=1 cargo test --manifest-path "$HERE/Cargo.toml" -p glyphcull-render --test golden_image

echo "golden rendering fixture regenerated; review the diff before committing"
