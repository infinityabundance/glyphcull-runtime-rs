//! The GlyphCull iOS app executable (the `.app` bundle's binary).
//!
//! Loads the packaged `.cull` document from the bundle (the `doc.cull`
//! resource beside the executable — the documented iOS asset contract) and
//! runs the shared winit application. winit's iOS backend calls
//! `UIApplicationMain` from `run_with`, so the bundle needs no Objective-C
//! shell: this binary IS the app's `main`.
//!
//! Built for `aarch64-apple-ios` by the Xcode/CI app-bundle pipeline
//! (`scripts/build-ipa.sh` + the `ios-ipa.yml` workflow). On every other
//! target this binary exists only so the workspace still builds — the app
//! bundle is an iOS artifact.

/// The iOS entry: load the bundle resource and run the app.
#[cfg(target_os = "ios")]
fn main() {
    use glyphcull_host::HostOptions;

    let bytes = match glyphcull_ios::load_packaged() {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("glyphcull-ios: {error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = glyphcull_ios::run_with(&bytes, HostOptions::default()) {
        eprintln!("glyphcull-ios: {error}");
        std::process::exit(1);
    }
}

/// Non-iOS builds: the bundle binary has nothing to do (the workspace keeps
/// building; the app bundle pipeline targets iOS only).
#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!("glyphcull-ios: the app bundle binary is an iOS-only artifact");
}
