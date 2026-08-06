//! Materialization scheduler tests: priority ordering, budgets, cooperative
//! yielding (no starvation), reconcile, eviction, and determinism (mirrors
//! the JS `test/materialize/scheduler.test.ts`).

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use proptest::prelude::*;

use glyphcull_core::clock::FakeClock;
use glyphcull_core::lifecycle::{ChunkLifecycleConfig, ChunkState, LifecycleManager};
use glyphcull_core::materialize::{
    priority_key, Direction, MaterializationScheduler, MaterializeWorker, SchedulerOptions,
    WorkResult,
};
use glyphcull_core::visibility::{Rect, Viewport};

fn fresh(clock: &FakeClock, frame_budget_ms: u64) -> MaterializationScheduler<'_, FakeClock> {
    let mut lifecycle = LifecycleManager::new(
        clock,
        glyphcull_core::lifecycle::LifecycleOptions {
            default_cooling_period_ms: 1000,
        },
    );
    for id in 1..=10 {
        lifecycle
            .register(
                id,
                ChunkLifecycleConfig {
                    hidden: false,
                    cooling_period_ms: 1000,
                },
            )
            .expect("register");
    }
    MaterializationScheduler::new(
        clock,
        lifecycle,
        SchedulerOptions {
            frame_budget_ms,
            yield_penalty: 1,
        },
    )
}

const VIEWPORT: Viewport = Viewport {
    x: 0.0,
    y: 0.0,
    w: 400.0,
    h: 100.0,
};

/// A worker that completes each chunk in a fixed number of visits.
#[derive(Debug, Default)]
struct VisitsWorker {
    visits_needed: u32,
    visits: Vec<u32>,
    released: Vec<u32>,
    seen: std::collections::HashMap<u32, u32>,
}

impl VisitsWorker {
    fn new(visits_needed: u32) -> Self {
        Self {
            visits_needed,
            visits: Vec::new(),
            released: Vec::new(),
            seen: std::collections::HashMap::new(),
        }
    }
}

impl MaterializeWorker for VisitsWorker {
    fn work(&mut self, chunk_id: u32, _budget_ms: u64, _elapsed_ms: u64) -> WorkResult {
        self.visits.push(chunk_id);
        let count = self.seen.get(&chunk_id).copied().unwrap_or(0) + 1;
        self.seen.insert(chunk_id, count);
        if count >= self.visits_needed {
            WorkResult::Complete
        } else {
            WorkResult::Yield
        }
    }

    fn release(&mut self, chunk_id: u32) {
        self.released.push(chunk_id);
    }
}

fn geometry_fn(map: &std::collections::HashMap<u32, Rect>) -> impl Fn(u32) -> Option<Rect> + '_ {
    move |id| map.get(&id).copied()
}

#[test]
fn priority_key_prioritizes_intersecting_chunks_over_distant_ones() {
    let vp = Viewport {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 100.0,
    };
    let on = priority_key(
        1,
        Some(Rect {
            x: 0.0,
            y: 50.0,
            w: 100.0,
            h: 20.0,
        }),
        vp,
        Direction::Down,
        0,
    );
    let below = priority_key(
        2,
        Some(Rect {
            x: 0.0,
            y: 500.0,
            w: 100.0,
            h: 20.0,
        }),
        vp,
        Direction::Down,
        1,
    );
    let far_below = priority_key(
        3,
        Some(Rect {
            x: 0.0,
            y: 1000.0,
            w: 100.0,
            h: 20.0,
        }),
        vp,
        Direction::Down,
        2,
    );
    assert!(on < below);
    assert!(below < far_below);
}

#[test]
fn priority_key_favors_chunks_ahead_of_the_direction_of_travel() {
    let vp = Viewport {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 100.0,
    };
    let below_rect = Rect {
        x: 0.0,
        y: 500.0,
        w: 100.0,
        h: 20.0,
    };
    let above_rect = Rect {
        x: 0.0,
        y: -500.0,
        w: 100.0,
        h: 20.0,
    };
    // Scrolling down: below (ahead) has priority over above (behind).
    let down_below = priority_key(1, Some(below_rect), vp, Direction::Down, 0);
    let down_above = priority_key(2, Some(above_rect), vp, Direction::Down, 1);
    assert!(down_below < down_above);
    // Scrolling up: the opposite.
    let up_below = priority_key(1, Some(below_rect), vp, Direction::Up, 0);
    let up_above = priority_key(2, Some(above_rect), vp, Direction::Up, 1);
    assert!(up_above < up_below);
}

