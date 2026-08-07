//! Build script: enable the `web` cfg on wasm32 targets.
//!
//! `web` is a custom cfg, not a rustc built-in. It marks the canvas-dependent
//! API (`load`/`load_inner`/`parse_options` and their canvas imports) so that
//! native builds — `cargo test`, clippy, doc — compile the crate without a DOM
//! canvas, while wasm32 builds actually type-check and ship the web entry
//! points. Doing it here (rather than `.cargo/config.toml` rustflags) keeps
//! the cfg correct no matter how the crate is built: as a workspace member,
//! as a published dependency, or through wasm-bindgen tooling.
//!
//! The same script declares the cfg for the `unexpected_cfgs` lint on every
//! target, so `#[cfg(web)]` never triggers a spurious warning natively.

fn main() {
    // Always declare the cfg so the lint stays clean on non-wasm targets
    // (where the cfg is not set, `#[cfg(web)]` items are simply stripped).
    println!("cargo:rustc-check-cfg=cfg(web)");
    if std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|arch| arch == "wasm32") {
        println!("cargo:rustc-cfg=web");
    }
}
