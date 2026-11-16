//! The host's book of turns.
//!
//! One place owns what is running, what is queued and what the client called
//! each turn. Before this, the same facts lived in seven maps of [`HostState`]
//! updated by hand from three files, which is how a released reservation, a
//! stale alias and a drained queue could disagree.
//!
//! The invariants are enforced here:
//!
//! - a session has at most one running turn, either reserved or active;
//! - a queued turn is only queued behind a running one;
//! - releasing a turn clears its reservation, its pending id and its pending
//!   output together;
//! - an alias exists only while its turn is running.

use std::collections::{HashMap, VecDeque};

use raep_types::{Modality, TurnId};

use super::stores::QueuedTurn;

/// Whether a turn is running and how far it got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Accepted and sent to the service, before its first event arrived.
    Reserved,
    /// The service reported it started.
    Active,
}

#[derive(Clone, Debug)]
struct RunningTurn {
    id: TurnId,
    phase: Phase,
}

/// What the host knows about the turns of every session.
#[derive(Default)]
pub(super) struct TurnBook {
    running: HashMap<String, RunningTurn>,
    queue: HashMap<String, VecDeque<QueuedTurn>>,
    pending: HashMap<String, VecDeque<TurnId>>,
    pending_output: HashMap<String, Vec<Modality>>,
    turn_output: HashMap<(String, u64), Vec<Modality>>,
    aliases: HashMap<(String, u64), TurnId>,
    alias_order: VecDeque<(String, u64)>,
}

impl TurnBook {
    /// Whether a turn is running (reserved or active) for a session.
    pub(super) fn is_occupied(&self, session: &str) -> bool {
        self.running.contains_key(session)
    }

    /// The active turn of a session, if the service reported it started.
    pub(super) fn active(&self, session: &str) -> Option<TurnId> {
        self.running
            .get(session)
            .filter(|turn| turn.phase == Phase::Active)
            .map(|turn| turn.id.clone())
    }

    /// The reserved turn of a session, if no event has arrived for it yet.
    pub(super) fn reserved(&self, session: &str) -> Option<TurnId> {
        self.running
            .get(session)
            .filter(|turn| turn.phase == Phase::Reserved)
            .map(|turn| turn.id.clone())
    }

    /// Whether this exact turn is active.
    pub(super) fn is_active(&self, session: &str, turn: &TurnId) -> bool {
        self.active(session).as_ref() == Some(turn)
    }

    /// Records a turn the host accepted but has not seen start.
    pub(super) fn reserve(&mut self, session: &str, turn: TurnId) {
        let previous = self.running.insert(
            session.to_owned(),
            RunningTurn {
                id: turn,
                phase: Phase::Reserved,
            },
        );
        debug_assert!(
            previous.is_none(),
            "a session must not reserve a second turn while one is running"
        );
    }

    /// Marks the turn as started, keeping its id.
    pub(super) fn activate(&mut self, session: &str, turn: TurnId) {
        self.running.insert(
            session.to_owned(),
            RunningTurn {
                id: turn,
                phase: Phase::Active,
            },
        );
    }

    /// Forgets the running turn of a session, whatever its phase.
    pub(super) fn clear_running(&mut self, session: &str) {
        self.running.remove(session);
    }

    /// Releases `turn` if it is the one running for `session`.
    ///
    /// A stale turn id must not clear a turn someone else is running, which
    /// is why this checks the id and [`Self::clear_running`] exists for the
    /// paths that know the session is over.
    pub(super) fn release(&mut self, session: &str, turn: &TurnId) {
        if self
            .running
            .get(session)
            .is_some_and(|running| &running.id == turn)
        {
            self.running.remove(session);
        }
        self.pending_retain(session, turn);
        self.pending_output.remove(session);
    }

    /// How many inputs wait behind the running turn.
    pub(super) fn queue_len(&self, session: &str) -> usize {
        self.queue.get(session).map_or(0, VecDeque::len)
    }

    /// Queues an input, at the front when it replaces the running turn.
    ///
    /// Returns its position, counting from one.
    pub(super) fn enqueue(&mut self, session: &str, turn: QueuedTurn, front: bool) -> u64 {
        let queue = self.queue.entry(session.to_owned()).or_default();
        if front {
            queue.push_front(turn);
            1
        } else {
            queue.push_back(turn);
            u64::try_from(queue.len()).unwrap_or(u64::MAX)
        }
    }

