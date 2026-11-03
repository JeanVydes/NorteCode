use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc;

use crate::runtime::event::{EngineEvent, StreamDelta};

/// A read-only watcher of the engine's events.
pub trait Observer: Send + Sync {
    /// The observer's name, as it appears in session metadata.
    fn name(&self) -> &str;

    /// Read-only, may not block the critical path.
    fn observe(&self, event: &EngineEvent);
}

/// Projection of stream deltas for downstream consumers (UI, metrics).
///
/// A transform may annotate or split deltas; it may not drop or reorder the
/// events the engine relies on for durability, so it can only affect the
/// observer side-channel.
pub trait StreamTransform: Send + Sync {
    /// The transform's name, as it appears in session metadata.
    fn name(&self) -> &str;

    /// Projects one delta into zero or more deltas.
    fn transform(&self, delta: StreamDelta) -> Vec<StreamDelta>;
}

struct Registration {
    observer: Arc<dyn Observer>,
    sender: mpsc::Sender<EngineEvent>,
}

/// Bounded, lossy fan-out for observers.
///
/// The durable event ordering lives in the engine's own stream; this set exists
/// so slow observers never block it. Events are dropped when a registration's
/// buffer is full.
pub struct ObserverSet {
    registrations: Vec<Registration>,
    transforms: Vec<Arc<dyn StreamTransform>>,
    dropped: AtomicU64,
}

impl ObserverSet {
    /// A set of `observers`, each with a channel holding `buffer` events (at
    /// least one) before it starts dropping.
    #[must_use]
    pub fn new(observers: Vec<Arc<dyn Observer>>, buffer: usize) -> Self {
        Self::with_transforms(observers, Vec::new(), buffer)
    }

    /// A set of `observers` and `transforms`, each observer with a
    /// `buffer`-sized channel (at least one).
    #[must_use]
    pub fn with_transforms(
        observers: Vec<Arc<dyn Observer>>,
        transforms: Vec<Arc<dyn StreamTransform>>,
        buffer: usize,
    ) -> Self {
        let buffer = buffer.max(1);
        let mut registrations = Vec::with_capacity(observers.len());
        for observer in observers {
            let (sender, receiver) = mpsc::channel(buffer);
            let worker = Arc::clone(&observer);
            tokio::spawn(async move {
                let mut receiver = receiver;
                while let Some(event) = receiver.recv().await {
                    worker.observe(&event);
                }
            });
            registrations.push(Registration { observer, sender });
        }
        Self {
            registrations,
            transforms,
            dropped: AtomicU64::new(0),
        }
    }

    /// Whether no observer is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registrations.is_empty()
    }

    /// Names recorded in session extension metadata for reproducibility.
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        self.registrations
            .iter()
            .map(|registration| registration.observer.name())
            .collect()
    }

    /// Transform names recorded in session extension metadata (FR-OBS-3).
    #[must_use]
    pub fn transform_names(&self) -> Vec<&str> {
        self.transforms
            .iter()
            .map(|transform| transform.name())
            .collect()
    }

    /// Runs a delta through the registered transforms in registration order.
    ///
    /// Transforms shape the delta side-channel that the host stream and the
    /// observers read: they may annotate, split or even drop deltas. They never
    /// touch the accumulator the engine commits, so durability is unaffected
    /// (FR-OBS-2).
    #[must_use]
    pub fn project(&self, delta: StreamDelta) -> Vec<StreamDelta> {
        let mut current = vec![delta];
        for transform in &self.transforms {
            current = current
                .into_iter()
                .flat_map(|delta| transform.transform(delta))
                .collect();
        }
        current
    }

    /// Offers `event` to every observer without blocking; an observer whose
    /// buffer is full counts as a drop.
    pub fn emit(&self, event: &EngineEvent) {
        for registration in &self.registrations {
            if registration.sender.try_send(event.clone()).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// How many events were dropped because an observer was not keeping up.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl std::fmt::Debug for ObserverSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObserverSet")
            .field("names", &self.names())
            .field("transforms", &self.transform_names())
            .field("dropped", &self.dropped())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Upper;

    impl StreamTransform for Upper {
        fn name(&self) -> &'static str {
            "upper"
        }

        fn transform(&self, delta: StreamDelta) -> Vec<StreamDelta> {
            match delta {
                StreamDelta::Text { index, text } => vec![StreamDelta::Text {
                    index,
                    text: text.to_uppercase(),
                }],
                other => vec![other],
            }
        }
    }

    struct DropText;

    impl StreamTransform for DropText {
        fn name(&self) -> &'static str {
            "drop-text"
        }

        fn transform(&self, delta: StreamDelta) -> Vec<StreamDelta> {
            match delta {
                StreamDelta::Text { .. } => Vec::new(),
                other => vec![other],
            }
        }
    }

    #[test]
    fn transforms_project_deltas_in_registration_order() {
        let set =
            ObserverSet::with_transforms(Vec::new(), vec![Arc::new(DropText), Arc::new(Upper)], 4);
        assert_eq!(set.transform_names(), vec!["drop-text", "upper"]);
        let text = StreamDelta::Text {
            index: 0,
            text: "hi".to_owned(),
        };
        assert!(
            set.project(text).is_empty(),
            "a dropped delta projects nothing"
        );
    }

    #[test]
    fn a_transform_annotates_without_touching_other_deltas() {
        let set = ObserverSet::with_transforms(Vec::new(), vec![Arc::new(Upper)], 4);
        let out = set.project(StreamDelta::Text {
            index: 0,
            text: "hi".to_owned(),
        });
        assert_eq!(
            out,
            vec![StreamDelta::Text {
                index: 0,
                text: "HI".to_owned()
            }]
        );
        let continuity = StreamDelta::Continuity { index: 0, len: 3 };
        assert_eq!(set.project(continuity.clone()), vec![continuity]);
    }
}
