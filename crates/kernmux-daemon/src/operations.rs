//! Thread-safe asynchronous operation registry and bounded event stream.

use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::Duration,
};

use kernmux_api::v1::{
    Actor, ApiError, Event, EventKind, EventSequence, Generation, Operation, OperationId,
    OperationKind, OperationState, ResourceReference,
};

/// Immutable fields supplied when an asynchronous operation is accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewOperation {
    pub kind: OperationKind,
    pub expected_generation: Generation,
    pub affected_resources: Vec<ResourceReference>,
    pub actor: Option<Actor>,
    pub audit_id: Option<String>,
    pub created_at: String,
}

/// Bounded event read from a monotonic sequence cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventBatch {
    pub events: Vec<Event>,
    pub overflowed: bool,
    pub latest_sequence: EventSequence,
}

/// Stable registry rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationRegistryError {
    InvalidCapacity,
    NotFound,
    InvalidTransition,
    InvalidProgress,
}

/// Shared operation history and event stream.
#[derive(Clone, Debug)]
pub struct OperationRegistry {
    shared: Arc<Shared>,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<RegistryState>,
    changed: Condvar,
    operation_capacity: usize,
    event_capacity: usize,
}

#[derive(Debug)]
struct RegistryState {
    next_operation: u64,
    next_sequence: u64,
    latest_generation: Generation,
    operations: VecDeque<Operation>,
    events: VecDeque<Event>,
}

impl OperationRegistry {
    /// Creates a registry with independent operation and event bounds.
    ///
    /// # Errors
    ///
    /// Rejects a zero history capacity.
    pub fn new(
        operation_capacity: usize,
        event_capacity: usize,
    ) -> Result<Self, OperationRegistryError> {
        if operation_capacity == 0 || event_capacity == 0 {
            return Err(OperationRegistryError::InvalidCapacity);
        }
        Ok(Self {
            shared: Arc::new(Shared {
                state: Mutex::new(RegistryState {
                    next_operation: 1,
                    next_sequence: 1,
                    latest_generation: Generation(0),
                    operations: VecDeque::with_capacity(operation_capacity),
                    events: VecDeque::with_capacity(event_capacity),
                }),
                changed: Condvar::new(),
                operation_capacity,
                event_capacity,
            }),
        })
    }

    /// Accepts a queued operation and publishes an operation event.
    #[must_use]
    pub fn create(&self, request: NewOperation) -> Operation {
        let mut state = self.lock();
        let operation = Operation {
            id: OperationId(format!("op-{}", state.next_operation)),
            kind: request.kind,
            state: OperationState::Queued,
            progress_percent: None,
            expected_generation: request.expected_generation,
            observed_generation: None,
            affected_resources: request.affected_resources,
            error: None,
            actor: request.actor,
            audit_id: request.audit_id,
            created_at: request.created_at,
            completed_at: None,
        };
        state.next_operation = state.next_operation.saturating_add(1);
        if state.operations.len() == self.shared.operation_capacity {
            state.operations.pop_front();
        }
        state.operations.push_back(operation.clone());
        self.publish_operation_event(&mut state, &operation);
        drop(state);
        self.shared.changed.notify_all();
        operation
    }

    /// Marks a queued operation as running.
    ///
    /// # Errors
    ///
    /// Returns not-found or invalid-transition when the operation cannot run.
    pub fn start(&self, id: &OperationId) -> Result<Operation, OperationRegistryError> {
        self.update(id, |operation| {
            if operation.state != OperationState::Queued {
                return Err(OperationRegistryError::InvalidTransition);
            }
            operation.state = OperationState::Running;
            operation.progress_percent = Some(0);
            Ok(())
        })
    }

    /// Updates progress for a running operation.
    ///
    /// # Errors
    ///
    /// Rejects missing, non-running, regressing, or greater-than-100 progress.
    pub fn set_progress(
        &self,
        id: &OperationId,
        progress_percent: u8,
    ) -> Result<Operation, OperationRegistryError> {
        self.update(id, |operation| {
            if operation.state != OperationState::Running {
                return Err(OperationRegistryError::InvalidTransition);
            }
            if progress_percent > 100
                || operation
                    .progress_percent
                    .is_some_and(|current| progress_percent < current)
            {
                return Err(OperationRegistryError::InvalidProgress);
            }
            operation.progress_percent = Some(progress_percent);
            Ok(())
        })
    }

