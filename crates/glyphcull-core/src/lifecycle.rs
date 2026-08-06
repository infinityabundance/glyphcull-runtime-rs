//! The chunk lifecycle state machine (Architecture.md §6; mirrors the JS
//! `src/lifecycle/lifecycle.ts`).
//!
//! Every chunk moves through deterministic states:
//!
//! ```text
//! Compressed ──(enqueue)──▶ Queued ──(begin)──▶ Materializing ──(complete)──▶ Visible
//!     ▲                        │  ▲                 │                            │
//!     │                        │  └──(budget exhausted: stays Queued)           │
//!     │                        │ (dequeue)          │ (cancel)                  │
//!     │                        ▼                    ▼                            ▼
//!     └───────────(requeue)── Cooling ◀──────────(cull: left visible set)───────┘
//!                                │
//!                                │ (expire: cooling elapsed, no selection)
//!                                ▼
//!                            Evicted ──(resources released)──▶ (re-enters at Queued on enqueue)
//! ```
//!
//! Every transition is explicit, guarded, and recorded in a transition log
//! (deterministic under an injected clock). Hidden chunks never enter the
//! queue; a cooling chunk with an active selection cannot be evicted. The
//! machine never panics: every violation is a typed [`LifecycleError`].

use std::collections::BTreeMap;
use std::fmt;

use crate::clock::Clock;

/// The six lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ChunkState {
    /// The chunk's compressed payload exists; no runtime resources.
    Compressed = 0,
    /// Waiting in the materialization queue.
    Queued = 1,
    /// The chunk's resources are being produced.
    Materializing = 2,
    /// The chunk is rendered and resident.
    Visible = 3,
    /// Left the visible set; waiting out its cooling period.
    Cooling = 4,
    /// Resources released; can re-enter at `Queued` on enqueue.
    Evicted = 5,
}

impl ChunkState {
    /// All six states, in code order (tests and enumeration).
    pub const ALL: [ChunkState; 6] = [
        ChunkState::Compressed,
        ChunkState::Queued,
        ChunkState::Materializing,
        ChunkState::Visible,
        ChunkState::Cooling,
        ChunkState::Evicted,
    ];
}

impl fmt::Display for ChunkState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ChunkState::Compressed => "Compressed",
            ChunkState::Queued => "Queued",
            ChunkState::Materializing => "Materializing",
            ChunkState::Visible => "Visible",
            ChunkState::Cooling => "Cooling",
            ChunkState::Evicted => "Evicted",
        })
    }
}

/// The lifecycle events that drive transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LifecycleEvent {
    /// Compressed or Evicted → Queued.
    Enqueue,
    /// Queued → Materializing.
    Begin,
    /// Queued → Compressed (priority changed before work started).
    Dequeue,
    /// Materializing → Visible.
    Complete,
    /// Materializing → Compressed (resources released).
    Cancel,
    /// Materializing → Queued (budget exhausted mid-work).
    Pause,
    /// Visible → Cooling (left the visible set).
    Cull,
    /// Cooling → Queued (needed again).
    Requeue,
    /// Cooling → Evicted (cooling elapsed, no selection).
    Expire,
}

impl LifecycleEvent {
    /// All nine events (tests and enumeration).
    pub const ALL: [LifecycleEvent; 9] = [
        LifecycleEvent::Enqueue,
        LifecycleEvent::Begin,
        LifecycleEvent::Dequeue,
        LifecycleEvent::Complete,
        LifecycleEvent::Cancel,
        LifecycleEvent::Pause,
        LifecycleEvent::Cull,
        LifecycleEvent::Requeue,
        LifecycleEvent::Expire,
    ];
}

impl fmt::Display for LifecycleEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            LifecycleEvent::Enqueue => "enqueue",
            LifecycleEvent::Begin => "begin",
            LifecycleEvent::Dequeue => "dequeue",
            LifecycleEvent::Complete => "complete",
            LifecycleEvent::Cancel => "cancel",
            LifecycleEvent::Pause => "pause",
            LifecycleEvent::Cull => "cull",
            LifecycleEvent::Requeue => "requeue",
            LifecycleEvent::Expire => "expire",
        })
    }
}

/// One recorded transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    /// The chunk that transitioned.
    pub chunk_id: u32,
    /// The event that drove the transition.
    pub event: LifecycleEvent,
    /// The state before the transition.
    pub from: ChunkState,
    /// The state after the transition.
    pub to: ChunkState,
    /// `Clock::now()` at the transition (deterministic under an injected
    /// clock).
    pub time: u64,
}

/// A lifecycle violation (guards / illegal transitions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleError {
    /// The chunk the violation is scoped to.
    pub chunk_id: u32,
    /// The attempted event (`None` for registration failures).
    pub event: Option<LifecycleEvent>,
    /// The state at the attempt (`None` for registration failures).
    pub state: Option<ChunkState>,
    /// A precise human-readable detail.
    pub detail: String,
}

