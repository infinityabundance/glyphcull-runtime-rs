//! The streaming materialization scheduler (Architecture.md §3.4; mirrors the
//! JS `src/materialize/scheduler.ts`).
//!
//! Chunks enter a deterministic priority queue; work is executed within a
//! per-frame time budget and yields cooperatively ([`WorkResult::Yield`] → the
//! chunk pauses back to Queued and is re-queued with a penalty, so no chunk
//! can starve others). Every state change goes through the lifecycle manager —
//! the scheduler never mutates chunk state directly.
//!
//! Priorities are pure functions of (geometry, viewport, direction of
//! travel): no wall clock affects decisions (the clock only *measures* the
//! frame budget), so behavior is reproducible. Eviction follows
//! LRU-with-age through the lifecycle's Cooling → Evicted path; memory
//! pressure evicts the furthest visible chunks first (never failing).

use std::collections::{BTreeMap, HashSet};

use crate::clock::Clock;
use crate::lifecycle::{ChunkState, LifecycleError, LifecycleEvent, LifecycleManager};
use crate::visibility::{Rect, Viewport};

/// The direction of travel for priority ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Scrolling down (content moves up): below the viewport is ahead.
    Down = 1,
    /// Scrolling up: above the viewport is ahead.
    Up = -1,
}

/// The result of one materialization visit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkResult {
    /// The chunk's materialization is done.
    Complete,
    /// The chunk needs another frame (budget exhausted or cooperatively
    /// yielding).
    Yield,
}

/// The unit of materialization work.
pub trait MaterializeWorker {
    /// Perform work for a chunk within the remaining frame budget.
    fn work(&mut self, chunk_id: u32, budget_ms: u64, elapsed_ms: u64) -> WorkResult;
    /// Release the resources of a chunk being evicted.
    fn release(&mut self, chunk_id: u32);
}

/// Options for the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerOptions {
    /// The default per-frame time budget in milliseconds.
    pub frame_budget_ms: u64,
    /// The priority penalty applied per cooperative yield (anti-starvation).
    pub yield_penalty: u64,
}

/// One queued item.
#[derive(Debug, Clone, Copy)]
struct QueueItem {
    chunk_id: u32,
    /// Deterministic priority: lower runs first.
    key: f64,
    /// Document-order tie-break.
    ordinal: u32,
}

/// `a` sorts before `b`: strictly lower key, then lower ordinal. The `f64`
/// comparison mirrors the JS `!==`/`<` exactly, including its behavior for
/// NaN keys (never less) — keys are finite for realistic documents either way.
fn less(a: &QueueItem, b: &QueueItem) -> bool {
    a.key < b.key || (a.key == b.key && a.ordinal < b.ordinal)
}

/// A deterministic binary min-heap over (key, ordinal) — mirrors the JS
/// `PriorityQueue` operation for operation (sift-up, sift-down, remove).
///
// The heap is a bounded-index data structure: every index below is guarded by
// an explicit `len` comparison before access (the same discipline as the
// reader's cursor), so the direct indexing is provably safe — the documented
// exception to the workspace's indexing policy, scoped to this impl.
#[derive(Debug, Default)]
struct PriorityQueue {
    items: Vec<QueueItem>,
}

#[allow(clippy::indexing_slicing)]
impl PriorityQueue {
    fn size(&self) -> usize {
        self.items.len()
    }

    fn push(&mut self, item: QueueItem) {
        self.items.push(item);
        let mut i = self.items.len() - 1;
        while i > 0 {
            let parent = (i - 1) >> 1;
            if less(&self.items[i], &self.items[parent]) {
                self.items.swap(i, parent);
                i = parent;
            } else {
                break;
            }
        }
    }

    fn pop(&mut self) -> Option<QueueItem> {
        if self.items.is_empty() {
            return None;
        }
        let top = self.items[0];
        if let Some(last) = self.items.pop() {
            if !self.items.is_empty() {
                self.items[0] = last;
                let mut i = 0;
                loop {
                    let left = i * 2 + 1;
                    let right = left + 1;
                    let mut smallest = i;
                    if left < self.items.len() && less(&self.items[left], &self.items[smallest]) {
                        smallest = left;
                    }
                    if right < self.items.len() && less(&self.items[right], &self.items[smallest]) {
                        smallest = right;
                    }
                    if smallest == i {
                        break;
                    }
                    self.items.swap(i, smallest);
                    i = smallest;
                }
            }
        }
        Some(top)
    }

    /// Remove an item by chunk id (used when a queued chunk is culled).
    fn remove(&mut self, chunk_id: u32) {
        let Some(idx) = self.items.iter().position(|item| item.chunk_id == chunk_id) else {
            return;
        };
        if let Some(last) = self.items.pop() {
            if idx < self.items.len() {
                self.items[idx] = last;
                // Restore the heap property from the replaced position (sift
                // up then down, exactly like the JS).
                let mut i = idx;
                while i > 0 {
                    let parent = (i - 1) >> 1;
                    if less(&self.items[i], &self.items[parent]) {
                        self.items.swap(i, parent);
                        i = parent;
                    } else {
                        break;
                    }
                }
                loop {
                    let left = i * 2 + 1;
                    let right = left + 1;
                    let mut smallest = i;
                    if left < self.items.len() && less(&self.items[left], &self.items[smallest]) {
                        smallest = left;
                    }
                    if right < self.items.len() && less(&self.items[right], &self.items[smallest]) {
                        smallest = right;
                    }
                    if smallest == i {
                        break;
                    }
                    self.items.swap(i, smallest);
                    i = smallest;
                }
            }
        }
    }
}