    /// Takes the next queued input, dropping an empty queue.
    pub(super) fn dequeue(&mut self, session: &str) -> Option<QueuedTurn> {
        let queue = self.queue.get_mut(session)?;
        let next = queue.pop_front();
        if queue.is_empty() {
            self.queue.remove(session);
        }
        next
    }

    /// Drops one queued input by id.
    pub(super) fn remove_queued(&mut self, session: &str, turn: &TurnId) -> bool {
        let Some(queue) = self.queue.get_mut(session) else {
            return false;
        };
        let Some(position) = queue.iter().position(|queued| &queued.turn_id == turn) else {
            return false;
        };
        queue.remove(position);
        if queue.is_empty() {
            self.queue.remove(session);
        }
        true
    }

    /// Records a client id waiting for its `TurnStarted`.
    pub(super) fn pending_push(&mut self, session: &str, turn: TurnId) {
        self.pending
            .entry(session.to_owned())
            .or_default()
            .push_back(turn);
    }

    /// Drops a pending client id, whether it was queued or reserved.
    pub(super) fn pending_retain(&mut self, session: &str, turn: &TurnId) {
        let Some(pending) = self.pending.get_mut(session) else {
            return;
        };
        pending.retain(|pending_turn| pending_turn != turn);
        if pending.is_empty() {
            self.pending.remove(session);
        }
    }

    /// Takes the oldest client id waiting for a `TurnStarted`.
    pub(super) fn take_pending(&mut self, session: &str) -> Option<TurnId> {
        let pending = self.pending.get_mut(session)?;
        let next = pending.pop_front();
        if pending.is_empty() {
            self.pending.remove(session);
        }
        next
    }

    /// Remembers which modalities the next turn may produce.
    pub(super) fn set_pending_output(&mut self, session: &str, output: Vec<Modality>) {
        self.pending_output.insert(session.to_owned(), output);
    }

    /// Takes the modality request for a turn that just started.
    pub(super) fn take_pending_output(&mut self, session: &str) -> Option<Vec<Modality>> {
        self.pending_output.remove(session)
    }

    /// Forgets the modality request of a turn that never started.
    pub(super) fn clear_pending_output(&mut self, session: &str) {
        self.pending_output.remove(session);
    }

    /// Remembers the modalities of a turn the service started.
    pub(super) fn set_turn_output(&mut self, session: &str, turn: u64, output: Vec<Modality>) {
        self.turn_output.insert((session.to_owned(), turn), output);
    }

    /// The modalities a running turn may produce.
    pub(super) fn turn_output(&self, session: &str, turn: u64) -> Option<&Vec<Modality>> {
        self.turn_output.get(&(session.to_owned(), turn))
    }

    /// Forgets the modalities of a turn that ended.
    pub(super) fn remove_turn_output(&mut self, session: &str, turn: u64) {
        self.turn_output.remove(&(session.to_owned(), turn));
    }

    /// The client id the service's numeric turn was given.
    pub(super) fn alias(&self, session: &str, turn: u64) -> Option<TurnId> {
        self.aliases.get(&(session.to_owned(), turn)).cloned()
    }

    /// Records the client id for a service turn, bounded like the requests.
    pub(super) fn set_alias(&mut self, session: &str, turn: u64, id: TurnId, capacity: usize) {
        let key = (session.to_owned(), turn);
        self.aliases.insert(key.clone(), id);
        self.alias_order.push_back(key);
        while self.alias_order.len() > capacity {
            if let Some(oldest) = self.alias_order.pop_front() {
                self.aliases.remove(&oldest);
            }
        }
    }

    /// The service's numeric turn for a client id.
    pub(super) fn service_turn(&self, session: &str, client: &TurnId) -> Option<u64> {
        self.aliases
            .iter()
            .find(|((alias_session, _), alias)| alias_session == session && *alias == client)
            .map(|((_, turn), _)| *turn)
    }
}

#[cfg(test)]
#[path = "turn_book_tests.rs"]
mod tests;
