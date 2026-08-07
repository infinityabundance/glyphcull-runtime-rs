//! Pure gesture math for the mobile host: fling/inertia kinematics and pinch
//! zoom. Everything here is a pure function of its inputs (no window, no GPU),
//! so the gesture contract is fully headless-testable; the winit wiring in
//! `app.rs` is the only place that touches touch events.
//!
//! Fling: the drag's release velocity (estimated from recent touch samples)
//! continues scrolling with an exponential velocity decay per frame — content
//! follows the finger, so the scroll velocity is the negation of the finger's
//! document-space movement.
//!
//! Pinch zoom: the viewport shrinks as the fingers spread (zoom level = the
//! document's fit-width ÷ the viewport width), keeping the document point
//! under the pinch midpoint stationary on screen. The drawing surface stays at
//! the window size (the host's `scroll_with_surface` seam), so the renderer
//! maps the zoomed viewport into the full surface — a crisp GPU zoom.

use glyphcull_core::visibility::Viewport;

/// The fling's exponential velocity decay per second.
pub const FLING_DECAY: f32 = 4.0;
/// The fling stop threshold (doc px/s): below this the animation halts.
pub const FLING_STOP_EPSILON: f32 = 8.0;
/// The release-velocity estimation window (ms of recent touch samples).
pub const FLING_SAMPLE_MS: u64 = 120;
/// The release-velocity clamp (doc px/s) — guards absurd flings.
pub const FLING_MAX_VELOCITY: f32 = 6000.0;
/// The minimum zoom (fraction of fit-width).
pub const MIN_ZOOM: f32 = 0.5;
/// The maximum zoom (fraction of fit-width).
pub const MAX_ZOOM: f32 = 4.0;

/// Estimate the scroll-continuation velocity at release (doc px/s; positive =
/// continue scrolling down) from recent `(elapsed_ms, doc_y)` touch samples,
/// newest last. Content follows the finger, so a finger moving up (y
/// decreasing) yields a positive (downward) scroll velocity. Under-determined
/// input (fewer than two samples, zero elapsed time) yields 0.
#[must_use]
pub fn scroll_release_velocity(samples: &[(u64, f32)]) -> f32 {
    let Some((t0, y0)) = samples.first().copied() else {
        return 0.0;
    };
    let Some((t1, y1)) = samples.last().copied() else {
        return 0.0;
    };
    if t1 <= t0 {
        return 0.0;
    }
    let velocity = (y0 - y1) * 1000.0 / (t1 - t0) as f32;
    velocity.clamp(-FLING_MAX_VELOCITY, FLING_MAX_VELOCITY)
}

/// A fling in progress: a scroll velocity that decays exponentially per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fling {
    /// The scroll velocity (doc px/s; positive = down).
    pub velocity: f32,
}

impl Fling {
    /// Start a fling at `velocity` (doc px/s; see [`scroll_release_velocity`]).
    #[must_use]
    pub const fn new(velocity: f32) -> Self {
        Self { velocity }
    }

    /// Advance one frame of `dt` seconds: returns the scroll delta (doc px)
    /// and decays the velocity (`v' = v·exp(−decay·dt)`); the fling halts
    /// below the stop epsilon.
    pub fn step(&mut self, dt: f32) -> f32 {
        let dt = dt.max(0.0);
        let dy = self.velocity * dt;
        self.velocity *= (-FLING_DECAY * dt).exp();
        if self.velocity.abs() < FLING_STOP_EPSILON {
            self.velocity = 0.0;
        }
        dy
    }

    /// Whether the fling has halted.
    #[must_use]
    pub const fn stopped(&self) -> bool {
        self.velocity == 0.0
    }
}

/// The zoom level of a viewport (1 = fit-width) for a document laid out at
/// `content_width`.
#[must_use]
pub fn zoom_of(viewport: Viewport, content_width: f32) -> f32 {
    content_width / viewport.w.max(1.0)
}

/// Zoom `viewport` by `factor` (> 1 zooms in), keeping the document point
/// under `anchor` (absolute doc px) stationary on screen. The zoom level is
/// clamped to [`MIN_ZOOM`]..=[`MAX_ZOOM`], and the viewport origin is clamped
/// to x ≥ 0, y ≥ 0 (the scroll's top clamp).
#[must_use]
pub fn zoom_viewport(
    viewport: Viewport,
    factor: f32,
    anchor: (f32, f32),
    content_width: f32,
) -> Viewport {
    let factor = factor.max(f32::MIN_POSITIVE);
    let zoom = (zoom_of(viewport, content_width) * factor).clamp(MIN_ZOOM, MAX_ZOOM);
    let scale = (content_width / zoom) / viewport.w.max(1.0);
    Viewport {
        x: (anchor.0 - (anchor.0 - viewport.x) * scale).max(0.0),
        y: (anchor.1 - (anchor.1 - viewport.y) * scale).max(0.0),
        w: (viewport.w * scale).max(1.0),
        h: (viewport.h * scale).max(1.0),
    }
}

