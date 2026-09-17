//! Ack/retry layer for "important" UDP messages — `Event`s only
//! (ARCHITECTURE.md §3.3). `WorldState` (state messages) is deliberately
//! NOT wrapped by this: it's unreliable/unordered/latest-wins by design
//! (§3.1), handled instead by whichever side applies it always preferring
//! the newest tick.
//!
//! Giving up after `max_attempts` is a real, designed-for outcome here —
//! which is exactly why no client state may depend on an event arriving at
//! all (membership is reconciled from `WorldState` every tick instead,
//! §3.3). This module only tracks *whether* to retry or give up; it does
//! no I/O and never logs, since `common` does no I/O of any kind (§2) —
//! the caller (which does have a socket and a logger) acts on what this
//! returns.
//!
//! No clock reads either: every function takes "now" as an explicit `u64`
//! millisecond timestamp instead of reading a real clock, which is what
//! makes deterministic, seeded tests possible (see `tests::LossyChannel`).

use std::collections::{HashMap, HashSet, VecDeque};

pub type EventId = u32;

struct PendingEvent<T> {
    payload: T,
    last_sent_at_ms: u64,
    attempts: u32,
}

/// What a `Sender::due_for_retry` tick produced: events to (re)send now,
/// and events that just exhausted their attempts and are being dropped —
/// the caller logs those (§3.3 "gives up and logs").
pub struct RetryTick<T> {
    pub to_resend: Vec<(EventId, T)>,
    pub gave_up: Vec<EventId>,
}

/// Sender-side retry table. Generic over the payload type so this module
/// doesn't need to know about `protocol::EventKind` specifically.
pub struct Sender<T> {
    retry_interval_ms: u64,
    max_attempts: u32,
    pending: HashMap<EventId, PendingEvent<T>>,
}

impl<T: Clone> Sender<T> {
    pub fn new(retry_interval_ms: u64, max_attempts: u32) -> Self {
        Sender {
            retry_interval_ms,
            max_attempts,
            pending: HashMap::new(),
        }
    }

    /// Register an event as needing reliable delivery. The caller is
    /// responsible for actually transmitting it now — this just starts the
    /// retry clock; only *retries* come back out of `due_for_retry`.
    pub fn send(&mut self, event_id: EventId, payload: T, now_ms: u64) {
        self.pending.insert(
            event_id,
            PendingEvent {
                payload,
                last_sent_at_ms: now_ms,
                attempts: 1,
            },
        );
    }

    /// The peer acked `event_id` — stop retrying it. A no-op if it's
    /// already gone (already acked, or already given up).
    pub fn ack(&mut self, event_id: EventId) {
        self.pending.remove(&event_id);
    }

    /// Call periodically (e.g. once per server tick) with the current
    /// time. Every pending event whose last send was at least
    /// `retry_interval_ms` ago either gets resent (attempts < max) or is
    /// dropped as given up (attempts == max) — so give-up is detected on
    /// the same cadence as retries, meaning total time-to-give-up is
    /// `max_attempts * retry_interval_ms`, matching §8.2's delivery-bound
    /// test.
    pub fn due_for_retry(&mut self, now_ms: u64) -> RetryTick<T> {
        let mut to_resend = Vec::new();
        let mut gave_up = Vec::new();
        self.pending.retain(|&event_id, pending| {
            if now_ms.saturating_sub(pending.last_sent_at_ms) < self.retry_interval_ms {
                return true; // not due yet
            }
            if pending.attempts >= self.max_attempts {
                gave_up.push(event_id);
                return false;
            }
            pending.last_sent_at_ms = now_ms;
            pending.attempts += 1;
            to_resend.push((event_id, pending.payload.clone()));
            true
        });
        RetryTick { to_resend, gave_up }
    }
}