    /// Completes a queued or running operation with an immutable terminal state.
    ///
    /// # Errors
    ///
    /// Rejects missing operations and non-terminal or repeated transitions.
    pub fn finish(
        &self,
        id: &OperationId,
        state: OperationState,
        observed_generation: Option<Generation>,
        error: Option<ApiError>,
        completed_at: String,
    ) -> Result<Operation, OperationRegistryError> {
        self.update(id, |operation| {
            if is_terminal(operation.state) || !is_terminal(state) {
                return Err(OperationRegistryError::InvalidTransition);
            }
            operation.state = state;
            operation.observed_generation = observed_generation;
            operation.error = error;
            operation.completed_at = Some(completed_at);
            if state == OperationState::Succeeded {
                operation.progress_percent = Some(100);
            }
            Ok(())
        })
    }

    /// Returns one retained operation.
    #[must_use]
    pub fn get(&self, id: &OperationId) -> Option<Operation> {
        self.lock()
            .operations
            .iter()
            .find(|operation| &operation.id == id)
            .cloned()
    }

    /// Returns retained operations from oldest to newest.
    #[must_use]
    pub fn operations(&self) -> Vec<Operation> {
        self.lock().operations.iter().cloned().collect()
    }

    /// Latest authoritative snapshot generation observed by operation events.
    #[must_use]
    pub fn latest_generation(&self) -> Generation {
        self.lock().latest_generation
    }

    /// Reads retained events after a client cursor.
    #[must_use]
    pub fn events_after(&self, cursor: EventSequence) -> EventBatch {
        event_batch(&self.lock(), cursor)
    }

    /// Creates a blocking subscription after a client cursor.
    #[must_use]
    pub fn subscribe(&self, cursor: EventSequence) -> EventSubscription {
        EventSubscription {
            registry: self.clone(),
            cursor,
        }
    }

    fn update(
        &self,
        id: &OperationId,
        mutation: impl FnOnce(&mut Operation) -> Result<(), OperationRegistryError>,
    ) -> Result<Operation, OperationRegistryError> {
        let mut state = self.lock();
        let operation = state
            .operations
            .iter_mut()
            .find(|operation| &operation.id == id)
            .ok_or(OperationRegistryError::NotFound)?;
        mutation(operation)?;
        let operation = operation.clone();
        self.publish_operation_event(&mut state, &operation);
        drop(state);
        self.shared.changed.notify_all();
        Ok(operation)
    }

    fn publish_operation_event(&self, state: &mut RegistryState, operation: &Operation) {
        let operation_generation = operation
            .observed_generation
            .unwrap_or(operation.expected_generation);
        if operation_generation > state.latest_generation {
            state.latest_generation = operation_generation;
        }
        let event = Event {
            sequence: EventSequence(state.next_sequence),
            snapshot_generation: state.latest_generation,
            kind: EventKind::OperationChanged,
            resource: operation.affected_resources.first().cloned(),
        };
        state.next_sequence = state.next_sequence.saturating_add(1);
        if state.events.len() == self.shared.event_capacity {
            state.events.pop_front();
        }
        state.events.push_back(event);
    }

