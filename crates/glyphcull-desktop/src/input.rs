//! Pure input translation for the desktop host: winit events → the
//! runtime's document-space operations.
//!
//! Everything here is a pure function of its inputs (no window, no GPU), so
//! the input contract is fully headless-testable. The winit-specific
//! [`MouseScrollDelta`] conversion is the only winit import, kept at the
//! boundary so the mapping functions themselves stay trivial.

use winit::event::MouseScrollDelta;

use glyphcull_core::visibility::Viewport;

/// A wheel event in either winit unit, decoupled from winit for testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelDelta {
    /// Line deltas (the platform's notion of one scroll notch).
    Lines(f32, f32),
    /// Physical-pixel deltas.
    PhysicalPixels(f64, f64),
}

impl From<MouseScrollDelta> for WheelDelta {
    fn from(delta: MouseScrollDelta) -> Self {
        match delta {
            MouseScrollDelta::LineDelta(x, y) => Self::Lines(x, y),
            MouseScrollDelta::PixelDelta(position) => Self::PhysicalPixels(position.x, position.y),
        }
    }
}

/// The document-pixel vertical scroll for a wheel event.
///
/// Line deltas scale by `line_height_px` (one notch ≈ one line); physical
/// pixels divide by the device-pixel ratio. Positive y scrolls the document
/// down (winit's natural wheel direction).
#[must_use]
pub fn scroll_for_wheel(delta: WheelDelta, dpr: f32, line_height_px: f32) -> f32 {
    match delta {
        WheelDelta::Lines(_x, y) => y * line_height_px,
        WheelDelta::PhysicalPixels(_x, y) => (y as f32) / dpr,
    }
}

/// The travel direction implied by a wheel delta (negative = up).
#[must_use]
pub fn direction_for_wheel(delta: WheelDelta) -> glyphcull_core::materialize::Direction {
    if scroll_for_wheel(delta, 1.0, 1.0) < 0.0 {
        glyphcull_core::materialize::Direction::Up
    } else {
        glyphcull_core::materialize::Direction::Down
    }
}

/// A cursor position in physical pixels → document space (÷ dpr).
#[must_use]
pub fn cursor_to_doc(position: (f64, f64), dpr: f32) -> (f32, f32) {
    ((position.0 as f32) / dpr, (position.1 as f32) / dpr)
}

/// Advance a viewport by a document-pixel scroll, clamped at the top.
#[must_use]
pub fn scrolled_viewport(viewport: Viewport, dy: f32) -> Viewport {
    Viewport {
        y: (viewport.y + dy).max(0.0),
        ..viewport
    }
}

/// A window's physical inner size → a viewport of that size (÷ dpr).
#[must_use]
pub fn viewport_for_size(width: u32, height: u32, dpr: f32) -> Viewport {
    Viewport {
        x: 0.0,
        y: 0.0,
        w: (width as f32) / dpr,
        h: (height as f32) / dpr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_wheels_scale_by_line_height() {
        assert_eq!(
            scroll_for_wheel(WheelDelta::Lines(0.0, 3.0), 1.0, 40.0),
            120.0
        );
        assert_eq!(
            scroll_for_wheel(WheelDelta::Lines(0.0, -1.0), 1.0, 40.0),
            -40.0
        );
        assert_eq!(
            scroll_for_wheel(WheelDelta::Lines(0.0, 0.0), 1.0, 40.0),
            0.0
        );
    }

    #[test]
    fn pixel_wheels_divide_by_dpr() {
        assert_eq!(
            scroll_for_wheel(WheelDelta::PhysicalPixels(0.0, 240.0), 2.0, 40.0),
            120.0
        );
        assert_eq!(
            scroll_for_wheel(WheelDelta::PhysicalPixels(0.0, 60.0), 1.5, 40.0),
            40.0
        );
    }

    #[test]
    fn direction_follows_the_sign() {
        assert_eq!(
            direction_for_wheel(WheelDelta::Lines(0.0, 1.0)),
            glyphcull_core::materialize::Direction::Down
        );
        assert_eq!(
            direction_for_wheel(WheelDelta::Lines(0.0, -1.0)),
            glyphcull_core::materialize::Direction::Up
        );
        // A zero delta defaults to Down (no motion — direction is moot).
        assert_eq!(
            direction_for_wheel(WheelDelta::Lines(0.0, 0.0)),
            glyphcull_core::materialize::Direction::Down
        );
    }

    #[test]
    fn cursor_physical_pixels_become_document_points() {
        assert_eq!(cursor_to_doc((200.0, 100.0), 2.0), (100.0, 50.0));
        assert_eq!(cursor_to_doc((0.0, 0.0), 1.0), (0.0, 0.0));
    }

    #[test]
    fn scrolling_clamps_at_the_top_and_never_goes_negative() {
        let viewport = Viewport {
            x: 0.0,
            y: 40.0,
            w: 800.0,
            h: 600.0,
        };
        assert_eq!(scrolled_viewport(viewport, 20.0).y, 60.0);
        assert_eq!(scrolled_viewport(viewport, -100.0).y, 0.0);
    }

    #[test]
    fn viewport_for_size_is_logical() {
        let viewport = viewport_for_size(1600, 1200, 2.0);
        assert_eq!(viewport.w, 800.0);
        assert_eq!(viewport.h, 600.0);
    }

    #[test]
    fn wheel_delta_from_winit() {
        let line = WheelDelta::from(MouseScrollDelta::LineDelta(0.0, 2.0));
        assert_eq!(line, WheelDelta::Lines(0.0, 2.0));
        let pixel = WheelDelta::from(MouseScrollDelta::PixelDelta((120.0, 60.0).into()));
        assert_eq!(pixel, WheelDelta::PhysicalPixels(120.0, 60.0));
    }
}
