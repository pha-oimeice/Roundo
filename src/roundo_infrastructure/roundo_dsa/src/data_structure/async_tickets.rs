//! Bounded, in-memory correlation of asynchronous work with one terminal notification.
//!
//! The owner submits a ticket, dispatches external work separately, then supplies its
//! reply (or local failure/cancellation). No transport, executor or clock is owned here.
//! Calls are synchronous and nonblocking; the owner must advance time explicitly.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TICKET_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque correlation identity, never recycled across books within a process.
/// Restarting the process does not preserve pending tickets or ID uniqueness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TicketId(u64);

#[derive(Debug, PartialEq, Eq)]
pub enum Completion<T, E> {
    Success(T),
    Error(E),
    TimedOut,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmitError {
    Full,
    DeadlineElapsed,
    ClockRegressed,
    IdExhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplyError {
    NotPending,
    ClockRegressed,
}

/// Single-owner ticket book; externally synchronized access is the caller's responsibility.
/// `capacity` bounds pending tickets plus terminal notifications waiting to be delivered.
/// Payload bytes and work outside the book are not counted; callers must bound them separately.
pub struct AsyncTickets<T, E> {
    capacity: usize,
    now: u64,
    pending: HashMap<TicketId, u64>,
    deadlines: BTreeMap<u64, BTreeSet<TicketId>>,
    notifications: VecDeque<(TicketId, Completion<T, E>)>,
}

impl<T, E> AsyncTickets<T, E> {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            now: 0,
            pending: HashMap::new(),
            deadlines: BTreeMap::new(),
            notifications: VecDeque::new(),
        }
    }

    /// Number of retained pending tickets and undelivered terminal notifications.
    pub fn len(&self) -> usize {
        self.pending.len() + self.notifications.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Admit a ticket with an exclusive deadline (`now < deadline`).
    /// Caller supplies a nondecreasing logical time on every timed operation.
    /// A rejected submission creates no ticket or notification.
    pub fn submit(&mut self, now: u64, deadline: u64) -> Result<TicketId, SubmitError> {
        self.advance(now).map_err(|_| SubmitError::ClockRegressed)?;
        if deadline <= now {
            return Err(SubmitError::DeadlineElapsed);
        }
        if self.len() >= self.capacity {
            return Err(SubmitError::Full);
        }
        let raw = NEXT_TICKET_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| SubmitError::IdExhausted)?;
        let id = TicketId(raw);
        self.pending.insert(id, deadline);
        self.deadlines.entry(deadline).or_default().insert(id);
        Ok(id)
    }

    /// Expire only due deadline buckets. Time regression leaves the book unchanged.
    /// Expiration does not free capacity until its notification is delivered.
    pub fn advance(&mut self, now: u64) -> Result<(), ReplyError> {
        if now < self.now {
            return Err(ReplyError::ClockRegressed);
        }
        self.now = now;
        while self
            .deadlines
            .first_key_value()
            .is_some_and(|(&due, _)| due <= now)
        {
            let (_, ids) = self.deadlines.pop_first().expect("due bucket exists");
            for id in ids {
                self.pending
                    .remove(&id)
                    .expect("scheduled ticket is pending");
                self.notifications.push_back((id, Completion::TimedOut));
            }
        }
        Ok(())
    }

    /// Deliver a successful or failed external result, once, strictly before deadline.
    pub fn reply(
        &mut self,
        id: TicketId,
        now: u64,
        result: Result<T, E>,
    ) -> Result<(), ReplyError> {
        self.advance(now)?;
        self.finish(
            id,
            match result {
                Ok(value) => Completion::Success(value),
                Err(error) => Completion::Error(error),
            },
        )
    }

    /// Explicit failure to dispatch or complete external work.
    pub fn fail(&mut self, id: TicketId, now: u64, error: E) -> Result<(), ReplyError> {
        self.advance(now)?;
        self.finish(id, Completion::Error(error))
    }

    /// Explicitly cancel outstanding work; external cancellation is caller-owned.
    pub fn cancel(&mut self, id: TicketId, now: u64) -> Result<(), ReplyError> {
        self.advance(now)?;
        self.finish(id, Completion::Cancelled)
    }

    fn finish(&mut self, id: TicketId, completion: Completion<T, E>) -> Result<(), ReplyError> {
        let deadline = self.pending.remove(&id).ok_or(ReplyError::NotPending)?;
        let ids = self
            .deadlines
            .get_mut(&deadline)
            .expect("pending deadline exists");
        ids.remove(&id);
        if ids.is_empty() {
            self.deadlines.remove(&deadline);
        }
        self.notifications.push_back((id, completion));
        Ok(())
    }

    /// Transfers ownership of one notification to the caller. Never drops an earlier reply
    /// to make room for a new request; consumers must drain to release capacity.
    pub fn try_next(&mut self) -> Option<(TicketId, Completion<T, E>)> {
        self.notifications.pop_front()
    }

    /// Deliver every currently queued notification to a synchronous callback.
    pub fn drain_with(&mut self, mut notify: impl FnMut(TicketId, Completion<T, E>)) {
        while let Some((id, completion)) = self.try_next() {
            notify(id, completion);
        }
    }
}