    fn lock(&self) -> MutexGuard<'_, RegistryState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Stateful blocking cursor for one event-stream client.
#[derive(Clone, Debug)]
pub struct EventSubscription {
    registry: OperationRegistry,
    cursor: EventSequence,
}

impl EventSubscription {
    /// Waits for events or the timeout and advances this subscription cursor.
    #[must_use]
    pub fn next_batch(&mut self, timeout: Duration) -> EventBatch {
        let state = self.registry.lock();
        let (state, _) = self
            .registry
            .shared
            .changed
            .wait_timeout_while(state, timeout, |state| {
                state.next_sequence.saturating_sub(1) <= self.cursor.0
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let batch = event_batch(&state, self.cursor);
        if let Some(last) = batch.events.last() {
            self.cursor = last.sequence;
        }
        batch
    }

    /// Current sequence cursor.
    #[must_use]
    pub const fn cursor(&self) -> EventSequence {
        self.cursor
    }
}

fn event_batch(state: &RegistryState, cursor: EventSequence) -> EventBatch {
    let latest = EventSequence(state.next_sequence.saturating_sub(1));
    let Some(oldest) = state.events.front() else {
        return EventBatch {
            events: Vec::new(),
            overflowed: false,
            latest_sequence: latest,
        };
    };
    let overflowed = cursor.0.saturating_add(1) < oldest.sequence.0;
    let mut events = Vec::new();
    if overflowed {
        events.push(Event {
            sequence: EventSequence(oldest.sequence.0.saturating_sub(1)),
            snapshot_generation: oldest.snapshot_generation,
            kind: EventKind::StreamOverflow,
            resource: None,
        });
    }
    events.extend(
        state
            .events
            .iter()
            .filter(|event| event.sequence.0 > cursor.0)
            .cloned(),
    );
    EventBatch {
        events,
        overflowed,
        latest_sequence: latest,
    }
}

const fn is_terminal(state: OperationState) -> bool {
    matches!(
        state,
        OperationState::Succeeded
            | OperationState::Failed
            | OperationState::Cancelled
            | OperationState::Indeterminate
    )
}

#[cfg(test)]
mod tests {
    use std::thread;

    use kernmux_api::v1::{ResourceKind, ResourceReference};

    use super::*;

    fn request(index: usize) -> NewOperation {
        NewOperation {
            kind: OperationKind::CreateInstance,
            expected_generation: Generation(4),
            affected_resources: vec![ResourceReference {
                kind: ResourceKind::Instance,
                id: format!("instance-{index}"),
            }],
            actor: None,
            audit_id: Some(format!("audit-{index}")),
            created_at: "2026-08-30T00:00:00Z".into(),
        }
    }

    #[test]
    fn validates_transitions_and_publishes_ordered_events() {
        let registry = OperationRegistry::new(8, 16).unwrap();
        let operation = registry.create(request(1));
        registry.start(&operation.id).unwrap();
        registry.set_progress(&operation.id, 40).unwrap();
        let completed = registry
            .finish(
                &operation.id,
                OperationState::Succeeded,
                Some(Generation(5)),
                None,
                "2026-08-30T00:00:01Z".into(),
            )
            .unwrap();

        assert_eq!(completed.progress_percent, Some(100));
        assert_eq!(completed.observed_generation, Some(Generation(5)));
        assert_eq!(
            registry.finish(
                &operation.id,
                OperationState::Failed,
                None,
                None,
                "later".into()
            ),
            Err(OperationRegistryError::InvalidTransition)
        );
        let _ = registry.create(request(2));
        let batch = registry.events_after(EventSequence(0));
        assert_eq!(batch.events.len(), 5);
        assert!(!batch.overflowed);
        assert_eq!(
            batch.events.last().unwrap().snapshot_generation,
            Generation(5)
        );
        for pair in batch.events.windows(2) {
            assert!(pair[1].is_contiguous_after(&pair[0]));
        }
    }

    #[test]
    fn slow_clients_receive_a_contiguous_overflow_marker() {
        let registry = OperationRegistry::new(8, 3).unwrap();
        for index in 0..5 {
            let _ = registry.create(request(index));
        }

        let batch = registry.events_after(EventSequence(0));

        assert!(batch.overflowed);
        assert_eq!(batch.events[0].kind, EventKind::StreamOverflow);
        assert_eq!(batch.events[0].sequence, EventSequence(2));
        assert_eq!(batch.latest_sequence, EventSequence(5));
        for pair in batch.events.windows(2) {
            assert!(pair[1].is_contiguous_after(&pair[0]));
        }
    }

    #[test]
    fn subscription_wakes_for_another_thread() {
        let registry = OperationRegistry::new(8, 8).unwrap();
        let mut subscription = registry.subscribe(EventSequence(0));
        let producer = registry.clone();
        let handle = thread::spawn(move || producer.create(request(1)));

        let batch = subscription.next_batch(Duration::from_secs(1));
        handle.join().unwrap();

        assert_eq!(batch.events.len(), 1);
        assert_eq!(subscription.cursor(), EventSequence(1));
    }

    #[test]
    fn concurrent_producers_keep_unique_monotonic_sequences() {
        let registry = OperationRegistry::new(100, 100).unwrap();
        let handles: Vec<_> = (0..4)
            .map(|worker| {
                let registry = registry.clone();
                thread::spawn(move || {
                    for index in 0..20 {
                        let _ = registry.create(request(worker * 20 + index));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let batch = registry.events_after(EventSequence(0));
        assert_eq!(batch.events.len(), 80);
        assert_eq!(registry.operations().len(), 80);
        assert_eq!(batch.latest_sequence, EventSequence(80));
        assert!(
            batch
                .events
                .iter()
                .enumerate()
                .all(|(index, event)| event.sequence.0 == index as u64 + 1)
        );
    }

    #[test]
    fn operation_history_remains_bounded() {
        let registry = OperationRegistry::new(2, 8).unwrap();
        let first = registry.create(request(1));
        let _ = registry.create(request(2));
        let _ = registry.create(request(3));

        assert_eq!(registry.operations().len(), 2);
        assert!(registry.get(&first.id).is_none());
    }
}