#[test]
fn priority_key_tie_breaks_by_document_order() {
    let vp = Viewport {
        x: 0.0,
        y: 0.0,
        w: 400.0,
        h: 100.0,
    };
    let a = priority_key(
        1,
        Some(Rect {
            x: 0.0,
            y: 500.0,
            w: 100.0,
            h: 20.0,
        }),
        vp,
        Direction::Down,
        0,
    );
    let b = priority_key(
        2,
        Some(Rect {
            x: 0.0,
            y: 500.0,
            w: 100.0,
            h: 20.0,
        }),
        vp,
        Direction::Down,
        1,
    );
    assert!(a < b);
}

#[test]
fn processes_intersecting_chunks_before_distant_ones_in_document_order() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 1000);
    // Geometry: chunk 3 intersects; chunks 1, 2, 5 distant.
    let mut map = std::collections::HashMap::new();
    map.insert(
        3,
        Rect {
            x: 0.0,
            y: 50.0,
            w: 100.0,
            h: 20.0,
        },
    );
    map.insert(
        1,
        Rect {
            x: 0.0,
            y: 5000.0,
            w: 100.0,
            h: 20.0,
        },
    );
    map.insert(
        2,
        Rect {
            x: 0.0,
            y: 4000.0,
            w: 100.0,
            h: 20.0,
        },
    );
    let geometry = geometry_fn(&map);
    scheduler
        .reconcile(&[1, 2, 3, 5], VIEWPORT, Direction::Down, &geometry)
        .expect("reconcile");
    let mut worker = VisitsWorker::new(1);
    scheduler.run_frame(&mut worker).expect("frame");
    // 3 (intersecting) first; 5 has no geometry → ordinal 4 → before 2.
    assert_eq!(worker.visits[0], 3);
    assert!(worker.visits.contains(&5));
    assert!(
        worker.visits.iter().position(|&v| v == 5).unwrap()
            < worker.visits.iter().position(|&v| v == 2).unwrap()
    );
    assert!(
        worker.visits.iter().position(|&v| v == 2).unwrap()
            < worker.visits.iter().position(|&v| v == 1).unwrap()
    );
    // Everything completed in one frame.
    assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Visible), 4);
}

#[test]
fn respects_the_frame_budget() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 10);
    let map = std::collections::HashMap::new();
    let geometry = geometry_fn(&map);
    scheduler
        .reconcile(&[1, 2, 3], VIEWPORT, Direction::Down, &geometry)
        .expect("reconcile");
    // The worker advances the (fake) clock, simulating real work time.
    struct ClockAdvancingWorker<'c> {
        clock: &'c FakeClock,
    }
    impl MaterializeWorker for ClockAdvancingWorker<'_> {
        fn work(&mut self, _chunk_id: u32, _budget_ms: u64, _elapsed_ms: u64) -> WorkResult {
            self.clock.advance(8);
            WorkResult::Complete
        }
        fn release(&mut self, _chunk_id: u32) {}
    }
    let mut worker = ClockAdvancingWorker { clock: &clock };
    let elapsed = scheduler.run_frame(&mut worker).expect("frame");
    // The budget is a soft ceiling: one item may overshoot cooperatively.
    assert!(elapsed < 20);
    assert!(scheduler.pending_count() > 0);
    // A second frame finishes the rest.
    scheduler.run_frame(&mut worker).expect("frame");
    assert_eq!(scheduler.pending_count(), 0);
}

#[test]
fn a_yielding_chunk_is_requeued_and_eventually_completes_no_starvation() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 10);
    let map = std::collections::HashMap::new();
    let geometry = geometry_fn(&map);
    scheduler
        .reconcile(&[1, 2, 3], VIEWPORT, Direction::Down, &geometry)
        .expect("reconcile");
    let mut worker = VisitsWorker::new(3); // each chunk needs 3 visits
    let mut frames = 0;
    while scheduler.pending_count() > 0 && frames < 1000 {
        clock.advance(10);
        scheduler.run_frame(&mut worker).expect("frame");
        frames += 1;
    }
    assert!(frames < 100);
    assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Visible), 3);
    // Every chunk was visited; none starved.
    let distinct: std::collections::HashSet<u32> = worker.visits.iter().copied().collect();
    assert_eq!(distinct.len(), 3);
}

