//! Desktop crate integration tests: the public surface that is testable
//! headlessly — the typed error contract and the input translation, composed
//! into end-to-end input scenarios. The window/GPU path (`DesktopDocument`)
//! needs a display server and is exercised by the `glyphcull-desktop` binary.

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use glyphcull_core::materialize::Direction;
use glyphcull_core::visibility::Viewport;
use glyphcull_desktop::input::{
    cursor_to_doc, direction_for_wheel, scroll_for_wheel, scrolled_viewport, viewport_for_size,
    WheelDelta,
};
use glyphcull_desktop::DesktopError;
use glyphcull_host::HostError;

fn viewport() -> Viewport {
    Viewport {
        x: 0.0,
        y: 40.0,
        w: 800.0,
        h: 600.0,
    }
}

#[test]
fn errors_format_with_their_kind() {
    assert_eq!(
        DesktopError::Window("oops".to_string()).to_string(),
        "window: oops"
    );
    assert_eq!(
        DesktopError::Renderer("no adapter".to_string()).to_string(),
        "renderer: no adapter"
    );
    assert_eq!(
        DesktopError::Load("bad bytes".to_string()).to_string(),
        "load: bad bytes"
    );
    assert_eq!(
        DesktopError::Host(HostError::Destroyed).to_string(),
        "host: destroyed"
    );
}

#[test]
fn host_errors_convert_into_desktop_errors() {
    let error: DesktopError = HostError::Destroyed.into();
    assert!(matches!(error, DesktopError::Host(HostError::Destroyed)));
}

#[test]
fn a_wheel_storm_scrolls_the_viewport_down_and_clamps_back() {
    let mut viewport = viewport();
    // Three notches of the wheel (line deltas), 40 px per line.
    for _ in 0..3 {
        let dy = scroll_for_wheel(WheelDelta::Lines(0.0, 1.0), 2.0, 40.0);
        viewport = scrolled_viewport(viewport, dy);
    }
    assert_eq!(viewport.y, 160.0);

    // Scroll far back up: clamped at the top, never negative.
    let dy = scroll_for_wheel(WheelDelta::Lines(0.0, -10.0), 2.0, 40.0);
    assert_eq!(scrolled_viewport(viewport, dy).y, 0.0);
}

#[test]
fn pixel_wheel_and_drag_share_the_dpr_space() {
    let dpr = 2.0;
    // A pixel-wheel notch equals 120 physical px → 60 doc px.
    let dy = scroll_for_wheel(WheelDelta::PhysicalPixels(0.0, 120.0), dpr, 40.0);
    assert_eq!(dy, 60.0);

    // Dragging across that same physical distance selects across 60 doc px.
    let (ax, ay) = cursor_to_doc((100.0, 100.0), dpr);
    let (bx, by) = cursor_to_doc((340.0, 100.0), dpr);
    assert_eq!((ax, ay), (50.0, 50.0));
    assert_eq!((bx - ax, by - ay), (120.0, 0.0));
}

#[test]
fn resize_recomputes_the_viewport_logically() {
    let vp = viewport_for_size(1920, 1080, 2.0);
    assert_eq!((vp.w, vp.h), (960.0, 540.0));
    let vp = viewport_for_size(800, 600, 1.0);
    assert_eq!((vp.w, vp.h), (800.0, 600.0));
}

#[test]
fn direction_tracks_the_wheel_sign() {
    assert_eq!(
        direction_for_wheel(WheelDelta::Lines(0.0, 1.0)),
        Direction::Down
    );
    assert_eq!(
        direction_for_wheel(WheelDelta::Lines(0.0, -1.0)),
        Direction::Up
    );
    assert_eq!(
        direction_for_wheel(WheelDelta::PhysicalPixels(0.0, -60.0)),
        Direction::Up
    );
}
