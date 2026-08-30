//! Asynchronous mutation scheduling backed by the operation event stream.

use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use kernmux_api::v1::{
    ApiError, Diagnostic, DiagnosticSeverity, ErrorCode, Generation, Operation, OperationId,
    OperationState,
};

use crate::{
    lifecycle::LifecycleRequest,
    lifecycle_executor::{
        KerfRunner, LifecycleExecutionError, LifecycleExecutor, LifecycleOutcome, SnapshotRefresher,
    },
    operations::{NewOperation, OperationRegistry, OperationRegistryError},
    resource_pool::{
        ResourcePoolExecutionError, ResourcePoolExecutor, ResourcePoolOutcome, ResourcePoolRequest,
    },
};

/// Cooperative cancellation observed only at safe task boundaries.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Terminal result returned by one mutation worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationTaskResult {
    pub state: OperationState,
    pub observed_generation: Option<Generation>,
    pub error: Option<ApiError>,
}

impl OperationTaskResult {
    /// Creates a successful task result.
    #[must_use]
    pub const fn succeeded(generation: Generation) -> Self {
        Self {
            state: OperationState::Succeeded,
            observed_generation: Some(generation),
            error: None,
        }
    }

    /// Creates a cooperatively cancelled task result.
    #[must_use]
    pub const fn cancelled(generation: Option<Generation>) -> Self {
        Self {
            state: OperationState::Cancelled,
            observed_generation: generation,
            error: None,
        }
    }
}

/// Converts a reconciled lifecycle result into an operation terminal result.
#[must_use]
pub fn lifecycle_task_result<E>(
    result: Result<LifecycleOutcome, LifecycleExecutionError<E>>,
) -> OperationTaskResult {
    match result {
        Ok(outcome) => outcome_result(
            outcome.state,
            outcome.snapshot.generation,
            outcome.diagnostics,
        ),
        Err(error) => failure_result(error.error_code(), Vec::new()),
    }
}

/// Converts a reconciled resource-pool result into an operation terminal result.
#[must_use]
pub fn resource_pool_task_result<E>(
    result: Result<ResourcePoolOutcome, ResourcePoolExecutionError<E>>,
) -> OperationTaskResult {
    match result {
        Ok(outcome) => outcome_result(
            outcome.state,
            outcome.snapshot.generation,
            outcome.diagnostics,
        ),
        Err(error) => failure_result(error.error_code(), Vec::new()),
    }
}

/// Thread-backed operation scheduler with retained worker handles.
#[derive(Clone)]
pub struct OperationScheduler {
    registry: OperationRegistry,
    workers: Arc<Mutex<Vec<Worker>>>,
    clock: Arc<dyn Fn() -> String + Send + Sync>,
}

impl std::fmt::Debug for OperationScheduler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationScheduler")
            .field("registry", &self.registry)
            .field("worker_count", &self.worker_count())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
struct Worker {
    operation_id: OperationId,
    cancellation: CancellationToken,
    handle: JoinHandle<()>,
}

impl OperationScheduler {
    /// Creates a scheduler with a caller-supplied timestamp source.
    #[must_use]
    pub fn new(
        registry: OperationRegistry,
        clock: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            registry,
            workers: Arc::new(Mutex::new(Vec::new())),
            clock: Arc::new(clock),
        }
    }

    /// Accepts and starts an asynchronous mutation.
    ///
    /// # Errors
    ///
    /// Returns a spawn error after marking the accepted operation failed when
    /// the worker thread cannot be created.
    pub fn submit<F>(&self, request: NewOperation, task: F) -> Result<Operation, ScheduleError>
    where
        F: FnOnce(CancellationToken) -> OperationTaskResult + Send + 'static,
    {
        self.reap_finished();
        let operation = self.registry.create(request);
        let operation_id = operation.id.clone();
        let registry = self.registry.clone();
        let clock = Arc::clone(&self.clock);
        let cancellation = CancellationToken {
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let task_cancellation = cancellation.clone();
        let thread_name = format!("kernmux-{}", operation_id.0);
        let handle = match thread::Builder::new().name(thread_name).spawn(move || {
            if registry.start(&operation_id).is_err() {
                return;
            }
            let result = catch_unwind(AssertUnwindSafe(|| task(task_cancellation)))
                .unwrap_or_else(|_| worker_panic_result());
            let _ = registry.finish(
                &operation_id,
                result.state,
                result.observed_generation,
                result.error,
                clock(),
            );
        }) {
            Ok(handle) => handle,
            Err(error) => {
                let failure = worker_spawn_result();
                let _ = self.registry.finish(
                    &operation.id,
                    failure.state,
                    failure.observed_generation,
                    failure.error,
                    (self.clock)(),
                );
                return Err(ScheduleError::Spawn(error));
            }
        };
        self.workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Worker {
                operation_id: operation.id.clone(),
                cancellation,
                handle,
            });
        Ok(operation)
    }

    /// Requests cooperative cancellation for a retained worker.
    ///
    /// # Errors
    ///
    /// Returns not-found when no active worker has the operation ID.
    pub fn cancel(&self, id: &OperationId) -> Result<(), OperationRegistryError> {
        self.reap_finished();
        let workers = self
            .workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let worker = workers
            .iter()
            .find(|worker| &worker.operation_id == id)
            .ok_or(OperationRegistryError::NotFound)?;
        worker.cancellation.cancelled.store(true, Ordering::Release);
        Ok(())
    }

    /// Joins every retained worker and leaves no running jobs.
    pub fn drain(&self) {
        let workers = {
            let mut workers = self
                .workers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *workers)
        };
        for worker in workers {
            let _ = worker.handle.join();
        }
    }

    /// Number of retained workers, including completed workers not yet reaped.
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn reap_finished(&self) {
        let mut workers = self
            .workers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut index = 0;
        while index < workers.len() {
            if workers[index].handle.is_finished() {
                let worker = workers.swap_remove(index);
                let _ = worker.handle.join();
            } else {
                index += 1;
            }
        }
    }
}

