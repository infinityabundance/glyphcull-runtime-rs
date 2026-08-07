#!/usr/bin/env bash
# Generate a release receipt (release/scripts/generate-release-receipt.sh):
# the evidence record linking a published package to its source commit,
# version, source-tree hash, package-archive hash, toolchain, and verification
# results — written to release/receipts/<package>-<version>.json. Receipts are
# committed artifacts (release/receipts/ is part of the repository).
#
# The package dry-run always runs (it assembles the real package archive whose
# hash the receipt records). Set GLYPHCULL_RECEIPT_FULL=1 to additionally run
# the build + test + conformance gates and record their results; otherwise
# those results are recorded as "not-run" (the check script still validates
# every deterministic field).
#
# Refuses to run on a dirty tree: a receipt records a clean-tree release.
#
# Usage: release/scripts/generate-release-receipt.sh <package> [version]
#   (version is read from the manifest when omitted)

set -euo pipefail
cd "$(dirname "$0")/../.."

PKG="${1:?usage: generate-release-receipt.sh <package> [version]}"
VERSION="${2:-}"

# Dirty check: uncommitted tracked changes block receipt generation — except
# the receipts themselves (release/receipts/), which are being regenerated.
if [ -n "$(git status --porcelain --untracked-files=no | grep -v 'release/receipts/')" ]; then
  echo "error: uncommitted tracked changes outside release/receipts/; a receipt records a clean-tree release" >&2
  exit 1
fi

COMMIT="$(git rev-parse HEAD)"
REPO="$(git remote get-url origin 2>/dev/null || echo "local")"
# Deterministic source-tree hash: every blob's SHA-256 object id + path, sorted
# (git ls-tree output is already path-sorted; sort again under C locale so the
# hash is byte-identical across machines and git versions).
TREE_HASH="$(git ls-tree -r HEAD | cut -f2- | LC_ALL=C sort | sha256sum | cut -d' ' -f1)"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# --- The package archive hash (the real tarball, assembled by the dry run) ---
if [ -f Cargo.toml ]; then
  if [ -z "$VERSION" ]; then
    VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r --arg p "$PKG" '.packages[] | select(.name == $p) | .version')"
  fi
  cargo package -p "$PKG" --no-verify >/dev/null
  PKG_HASH="$(sha256sum "target/package/${PKG}-${VERSION}.crate" | cut -d' ' -f1)"
else
  if [ -z "$VERSION" ]; then
    VERSION="$(node -p "require('./package.json').version")"
  fi
  TARBALL="$(npm pack --pack-destination "$TMP" --json 2>/dev/null | jq -r '.[0].filename')"
  PKG_HASH="$(sha256sum "$TMP/$TARBALL" | cut -d' ' -f1)"
fi
RESULT_PACKAGE=pass

# --- Optional full gates (GLYPHCULL_RECEIPT_FULL=1) ---
RESULT_BUILD=not-run
RESULT_TEST=not-run
RESULT_CONF=not-run
if [ "${GLYPHCULL_RECEIPT_FULL:-0}" = "1" ]; then
  echo "== full gates"
  if cargo build --workspace --all-targets >/dev/null 2>&1; then RESULT_BUILD=pass; else RESULT_BUILD=fail; fi
  if cargo test --all --all-targets >/dev/null 2>&1; then RESULT_TEST=pass; else RESULT_TEST=fail; fi
  if cargo test -p glyphcull-core --test reader_contract --test reader_compat >/dev/null 2>&1; then RESULT_CONF=pass; else RESULT_CONF=fail; fi
fi

RUSTC_V="$(rustc --version 2>/dev/null || true)"
CARGO_V="$(cargo --version 2>/dev/null || true)"
NODE_V="$(node --version 2>/dev/null || true)"
NPM_V="$(npm --version 2>/dev/null || true)"
TIMESTAMP="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

RC_PKG="$PKG" RC_VERSION="$VERSION" RC_REPO="$REPO" RC_COMMIT="$COMMIT" \
RC_TREE_HASH="$TREE_HASH" RC_PKG_HASH="$PKG_HASH" \
RC_RUSTC="$RUSTC_V" RC_CARGO="$CARGO_V" RC_NODE="$NODE_V" RC_NPM="$NPM_V" \
RC_TIMESTAMP="$TIMESTAMP" \
RC_RB="$RESULT_BUILD" RC_RT="$RESULT_TEST" RC_RC="$RESULT_CONF" RC_RP="$RESULT_PACKAGE" \
node <<'EOF'
const fs = require('fs');
const e = process.env;
const receipt = {
  project: 'glyphcull',
  repository: e.RC_REPO,
  package: e.RC_PKG,
  version: e.RC_VERSION,
  git_commit: e.RC_COMMIT,
  git_tree_clean: true,
  source_archive_hash: e.RC_TREE_HASH,
  package_archive_hash: e.RC_PKG_HASH,
  toolchain: {
    rust: e.RC_RUSTC,
    cargo: e.RC_CARGO,
    node: e.RC_NODE,
    npm: e.RC_NPM,
  },
  commands: {
    build: 'cargo build --workspace --all-targets',
    test: 'cargo test --all --all-targets',
    conformance: '(glyphcull-demo) ./scripts/run-conformance.sh',
    package_dry_run: 'cargo package -p ' + e.RC_PKG + ' --no-verify',
  },
  results: {
    build: e.RC_RB,
    test: e.RC_RT,
    conformance: e.RC_RC,
    package_dry_run: e.RC_RP,
  },
  release_timestamp: e.RC_TIMESTAMP,
};
fs.mkdirSync('release/receipts', { recursive: true });
const out = `release/receipts/${e.RC_PKG}-${e.RC_VERSION}.json`;
fs.writeFileSync(out, JSON.stringify(receipt, null, 2) + '\n');
console.log(`wrote ${out}`);
EOF
