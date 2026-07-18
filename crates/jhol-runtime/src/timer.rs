//! Shared timer queue used by both the QuickJS and JavaScriptCore backends.
//!
//! JS callbacks push timer entries into a process-global queue; the active
//! runtime drains ready entries during its event loop and evaluates the stored
//! JS callback string in the current context.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static TIMER_QUEUE: OnceLock<Mutex<Vec<TimerEntry>>> = OnceLock::new();
static CANCELLED_IDS: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();
static NEXT_TIMER_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Clone)]
pub struct TimerEntry {
    pub id: u32,
    pub deadline: Instant,
    pub callback: String,
    pub repeat: Option<Duration>,
}

impl PartialEq for TimerEntry {
    fn eq(&self, other: &Self) -> bool {
        self.deadline == other.deadline && self.id == other.id
    }
}
impl Eq for TimerEntry {}
impl PartialOrd for TimerEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for TimerEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Min-heap: earliest deadline first, then smallest id.
        other.deadline.cmp(&self.deadline).then_with(|| other.id.cmp(&self.id))
    }
}

/// Ingest timers scheduled by JS callbacks into a local `TimerState`.
pub fn ingest_into(state: &mut TimerState) {
    if let Some(queue) = TIMER_QUEUE.get() {
        if let Ok(mut q) = queue.lock() {
            for entry in q.drain(..) {
                state.push(entry);
            }
        }
    }
    if let Some(cancelled) = CANCELLED_IDS.get() {
        if let Ok(mut c) = cancelled.lock() {
            for id in c.drain() {
                state.cancel(id);
            }
        }
    }
}

/// Schedule a one-shot timer.  Returns the timer id.
pub fn set_timeout(callback: String, ms: u32) -> u32 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed) + 1;
    if let Some(queue) = TIMER_QUEUE.get() {
        if let Ok(mut q) = queue.lock() {
            q.push(TimerEntry {
                id,
                deadline: Instant::now() + Duration::from_millis(ms as u64),
                callback,
                repeat: None,
            });
        }
    }
    id
}

/// Schedule a repeating timer.  Returns the timer id.
pub fn set_interval(callback: String, ms: u32) -> u32 {
    let id = NEXT_TIMER_ID.fetch_add(1, Ordering::Relaxed) + 1;
    let interval = Duration::from_millis(ms as u64);
    if let Some(queue) = TIMER_QUEUE.get() {
        if let Ok(mut q) = queue.lock() {
            q.push(TimerEntry {
                id,
                deadline: Instant::now() + interval,
                callback,
                repeat: Some(interval),
            });
        }
    }
    id
}

/// Cancel a timer by id.
pub fn clear_timer(id: u32) {
    if let Some(queue) = TIMER_QUEUE.get() {
        if let Ok(mut q) = queue.lock() {
            q.retain(|e| e.id != id);
        }
    }
    if let Some(cancelled) = CANCELLED_IDS.get() {
        if let Ok(mut c) = cancelled.lock() {
            c.insert(id);
        }
    }
}

/// Initialize the global timer queues.  Safe to call multiple times.
pub fn init() {
    let _ = TIMER_QUEUE.set(Mutex::new(Vec::new()));
    let _ = CANCELLED_IDS.set(Mutex::new(HashSet::new()));
}

/// Local event-loop timer state.
pub struct TimerState {
    timers: BinaryHeap<Reverse<TimerEntry>>,
    cancelled: HashSet<u32>,
}

impl Default for TimerState {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerState {
    pub fn new() -> Self {
        Self {
            timers: BinaryHeap::new(),
            cancelled: HashSet::new(),
        }
    }

    pub fn push(&mut self, entry: TimerEntry) {
        self.timers.push(Reverse(entry));
    }

    pub fn cancel(&mut self, id: u32) {
        self.cancelled.insert(id);
    }

    pub fn drain_ready(&mut self, now: Instant) -> Vec<TimerEntry> {
        let mut ready = Vec::new();
        while let Some(Reverse(entry)) = self.timers.peek() {
            if entry.deadline > now {
                break;
            }
            let entry = self.timers.pop().unwrap().0;
            if self.cancelled.remove(&entry.id) {
                continue;
            }
            if let Some(interval) = entry.repeat {
                self.timers.push(Reverse(TimerEntry {
                    id: entry.id,
                    deadline: now + interval,
                    callback: entry.callback.clone(),
                    repeat: Some(interval),
                }));
            }
            ready.push(entry);
        }
        ready
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.timers.peek().map(|Reverse(e)| e.deadline)
    }
}