/// Schedules one lifecycle mutation against a serialized executor.
///
/// Cancellation is observed before the executor lock is acquired and again
/// immediately before Kerf is invoked. Once execution starts, reconciliation
/// is allowed to complete so the operation records authoritative host state.
///
/// # Errors
///
/// Returns a scheduling error when the worker cannot be started.
pub fn submit_lifecycle<R, S>(
    scheduler: &OperationScheduler,
    operation: NewOperation,
    executor: Arc<Mutex<LifecycleExecutor<R, S>>>,
    request: LifecycleRequest,
) -> Result<Operation, ScheduleError>
where
    R: KerfRunner + Send + 'static,
    S: SnapshotRefresher + Send + 'static,
{
    scheduler.submit(operation, move |cancellation| {
        if cancellation.is_cancelled() {
            return OperationTaskResult::cancelled(None);
        }
        let mut executor = executor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancellation.is_cancelled() {
            return OperationTaskResult::cancelled(None);
        }
        lifecycle_task_result(executor.execute(&request))
    })
}

/// Schedules one resource-pool mutation against a serialized executor.
///
/// Cancellation is observed only before mutation starts. An in-flight Kerf
/// request always completes postflight reconciliation before becoming
/// terminal.
///
/// # Errors
///
/// Returns a scheduling error when the worker cannot be started.
pub fn submit_resource_pool<R, S>(
    scheduler: &OperationScheduler,
    operation: NewOperation,
    executor: Arc<Mutex<ResourcePoolExecutor<R, S>>>,
    request: ResourcePoolRequest,
) -> Result<Operation, ScheduleError>
where
    R: KerfRunner + Send + 'static,
    S: SnapshotRefresher + Send + 'static,
{
    scheduler.submit(operation, move |cancellation| {
        if cancellation.is_cancelled() {
            return OperationTaskResult::cancelled(None);
        }
        let mut executor = executor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cancellation.is_cancelled() {
            return OperationTaskResult::cancelled(None);
        }
        resource_pool_task_result(executor.execute(&request))
    })
}

/// Failure to schedule an accepted operation.
#[derive(Debug)]
pub enum ScheduleError {
    Spawn(io::Error),
}

fn outcome_result(
    state: OperationState,
    generation: Generation,
    diagnostics: Vec<Diagnostic>,
) -> OperationTaskResult {
    let error = match state {
        OperationState::Succeeded | OperationState::Cancelled => None,
        OperationState::Indeterminate => Some(api_error(
            ErrorCode::BackendUnavailable,
            "operation outcome is indeterminate",
            true,
            diagnostics,
        )),
        _ => Some(api_error(
            ErrorCode::BackendUnavailable,
            "host mutation did not reach the requested result",
            true,
            diagnostics,
        )),
    };
    OperationTaskResult {
        state,
        observed_generation: Some(generation),
        error,
    }
}

fn failure_result(code: ErrorCode, diagnostics: Vec<Diagnostic>) -> OperationTaskResult {
    OperationTaskResult {
        state: OperationState::Failed,
        observed_generation: None,
        error: Some(api_error(
            code,
            "operation was rejected before execution",
            matches!(code, ErrorCode::BackendUnavailable | ErrorCode::Timeout),
            diagnostics,
        )),
    }
}

