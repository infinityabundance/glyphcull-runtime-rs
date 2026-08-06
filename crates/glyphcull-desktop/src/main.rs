//! The glyphcull-desktop demo binary: open a compiled `.cull` package in a
//! window (wheel to scroll, drag to select, Esc/close to quit).
//!
//! The desktop host is for native targets; on wasm the binary is empty (the
//! library still compiles — CI builds every workspace member for wasm32).

fn main() {
    #[cfg(not(target_family = "wasm"))]
    std::process::exit(real_main());
}

#[cfg(not(target_family = "wasm"))]
fn real_main() -> i32 {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: glyphcull-desktop <file.cull>");
        return 2;
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("glyphcull-desktop: {path}: {error}");
            return 1;
        }
    };
    match glyphcull_desktop::run(&bytes, Default::default()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("glyphcull-desktop: {error}");
            1
        }
    }
}
