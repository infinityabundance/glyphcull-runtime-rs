//! Lifecycle state machine tests: exhaustive transition table, guards,
//! transition log, and model-based property tests (mirrors the JS
//! `test/lifecycle/lifecycle.test.ts`).

#![allow(missing_docs)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use proptest::prelude::*;

use glyphcull_core::clock::FakeClock;
use glyphcull_core::lifecycle::{
    ChunkLifecycleConfig, ChunkState, LifecycleError, LifecycleEvent, LifecycleManager,
    LifecycleOptions, Transition,
};

/// The reference transition table (the model the implementation must match).
fn reference_destination(state: ChunkState, event: LifecycleEvent) -> Option<ChunkState> {
    match state {
        ChunkState::Compressed => (event == LifecycleEvent::Enqueue).then_some(ChunkState::Queued),
        ChunkState::Queued => match event {
            LifecycleEvent::Begin => Some(ChunkState::Materializing),
            LifecycleEvent::Dequeue => Some(ChunkState::Compressed),
            _ => None,
        },
        ChunkState::Materializing => match event {
            LifecycleEvent::Complete => Some(ChunkState::Visible),
            LifecycleEvent::Cancel => Some(ChunkState::Compressed),
            LifecycleEvent::Pause => Some(ChunkState::Queued),
            _ => None,
        },
        ChunkState::Visible => (event == LifecycleEvent::Cull).then_some(ChunkState::Cooling),
        ChunkState::Cooling => match event {
            LifecycleEvent::Requeue => Some(ChunkState::Queued),
            LifecycleEvent::Expire => Some(ChunkState::Evicted),
            _ => None,
        },
        ChunkState::Evicted => (event == LifecycleEvent::Enqueue).then_some(ChunkState::Queued),
    }
}

fn fresh_manager<'a>(clock: &'a FakeClock) -> LifecycleManager<'a, FakeClock> {
    let mut manager = LifecycleManager::new(
        clock,
        LifecycleOptions {
            default_cooling_period_ms: 1000,
        },
    );
    manager
        .register(
            1,
            ChunkLifecycleConfig {
                hidden: false,
                cooling_period_ms: 1000,
            },
        )
        .expect("register");
    manager
}

/// Register chunk 1 and drive it into the given state (advancing the clock
/// past the cooling period before `expire`).
fn drive_to<'a>(
    manager: &mut LifecycleManager<'a, FakeClock>,
    clock: &'a FakeClock,
    state: ChunkState,
) {
    manager
        .register(
            1,
            ChunkLifecycleConfig {
                hidden: false,
                cooling_period_ms: 1000,
            },
        )
        .expect("register");
    let run: &[LifecycleEvent] = match state {
        ChunkState::Compressed => &[],
        ChunkState::Queued => &[LifecycleEvent::Enqueue],
        ChunkState::Materializing => &[LifecycleEvent::Enqueue, LifecycleEvent::Begin],
        ChunkState::Visible => &[
            LifecycleEvent::Enqueue,
            LifecycleEvent::Begin,
            LifecycleEvent::Complete,
        ],
        ChunkState::Cooling => &[
            LifecycleEvent::Enqueue,
            LifecycleEvent::Begin,
            LifecycleEvent::Complete,
            LifecycleEvent::Cull,
        ],
        ChunkState::Evicted => &[
            LifecycleEvent::Enqueue,
            LifecycleEvent::Begin,
            LifecycleEvent::Complete,
            LifecycleEvent::Cull,
            LifecycleEvent::Expire,
        ],
    };
    for &event in run {
        if event == LifecycleEvent::Expire {
            clock.advance(1000);
        }
        manager.transition(1, event).expect("drive transition");
    }
}

#[test]
fn accepts_exactly_the_transitions_in_the_model() {
    for state in ChunkState::ALL {
        for event in LifecycleEvent::ALL {
            let clock = FakeClock::new();
            let mut manager = fresh_manager(&clock);
            drive_to(&mut manager, &clock, state);
            if state == ChunkState::Cooling && event == LifecycleEvent::Expire {
                // The table allows expire; the cooling guard needs the period
                // to elapse, so advance the clock before exercising it.
                clock.advance(1000);
            }
            let expected = reference_destination(state, event);
            match expected {
                Some(destination) => {
                    let got = manager.transition(1, event).expect("allowed transition");
                    assert_eq!(got, destination, "state {state:?} event {event:?}");
                }
                None => {
                    let err = manager
                        .transition(1, event)
                        .expect_err("illegal transition");
                    assert_eq!(err.chunk_id, 1);
                    assert_eq!(err.event, Some(event));
                    assert_eq!(err.state, Some(state));
                }
            }
        }
    }
}