fn worker_panic_result() -> OperationTaskResult {
    OperationTaskResult {
        state: OperationState::Failed,
        observed_generation: None,
        error: Some(api_error(
            ErrorCode::Internal,
            "operation worker terminated unexpectedly",
            false,
            vec![Diagnostic {
                code: "worker_panicked".into(),
                severity: DiagnosticSeverity::Error,
                message: "operation worker terminated unexpectedly".into(),
                detail: None,
                redacted: true,
            }],
        )),
    }
}

fn worker_spawn_result() -> OperationTaskResult {
    OperationTaskResult {
        state: OperationState::Failed,
        observed_generation: None,
        error: Some(api_error(
            ErrorCode::Internal,
            "operation worker could not be started",
            true,
            Vec::new(),
        )),
    }
}

fn api_error(
    code: ErrorCode,
    message: &str,
    retryable: bool,
    diagnostics: Vec<Diagnostic>,
) -> ApiError {
    ApiError {
        code,
        message: message.into(),
        retryable,
        current_generation: None,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, sync::mpsc};

    use kernmux_api::v1::{EventSequence, OperationKind};

    use super::*;

    fn request() -> NewOperation {
        NewOperation {
            kind: OperationKind::CreateInstance,
            expected_generation: Generation(3),
            affected_resources: Vec::new(),
            actor: None,
            audit_id: Some("audit-1".into()),
            created_at: "2026-08-30T00:00:00Z".into(),
        }
    }

    fn scheduler() -> (OperationRegistry, OperationScheduler) {
        let registry = OperationRegistry::new(16, 32).unwrap();
        let scheduler = OperationScheduler::new(registry.clone(), || "2026-08-30T00:00:01Z".into());
        (registry, scheduler)
    }

    #[test]
    fn successful_worker_reaches_terminal_operation_and_ordered_events() {
        let (registry, scheduler) = scheduler();
        let accepted = scheduler
            .submit(request(), |_| OperationTaskResult::succeeded(Generation(4)))
            .unwrap();

        scheduler.drain();

        let completed = registry.get(&accepted.id).unwrap();
        assert_eq!(completed.state, OperationState::Succeeded);
        assert_eq!(completed.observed_generation, Some(Generation(4)));
        assert_eq!(completed.audit_id.as_deref(), Some("audit-1"));
        let events = registry.events_after(EventSequence(0)).events;
        assert_eq!(events.len(), 3);
        assert!(events[1].is_contiguous_after(&events[0]));
        assert!(events[2].is_contiguous_after(&events[1]));
        assert_eq!(scheduler.worker_count(), 0);
    }

    #[test]
    fn worker_panic_is_captured_as_a_terminal_failure() {
        let (registry, scheduler) = scheduler();
        let accepted = scheduler
            .submit(request(), |_| -> OperationTaskResult {
                panic!("test panic")
            })
            .unwrap();

        scheduler.drain();

        let completed = registry.get(&accepted.id).unwrap();
        assert_eq!(completed.state, OperationState::Failed);
        let error = completed.error.unwrap();
        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(error.diagnostics[0].code, "worker_panicked");
    }

    #[test]
    fn cancellation_is_observed_at_a_cooperative_boundary() {
        let (registry, scheduler) = scheduler();
        let (started_tx, started_rx) = mpsc::channel();
        let accepted = scheduler
            .submit(request(), move |cancellation| {
                started_tx.send(()).unwrap();
                while !cancellation.is_cancelled() {
                    thread::yield_now();
                }
                OperationTaskResult::cancelled(Some(Generation(3)))
            })
            .unwrap();
        started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        scheduler.cancel(&accepted.id).unwrap();
        scheduler.drain();

        assert_eq!(
            registry.get(&accepted.id).unwrap().state,
            OperationState::Cancelled
        );
    }

    #[test]
    fn preflight_error_maps_to_stable_failed_result() {
        let result = lifecycle_task_result::<Infallible>(Err(
            LifecycleExecutionError::SnapshotIndeterminate,
        ));

        assert_eq!(result.state, OperationState::Failed);
        let error = result.error.unwrap();
        assert_eq!(error.code, ErrorCode::BackendUnavailable);
        assert!(error.retryable);
    }

    #[test]
    fn indeterminate_outcome_preserves_diagnostics() {
        let diagnostic = Diagnostic {
            code: "inventory_unavailable".into(),
            severity: DiagnosticSeverity::Error,
            message: "host inventory refresh did not complete".into(),
            detail: None,
            redacted: true,
        };

        let result = outcome_result(
            OperationState::Indeterminate,
            Generation(5),
            vec![diagnostic.clone()],
        );

        assert_eq!(result.state, OperationState::Indeterminate);
        assert_eq!(result.observed_generation, Some(Generation(5)));
        assert_eq!(result.error.unwrap().diagnostics, [diagnostic]);
    }
}