/// The pinch zoom factor for a finger distance change (spreading = zoom in).
#[must_use]
pub fn pinch_factor(prev_dist: f32, cur_dist: f32) -> f32 {
    if prev_dist <= 0.0 {
        1.0
    } else {
        (cur_dist / prev_dist).clamp(1.0 / MAX_ZOOM, MAX_ZOOM)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

    use super::*;

    #[test]
    fn release_velocity_follows_the_scroll_convention() {
        // Finger moved up (y 500 → 300 over 100 ms): scroll continues down.
        let samples = [(0, 500.0), (50, 400.0), (100, 300.0)];
        assert!((scroll_release_velocity(&samples) - 2000.0).abs() < 1.0);
        // Finger moved down: scroll continues up.
        let samples = [(0, 300.0), (100, 500.0)];
        assert!((scroll_release_velocity(&samples) + 2000.0).abs() < 1.0);
    }

    #[test]
    fn release_velocity_is_undefined_for_poor_samples() {
        assert_eq!(scroll_release_velocity(&[]), 0.0);
        assert_eq!(scroll_release_velocity(&[(0, 100.0)]), 0.0);
        assert_eq!(scroll_release_velocity(&[(0, 100.0), (0, 200.0)]), 0.0);
    }

    #[test]
    fn release_velocity_is_clamped() {
        let wild = [(0, 10000.0), (10, 0.0)];
        assert!((scroll_release_velocity(&wild).abs() - FLING_MAX_VELOCITY).abs() < 1.0);
    }

    #[test]
    fn fling_decays_and_stops() {
        let mut fling = Fling::new(1000.0);
        let mut total = 0.0;
        for _ in 0..120 {
            let dt = 1.0 / 60.0;
            total += fling.step(dt);
            if fling.stopped() {
                break;
            }
        }
        assert!(fling.stopped(), "the fling must halt");
        assert!(total > 0.0, "the fling must scroll");
        // The discrete Euler sum v0·dt·Σ exp(−decay·dt·k) converges to
        // v0·dt/(1−exp(−decay·dt)) ≈ 258 at 60 fps (the continuous-time
        // integral v0/decay = 250 is the Euler limit's floor).
        assert!((250.0..260.0).contains(&total));
    }

    #[test]
    fn fling_zero_time_does_nothing() {
        let mut fling = Fling::new(500.0);
        assert_eq!(fling.step(0.0), 0.0);
        assert!(!fling.stopped());
    }

    #[test]
    fn zoom_of_fit_width_is_one() {
        let viewport = Viewport {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        assert!((zoom_of(viewport, 800.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_keeps_the_anchor_document_point_stationary() {
        let viewport = Viewport {
            x: 100.0,
            y: 50.0,
            w: 800.0,
            h: 600.0,
        };
        // The anchor's fraction within the viewport must be invariant.
        let anchor = (300.0, 250.0);
        let frac = |v: Viewport| ((anchor.0 - v.x) / v.w, (anchor.1 - v.y) / v.h);
        let before = frac(viewport);
        let zoomed = zoom_viewport(viewport, 2.0, anchor, 800.0);
        let after = frac(zoomed);
        assert!((before.0 - after.0).abs() < 1e-4);
        assert!((before.1 - after.1).abs() < 1e-4);
        assert!(zoomed.w < viewport.w, "zooming in shrinks the viewport");
        assert!((zoom_of(zoomed, 800.0) - 2.0).abs() < 1e-4);
    }

    #[test]
    fn zoom_clamps_at_the_bounds_and_top() {
        let viewport = Viewport {
            x: 10.0,
            y: 5.0,
            w: 800.0,
            h: 600.0,
        };
        let maxed = zoom_viewport(viewport, 100.0, (0.0, 0.0), 800.0);
        assert!((zoom_of(maxed, 800.0) - MAX_ZOOM).abs() < 1e-4);
        let mined = zoom_viewport(viewport, 0.001, (400.0, 300.0), 800.0);
        assert!((zoom_of(mined, 800.0) - MIN_ZOOM).abs() < 1e-4);
        // The origin clamps at the top-left.
        assert_eq!(mined.x, 0.0);
        assert_eq!(mined.y, 0.0);
    }

    #[test]
    fn pinch_factor_scales_with_the_distance_ratio() {
        assert!((pinch_factor(100.0, 200.0) - 2.0).abs() < 1e-6);
        assert!((pinch_factor(200.0, 100.0) - 0.5).abs() < 1e-6);
        assert_eq!(pinch_factor(0.0, 100.0), 1.0);
    }
}
