//! Bounded Kerf execution and authoritative lifecycle reconciliation.

use std::{
    ffi::OsString,
    fmt,
    fs::File,
    io::{self, Read, Seek},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use kernmux_api::v1::{
    Diagnostic, DiagnosticSeverity, ErrorCode, HostSnapshot, OperationState, SnapshotHealth,
};
use tempfile::tempfile;

use crate::{
    inventory::{InventoryService, InventorySource},
    lifecycle::{ExpectedState, KerfInvocation, LifecyclePlanError, LifecycleRequest, plan},
};

const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Refreshes the authoritative host snapshot before and after a mutation.
pub trait SnapshotRefresher {
    type Error;

    /// Reads and assembles current host state.
    ///
    /// # Errors
    ///
    /// Returns the backend error when no snapshot can be produced.
    fn refresh_snapshot(&mut self) -> Result<HostSnapshot, Self::Error>;
}

/// Cloneable access to one generation-owning snapshot refresher.
#[derive(Debug)]
pub struct SharedSnapshotRefresher<S> {
    inner: Arc<Mutex<S>>,
}

impl<S> SharedSnapshotRefresher<S> {
    /// Wraps a refresher so every clone observes the same generation history.
    #[must_use]
    pub fn new(inner: S) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }
}

impl<S> Clone for SharedSnapshotRefresher<S> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<S> SnapshotRefresher for SharedSnapshotRefresher<S>
where
    S: SnapshotRefresher,
{
    type Error = S::Error;

    fn refresh_snapshot(&mut self) -> Result<HostSnapshot, Self::Error> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh_snapshot()
    }
}

impl<S> SnapshotRefresher for InventoryService<S>
where
    S: InventorySource,
{
    type Error = S::Error;

    fn refresh_snapshot(&mut self) -> Result<HostSnapshot, Self::Error> {
        self.refresh()
    }
}

/// Termination observed for one Kerf process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KerfTermination {
    Exited(i32),
    Signalled,
    TimedOut,
}

/// Bounded output and termination from one Kerf invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KerfRunResult {
    pub termination: KerfTermination,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl KerfRunResult {
    /// Whether Kerf itself reported success.
    #[must_use]
    pub const fn process_succeeded(&self) -> bool {
        matches!(self.termination, KerfTermination::Exited(0))
    }
}

/// Replaceable Kerf process backend.
pub trait KerfRunner {
    type Error;

    /// Executes one previously validated invocation.
    ///
    /// # Errors
    ///
    /// Returns the backend error when the process cannot be supervised.
    fn run(&mut self, invocation: &KerfInvocation) -> Result<KerfRunResult, Self::Error>;
}

/// Direct, shell-free Kerf process runner with bounded time and output.
#[derive(Debug)]
pub struct ProcessKerfRunner {
    executable: PathBuf,
    deadline: Duration,
    output_limit: u64,
    pending: Option<PendingProcess>,
}

impl ProcessKerfRunner {
    /// Creates a runner for the system Kerf executable.
    #[must_use]
    pub fn system(deadline: Duration, output_limit: u64) -> Self {
        Self::new("kerf", deadline, output_limit)
    }

    /// Creates a runner for an explicit Kerf-compatible executable.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, deadline: Duration, output_limit: u64) -> Self {
        Self {
            executable: executable.into(),
            deadline,
            output_limit,
            pending: None,
        }
    }

    fn reap_pending(&mut self) -> Result<(), KerfRunError> {
        let Some(pending) = &mut self.pending else {
            return Ok(());
        };
        if pending
            .child
            .try_wait()
            .map_err(KerfRunError::Inspect)?
            .is_none()
        {
            return Err(KerfRunError::PreviousProcessBlocked);
        }
        self.pending = None;
        Ok(())
    }

    fn spawn(&self, arguments: &[OsString]) -> Result<PendingProcess, KerfRunError> {
        let stdout = tempfile().map_err(KerfRunError::Output)?;
        let stderr = tempfile().map_err(KerfRunError::Output)?;
        let child = Command::new(&self.executable)
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                stdout.try_clone().map_err(KerfRunError::Output)?,
            ))
            .stderr(Stdio::from(
                stderr.try_clone().map_err(KerfRunError::Output)?,
            ))
            .spawn()
            .map_err(KerfRunError::Spawn)?;
        Ok(PendingProcess {
            child,
            stdout,
            stderr,
        })
    }
}

