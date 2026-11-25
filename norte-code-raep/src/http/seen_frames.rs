//! A bounded replay guard for one event stream.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;

const DEFAULT_CAPACITY: usize = 2048;

#[derive(Debug)]
pub(super) struct SeenFrames<T> {
    ids: HashSet<T>,
    order: VecDeque<T>,
    capacity: usize,
}

impl<T> SeenFrames<T>
where
    T: Clone + Eq + Hash,
{
    pub(super) fn from_initial(initial: impl IntoIterator<Item = T>) -> Self {
        let mut seen = Self::with_capacity(DEFAULT_CAPACITY);
        for id in initial {
            seen.insert(id);
        }
        seen
    }

    fn with_capacity(capacity: usize) -> Self {
        Self {
            ids: HashSet::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    pub(super) fn insert(&mut self, id: T) -> bool {
        if !self.ids.insert(id.clone()) {
            return false;
        }
        self.order.push_back(id);
        while self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.ids.remove(&oldest);
        }
        true
    }
}

#[cfg(test)]
#[path = "seen_frames_tests.rs"]
mod tests;
