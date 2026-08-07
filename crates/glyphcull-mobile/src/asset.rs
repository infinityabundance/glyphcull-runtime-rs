//! The packaged document: which APK asset holds the `.cull` package, how the
//! asset name is validated, and how the bytes are read on Android.
//!
//! The asset name is a fixed contract between the APK packaging (the Docker
//! build places the compiled document at `assets/<ASSET_NAME>`) and the host.
//! The validation is pure and unit-tested; the actual read is Android-only
//! (the `AssetManager` is an NDK type).

#[cfg(target_os = "android")]
use crate::app::MobileError;
#[cfg(target_os = "android")]
use std::ffi::CString;

/// The packaged document's asset name (the APK's `assets/` directory).
pub const ASSET_NAME: &str = "doc.cull";

/// Validate an asset name for the Android `AssetManager` before opening it:
/// a non-empty relative path with no empty/`.`/`..` components, no leading
/// `/` (the manager resolves against `assets/`), and no backslashes (the
/// manager is not a Windows-style path parser). Pure (unit-tested).
pub fn validate_asset_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("asset name is empty".to_string());
    }
    if name.starts_with('/') {
        return Err(format!(
            "\"{name}\" is absolute; asset names are relative to assets/"
        ));
    }
    if name.contains('\\') {
        return Err(format!("\"{name}\" contains a backslash"));
    }
    for component in name.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!(
                "\"{name}\" has an empty, '.', or '..' path component"
            ));
        }
    }
    Ok(())
}

/// Load the packaged `.cull` document from the APK assets. Android-only: the
/// `AndroidApp` handle comes from `android_main` (see `lib.rs`); the asset is
/// opened by name and read to the end. On any failure a typed [`MobileError`]
/// carries the reason (the host logs it and stays up for the next resume).
#[cfg(target_os = "android")]
pub fn load_packaged(app: &android_activity::AndroidApp) -> Result<Vec<u8>, MobileError> {
    use std::io::Read;

    validate_asset_name(ASSET_NAME).map_err(MobileError::Asset)?;
    let manager = app.asset_manager();
    let name = CString::new(ASSET_NAME)
        .map_err(|_| MobileError::Asset(format!("\"{ASSET_NAME}\" contains a NUL byte")))?;
    let mut asset = manager
        .open(&name)
        .ok_or_else(|| MobileError::Asset(format!("asset \"{ASSET_NAME}\" not found")))?;
    let mut bytes = Vec::new();
    asset
        .read_to_end(&mut bytes)
        .map_err(|error| MobileError::Asset(format!("read \"{ASSET_NAME}\": {error}")))?;
    if bytes.is_empty() {
        return Err(MobileError::Asset(format!(
            "asset \"{ASSET_NAME}\" is empty"
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn the_packaged_asset_name_is_valid() {
        validate_asset_name(ASSET_NAME).expect("the packaged name must validate");
        assert_eq!(ASSET_NAME, "doc.cull");
    }

    #[test]
    fn nested_relative_names_are_valid() {
        validate_asset_name("docs/guide.cull").expect("nested relative names are fine");
        validate_asset_name("a/b/c.cull").expect("deep nesting is fine");
    }

    #[test]
    fn empty_names_are_rejected() {
        assert!(validate_asset_name("").is_err());
    }

    #[test]
    fn absolute_names_are_rejected() {
        assert!(validate_asset_name("/doc.cull").is_err());
        assert!(validate_asset_name("assets/doc.cull").is_ok()); // relative, fine
    }

    #[test]
    fn parent_and_current_components_are_rejected() {
        assert!(validate_asset_name("a/../doc.cull").is_err());
        assert!(validate_asset_name("./doc.cull").is_err());
        assert!(validate_asset_name("a//b.cull").is_err());
        assert!(validate_asset_name("a/.b.cull").is_ok()); // a plain dot-prefixed file
    }

    #[test]
    fn backslashes_are_rejected() {
        assert!(validate_asset_name("a\\b.cull").is_err());
    }
}