impl KerfRunner for ProcessKerfRunner {
    type Error = KerfRunError;

    fn run(&mut self, invocation: &KerfInvocation) -> Result<KerfRunResult, Self::Error> {
        self.reap_pending()?;
        let mut process = self.spawn(&invocation.arguments)?;
        let started = Instant::now();
        loop {
            if let Some(status) = process.child.try_wait().map_err(KerfRunError::Inspect)? {
                return collect_result(
                    status,
                    &mut process.stdout,
                    &mut process.stderr,
                    self.output_limit,
                );
            }
            if started.elapsed() >= self.deadline {
                let _ = process.child.kill();
                if process
                    .child
                    .try_wait()
                    .map_err(KerfRunError::Inspect)?
                    .is_none()
                {
                    self.pending = Some(process);
                    return Ok(KerfRunResult {
                        termination: KerfTermination::TimedOut,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    });
                }
                return collect_timeout_result(
                    &mut process.stdout,
                    &mut process.stderr,
                    self.output_limit,
                );
            }
            thread::sleep(POLL_INTERVAL.min(self.deadline));
        }
    }
}

#[derive(Debug)]
struct PendingProcess {
    child: Child,
    stdout: File,
    stderr: File,
}

/// Failure to start or supervise Kerf.
#[derive(Debug)]
pub enum KerfRunError {
    Spawn(io::Error),
    Inspect(io::Error),
    Output(io::Error),
    OutputTooLarge,
    PreviousProcessBlocked,
}

impl fmt::Display for KerfRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "failed to start Kerf: {error}"),
            Self::Inspect(error) => write!(formatter, "failed to inspect Kerf: {error}"),
            Self::Output(error) => write!(formatter, "failed to read Kerf output: {error}"),
            Self::OutputTooLarge => formatter.write_str("Kerf output exceeded its limit"),
            Self::PreviousProcessBlocked => {
                formatter.write_str("previous Kerf process is still blocked")
            }
        }
    }
}

impl std::error::Error for KerfRunError {}

/// Reconciled result of one attempted lifecycle mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LifecycleOutcome {
    pub state: OperationState,
    pub snapshot: HostSnapshot,
    pub process: Option<KerfRunResult>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Coordinates preflight, Kerf execution, and authoritative reconciliation.
#[derive(Debug)]
pub struct LifecycleExecutor<R, S> {
    runner: R,
    snapshots: S,
}

impl<R, S> LifecycleExecutor<R, S>
where
    R: KerfRunner,
    S: SnapshotRefresher,
{
    /// Creates an executor over replaceable process and snapshot backends.
    #[must_use]
    pub fn new(runner: R, snapshots: S) -> Self {
        Self { runner, snapshots }
    }

    /// Refreshes host state without attempting a mutation.
    ///
    /// # Errors
    ///
    /// Returns the snapshot backend error when no state can be produced.
    pub fn refresh_snapshot(&mut self) -> Result<HostSnapshot, S::Error> {
        self.snapshots.refresh_snapshot()
    }

    /// Executes and reconciles one mutation.
    ///
    /// # Errors
    ///
    /// Rejects requests that cannot pass preflight or whose authoritative
    /// state cannot be read. Once Kerf is attempted, a postflight refresh is
    /// performed regardless of process outcome.
    pub fn execute(
        &mut self,
        request: &LifecycleRequest,
    ) -> Result<LifecycleOutcome, LifecycleExecutionError<S::Error>> {
        let before = self
            .snapshots
            .refresh_snapshot()
            .map_err(LifecycleExecutionError::Inventory)?;
        if before.health != SnapshotHealth::Healthy {
            return Err(LifecycleExecutionError::SnapshotIndeterminate);
        }
        let invocation = plan(request, &before).map_err(LifecycleExecutionError::Plan)?;
        let process = self.runner.run(&invocation);
        let after = self
            .snapshots
            .refresh_snapshot()
            .map_err(LifecycleExecutionError::Inventory)?;
        Ok(reconcile(&invocation, process, after))
    }
}

