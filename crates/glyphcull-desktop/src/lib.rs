//! glyphcull-desktop: the winit host (Phase 4.11).
//!
//! A native window that loads a compiled `.cull` package and serves the
//! identical six-operation runtime API — `load`, `scroll`, `paint`, `select`,
//! `copy`, `destroy` — over a wgpu surface (WebGPU or GL), mirroring the wasm
//! binding exactly. The runtime logic itself lives in `glyphcull-host`; this
//! crate is the winit face of it:
//!
//! - `DesktopDocument` — a window + the shared host, with the six operations
//!   and the input wiring (wheel → scroll, drag → select, Esc/close → exit).
//! - `input` — pure, headless-testable input translation (winit events →
//!   document-space operations).
//! - `sink` — the native wgpu `FrameSink` (re-attachable `Surface<'static>`
//!   from the Arc-backed window handle, non-sRGB target, per-frame present)
//!   with headless frame capture (DESIGN.md D31); shared with the mobile host
//!   (DESIGN.md D29).
//! - the `glyphcull-desktop` binary — open a `.cull` file in a window, or
//!   `--screenshot out.png --size WxH` for the headless-capture smoke mode
//!   (the demo's `scripts/desktop-smoke.sh` validates the capture against the
//!   CPU reference compositor).

mod app;
pub mod input;
mod sink;

pub use app::{run, run_with, DesktopDocument, DesktopError, HostLaunch};
pub use sink::DesktopSink;

use std::path::PathBuf;

/// The binary's command-line surface (pure; unit-tested).
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchArgs {
    /// The `.cull` package path.
    pub package: PathBuf,
    /// The requested physical window size (`--size WxH`).
    pub window_size: Option<(u32, u32)>,
    /// The screenshot output path (`--screenshot <path>`).
    pub screenshot: Option<PathBuf>,
    /// The initial document scroll (`--scroll <y>`, document px).
    pub scroll_y: Option<f32>,
}

/// Parse the binary's arguments: `glyphcull-desktop <file.cull>
/// [--screenshot <path>] [--size WxH] [--scroll <y>]`. Unknown flags and a
/// missing package are errors (a `String` message; the binary maps it to exit
/// code 2).
pub fn parse_args(args: &[String]) -> Result<LaunchArgs, String> {
    let mut package: Option<PathBuf> = None;
    let mut window_size: Option<(u32, u32)> = None;
    let mut screenshot: Option<PathBuf> = None;
    let mut scroll_y: Option<f32> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--screenshot" => {
                let value = iter.next().ok_or("--screenshot requires a path")?;
                screenshot = Some(PathBuf::from(value));
            }
            "--size" => {
                let value = iter.next().ok_or("--size requires WxH (e.g. 800x600)")?;
                let (w, h) = value
                    .split_once('x')
                    .ok_or_else(|| format!("--size expects WxH, got \"{value}\""))?;
                let width = w
                    .parse::<u32>()
                    .map_err(|_| format!("--size width must be a number, got \"{w}\""))?;
                let height = h
                    .parse::<u32>()
                    .map_err(|_| format!("--size height must be a number, got \"{h}\""))?;
                if width == 0 || height == 0 {
                    return Err("--size dimensions must be positive".to_string());
                }
                window_size = Some((width, height));
            }
            "--scroll" => {
                let value = iter.next().ok_or("--scroll requires a y (document px)")?;
                let y = value
                    .parse::<f32>()
                    .map_err(|_| format!("--scroll y must be a number, got \"{value}\""))?;
                if !y.is_finite() || y < 0.0 {
                    return Err("--scroll y must be a finite non-negative number".to_string());
                }
                scroll_y = Some(y);
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown flag \"{flag}\""));
            }
            _ => {
                if package.is_some() {
                    return Err(format!("unexpected extra argument \"{arg}\""));
                }
                package = Some(PathBuf::from(arg));
            }
        }
    }
    let package = package.ok_or("missing package path (usage: glyphcull-desktop <file.cull> [--screenshot <path>] [--size WxH] [--scroll <y>])")?;
    Ok(LaunchArgs {
        package,
        window_size,
        screenshot,
        scroll_y,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::parse_args;
    use std::path::PathBuf;

    #[test]
    fn package_only() {
        let args = parse_args(&["doc.cull".to_string()]).expect("parses");
        assert_eq!(args.package, PathBuf::from("doc.cull"));
        assert_eq!(args.window_size, None);
        assert_eq!(args.screenshot, None);
        assert_eq!(args.scroll_y, None);
    }

    #[test]
    fn full_launch_flags() {
        let args = parse_args(&[
            "doc.cull".to_string(),
            "--screenshot".to_string(),
            "out.png".to_string(),
            "--size".to_string(),
            "800x600".to_string(),
            "--scroll".to_string(),
            "1174".to_string(),
        ])
        .expect("parses");
        assert_eq!(args.window_size, Some((800, 600)));
        assert_eq!(args.screenshot, Some(PathBuf::from("out.png")));
        assert_eq!(args.scroll_y, Some(1174.0));
    }

    #[test]
    fn flag_order_is_free() {
        let args = parse_args(&[
            "--size".to_string(),
            "1024x768".to_string(),
            "doc.cull".to_string(),
        ])
        .expect("parses");
        assert_eq!(args.window_size, Some((1024, 768)));
    }

    #[test]
    fn missing_package_is_an_error() {
        assert!(parse_args(&[]).is_err());
        assert!(parse_args(&["--screenshot".to_string(), "o.png".to_string()]).is_err());
    }

    #[test]
    fn malformed_flags_are_errors() {
        assert!(parse_args(&["a.cull".to_string(), "--bogus".to_string()]).is_err());
        assert!(parse_args(&[
            "a.cull".to_string(),
            "--size".to_string(),
            "wide".to_string()
        ])
        .is_err());
        assert!(parse_args(&[
            "a.cull".to_string(),
            "--size".to_string(),
            "0x600".to_string()
        ])
        .is_err());
        assert!(parse_args(&["a.cull".to_string(), "--screenshot".to_string()]).is_err());
        assert!(parse_args(&[
            "a.cull".to_string(),
            "--scroll".to_string(),
            "-1".to_string()
        ])
        .is_err());
        assert!(parse_args(&[
            "a.cull".to_string(),
            "--scroll".to_string(),
            "nan".to_string()
        ])
        .is_err());
        assert!(parse_args(&["a.cull".to_string(), "b.cull".to_string()]).is_err());
    }
}
