//! The glyphcull-desktop demo binary: open a compiled `.cull` package in a
//! window (wheel to scroll, drag to select, Esc/close to quit), or capture
//! the first frame to a PNG and exit (`--screenshot out.png [--size WxH]` —
//! the headless-capture mode the demo smoke harness validates).
//!
//! The desktop host is for native targets; on wasm the binary is empty (the
//! library still compiles — CI builds every workspace member for wasm32).

fn main() {
    #[cfg(not(target_family = "wasm"))]
    std::process::exit(real_main());
}

#[cfg(not(target_family = "wasm"))]
fn real_main() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let launch = match glyphcull_desktop::parse_args(&args) {
        Ok(launch) => launch,
        Err(message) => {
            eprintln!("usage: glyphcull-desktop <file.cull> [--screenshot <path>] [--size WxH]");
            eprintln!("error: {message}");
            return 2;
        }
    };
    let bytes = match std::fs::read(&launch.package) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("glyphcull-desktop: {}: {error}", launch.package.display());
            return 1;
        }
    };
    let host_launch = glyphcull_desktop::HostLaunch {
        window_size: launch.window_size,
        screenshot: launch.screenshot,
        scroll_y: launch.scroll_y,
    };
    match glyphcull_desktop::run_with(&bytes, Default::default(), host_launch) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("glyphcull-desktop: {error}");
            1
        }
    }
}
