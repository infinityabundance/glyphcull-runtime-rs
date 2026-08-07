#!/bin/sh
# The aarch64-apple-ios linker: clang with the iOS target triple and the
# iPhoneOS SDK sysroot. cargo's default linker for the target (`cc`) would
# target macOS on a macOS host; this wrapper supplies the -target/-isysroot
# pair so `cargo build --target aarch64-apple-ios` links against the real
# iOS SDK. macOS-only (needs Xcode for the SDK path).
set -e

if [ -z "${SDKROOT:-}" ]; then
    SDKROOT="$(xcrun --sdk iphoneos --show-sdk-path)"
fi
exec /usr/bin/clang \
    -target "arm64-apple-ios${IOS_DEPLOYMENT_TARGET:-13.0}" \
    -isysroot "$SDKROOT" \
    "$@"
