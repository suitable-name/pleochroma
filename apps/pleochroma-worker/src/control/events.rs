//! The coordinator's event fan-out.
//!
//! Every [`EventHub::subscribe`] call gets its own receiver, [`EventHub::publish`] sends one event
//! to all of them, and a receiver that was dropped is forgotten the next time something is
//! published.

use pleochroma_net::admin::status::CoordinatorEvent;
use std::sync::{
    Arc, Mutex, MutexGuard, PoisonError,
    mpsc::{self, Receiver, SyncSender, TrySendError},
};

/// How many events a subscriber may have waiting.
///
/// A subscriber that falls this far behind (a stalled remote session) misses the events published
/// while its queue is full; the hub never grows without bound and never blocks the publisher. The
/// standard receiver cannot be emptied from the sending side, so it is the newest events that are
/// skipped.
pub const SUBSCRIBER_QUEUE: usize = 1024;

/// Sends [`CoordinatorEvent`]s to any number of receivers. Clones share the subscriber list.
#[derive(Debug, Clone, Default)]
pub struct EventHub {
    senders: Arc<Mutex<Vec<SyncSender<CoordinatorEvent>>>>,
}

impl EventHub {
    /// A hub with no subscribers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Vec<SyncSender<CoordinatorEvent>>> {
        self.senders.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A new, independent receiver that gets every event published from now on (up to
    /// [`SUBSCRIBER_QUEUE`] waiting). Dropping the receiver unsubscribes it.
    #[must_use]
    pub fn subscribe(&self) -> Receiver<CoordinatorEvent> {
        let (tx, rx) = mpsc::sync_channel(SUBSCRIBER_QUEUE);
        self.lock().push(tx);
        rx
    }

    /// Sends `event` to every live subscriber and forgets the ones that went away. A
    /// subscriber whose queue is full skips the event.
    pub fn publish(&self, event: &CoordinatorEvent) {
        self.lock().retain(|tx| {
            !matches!(
                tx.try_send(event.clone()),
                Err(TrySendError::Disconnected(_))
            )
        });
    }

    /// How many receivers are still subscribed (as far as the last publish knows).
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::EventHub;
    use pleochroma_net::admin::status::{CoordinatorEvent, TokenId};

    #[test]
    fn every_subscriber_gets_every_event_and_dropped_ones_are_pruned() {
        let hub = EventHub::new();
        let first = hub.subscribe();
        let second = hub.subscribe();
        let gone = hub.subscribe();
        assert_eq!(hub.subscriber_count(), 3);
        drop(gone);

        let event = CoordinatorEvent::TokenCancelled { id: TokenId(7) };
        hub.publish(&event);
        assert_eq!(hub.subscriber_count(), 2, "the dropped receiver is pruned");
        assert_eq!(first.try_recv().unwrap(), event);
        assert_eq!(second.try_recv().unwrap(), event);
        assert!(first.try_recv().is_err(), "each event arrives once");

        let later = hub.subscribe();
        hub.publish(&CoordinatorEvent::TokenExpired { id: TokenId(8) });
        assert_eq!(
            later.try_recv().unwrap(),
            CoordinatorEvent::TokenExpired { id: TokenId(8) }
        );
        assert_eq!(
            first.try_recv().unwrap(),
            CoordinatorEvent::TokenExpired { id: TokenId(8) }
        );
    }

    #[test]
    fn a_stalled_subscriber_is_bounded_and_keeps_its_place() {
        let hub = EventHub::new();
        let stalled = hub.subscribe();
        for n in 0..(super::SUBSCRIBER_QUEUE as u64 + 50) {
            hub.publish(&CoordinatorEvent::TokenExpired { id: TokenId(n) });
        }
        assert_eq!(hub.subscriber_count(), 1, "a full queue is not a gone one");
        assert_eq!(stalled.try_iter().count(), super::SUBSCRIBER_QUEUE);
    }
}
