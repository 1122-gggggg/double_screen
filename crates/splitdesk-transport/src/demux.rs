use crate::unavailable;
use parking_lot::Mutex;
use splitdesk_core::Error;
use splitdesk_media::{BoundedSlot, InputEvent};
use std::collections::VecDeque;

const RELIABLE_CAP: usize = 128;

/// Splits input: motion/scroll occupy a depth-1 slot; keys/buttons queue reliably.
pub struct InputDemux {
    latest: BoundedSlot<InputEvent>,
    reliable: Mutex<VecDeque<InputEvent>>,
}

impl InputDemux {
    pub fn new() -> Self {
        Self {
            latest: BoundedSlot::new(),
            reliable: Mutex::new(VecDeque::with_capacity(RELIABLE_CAP)),
        }
    }

    pub fn push(&self, event: InputEvent) -> Result<Option<InputEvent>, Error> {
        if event.is_latest_wins() {
            return Ok(self.latest.push(event));
        }
        let mut q = self.reliable.lock();
        if q.len() >= RELIABLE_CAP {
            return Err(unavailable(
                "reliable input queue full; refusing to drop keys/buttons",
            ));
        }
        q.push_back(event);
        Ok(None)
    }

    pub fn take_latest(&self) -> Option<InputEvent> {
        self.latest.take()
    }

    pub fn pop_reliable(&self) -> Option<InputEvent> {
        self.reliable.lock().pop_front()
    }

    /// Reliable events first so keys are not starved by motion.
    pub fn pop(&self) -> Option<InputEvent> {
        if let Some(event) = self.pop_reliable() {
            return Some(event);
        }
        self.take_latest()
    }

    pub fn dropped_latest(&self) -> u64 {
        self.latest.dropped()
    }

    pub fn clear(&self) {
        self.latest.clear();
        self.reliable.lock().clear();
    }
}

impl Default for InputDemux {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) struct ReliableQueue<T> {
    inner: Mutex<VecDeque<T>>,
    cap: usize,
}

impl<T> ReliableQueue<T> {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(cap)),
            cap,
        }
    }

    pub(crate) fn push(&self, item: T) -> Result<(), Error> {
        let mut q = self.inner.lock();
        if q.len() >= self.cap {
            return Err(unavailable(
                "reliable control queue full; refusing unbounded growth",
            ));
        }
        q.push_back(item);
        Ok(())
    }

    pub(crate) fn pop(&self) -> Option<T> {
        self.inner.lock().pop_front()
    }

    pub(crate) fn clear(&self) {
        self.inner.lock().clear();
    }
}
