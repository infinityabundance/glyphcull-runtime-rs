#!/usr/bin/env bash
# Validate the committed release receipts (release/scripts/check-release-receipts.sh).
#
# --fast (the CI gate): schema, filename, commit existence, source-tree-hash
#   honesty, and the clean-tree claim, for every receipt — plus completeness
#   (every package in the release order has a receipt). Cheap; runs on every
#   push/PR.
# --full (the release procedure): everything in --fast, then recomputes every
#   package archive hash from the recorded commit and compares (proves the
#   package tarball is reproducible from its source commit).
#
# Requires: git, jq, sha256sum (ubuntu-latest ships all three); --full also
# requires cargo (Rust repos) or npm (JS repos).
#
# Usage: release/scripts/check-release-receipts.sh [--fast|--full]

set -euo pipefail
cd "$(dirname "$0")/../.."

MODE="${1:---fast}"
case "$MODE" in
  --fast|--full) ;;
  *) echo "usage: check-release-receipts.sh [--fast|--full]" >&2; exit 2 ;;
esac

# The release order (release/README.md): publish in this order, and every
# package must have a receipt.
PACKAGES="glyphcull-core glyphcull-render glyphcull-host glyphcull-wasm glyphcull-desktop glyphcull-mobile glyphcull-ios"

fail=0

# --- Completeness: every package in the release order has a receipt ---
for pkg in $PACKAGES; do
  if ! ls release/receipts/"$pkg"-*.json >/dev/null 2>&1; then
    echo "error: no receipt for $pkg (release order: $PACKAGES)" >&2
    fail=1
  fi
done

# --- Per-receipt validation ---
for receipt in release/receipts/*.json; do
  [ -e "$receipt" ] || continue
  name="$(basename "$receipt")"

  # Schema: required fields, types, formats.
  jq -e '
    .project == "glyphcull" and
    (.repository | type == "string") and (.repository | length > 0) and
    (.package | type == "string") and (.package | length > 0) and
    (.version | type == "string") and (.version | length > 0) and
    (.git_commit | test("^[0-9a-f]{40}$")) and
    (.git_tree_clean == true) and
    (.source_archive_hash | test("^[0-9a-f]{64}$")) and
    (.package_archive_hash | test("^[0-9a-f]{64}$")) and
    (.toolchain | type == "object") and
    (.commands | type == "object") and
    (.results | type == "object") and
    (.results.build == "pass" or .results.build == "not-run" or .results.build == "fail") and
    (.results.test == "pass" or .results.test == "not-run" or .results.test == "fail") and
    (.results.conformance == "pass" or .results.conformance == "not-run" or .results.conformance == "fail") and
    (.results.package_dry_run == "pass" or .results.package_dry_run == "fail") and
    (.release_timestamp | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$"))
  ' "$receipt" >/dev/null || {
    echo "error: $name: schema violation" >&2
    fail=1
    continue
  }

  pkg="$(jq -r '.package' "$receipt")"
  version="$(jq -r '.version' "$receipt")"
  commit="$(jq -r '.git_commit' "$receipt")"
  src_hash="$(jq -r '.source_archive_hash' "$receipt")"

  # Filename must match the package/version it records.
  if [ "$name" != "$pkg-$version.json" ]; then
    echo "error: $name: filename does not match package $pkg version $version" >&2
    fail=1
  fi

  # The recorded commit must exist.
  if ! git cat-file -e "$commit^{commit}" 2>/dev/null; then
    echo "error: $name: git_commit $commit does not exist in this repository" >&2
    fail=1
    continue
  fi

  # Source-tree hash honesty: recompute from the recorded commit (deterministic
  # given the commit, independent of HEAD).
  actual="$(git ls-tree -r "$commit" | cut -f2- | LC_ALL=C sort | sha256sum | cut -d' ' -f1)"
  if [ "$actual" != "$src_hash" ]; then
    echo "error: $name: source_archive_hash $src_hash != $actual (computed from $commit)" >&2
    fail=1
  fi

  # Package-archive hash honesty (--full): assemble the package from a real
  # git checkout of the recorded commit (a worktree — cargo embeds
  # .cargo_vcs_info.json only when packaging from an actual git checkout, so
  # plain tar extraction would not reproduce the bytes) and compare.
  if [ "$MODE" = "--full" ]; then
    TMP="$(mktemp -d)"
    if git worktree add --detach "$TMP" "$commit" >/dev/null 2>&1; then
      real="$(
        cd "$TMP"
        if [ -f Cargo.toml ]; then
          cargo package -p "$pkg" --no-verify >/dev/null 2>&1 \
            && sha256sum "target/package/${pkg}-${version}.crate" | cut -d' ' -f1 \
            || true
        else
          tb="$(npm pack --pack-destination "$TMP" --json 2>/dev/null | jq -r '.[0].filename')"
          [ -n "$tb" ] && sha256sum "$TMP/$tb" | cut -d' ' -f1 || true
        fi
      )"
      if [ -z "$real" ]; then
        echo "error: $name: could not assemble the package from $commit" >&2
        fail=1
      elif [ "$real" != "$(jq -r '.package_archive_hash' "$receipt")" ]; then
        echo "error: $name: package_archive_hash drifted (recorded $(jq -r '.package_archive_hash' "$receipt") != $real from $commit)" >&2
        fail=1
      fi
    else
      echo "error: $name: could not check out $commit as a worktree" >&2
      fail=1
    fi
    git worktree remove --force "$TMP" >/dev/null 2>&1 || true
    rm -rf "$TMP"
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "release receipts: FAIL" >&2
  exit 1
fi
echo "release receipts OK ($MODE): schema, git, source hashes, completeness"