/// Failure before a trustworthy lifecycle outcome can be produced.
#[derive(Debug)]
pub enum LifecycleExecutionError<E> {
    Inventory(E),
    SnapshotIndeterminate,
    Plan(LifecyclePlanError),
}

impl<E> LifecycleExecutionError<E> {
    /// Maps the failure to the public API error contract.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Inventory(_) | Self::SnapshotIndeterminate => ErrorCode::BackendUnavailable,
            Self::Plan(error) => error.error_code(),
        }
    }
}

fn reconcile<E>(
    invocation: &KerfInvocation,
    process: Result<KerfRunResult, E>,
    snapshot: HostSnapshot,
) -> LifecycleOutcome {
    let observed = expected_state_observed(&invocation.expected_state, &snapshot);
    let process_succeeded = process.as_ref().is_ok_and(KerfRunResult::process_succeeded);
    let (state, diagnostics) = if snapshot.health != SnapshotHealth::Healthy {
        (OperationState::Indeterminate, snapshot.diagnostics.clone())
    } else if !invocation.mutates_kernel && process_succeeded {
        (OperationState::Succeeded, Vec::new())
    } else if invocation.mutates_kernel && observed {
        let diagnostics = (!process_succeeded)
            .then(|| Diagnostic {
                code: "kerf_result_reconciled".into(),
                severity: DiagnosticSeverity::Warning,
                message:
                    "kernel state reached the requested result despite the Kerf process result"
                        .into(),
                detail: None,
                redacted: true,
            })
            .into_iter()
            .collect();
        (OperationState::Succeeded, diagnostics)
    } else if invocation.mutates_kernel {
        (
            OperationState::Failed,
            vec![Diagnostic {
                code: "expected_state_not_observed".into(),
                severity: DiagnosticSeverity::Error,
                message: "kernel state did not reach the requested result".into(),
                detail: None,
                redacted: true,
            }],
        )
    } else {
        (
            OperationState::Failed,
            vec![Diagnostic {
                code: "kerf_command_failed".into(),
                severity: DiagnosticSeverity::Error,
                message: "Kerf did not accept the requested validation".into(),
                detail: None,
                redacted: true,
            }],
        )
    };
    LifecycleOutcome {
        state,
        snapshot,
        process: process.ok(),
        diagnostics,
    }
}

fn expected_state_observed(expected: &ExpectedState, snapshot: &HostSnapshot) -> bool {
    match expected {
        ExpectedState::Instance(id, state) => snapshot
            .instances
            .iter()
            .any(|instance| instance.id == *id && instance.state == *state),
        ExpectedState::InstanceResources {
            id,
            state,
            name,
            cpu_hardware_ids,
            memory_bytes,
        } => snapshot.instances.iter().any(|instance| {
            instance.id == *id
                && instance.state == *state
                && name
                    .as_ref()
                    .is_none_or(|expected| instance.name == *expected)
                && cpu_hardware_ids.as_ref().is_none_or(|expected| {
                    expected
                        .iter()
                        .copied()
                        .collect::<std::collections::BTreeSet<_>>()
                        == instance
                            .resources
                            .cpu_hardware_ids
                            .iter()
                            .copied()
                            .collect()
                })
                && memory_bytes.is_none_or(|expected| instance.resources.memory_bytes == expected)
        }),
        ExpectedState::ResourcePool {
            cpu_hardware_ids,
            memory_bytes,
        } => {
            let observed_cpus = snapshot
                .resource_pool
                .cpu_hardware_ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            let expected_cpus = cpu_hardware_ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            let observed_memory = snapshot
                .resource_pool
                .memory_regions
                .iter()
                .map(|region| region.bytes)
                .sum::<u64>();
            observed_cpus == expected_cpus && observed_memory == *memory_bytes
        }
        ExpectedState::Absent(id) => snapshot.instances.iter().all(|instance| instance.id != *id),
    }
}

