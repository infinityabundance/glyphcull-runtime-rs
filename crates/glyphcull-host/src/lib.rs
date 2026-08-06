//! glyphcull-host: the platform-agnostic document host (mirrors the JS
//! `DocumentHost` in `src/api/runtime.ts`).
//!
//! This is the single home of the runtime's six-operation contract — `load`,
//! `scroll`, `paint`, `select`, `copy`, `destroy` — composed from
//! `glyphcull-core` (layout, glyph cache, lifecycle, scheduler, draw list,
//! selection) and `glyphcull-render` (the render plan), against a pluggable
//! [`FrameSink`]. The wasm and desktop bindings are thin layers over it:
//! they supply a sink (wgpu surface for the canvas / the window), translate
//! host input into [`glyphcull_core::visibility::Viewport`]s and
//! [`glyphcull_core::selection::TextPosition`]s, and surface the typed
//! [`HostError`]s.
//!
//! Everything here is native-testable — no wasm-bindgen, no winit, no GPU
//! (the tests drive a recording sink).

mod host;

pub use host::{
    validate_options, FrameSink, HostDocument, HostError, HostOptions, DEFAULT_COOLING_MS,
    DEFAULT_FRAME_BUDGET_MS, DEFAULT_GLYPH_BUDGET, DEFAULT_MARGIN,
};

/// Pad an RGB8 byte run to RGBA8 (alpha 255). wgpu has no RGB8 texture
/// format, so every host sink converts at upload; shared here so the wasm and
/// desktop sinks are byte-identical.
#[must_use]
pub fn pad_rgb8_to_rgba8(pixels: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixels.len() / 3 * 4);
    for chunk in pixels.chunks_exact(3) {
        rgba.extend_from_slice(chunk);
        rgba.push(255);
    }
    rgba
}

/// Whether an image record's format needs the RGB→RGBA padding at upload.
#[must_use]
pub const fn image_is_rgb(format: glyphcull_core::reader::image::ImageFormat) -> bool {
    match format {
        glyphcull_core::reader::image::ImageFormat::Rgb8 => true,
        glyphcull_core::reader::image::ImageFormat::Rgba8 => false,
    }
}

#[cfg(test)]
mod tests {
    use glyphcull_core::reader::image::ImageFormat;

    use super::{image_is_rgb, pad_rgb8_to_rgba8};

    #[test]
    fn rgb_padding_appends_alpha_255() {
        assert_eq!(pad_rgb8_to_rgba8(&[]), Vec::<u8>::new());
        assert_eq!(pad_rgb8_to_rgba8(&[1, 2, 3]), vec![1, 2, 3, 255]);
        assert_eq!(
            pad_rgb8_to_rgba8(&[1, 2, 3, 4, 5, 6]),
            vec![1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    #[test]
    fn image_format_flag_matches_the_padding_need() {
        assert!(image_is_rgb(ImageFormat::Rgb8));
        assert!(!image_is_rgb(ImageFormat::Rgba8));
    }
}
