//! The injected clock — the determinism seam for time-based behavior
//! (mirrors the JS `src/clock.ts`).
//!
//! The lifecycle's cooling periods and the materializer's budgets are the
//! only time-sensitive decisions in the runtime. They never read the wall
//! clock directly: they call [`Clock::now`], so tests inject a
//! [`FakeClock`] and the transition log is byte-deterministic
//! (Architecture.md §5).

/// A time source. `now()` returns monotonic-ish milliseconds.
pub trait Clock {
    /// The current time in milliseconds.
    fn now(&self) -> u64;
}

/// The wall clock (production).
#[derive(Debug, Default, Clone, Copy)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
    }
}

/// A deterministic, test-injectable clock.
///
/// Interior mutability (`Cell`) lets the manager hold a shared reference
/// while tests advance time — the same shape as the JS runtime, which passes
/// the clock object by reference.
#[derive(Debug, Default, Clone)]
pub struct FakeClock {
    t: std::cell::Cell<u64>,
}

impl FakeClock {
    /// Create a clock at time 0.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            t: std::cell::Cell::new(0),
        }
    }

    /// Advance the clock by `ms` milliseconds.
    pub fn advance(&self, ms: u64) {
        self.t.set(self.t.get().saturating_add(ms));
    }
}

impl Clock for FakeClock {
    fn now(&self) -> u64 {
        self.t.get()
    }
}

#[cfg(test)]
mod tests {
    use super::{Clock, FakeClock, RealClock};

    #[test]
    fn fake_clock_advances_monotonically() {
        let clock = FakeClock::new();
        assert_eq!(clock.now(), 0);
        clock.advance(500);
        assert_eq!(clock.now(), 500);
        clock.advance(0);
        assert_eq!(clock.now(), 500);
    }

    #[test]
    fn real_clock_reports_an_epoch_timestamp() {
        // The real clock reads the wall clock; it must be a large positive
        // millisecond timestamp (well past the epoch).
        assert!(RealClock.now() > 1_700_000_000_000);
    }
}