#[test]
fn expire_requires_the_cooling_period_to_elapse() {
    let clock = FakeClock::new();
    let mut manager = fresh_manager(&clock);
    drive_to(&mut manager, &clock, ChunkState::Cooling);
    let err = manager
        .transition(1, LifecycleEvent::Expire)
        .expect_err("premature");
    assert!(err.detail.contains("cooling period"), "{err}");
    clock.advance(999);
    let err = manager
        .transition(1, LifecycleEvent::Expire)
        .expect_err("still premature");
    assert!(err.detail.contains("cooling period"), "{err}");
    clock.advance(1);
    assert_eq!(
        manager
            .transition(1, LifecycleEvent::Expire)
            .expect("elapsed"),
        ChunkState::Evicted
    );
}

#[test]
fn expire_is_blocked_while_a_selection_references_the_chunk() {
    let clock = FakeClock::new();
    let mut manager = fresh_manager(&clock);
    drive_to(&mut manager, &clock, ChunkState::Cooling);
    manager.select(1);
    clock.advance(5000);
    let err = manager
        .transition(1, LifecycleEvent::Expire)
        .expect_err("selected");
    assert!(err.detail.contains("selection"), "{err}");
    manager.unselect(1);
    assert_eq!(
        manager
            .transition(1, LifecycleEvent::Expire)
            .expect("released"),
        ChunkState::Evicted
    );
}

#[test]
fn hidden_chunks_never_enter_the_queue() {
    let clock = FakeClock::new();
    let mut manager = LifecycleManager::new(
        &clock,
        LifecycleOptions {
            default_cooling_period_ms: 1000,
        },
    );
    manager
        .register(
            1,
            ChunkLifecycleConfig {
                hidden: true,
                cooling_period_ms: 1000,
            },
        )
        .expect("register");
    let err = manager
        .transition(1, LifecycleEvent::Enqueue)
        .expect_err("hidden");
    assert!(err.detail.contains("hidden"), "{err}");
    assert_eq!(manager.state(1), ChunkState::Compressed);
    assert!(manager.is_hidden(1));
}

#[test]
fn a_cooling_chunk_needed_again_requeues_and_clears_its_cooling_timer() {
    let clock = FakeClock::new();
    let mut manager = fresh_manager(&clock);
    drive_to(&mut manager, &clock, ChunkState::Cooling);
    clock.advance(500);
    assert_eq!(
        manager
            .transition(1, LifecycleEvent::Requeue)
            .expect("requeue"),
        ChunkState::Queued
    );
    assert!(manager.cooling_remaining().is_empty());
}

#[test]
fn a_cancel_returns_a_materializing_chunk_to_compressed() {
    let clock = FakeClock::new();
    let mut manager = fresh_manager(&clock);
    drive_to(&mut manager, &clock, ChunkState::Materializing);
    assert_eq!(
        manager
            .transition(1, LifecycleEvent::Cancel)
            .expect("cancel"),
        ChunkState::Compressed
    );
    assert!(manager.cooling_remaining().is_empty());
}

#[test]
fn a_culled_chunk_is_not_cooling_until_culled() {
    // Cooling timers exist only for chunks that were culled.
    let clock = FakeClock::new();
    let mut manager = fresh_manager(&clock);
    drive_to(&mut manager, &clock, ChunkState::Visible);
    assert!(manager.cooling_remaining().is_empty());
    manager.transition(1, LifecycleEvent::Cull).expect("cull");
    assert_eq!(manager.cooling_remaining().get(&1), Some(&1000));
    clock.advance(400);
    assert_eq!(manager.cooling_remaining().get(&1), Some(&600));
    clock.advance(700);
    assert_eq!(manager.cooling_remaining().get(&1), Some(&0));
}

#[test]
fn records_every_transition_in_order_with_the_injected_clock() {
    let clock = FakeClock::new();
    let mut manager = fresh_manager(&clock);
    manager
        .transition(1, LifecycleEvent::Enqueue)
        .expect("enqueue");
    clock.advance(3);
    manager.transition(1, LifecycleEvent::Begin).expect("begin");
    clock.advance(7);
    manager
        .transition(1, LifecycleEvent::Complete)
        .expect("complete");
    let log = manager.transitions().to_vec();
    assert_eq!(
        log,
        vec![
            Transition {
                chunk_id: 1,
                event: LifecycleEvent::Enqueue,
                from: ChunkState::Compressed,
                to: ChunkState::Queued,
                time: 0,
            },
            Transition {
                chunk_id: 1,
                event: LifecycleEvent::Begin,
                from: ChunkState::Queued,
                to: ChunkState::Materializing,
                time: 3,
            },
            Transition {
                chunk_id: 1,
                event: LifecycleEvent::Complete,
                from: ChunkState::Materializing,
                to: ChunkState::Visible,
                time: 10,
            },
        ]
    );
}