#[test]
fn is_deterministic_identical_inputs_yield_identical_visit_sequences() {
    let run = || -> Vec<u32> {
        let clock = FakeClock::new();
        let mut scheduler = fresh(&clock, 10);
        let map: std::collections::HashMap<u32, Rect> = (1..=4)
            .map(|id| {
                (
                    id,
                    Rect {
                        x: 0.0,
                        y: id as f32 * 1000.0,
                        w: 100.0,
                        h: 20.0,
                    },
                )
            })
            .collect();
        let geometry = geometry_fn(&map);
        scheduler
            .reconcile(&[1, 2, 3, 4], VIEWPORT, Direction::Down, &geometry)
            .expect("reconcile");
        let mut worker = VisitsWorker::new(2);
        let mut frames = 0;
        while scheduler.pending_count() > 0 && frames < 100 {
            clock.advance(10);
            scheduler.run_frame(&mut worker).expect("frame");
            frames += 1;
        }
        worker.visits
    };
    assert_eq!(run(), run());
}

#[test]
fn reconcile_culls_chunks_that_left_the_visible_set_and_cancels_materializing_ones() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 1000);
    let no_geom_map = std::collections::HashMap::new();
    let no_geometry = geometry_fn(&no_geom_map);
    scheduler
        .reconcile(&[1, 2, 3], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    let mut worker = VisitsWorker::new(1);
    scheduler.run_frame(&mut worker).expect("frame");
    assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Visible), 3);
    // Chunk 2 leaves the visible set.
    scheduler
        .reconcile(&[1, 3], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    assert_eq!(scheduler.lifecycle().state(2), ChunkState::Cooling);
    // Queued chunk: dequeue path.
    scheduler
        .reconcile(&[1, 2, 3, 4], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    assert_eq!(scheduler.lifecycle().state(4), ChunkState::Queued);
    scheduler
        .reconcile(&[1, 2, 3], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    assert_eq!(scheduler.lifecycle().state(4), ChunkState::Compressed);
    assert!(!scheduler.is_pending(4));
}

#[test]
fn reconcile_requeues_cooling_chunks_that_reenter_the_visible_set() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 1000);
    let no_geom_map = std::collections::HashMap::new();
    let no_geometry = geometry_fn(&no_geom_map);
    scheduler
        .reconcile(&[1], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    let mut worker = VisitsWorker::new(1);
    scheduler.run_frame(&mut worker).expect("frame");
    scheduler
        .reconcile(&[], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    assert_eq!(scheduler.lifecycle().state(1), ChunkState::Cooling);
    scheduler
        .reconcile(&[1], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    // Cooling chunks re-enter at Queued (requeue) when needed again.
    assert!(
        matches!(
            scheduler.lifecycle().state(1),
            ChunkState::Queued | ChunkState::Compressed
        ),
        "state {:?}",
        scheduler.lifecycle().state(1)
    );
}

#[test]
fn tick_expires_cooling_chunks_after_the_period_when_not_selected() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 1000);
    let no_geom_map = std::collections::HashMap::new();
    let no_geometry = geometry_fn(&no_geom_map);
    scheduler
        .reconcile(&[1, 2], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    let mut worker = VisitsWorker::new(1);
    scheduler.run_frame(&mut worker).expect("frame");
    scheduler
        .reconcile(&[], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile"); // both cool
    let mut tick_worker = VisitsWorker::new(1);
    assert_eq!(scheduler.tick(&mut tick_worker).expect("tick"), 0); // period not elapsed
    clock.advance(1000);
    assert_eq!(scheduler.tick(&mut tick_worker).expect("tick"), 2);
    assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Evicted), 2);
}

#[test]
fn tick_releases_resources_through_the_worker_and_respects_selections() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 1000);
    let no_geom_map = std::collections::HashMap::new();
    let no_geometry = geometry_fn(&no_geom_map);
    scheduler
        .reconcile(&[1, 2], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    let mut worker = VisitsWorker::new(1);
    scheduler.run_frame(&mut worker).expect("frame");
    scheduler
        .reconcile(&[], VIEWPORT, Direction::Down, &no_geometry)
        .expect("reconcile");
    scheduler.lifecycle_mut().select(1); // selection pins chunk 1
    clock.advance(2000);
    let mut tick_worker = VisitsWorker::new(1);
    scheduler.tick(&mut tick_worker).expect("tick");
    assert_eq!(tick_worker.released, vec![2]); // only chunk 2 released
    assert_eq!(scheduler.lifecycle().state(1), ChunkState::Cooling);
    scheduler.lifecycle_mut().unselect(1);
    scheduler.tick(&mut tick_worker).expect("tick");
    assert_eq!(scheduler.lifecycle().state(1), ChunkState::Evicted);
}