/// The deterministic priority key for a chunk: intersecting chunks first (in
/// document order), then distance tiers (1024 px), with chunks ahead of the
/// direction of travel preferred within a tier, then document order. Chunks
/// without geometry sort by document order (the sequential frontier).
#[must_use]
pub fn priority_key(
    chunk_id: u32,
    rect: Option<Rect>,
    viewport: Viewport,
    direction: Direction,
    ordinal: u32,
) -> f64 {
    let Some(rect) = rect else {
        return f64::from(ordinal);
    };
    let above = f64::from(viewport.y - (rect.y + rect.h));
    let below = f64::from(rect.y - (viewport.y + viewport.h));
    let distance = if above > 0.0 {
        above
    } else if below > 0.0 {
        below
    } else {
        0.0
    };
    if distance == 0.0 {
        // Intersecting the viewport: highest priority, document order.
        return f64::from(ordinal);
    }
    let tier = 1.0 + (distance / 1024.0).floor();
    let ahead = match direction {
        Direction::Down => below > 0.0,
        Direction::Up => above > 0.0,
    };
    let _ = chunk_id; // the ordinal carries document order
    tier * 4_294_967_296.0 + if ahead { 0.0 } else { 2_147_483_648.0 } + f64::from(ordinal)
}

/// The materialization scheduler (mirrors the JS `MaterializationScheduler`).
///
/// Every state change is driven through the owned [`LifecycleManager`]; the
/// clock is borrowed (like the lifecycle) and only *measures* the frame
/// budget — it never influences decisions. The scheduler owns the lifecycle
/// (composition): read it via [`Self::lifecycle`] and mutate selection or
/// registration state via [`Self::lifecycle_mut`].
#[derive(Debug)]
pub struct MaterializationScheduler<'a, C: Clock> {
    clock: &'a C,
    lifecycle: LifecycleManager<'a, C>,
    frame_budget_ms: u64,
    yield_penalty: u64,
    queue: PriorityQueue,
    pending: HashSet<u32>,
    attempts: BTreeMap<u32, u64>,
    last_visible: HashSet<u32>,
}

impl<'a, C: Clock> MaterializationScheduler<'a, C> {
    /// Create a scheduler over a clock and an owned lifecycle manager.
    #[must_use]
    pub fn new(
        clock: &'a C,
        lifecycle: LifecycleManager<'a, C>,
        options: SchedulerOptions,
    ) -> Self {
        Self {
            clock,
            lifecycle,
            frame_budget_ms: options.frame_budget_ms,
            yield_penalty: options.yield_penalty,
            queue: PriorityQueue::default(),
            pending: HashSet::new(),
            attempts: BTreeMap::new(),
            last_visible: HashSet::new(),
        }
    }