/// Receiver-side dedupe (§3.3): drops duplicate deliveries before handing
/// them to game logic, so callers never have to think about duplicate
/// delivery. Bounded ring, not an ever-growing set — old ids age out
/// instead of leaking memory on a long-running server.
pub struct Receiver {
    seen_order: VecDeque<EventId>,
    seen_set: HashSet<EventId>,
    capacity: usize,
}

impl Receiver {
    pub fn new(capacity: usize) -> Self {
        Receiver {
            seen_order: VecDeque::with_capacity(capacity),
            seen_set: HashSet::with_capacity(capacity),
            capacity,
        }
    }

    /// Returns `true` the first time `event_id` is seen (caller should act
    /// on it), `false` on a duplicate (caller should drop it silently).
    pub fn accept(&mut self, event_id: EventId) -> bool {
        if !self.seen_set.insert(event_id) {
            return false;
        }
        self.seen_order.push_back(event_id);
        if self.seen_order.len() > self.capacity {
            if let Some(oldest) = self.seen_order.pop_front() {
                self.seen_set.remove(&oldest);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{MAX_RETRY_ATTEMPTS, RETRY_INTERVAL_MS};
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    /// Seeded fake channel: drops and duplicates individual messages, and
    /// can reorder a whole batch — test infrastructure only, never shipped
    /// (PLAN.md Milestone 3's own instruction).
    struct LossyChannel {
        rng: ChaCha8Rng,
        drop_pct: f64,
        dup_pct: f64,
    }

    impl LossyChannel {
        fn new(seed: u64, drop_pct: f64, dup_pct: f64) -> Self {
            LossyChannel {
                rng: ChaCha8Rng::seed_from_u64(seed),
                drop_pct,
                dup_pct,
            }
        }

        /// One send in; 0, 1, or 2 arrivals out, simulating loss and
        /// duplication.
        fn transmit(&mut self, msg: EventId) -> Vec<EventId> {
            let mut out = Vec::new();
            if self.rng.r#gen::<f64>() < self.drop_pct {
                return out;
            }
            out.push(msg);
            if self.rng.r#gen::<f64>() < self.dup_pct {
                out.push(msg);
            }
            out
        }

        /// A whole batch, shuffled — simulates UDP delivering a burst of
        /// messages out of send order.
        fn reordered_batch<T>(&mut self, mut batch: Vec<T>) -> Vec<T> {
            batch.shuffle(&mut self.rng);
            batch
        }
    }

    // --- §8.2 item 1: delivery under loss, bounded, multiple seeds -------

    #[test]
    fn delivery_under_loss_within_bound() {
        let deadline_ms = MAX_RETRY_ATTEMPTS as u64 * RETRY_INTERVAL_MS;

        for seed in 0u64..8 {
            let mut channel = LossyChannel::new(seed, 0.3, 0.0);
            let mut sender: Sender<()> = Sender::new(RETRY_INTERVAL_MS, MAX_RETRY_ATTEMPTS);
            let mut receiver = Receiver::new(64);
            let mut delivered: HashSet<EventId> = HashSet::new();

            let event_ids: Vec<EventId> = (0..20).collect();
            for &id in &event_ids {
                sender.send(id, (), 0);
                for arrived in channel.transmit(id) {
                    if receiver.accept(arrived) {
                        delivered.insert(arrived);
                    }
                }
            }

            let mut now = 0u64;
            while now <= deadline_ms {
                now += RETRY_INTERVAL_MS;
                let tick = sender.due_for_retry(now);
                for (id, ()) in tick.to_resend {
                    for arrived in channel.transmit(id) {
                        if receiver.accept(arrived) {
                            delivered.insert(arrived);
                        }
                    }
                }
            }

            for &id in &event_ids {
                assert!(
                    delivered.contains(&id),
                    "seed {seed}: event {id} never delivered within {deadline_ms}ms (30% loss)"
                );
            }
        }
    }

    // --- §8.2 item 2: exactly-once delivery under duplication -------------

    #[test]
    fn receiver_fires_exactly_once_under_duplication() {
        for seed in 0u64..8 {
            // drop_pct = 0.0: every event arrives at least once, so any id
            // absent from `accepted_count` below would itself be a bug in
            // the test, not a real gap — this test is purely about
            // duplicates, loss is covered separately above.
            let mut channel = LossyChannel::new(seed, 0.0, 0.5);
            let mut receiver = Receiver::new(64);
            let mut accepted_count: HashMap<EventId, u32> = HashMap::new();

            for id in 0..50u32 {
                for arrived in channel.transmit(id) {
                    if receiver.accept(arrived) {
                        *accepted_count.entry(arrived).or_insert(0) += 1;
                    }
                }
            }

            for id in 0..50u32 {
                assert_eq!(
                    accepted_count.get(&id).copied().unwrap_or(0),
                    1,
                    "seed {seed}: event {id} should fire the accept callback exactly once"
                );
            }
        }
    }

    // --- §8.2 item 3: reordering must never let a stale tick win ---------

    /// Minimal stand-in for how a state-message consumer applies
    /// `WorldState` (§3.1: unordered, latest-tick-wins, no retry layer
    /// involved at all). `common::reliability` doesn't own this logic —
    /// nothing owns it yet, since the real `WorldState` consumer isn't
    /// built until later milestones — but the *shared fake channel* this
    /// milestone builds is exactly what should validate the pattern before
    /// then. See PLAN.md's Milestone 3 log entry for the full scope note.
    struct LatestTickWins<T> {
        best_tick: Option<u32>,
        value: Option<T>,
    }

    impl<T: Clone> LatestTickWins<T> {
        fn new() -> Self {
            LatestTickWins {
                best_tick: None,
                value: None,
            }
        }

        fn apply(&mut self, tick: u32, value: T) {
            if self.best_tick.is_none_or(|best| tick > best) {
                self.best_tick = Some(tick);
                self.value = Some(value);
            }
        }
    }

    #[test]
    fn reordering_never_lets_a_stale_tick_win() {
        for seed in 0u64..8 {
            let mut channel = LossyChannel::new(seed, 0.0, 0.0);
            let n = 30u32;
            let in_order: Vec<(u32, u32)> = (0..n).map(|tick| (tick, tick * 10)).collect();
            let shuffled = channel.reordered_batch(in_order);

            let mut reducer = LatestTickWins::new();
            for (tick, value) in shuffled {
                reducer.apply(tick, value);
            }

            assert_eq!(
                reducer.best_tick,
                Some(n - 1),
                "seed {seed}: a stale tick overwrote the newest one"
            );
            assert_eq!(reducer.value, Some((n - 1) * 10));
        }
    }

    // --- Sender/Receiver unit tests, independent of the fake channel -----

    #[test]
    fn ack_stops_retries() {
        let mut sender: Sender<()> = Sender::new(RETRY_INTERVAL_MS, MAX_RETRY_ATTEMPTS);
        sender.send(1, (), 0);
        sender.ack(1);
        let tick = sender.due_for_retry(RETRY_INTERVAL_MS);
        assert!(tick.to_resend.is_empty());
        assert!(tick.gave_up.is_empty());
    }

    #[test]
    fn gives_up_after_max_attempts_without_panicking() {
        let mut sender: Sender<()> = Sender::new(RETRY_INTERVAL_MS, 3);
        sender.send(1, (), 0);

        // attempt 2
        let tick = sender.due_for_retry(RETRY_INTERVAL_MS);
        assert_eq!(tick.to_resend.len(), 1);
        // attempt 3
        let tick = sender.due_for_retry(RETRY_INTERVAL_MS * 2);
        assert_eq!(tick.to_resend.len(), 1);
        // now at max_attempts (3): next due check gives up instead of resending
        let tick = sender.due_for_retry(RETRY_INTERVAL_MS * 3);
        assert!(tick.to_resend.is_empty());
        assert_eq!(tick.gave_up, vec![1]);
    }
}