#[test]
fn evict_for_memory_releases_the_furthest_visible_chunks_first() {
    let clock = FakeClock::new();
    let mut scheduler = fresh(&clock, 1000);
    let map: std::collections::HashMap<u32, Rect> = (1..=4)
        .map(|id| {
            (
                id,
                Rect {
                    x: 0.0,
                    y: id as f32 * 1000.0,
                    w: 100.0,
                    h: 20.0,
                },
            )
        })
        .collect();
    let geometry = geometry_fn(&map);
    scheduler
        .reconcile(&[1, 2, 3, 4], VIEWPORT, Direction::Down, &geometry)
        .expect("reconcile");
    let mut worker = VisitsWorker::new(1);
    scheduler.run_frame(&mut worker).expect("frame");
    assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Visible), 4);
    let mut evict_worker = VisitsWorker::new(1);
    let freed = |id: u32| u64::from(id) * 100;
    // Target 550 bytes: evict chunk 4 (400) then 3 (300) → 700 ≥ 550.
    let evicted = scheduler
        .evict_for_memory(
            &mut evict_worker,
            550,
            &freed,
            VIEWPORT,
            Direction::Down,
            &geometry,
        )
        .expect("evict");
    assert_eq!(evicted, 2);
    assert_eq!(evict_worker.released, vec![4, 3]);
    assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Cooling), 2);
    assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Visible), 2);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 100, ..ProptestConfig::default() })]

    /// Every queued chunk eventually completes in bounded frames under
    /// arbitrary yield requirements (no starvation).
    #[test]
    fn every_queued_chunk_eventually_completes_in_bounded_frames(
        visits_needed in proptest::collection::vec(1_u32..=5, 1..=20),
    ) {
        let clock = FakeClock::new();
        let mut scheduler = fresh(&clock, 7);
        let map: std::collections::HashMap<u32, Rect> = (1..=10)
            .map(|id| {
                (
                    id,
                    Rect {
                        x: 0.0,
                        y: id as f32 * 500.0,
                        w: 100.0,
                        h: 20.0,
                    },
                )
            })
            .collect();
        let geometry = geometry_fn(&map);
        let ids: Vec<u32> = (1..=10).collect();
        scheduler.reconcile(&ids, VIEWPORT, Direction::Down, &geometry).expect("reconcile");

        struct NeedsWorker {
            needs: std::collections::HashMap<u32, u32>,
            visits_needed: Vec<u32>,
        }
        impl MaterializeWorker for NeedsWorker {
            fn work(&mut self, chunk_id: u32, _budget_ms: u64, _elapsed_ms: u64) -> WorkResult {
                let required = self
                    .visits_needed
                    .get(chunk_id as usize % self.visits_needed.len())
                    .copied()
                    .unwrap_or(1);
                let remaining = self.needs.get(&chunk_id).copied().unwrap_or(required).saturating_sub(1);
                self.needs.insert(chunk_id, remaining);
                if remaining == 0 {
                    WorkResult::Complete
                } else {
                    WorkResult::Yield
                }
            }
            fn release(&mut self, _chunk_id: u32) {}
        }
        let mut worker = NeedsWorker {
            needs: std::collections::HashMap::new(),
            visits_needed,
        };
        let mut frames = 0;
        while scheduler.pending_count() > 0 && frames < 10_000 {
            clock.advance(7);
            scheduler.run_frame(&mut worker).expect("frame");
            frames += 1;
        }
        assert_eq!(scheduler.pending_count(), 0);
        assert_eq!(scheduler.lifecycle().count_in_state(ChunkState::Visible), 10);
        assert!(frames < 10_000);
    }
}
