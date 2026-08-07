#!/usr/bin/env bash
# Build the GlyphCull iOS app bundle (.ipa) with a real Xcode toolchain.
#
# macOS-only: needs Xcode (the iPhoneOS SDK for the linker sysroot, codesign,
# and xcrun). This is the app-bundle step of the iOS pipeline — the crate
# itself type-checks everywhere (CI), but linking + bundling the app happens
# here and in the ios-ipa.yml workflow (a macOS runner).
#
# Produces: dist/GlyphCull-<version>.ipa (ad-hoc signed — installable on the
# simulator; on-device install needs a real signing team, recorded in
# docs/ios-build.md).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO_ROOT"

TARGET="aarch64-apple-ios"
BIN="glyphcull-ios"
VERSION="0.1.0"
APP_NAME="GlyphCull"
APP_DIR="crates/glyphcull-ios/dist/Payload/$APP_NAME.app"
IPA="crates/glyphcull-ios/dist/GlyphCull-$VERSION.ipa"

export SDKROOT="${SDKROOT:-$(xcrun --sdk iphoneos --show-sdk-path)}"
export CARGO_TARGET_AARCH64_APPLE_IOS_LINKER="$REPO_ROOT/crates/glyphcull-ios/app/ios-linker.sh"

echo "==> Building $BIN for $TARGET (SDK: $SDKROOT)"
cargo build --release -p glyphcull-ios --bin "$BIN" --target "$TARGET"

echo "==> Assembling $APP_NAME.app"
rm -rf "crates/glyphcull-ios/dist/Payload"
mkdir -p "$APP_DIR"
cp "target/$TARGET/release/$BIN" "$APP_DIR/$APP_NAME"
cp "crates/glyphcull-ios/app/Info.plist" "$APP_DIR/Info.plist"
printf 'APPL????' > "$APP_DIR/PkgInfo"
# The packaged document: the documented iOS asset contract (the app reads
# doc.cull from the bundle root, next to the executable).
cp "crates/glyphcull-core/tests/fixtures/v1-minimal.cull" "$APP_DIR/doc.cull"

echo "==> Signing (ad-hoc)"
xcrun codesign --force --sign - --timestamp=none "$APP_DIR"

echo "==> Packaging $IPA"
(cd "crates/glyphcull-ios/dist" && zip -qry - Payload > "../$IPA")
ls -la "$IPA"

echo "==> Verification"
xcrun codesign --verify --deep --strict "$APP_DIR"
plutil -lint "$APP_DIR/Info.plist"
file "$APP_DIR/$APP_NAME"
unzip -l "$IPA"
