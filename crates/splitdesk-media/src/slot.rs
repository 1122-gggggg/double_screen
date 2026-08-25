use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// Capacity-1 slot. A new push replaces the occupant (latest-wins, drop-old).
pub struct BoundedSlot<T> {
    slot: Mutex<Option<T>>,
    dropped: AtomicU64,
}

impl<T> BoundedSlot<T> {
    pub const CAPACITY: usize = 1;

    pub fn new() -> Self {
        Self {
            slot: Mutex::new(None),
            dropped: AtomicU64::new(0),
        }
    }

    /// Inserts `item`. Returns the previous value when it was dropped.
    pub fn push(&self, item: T) -> Option<T> {
        let previous = self.slot.lock().replace(item);
        if previous.is_some() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        previous
    }

    pub fn take(&self) -> Option<T> {
        self.slot.lock().take()
    }

    pub fn peek_dropped(&self) -> u64 {
        self.dropped()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.slot.lock().is_none()
    }

    pub fn clear(&self) {
        *self.slot.lock() = None;
    }
}

impl<T> Default for BoundedSlot<T> {
    fn default() -> Self {
        Self::new()
    }
}
