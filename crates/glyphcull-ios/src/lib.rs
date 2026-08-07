//! glyphcull-ios: the iOS host (Phase 4.13 follow-up).
//!
//! The iOS face of the six-operation runtime API — `load`, `scroll`, `paint`,
//! `select`, `copy`, `destroy` — over the shared winit application and the
//! re-attachable [`glyphcull_desktop::DesktopSink`] (wgpu's Metal backend via
//! `Backends::PRIMARY`). Per the recorded plan, this is the **same crate split
//! as the Android host: entry + asset read only** — everything else is the
//! shared `glyphcull-mobile` app (the winit lifecycle, the gesture layer, the
//! six operations):
//!
//! - The app: `glyphcull-mobile`'s `MobileApp`/`MobileDocument`/`run_with`
//!   (winit's `EventLoop::run_app` drives the whole UIKit app — winit's iOS
//!   backend calls `UIApplicationMain`).
//! - The asset read: the packaged document is a **bundle resource** — the app
//!   bundle pipeline embeds `doc.cull` at the bundle root, and this crate reads
//!   it from the executable's directory (the `.app` root), a documented
//!   convention (no FFI: `std::env::current_exe` only).
//! - The entry: the app bundle's binary is the crate's `main` (`src/main.rs`),
//!   which calls [`run_with`] — there is no `#[no_mangle]` shim or Objective-C
//!   shell, because iOS apps start from `UIApplicationMain`, which winit
//!   invokes from `run_with`.
//!
//! CI type-checks this crate for `aarch64-apple-ios`, `aarch64-apple-ios-sim`,
//! and `x86_64-apple-ios`, and the `ios-ipa.yml` workflow links + bundles the
//! real `.ipa` on a macOS runner (Xcode's iPhoneOS SDK):
//! `crates/glyphcull-ios/scripts/build-ipa.sh`.

use std::path::PathBuf;

pub use glyphcull_mobile::{
    direction_for_delta, run_with, scroll_delta_for_touch, validate_asset_name, MobileApp,
    MobileDocument, MobileError, ASSET_NAME,
};
// The gesture module's pure math is shared verbatim (the mobile host's
// fling/pinch layer runs identically on iOS).
pub use glyphcull_mobile::{
    pinch_factor, scroll_release_velocity, zoom_of, zoom_viewport, Fling, FLING_DECAY,
    FLING_MAX_VELOCITY, FLING_SAMPLE_MS, FLING_STOP_EPSILON, MAX_ZOOM, MIN_ZOOM,
};

/// The packaged document's bundle resource name (Xcode "Copy Bundle
/// Resources" must embed it at the bundle root).
pub const BUNDLE_RESOURCE: &str = ASSET_NAME;

/// Resolve the bundle resource's path: the packaged document sits next to the
/// app executable (the `.app` directory root). Pure path logic (unit-tested);
/// the iOS-only read uses it.
#[must_use]
pub fn bundle_resource_path(exe: &std::path::Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let path = dir.join(BUNDLE_RESOURCE);
    Some(path)
}

/// Load the packaged `.cull` document from the app bundle. iOS-only: the
/// resource path comes from [`bundle_resource_path`] (the executable's
/// directory — `std::env::current_exe` on iOS is the app binary inside the
/// `.app` bundle). On any failure a typed [`MobileError`] carries the reason.
#[cfg(target_os = "ios")]
pub fn load_packaged() -> Result<Vec<u8>, MobileError> {
    let exe = std::env::current_exe()
        .map_err(|error| MobileError::Asset(format!("current_exe: {error}")))?;
    let path = bundle_resource_path(&exe)
        .ok_or_else(|| MobileError::Asset("executable has no parent directory".to_string()))?;
    let bytes = std::fs::read(&path)
        .map_err(|error| MobileError::Asset(format!("read {:?}: {error}", path)))?;
    if bytes.is_empty() {
        return Err(MobileError::Asset(format!(
            "bundle resource {:?} is empty",
            path
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_bundle_resource_resolves_next_to_the_executable() {
        // The app binary lives at .../Foo.app/Foo; the resource sits beside
        // it in the bundle root.
        let exe = std::path::Path::new("/app/GlyphCull.app/GlyphCull");
        let path = bundle_resource_path(exe).expect("a parent exists");
        assert_eq!(path, PathBuf::from("/app/GlyphCull.app/doc.cull"));
    }

    #[test]
    fn a_relative_executable_resolves_relative_to_cwd() {
        // A relative exe path's parent is "" — the resource resolves
        // relative to the working directory (iOS always passes an absolute
        // app path; this pins the fallback contract).
        let exe = std::path::Path::new("GlyphCull");
        assert_eq!(bundle_resource_path(exe), Some(PathBuf::from("doc.cull")));
    }

    #[test]
    fn the_resource_name_is_the_packaged_document() {
        assert_eq!(BUNDLE_RESOURCE, "doc.cull");
        validate_asset_name(BUNDLE_RESOURCE).expect("the resource name must validate");
    }
}