fn collect_result(
    status: ExitStatus,
    stdout: &mut File,
    stderr: &mut File,
    limit: u64,
) -> Result<KerfRunResult, KerfRunError> {
    Ok(KerfRunResult {
        termination: status
            .code()
            .map_or(KerfTermination::Signalled, KerfTermination::Exited),
        stdout: read_bounded(stdout, limit)?,
        stderr: read_bounded(stderr, limit)?,
    })
}

fn collect_timeout_result(
    stdout: &mut File,
    stderr: &mut File,
    limit: u64,
) -> Result<KerfRunResult, KerfRunError> {
    Ok(KerfRunResult {
        termination: KerfTermination::TimedOut,
        stdout: read_bounded(stdout, limit)?,
        stderr: read_bounded(stderr, limit)?,
    })
}

fn read_bounded(file: &mut File, limit: u64) -> Result<Vec<u8>, KerfRunError> {
    file.rewind().map_err(KerfRunError::Output)?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(KerfRunError::Output)?;
    if bytes.len() as u64 > limit {
        return Err(KerfRunError::OutputTooLarge);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, convert::Infallible};

    use kernmux_api::v1::{
        CpuTopology, Generation, HostMemory, Instance, InstanceId, InstanceState, KernelImage,
        KernelInfo, ResourceAllocation, ResourcePool,
    };

    use super::*;
    use crate::lifecycle::{CreateRequest, InstanceRequest, LifecycleRequest, UpdateRequest};

    struct SequenceSnapshots {
        values: VecDeque<Result<HostSnapshot, Infallible>>,
    }

    impl SnapshotRefresher for SequenceSnapshots {
        type Error = Infallible;

        fn refresh_snapshot(&mut self) -> Result<HostSnapshot, Self::Error> {
            self.values
                .pop_front()
                .expect("test snapshot sequence must not be empty")
        }
    }

    struct FakeRunner {
        result: Option<Result<KerfRunResult, ()>>,
        calls: usize,
    }

    impl KerfRunner for FakeRunner {
        type Error = ();

        fn run(&mut self, _invocation: &KerfInvocation) -> Result<KerfRunResult, Self::Error> {
            self.calls += 1;
            self.result.take().expect("runner must only be called once")
        }
    }

    fn snapshot(generation: u64, state: Option<InstanceState>) -> HostSnapshot {
        HostSnapshot {
            generation: Generation(generation),
            health: SnapshotHealth::Healthy,
            diagnostics: Vec::new(),
            kernel: KernelInfo {
                release: "7.0.0-mk".into(),
                multikernel_enabled: true,
            },
            capabilities: Vec::new(),
            topology: CpuTopology {
                architecture: "x86_64".into(),
                cpus: Vec::new(),
                numa_nodes: Vec::new(),
            },
            memory: HostMemory {
                total_bytes: 0,
                host_reserved_bytes: 0,
                assignable_bytes: 0,
                assigned_bytes: 0,
            },
            resource_pool: ResourcePool {
                cpu_hardware_ids: vec![4, 5, 6],
                available_cpu_hardware_ids: vec![4, 5, 6],
                memory_regions: Vec::new(),
            },
            instances: state
                .map(|state| Instance {
                    id: InstanceId(1),
                    name: "lab".into(),
                    generation: Generation(generation),
                    state,
                    resources: ResourceAllocation::default(),
                    image: KernelImage::default(),
                })
                .into_iter()
                .collect(),
            transactions: Vec::new(),
            operations: Vec::new(),
        }
    }

    fn create_request() -> LifecycleRequest {
        LifecycleRequest::Create(CreateRequest {
            expected_generation: Generation(1),
            id: InstanceId(1),
            name: "lab".into(),
            cpu_hardware_ids: vec![4],
            memory_bytes: 1_073_741_824,
        })
    }

    fn process(exit_code: i32) -> KerfRunResult {
        KerfRunResult {
            termination: KerfTermination::Exited(exit_code),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn shared_refreshers_consume_one_generation_history() {
        let mut first = SharedSnapshotRefresher::new(SequenceSnapshots {
            values: VecDeque::from([
                Ok(snapshot(7, None)),
                Ok(snapshot(8, Some(InstanceState::Ready))),
            ]),
        });
        let mut second = first.clone();

        assert_eq!(first.refresh_snapshot().unwrap().generation, Generation(7));
        assert_eq!(second.refresh_snapshot().unwrap().generation, Generation(8));
    }

    #[test]
    fn trusts_observed_state_over_nonzero_exit() {
        let runner = FakeRunner {
            result: Some(Ok(process(1))),
            calls: 0,
        };
        let mut after = snapshot(2, Some(InstanceState::Ready));
        after.instances[0].resources.cpu_hardware_ids = vec![4];
        after.instances[0].resources.memory_bytes = 1_073_741_824;
        let snapshots = SequenceSnapshots {
            values: VecDeque::from([Ok(snapshot(1, None)), Ok(after)]),
        };
        let mut executor = LifecycleExecutor::new(runner, snapshots);

        let outcome = executor.execute(&create_request()).unwrap();

        assert_eq!(outcome.state, OperationState::Succeeded);
        assert_eq!(outcome.diagnostics[0].code, "kerf_result_reconciled");
        assert_eq!(outcome.snapshot.generation, Generation(2));
    }

    #[test]
    fn rejects_zero_exit_when_expected_state_is_missing() {
        let runner = FakeRunner {
            result: Some(Ok(process(0))),
            calls: 0,
        };
        let snapshots = SequenceSnapshots {
            values: VecDeque::from([Ok(snapshot(1, None)), Ok(snapshot(1, None))]),
        };
        let mut executor = LifecycleExecutor::new(runner, snapshots);

        let outcome = executor.execute(&create_request()).unwrap();

        assert_eq!(outcome.state, OperationState::Failed);
        assert_eq!(outcome.diagnostics[0].code, "expected_state_not_observed");
    }

    #[test]
    fn update_requires_observed_resource_changes() {
        let runner = FakeRunner {
            result: Some(Ok(process(1))),
            calls: 0,
        };
        let mut before = snapshot(1, Some(InstanceState::Ready));
        before.instances[0].resources.cpu_hardware_ids = vec![4];
        before.instances[0].resources.memory_bytes = 1024;
        let snapshots = SequenceSnapshots {
            values: VecDeque::from([Ok(before.clone()), Ok(before)]),
        };
        let request = LifecycleRequest::Update(UpdateRequest {
            instance: InstanceRequest {
                expected_generation: Generation(1),
                id: InstanceId(1),
            },
            cpu_hardware_ids: Some(vec![4, 5]),
            memory_bytes: Some(2048),
            dry_run: false,
        });
        let mut executor = LifecycleExecutor::new(runner, snapshots);

        let outcome = executor.execute(&request).unwrap();

        assert_eq!(outcome.state, OperationState::Failed);
        assert_eq!(outcome.diagnostics[0].code, "expected_state_not_observed");
    }

    #[test]
    fn marks_outcome_indeterminate_when_postflight_is_stale() {
        let runner = FakeRunner {
            result: Some(Err(())),
            calls: 0,
        };
        let mut stale = snapshot(1, None);
        stale.health = SnapshotHealth::Indeterminate;
        stale.diagnostics.push(Diagnostic {
            code: "inventory_unavailable".into(),
            severity: DiagnosticSeverity::Error,
            message: "host inventory refresh did not complete".into(),
            detail: None,
            redacted: true,
        });
        let snapshots = SequenceSnapshots {
            values: VecDeque::from([Ok(snapshot(1, None)), Ok(stale)]),
        };
        let mut executor = LifecycleExecutor::new(runner, snapshots);

        let outcome = executor.execute(&create_request()).unwrap();

        assert_eq!(outcome.state, OperationState::Indeterminate);
        assert_eq!(outcome.diagnostics[0].code, "inventory_unavailable");
        assert!(outcome.process.is_none());
    }

    #[test]
    fn process_runner_enforces_deadline() {
        let invocation = KerfInvocation {
            arguments: vec!["-c".into(), "sleep 5".into()],
            expected_state: ExpectedState::Absent(InstanceId(1)),
            mutates_kernel: true,
        };
        let mut runner = ProcessKerfRunner::new("/bin/sh", Duration::from_millis(25), 4096);
        let started = Instant::now();

        let result = runner.run(&invocation).unwrap();

        assert_eq!(result.termination, KerfTermination::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
