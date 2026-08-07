//! glyphcull-mobile: the Android host (Phase 4.13).
//!
//! A winit application that loads a compiled `.cull` package from the APK
//! assets and serves the identical six-operation runtime API — `load`,
//! `scroll`, `paint`, `select`, `copy`, `destroy` — over the shared
//! [`glyphcull_desktop::DesktopSink`] (winit + wgpu, WebGPU/GL). The runtime
//! logic lives in `glyphcull-host`; this crate is the Android face of it:
//!
//! - `app` — the winit application (`MobileApp`/`MobileDocument`): the
//!   Android lifecycle (rebuild on `resumed`, drop on `suspended` — the
//!   native window and its surface die with the activity's pause), input
//!   wiring (wheel + touch drag, fling/inertia, pinch zoom, drag select), and
//!   the six operations.
//! - `asset` — the packaged document contract: the `assets/doc.cull` name,
//!   its validation, and the Android `AssetManager` read.
//! - `gesture` — the pure gesture math (fling kinematics + pinch zoom),
//!   fully headless-tested.
//!
//! The crate is one binary entry point: `android_main` (Android) loads the
//! packaged asset and runs the same app [`run_with`] serves on every other
//! platform (desktop runs double as smoke validation of the app logic).
//!
//! The same crate split, host structure, and sink serve iOS (Metal via
//! wgpu) with only the entry point and asset read differing — the recorded
//! roadmap item.

mod app;
mod asset;
mod gesture;

pub use app::{
    direction_for_delta, run_with, scroll_delta_for_touch, MobileApp, MobileDocument, MobileError,
};
pub use asset::{validate_asset_name, ASSET_NAME};
pub use gesture::{
    pinch_factor, scroll_release_velocity, zoom_of, zoom_viewport, Fling, FLING_DECAY,
    FLING_MAX_VELOCITY, FLING_SAMPLE_MS, FLING_STOP_EPSILON, MAX_ZOOM, MIN_ZOOM,
};

/// The Android entry point: android-activity calls `android_main` with the
/// app handle (winit needs it for the event loop — it has no other way to
/// reach the activity). Loads the packaged document and runs the same app as
/// [`run_with`]. Errors are logged; android-activity keeps the process alive.
///
/// `#[no_mangle]` is the platform's documented entry contract (android-activity
/// looks the unmangled symbol up via `dlsym`); rustc 1.82+ flags it under
/// `unsafe_code` because symbol export overrides name mangling. This is the
/// workspace's only unsafe surface, kept to this one ABI shim — the function
/// body itself is ordinary safe code.
#[cfg(target_os = "android")]
#[allow(unsafe_code)] // the unmangled `android_main` export (see above)
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    use winit::event_loop::EventLoop;
    use winit::platform::android::EventLoopBuilderExtAndroid;

    // Load the packaged document before the app handle moves into the event
    // loop builder (AndroidApp is Clone, but the asset read is order-free).
    let bytes = match asset::load_packaged(&app) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("glyphcull-mobile: {error}");
            return;
        }
    };
    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("winit event loop");
    let mut application = MobileApp::new(bytes, glyphcull_host::HostOptions::default());
    event_loop.run_app(&mut application).expect("winit app run");
}