    /// Read access to the owned lifecycle manager.
    #[must_use]
    pub fn lifecycle(&self) -> &LifecycleManager<'a, C> {
        &self.lifecycle
    }

    /// Mutable access to the owned lifecycle manager (selection pins,
    /// registration, eviction bookkeeping).
    pub fn lifecycle_mut(&mut self) -> &mut LifecycleManager<'a, C> {
        &mut self.lifecycle
    }

    /// Whether a chunk is currently queued.
    #[must_use]
    pub fn is_pending(&self, chunk_id: u32) -> bool {
        self.pending.contains(&chunk_id)
    }

    /// The number of queued chunks.
    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.queue.size()
    }

    /// Reconcile the visible set: enqueue newly visible chunks (priority from
    /// geometry, viewport, and direction of travel) and cull/dequeue chunks
    /// that left the visible set.
    pub fn reconcile(
        &mut self,
        visible: &[u32],
        viewport: Viewport,
        direction: Direction,
        geometry: &dyn Fn(u32) -> Option<Rect>,
    ) -> Result<(), LifecycleError> {
        let now_visible: HashSet<u32> = visible.iter().copied().collect();
        for &id in visible {
            let state = self.lifecycle.state(id);
            if state == ChunkState::Compressed || state == ChunkState::Evicted {
                self.lifecycle.transition(id, LifecycleEvent::Enqueue)?;
            } else if state == ChunkState::Cooling {
                // A cooling chunk needed again re-enters the queue
                // immediately.
                self.lifecycle.transition(id, LifecycleEvent::Requeue)?;
            }
            if self.lifecycle.state(id) == ChunkState::Queued && !self.pending.contains(&id) {
                self.enqueue_with_priority(id, viewport, direction, geometry);
            }
        }
        let last: Vec<u32> = self.last_visible.iter().copied().collect();
        for id in last {
            if now_visible.contains(&id) {
                continue;
            }
            match self.lifecycle.state(id) {
                ChunkState::Queued => {
                    self.lifecycle.transition(id, LifecycleEvent::Dequeue)?;
                    self.queue.remove(id);
                    self.pending.remove(&id);
                    self.attempts.remove(&id);
                }
                ChunkState::Materializing => {
                    self.lifecycle.transition(id, LifecycleEvent::Cancel)?;
                }
                ChunkState::Visible => {
                    self.lifecycle.transition(id, LifecycleEvent::Cull)?;
                }
                _ => {}
            }
        }
        self.last_visible = now_visible;
        Ok(())
    }

    fn enqueue_with_priority(
        &mut self,
        chunk_id: u32,
        viewport: Viewport,
        direction: Direction,
        geometry: &dyn Fn(u32) -> Option<Rect>,
    ) {
        let ordinal = chunk_id - 1; // ids are dense in document order
        let key = priority_key(chunk_id, geometry(chunk_id), viewport, direction, ordinal);
        self.queue.push(QueueItem {
            chunk_id,
            key,
            ordinal,
        });
        self.pending.insert(chunk_id);
    }

    /// Run one frame of materialization work within the time budget. Returns
    /// the elapsed milliseconds (measured, never affecting decisions).
    pub fn run_frame(&mut self, worker: &mut dyn MaterializeWorker) -> Result<u64, LifecycleError> {
        let start = self.clock.now();
        let budget = self.frame_budget_ms;
        while self.queue.size() > 0 {
            let elapsed = self.clock.now() - start;
            if elapsed >= budget {
                break;
            }
            let Some(item) = self.queue.pop() else {
                break;
            };
            self.pending.remove(&item.chunk_id);
            if self.lifecycle.state(item.chunk_id) != ChunkState::Queued {
                // Culled or otherwise moved while queued; nothing to do.
                self.attempts.remove(&item.chunk_id);
                continue;
            }
            self.lifecycle
                .transition(item.chunk_id, LifecycleEvent::Begin)?;
            let remaining = budget.saturating_sub(elapsed);
            let result = worker.work(item.chunk_id, remaining, elapsed);
            if result == WorkResult::Complete {
                self.lifecycle
                    .transition(item.chunk_id, LifecycleEvent::Complete)?;
                self.attempts.remove(&item.chunk_id);
            } else {
                // Cooperative yield: pause back to Queued and re-queue behind
                // a penalty so it cannot starve other chunks.
                self.lifecycle
                    .transition(item.chunk_id, LifecycleEvent::Pause)?;
                let attempts = self.attempts.get(&item.chunk_id).copied().unwrap_or(0) + 1;
                self.attempts.insert(item.chunk_id, attempts);
                self.queue.push(QueueItem {
                    chunk_id: item.chunk_id,
                    key: item.key + attempts as f64 * self.yield_penalty as f64,
                    ordinal: item.ordinal,
                });
                self.pending.insert(item.chunk_id);
            }
        }
        Ok(self.clock.now() - start)
    }

    /// Evict expired cooling chunks: releases resources through the worker,
    /// then transitions Cooling → Evicted. Returns the number evicted.
    pub fn tick(&mut self, worker: &mut dyn MaterializeWorker) -> Result<usize, LifecycleError> {
        let mut evicted = 0;
        let mut ready: Vec<u32> = Vec::new();
        for (chunk_id, remaining) in self.lifecycle.cooling_remaining() {
            if remaining == 0 && !self.lifecycle.is_selected(chunk_id) {
                ready.push(chunk_id);
            }
        }
        ready.sort_unstable();
        for chunk_id in ready {
            worker.release(chunk_id);
            self.lifecycle
                .transition(chunk_id, LifecycleEvent::Expire)?;
            evicted += 1;
        }
        Ok(evicted)
    }

    /// Evict visible chunks for memory pressure: release the furthest-from-
    /// viewport visible chunks first (deterministic), transitioning them to
    /// Cooling. `freed` reports how many bytes each chunk's release frees.
    /// Returns the number of chunks evicted.
    pub fn evict_for_memory(
        &mut self,
        worker: &mut dyn MaterializeWorker,
        target_bytes: u64,
        freed: &dyn Fn(u32) -> u64,
        viewport: Viewport,
        direction: Direction,
        geometry: &dyn Fn(u32) -> Option<Rect>,
    ) -> Result<usize, LifecycleError> {
        let mut freed_so_far = 0_u64;
        let mut evicted = 0;
        let mut visible_chunks: Vec<(u32, f64)> = self
            .lifecycle
            .chunks_in_state(ChunkState::Visible)
            .into_iter()
            .map(|id| {
                let key = priority_key(id, geometry(id), viewport, direction, id - 1);
                (id, key)
            })
            .collect();
        // Furthest first: highest key, then highest id (deterministic).
        visible_chunks.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.0.cmp(&a.0))
        });
        for (id, _) in visible_chunks {
            if freed_so_far >= target_bytes {
                break;
            }
            worker.release(id);
            self.lifecycle.transition(id, LifecycleEvent::Cull)?;
            freed_so_far = freed_so_far.saturating_add(freed(id));
            evicted += 1;
        }
        Ok(evicted)
    }
}