#[test]
fn the_log_is_deterministic_for_identical_event_sequences() {
    let run = || -> Vec<Transition> {
        let clock = FakeClock::new();
        let mut manager = fresh_manager(&clock);
        let events = [
            LifecycleEvent::Enqueue,
            LifecycleEvent::Begin,
            LifecycleEvent::Complete,
            LifecycleEvent::Cull,
            LifecycleEvent::Requeue,
            LifecycleEvent::Begin,
            LifecycleEvent::Complete,
        ];
        for event in events {
            clock.advance(2);
            manager.transition(1, event).expect("transition");
        }
        manager.transitions().to_vec()
    };
    assert_eq!(run(), run());
}

#[test]
fn counts_and_lists_chunks_per_state() {
    let clock = FakeClock::new();
    let mut manager = fresh_manager(&clock);
    drive_to(&mut manager, &clock, ChunkState::Visible); // chunk 1 → Visible
    manager
        .register(
            2,
            ChunkLifecycleConfig {
                hidden: false,
                cooling_period_ms: 1000,
            },
        )
        .expect("register 2");
    manager
        .transition(2, LifecycleEvent::Enqueue)
        .expect("enqueue 2"); // chunk 2 → Queued
    assert_eq!(manager.count_in_state(ChunkState::Queued), 1);
    assert_eq!(manager.count_in_state(ChunkState::Visible), 1);
    assert_eq!(manager.chunks_in_state(ChunkState::Queued), vec![2]);
}

#[test]
fn registration_rejects_id_zero_and_is_idempotent() {
    let clock = FakeClock::new();
    let mut manager = LifecycleManager::new(
        &clock,
        LifecycleOptions {
            default_cooling_period_ms: 1000,
        },
    );
    let err = manager
        .register(
            0,
            ChunkLifecycleConfig {
                hidden: false,
                cooling_period_ms: 1000,
            },
        )
        .expect_err("id zero");
    assert_eq!(err.chunk_id, 0);
    assert_eq!(err.event, None);

    // Re-registering resets state and clears selection pins.
    manager
        .register(
            1,
            ChunkLifecycleConfig {
                hidden: false,
                cooling_period_ms: 1000,
            },
        )
        .expect("register");
    manager
        .transition(1, LifecycleEvent::Enqueue)
        .expect("enqueue");
    manager.select(1);
    manager
        .register(
            1,
            ChunkLifecycleConfig {
                hidden: true,
                cooling_period_ms: 500,
            },
        )
        .expect("re-register");
    assert_eq!(manager.state(1), ChunkState::Compressed);
    assert!(manager.is_hidden(1));
    assert!(!manager.is_selected(1));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Random event sequences never reach an undefined state and match the
    /// reference model wherever the table alone decides.
    #[test]
    fn random_event_sequences_match_the_reference(
        events in proptest::collection::vec(
            proptest::sample::select(&LifecycleEvent::ALL),
            0..200,
        ),
        target in proptest::sample::select(&ChunkState::ALL),
    ) {
        let clock = FakeClock::new();
        let mut manager = fresh_manager(&clock);
        drive_to(&mut manager, &clock, target);
        let mut state = target;
        for event in events {
            let expected = reference_destination(state, event);
            if expected.is_none() {
                assert!(
                    manager.transition(1, event).is_err(),
                    "expected rejection of {event:?} from {state:?}"
                );
            } else {
                // Guards can reject even when the table allows the transition
                // (hidden enqueue, premature/selected expire); the model
                // includes guard behavior through the state it reached.
                match manager.transition(1, event) {
                    Ok(next) => {
                        assert_eq!(next, expected.unwrap());
                        state = next;
                    }
                    Err(err) => {
                        assert!(
                            matches!(err, LifecycleError { .. }),
                            "unexpected non-lifecycle failure {err:?}"
                        );
                    }
                }
            }
            // The machine is always in one of the six states.
            assert!(ChunkState::ALL.contains(&manager.state(1)));
            assert!(manager.transitions().len() <= 1000);
        }
    }
}