impl LifecycleError {
    /// A transition violation.
    #[must_use]
    pub fn transition(
        chunk_id: u32,
        event: LifecycleEvent,
        state: ChunkState,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            chunk_id,
            event: Some(event),
            state: Some(state),
            detail: detail.into(),
        }
    }

    /// A registration violation (invalid chunk id).
    #[must_use]
    pub fn registration(chunk_id: u32, detail: impl Into<String>) -> Self {
        Self {
            chunk_id,
            event: None,
            state: None,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.event, self.state) {
            (Some(event), Some(state)) => write!(
                f,
                "lifecycle: chunk {} cannot {event} from {state}: {}",
                self.chunk_id, self.detail
            ),
            _ => write!(
                f,
                "lifecycle: chunk {} cannot register: {}",
                self.chunk_id, self.detail
            ),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// The allowed events for each state (SPEC §4.3; mirrors the JS `TRANSITIONS`).
fn allowed_events(state: ChunkState) -> &'static [LifecycleEvent] {
    match state {
        ChunkState::Compressed => &[LifecycleEvent::Enqueue],
        ChunkState::Queued => &[LifecycleEvent::Begin, LifecycleEvent::Dequeue],
        ChunkState::Materializing => &[
            LifecycleEvent::Complete,
            LifecycleEvent::Cancel,
            LifecycleEvent::Pause,
        ],
        ChunkState::Visible => &[LifecycleEvent::Cull],
        ChunkState::Cooling => &[LifecycleEvent::Requeue, LifecycleEvent::Expire],
        ChunkState::Evicted => &[LifecycleEvent::Enqueue],
    }
}

/// The destination state for each allowed transition (mirrors the JS
/// `destination`; exhaustive by construction).
fn destination(event: LifecycleEvent) -> ChunkState {
    match event {
        LifecycleEvent::Enqueue => ChunkState::Queued,
        LifecycleEvent::Begin => ChunkState::Materializing,
        LifecycleEvent::Dequeue => ChunkState::Compressed,
        LifecycleEvent::Complete => ChunkState::Visible,
        LifecycleEvent::Cancel => ChunkState::Compressed,
        LifecycleEvent::Pause => ChunkState::Queued,
        LifecycleEvent::Cull => ChunkState::Cooling,
        LifecycleEvent::Requeue => ChunkState::Queued,
        LifecycleEvent::Expire => ChunkState::Evicted,
    }
}

/// Per-chunk lifecycle configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkLifecycleConfig {
    /// Chunks excluded by semantic culling never enter the queue.
    pub hidden: bool,
    /// The cooling period (ms) before an evicted chunk can be released.
    pub cooling_period_ms: u64,
}

/// Options for the lifecycle manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleOptions {
    /// Default cooling period for chunks without explicit configuration.
    pub default_cooling_period_ms: u64,
}

/// The lifecycle manager: owns every chunk's state, guards every transition,
/// and records the transition log (mirrors the JS `LifecycleManager`).
///
/// Chunks register once at load with their hidden flag; selection references
/// pin cooling chunks against eviction. The clock is borrowed (the JS passes
/// it by reference) — tests share a [`FakeClock`] between the manager and the
/// test body. All per-chunk tables are ordered by chunk id (`BTreeMap`), so
/// enumeration is deterministic.
#[derive(Debug)]
pub struct LifecycleManager<'a, C: Clock> {
    clock: &'a C,
    default_cooling_period_ms: u64,
    states: BTreeMap<u32, ChunkState>,
    hidden: BTreeMap<u32, bool>,
    cooling_period: BTreeMap<u32, u64>,
    cooling_started_at: BTreeMap<u32, u64>,
    selection_refs: BTreeMap<u32, u32>,
    log: Vec<Transition>,
}

impl<'a, C: Clock> LifecycleManager<'a, C> {
    /// Create a manager over the given clock.
    #[must_use]
    pub const fn new(clock: &'a C, options: LifecycleOptions) -> Self {
        Self {
            clock,
            default_cooling_period_ms: options.default_cooling_period_ms,
            states: BTreeMap::new(),
            hidden: BTreeMap::new(),
            cooling_period: BTreeMap::new(),
            cooling_started_at: BTreeMap::new(),
            selection_refs: BTreeMap::new(),
            log: Vec::new(),
        }
    }

    /// Register a chunk (idempotent); resets its state to `Compressed`.
    pub fn register(
        &mut self,
        chunk_id: u32,
        config: ChunkLifecycleConfig,
    ) -> Result<(), LifecycleError> {
        if chunk_id < 1 {
            return Err(LifecycleError::registration(
                chunk_id,
                "chunk id must be a positive integer",
            ));
        }
        self.states.insert(chunk_id, ChunkState::Compressed);
        self.hidden.insert(chunk_id, config.hidden);
        self.cooling_period
            .insert(chunk_id, config.cooling_period_ms);
        self.cooling_started_at.remove(&chunk_id);
        self.selection_refs.remove(&chunk_id);
        Ok(())
    }

    /// Whether a chunk is registered.
    #[must_use]
    pub fn has(&self, chunk_id: u32) -> bool {
        self.states.contains_key(&chunk_id)
    }

    /// The current state of a registered chunk (`Compressed` when
    /// unregistered).
    #[must_use]
    pub fn state(&self, chunk_id: u32) -> ChunkState {
        self.states
            .get(&chunk_id)
            .copied()
            .unwrap_or(ChunkState::Compressed)
    }

    /// Whether the chunk is excluded by semantic culling.
    #[must_use]
    pub fn is_hidden(&self, chunk_id: u32) -> bool {
        self.hidden.get(&chunk_id).copied().unwrap_or(false)
    }

    /// Pin a chunk against eviction (selection). Idempotent.
    pub fn select(&mut self, chunk_id: u32) {
        let current = self.selection_refs.get(&chunk_id).copied().unwrap_or(0);
        self.selection_refs.insert(chunk_id, current + 1);
    }

    /// Release a selection pin.
    pub fn unselect(&mut self, chunk_id: u32) {
        let current = self.selection_refs.get(&chunk_id).copied().unwrap_or(0);
        if current <= 1 {
            self.selection_refs.remove(&chunk_id);
        } else {
            self.selection_refs.insert(chunk_id, current - 1);
        }
    }

    /// Whether the chunk is referenced by an active selection.
    #[must_use]
    pub fn is_selected(&self, chunk_id: u32) -> bool {
        self.selection_refs
            .get(&chunk_id)
            .is_some_and(|&count| count > 0)
    }

    /// The recorded transition log, in order.
    #[must_use]
    pub fn transitions(&self) -> &[Transition] {
        &self.log
    }

    /// The number of chunks in a given state.
    #[must_use]
    pub fn count_in_state(&self, state: ChunkState) -> usize {
        self.states.values().filter(|&&s| s == state).count()
    }

    /// All chunk ids in a given state, in ascending id order.
    #[must_use]
    pub fn chunks_in_state(&self, state: ChunkState) -> Vec<u32> {
        self.states
            .iter()
            .filter(|(_, &s)| s == state)
            .map(|(&id, _)| id)
            .collect()
    }

    /// The chunk ids currently cooling, with the time remaining (ms).
    ///
    /// A clock that runs backwards is treated as zero elapsed time (the
    /// cooling period starts over), which is strictly safer for eviction.
    #[must_use]
    pub fn cooling_remaining(&self) -> BTreeMap<u32, u64> {
        let now = self.clock.now();
        self.cooling_started_at
            .iter()
            .map(|(&id, &started)| {
                let period = self
                    .cooling_period
                    .get(&id)
                    .copied()
                    .unwrap_or(self.default_cooling_period_ms);
                (id, period.saturating_sub(now.saturating_sub(started)))
            })
            .collect()
    }

    /// Apply an event. Returns the destination state on success, or a typed
    /// [`LifecycleError`] when the transition is illegal or a guard fails.
    pub fn transition(
        &mut self,
        chunk_id: u32,
        event: LifecycleEvent,
    ) -> Result<ChunkState, LifecycleError> {
        let state = self.state(chunk_id);
        let allowed = allowed_events(state);
        if !allowed.contains(&event) {
            let list = allowed
                .iter()
                .map(LifecycleEvent::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(LifecycleError::transition(
                chunk_id,
                event,
                state,
                format!("transition not in {{{list}}}"),
            ));
        }
        match event {
            LifecycleEvent::Enqueue => {
                if self.is_hidden(chunk_id) {
                    return Err(LifecycleError::transition(
                        chunk_id,
                        event,
                        state,
                        "hidden chunks never enter the queue",
                    ));
                }
            }
            LifecycleEvent::Expire => {
                let Some(started) = self.cooling_started_at.get(&chunk_id).copied() else {
                    return Err(LifecycleError::transition(
                        chunk_id,
                        event,
                        state,
                        "no cooling start recorded",
                    ));
                };
                let period = self
                    .cooling_period
                    .get(&chunk_id)
                    .copied()
                    .unwrap_or(self.default_cooling_period_ms);
                if self.clock.now().saturating_sub(started) < period {
                    return Err(LifecycleError::transition(
                        chunk_id,
                        event,
                        state,
                        "cooling period has not elapsed",
                    ));
                }
                if self.is_selected(chunk_id) {
                    return Err(LifecycleError::transition(
                        chunk_id,
                        event,
                        state,
                        "chunk is referenced by a selection",
                    ));
                }
            }
            LifecycleEvent::Cull => {
                self.cooling_started_at.insert(chunk_id, self.clock.now());
            }
            LifecycleEvent::Begin
            | LifecycleEvent::Dequeue
            | LifecycleEvent::Complete
            | LifecycleEvent::Cancel
            | LifecycleEvent::Pause
            | LifecycleEvent::Requeue => {}
        }
        let to = destination(event);
        self.states.insert(chunk_id, to);
        self.log.push(Transition {
            chunk_id,
            event,
            from: state,
            to,
            time: self.clock.now(),
        });
        if matches!(
            event,
            LifecycleEvent::Requeue
                | LifecycleEvent::Cancel
                | LifecycleEvent::Dequeue
                | LifecycleEvent::Expire
        ) {
            self.cooling_started_at.remove(&chunk_id);
        }
        if to == ChunkState::Queued {
            self.cooling_started_at.remove(&chunk_id);
        }
        Ok(to)
    }
}
