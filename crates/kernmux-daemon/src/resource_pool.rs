//! Resource-pool planning and partial-application reconciliation.

use std::{collections::BTreeSet, ffi::OsString};

use kernmux_api::v1::{
    Diagnostic, DiagnosticSeverity, ErrorCode, Generation, HostSnapshot, OperationState,
    SnapshotHealth,
};

use crate::{
    lifecycle::{ExpectedState, KerfInvocation},
    lifecycle_executor::{KerfRunResult, KerfRunner, SnapshotRefresher},
    placement::{CpuPlacementError, validate_pool_cpus},
};

/// Desired Multikernel CPU and memory pool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourcePoolRequest {
    pub expected_generation: Generation,
    pub cpu_hardware_ids: Vec<u32>,
    pub memory_bytes: u64,
}

/// Exact before, requested, and observed resource-pool transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourcePoolTransition {
    pub before_cpu_hardware_ids: Vec<u32>,
    pub requested_cpu_hardware_ids: Vec<u32>,
    pub observed_cpu_hardware_ids: Vec<u32>,
    pub added_cpu_hardware_ids: Vec<u32>,
    pub returned_cpu_hardware_ids: Vec<u32>,
    pub before_memory_bytes: u64,
    pub requested_memory_bytes: u64,
    pub observed_memory_bytes: u64,
}

impl ResourcePoolTransition {
    /// Whether the observed pool exactly matches the request.
    #[must_use]
    pub fn request_reached(&self) -> bool {
        self.requested_cpu_hardware_ids == self.observed_cpu_hardware_ids
            && self.requested_memory_bytes == self.observed_memory_bytes
    }

    /// Whether some resources changed despite an unmet request.
    #[must_use]
    pub fn partially_applied(&self) -> bool {
        !self.request_reached()
            && (self.before_cpu_hardware_ids != self.observed_cpu_hardware_ids
                || self.before_memory_bytes != self.observed_memory_bytes)
    }
}

/// Reconciled result of one resource-pool request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourcePoolOutcome {
    pub state: OperationState,
    pub snapshot: HostSnapshot,
    pub process: Option<KerfRunResult>,
    pub transition: ResourcePoolTransition,
    pub recovery: ResourcePoolRecovery,
    pub diagnostics: Vec<Diagnostic>,
}

/// Safest next action after resource-pool reconciliation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourcePoolRecovery {
    None,
    RefreshRequired,
    RetryRequest,
    HostRestartRequired,
}

/// Coordinates Kerf pool changes with authoritative postflight inventory.
#[derive(Debug)]
pub struct ResourcePoolExecutor<R, S> {
    runner: R,
    snapshots: S,
}

impl<R, S> ResourcePoolExecutor<R, S>
where
    R: KerfRunner,
    S: SnapshotRefresher,
{
    /// Creates a resource-pool executor.
    #[must_use]
    pub fn new(runner: R, snapshots: S) -> Self {
        Self { runner, snapshots }
    }

    /// Executes and reconciles one pool request.
    ///
    /// # Errors
    ///
    /// Rejects unavailable inventory, stale generations, invalid requests,
    /// and requests that would exclude assigned instance resources.
    pub fn execute(
        &mut self,
        request: &ResourcePoolRequest,
    ) -> Result<ResourcePoolOutcome, ResourcePoolExecutionError<S::Error>> {
        let before = self
            .snapshots
            .refresh_snapshot()
            .map_err(ResourcePoolExecutionError::Inventory)?;
        let invocation = plan(request, &before).map_err(ResourcePoolExecutionError::Plan)?;
        let process = self.runner.run(&invocation);
        let after = self
            .snapshots
            .refresh_snapshot()
            .map_err(ResourcePoolExecutionError::Inventory)?;
        Ok(reconcile(request, &before, after, process))
    }
}

/// Stable rejection before a pool request is attempted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResourcePoolPlanError {
    SnapshotIndeterminate,
    StaleGeneration {
        expected: Generation,
        actual: Generation,
    },
    DuplicateCpu,
    CpuPlacement(CpuPlacementError),
    AssignedResourcesExcluded,
}

impl ResourcePoolPlanError {
    /// Maps the rejection to the public API error contract.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::SnapshotIndeterminate => ErrorCode::BackendUnavailable,
            Self::StaleGeneration { .. } => ErrorCode::PreconditionFailed,
            Self::DuplicateCpu | Self::CpuPlacement(_) => ErrorCode::InvalidRequest,
            Self::AssignedResourcesExcluded => ErrorCode::Conflict,
        }
    }
}

/// Failure before a trustworthy pool outcome can be produced.
#[derive(Debug)]
pub enum ResourcePoolExecutionError<E> {
    Inventory(E),
    Plan(ResourcePoolPlanError),
}

impl<E> ResourcePoolExecutionError<E> {
    /// Maps the failure to the public API error contract.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::Inventory(_) => ErrorCode::BackendUnavailable,
            Self::Plan(error) => error.error_code(),
        }
    }
}

fn plan(
    request: &ResourcePoolRequest,
    snapshot: &HostSnapshot,
) -> Result<KerfInvocation, ResourcePoolPlanError> {
    if snapshot.health != SnapshotHealth::Healthy {
        return Err(ResourcePoolPlanError::SnapshotIndeterminate);
    }
    if request.expected_generation != snapshot.generation {
        return Err(ResourcePoolPlanError::StaleGeneration {
            expected: request.expected_generation,
            actual: snapshot.generation,
        });
    }
    let requested_cpus = request
        .cpu_hardware_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if requested_cpus.len() != request.cpu_hardware_ids.len() {
        return Err(ResourcePoolPlanError::DuplicateCpu);
    }
    validate_pool_cpus(snapshot, &request.cpu_hardware_ids)
        .map_err(ResourcePoolPlanError::CpuPlacement)?;
    let assigned_memory = snapshot
        .instances
        .iter()
        .map(|instance| instance.resources.memory_bytes)
        .sum::<u64>();
    let excludes_cpu = snapshot.instances.iter().any(|instance| {
        instance
            .resources
            .cpu_hardware_ids
            .iter()
            .any(|cpu| !requested_cpus.contains(cpu))
    });
    if excludes_cpu || assigned_memory > request.memory_bytes {
        return Err(ResourcePoolPlanError::AssignedResourcesExcluded);
    }

    let mut cpus = request.cpu_hardware_ids.clone();
    cpus.sort_unstable();
    let cpu_argument = if cpus.is_empty() {
        OsString::from("--cpus=none")
    } else {
        format!(
            "--cpus={}",
            cpus.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
        .into()
    };
    let memory_argument = if request.memory_bytes == 0 {
        OsString::from("--memory=none")
    } else {
        format!("--memory={}", request.memory_bytes).into()
    };
    Ok(KerfInvocation {
        arguments: vec!["init".into(), cpu_argument, memory_argument],
        expected_state: ExpectedState::ResourcePool {
            cpu_hardware_ids: cpus,
            memory_bytes: request.memory_bytes,
        },
        mutates_kernel: true,
    })
}

fn reconcile<E>(
    request: &ResourcePoolRequest,
    before: &HostSnapshot,
    after: HostSnapshot,
    process: Result<KerfRunResult, E>,
) -> ResourcePoolOutcome {
    let transition = transition(request, before, &after);
    let (state, recovery, diagnostics) = if after.health != SnapshotHealth::Healthy {
        (
            OperationState::Indeterminate,
            ResourcePoolRecovery::RefreshRequired,
            after.diagnostics.clone(),
        )
    } else if transition.request_reached() {
        let diagnostics = (!matches!(
            process.as_ref(),
            Ok(result) if result.process_succeeded()
        ))
        .then(|| Diagnostic {
            code: "kerf_result_reconciled".into(),
            severity: DiagnosticSeverity::Warning,
            message: "resource pool reached the request despite the Kerf process result".into(),
            detail: None,
            redacted: true,
        })
        .into_iter()
        .collect();
        (
            OperationState::Succeeded,
            ResourcePoolRecovery::None,
            diagnostics,
        )
    } else {
        let partial = transition.partially_applied();
        let recovery = if partial
            && request.cpu_hardware_ids.is_empty()
            && request.memory_bytes == 0
            && transition.observed_cpu_hardware_ids.is_empty()
            && transition.observed_memory_bytes != 0
        {
            ResourcePoolRecovery::HostRestartRequired
        } else {
            ResourcePoolRecovery::RetryRequest
        };
        (
            OperationState::Failed,
            recovery,
            vec![Diagnostic {
                code: if partial {
                    "resource_pool_partially_applied"
                } else {
                    "resource_pool_unchanged"
                }
                .into(),
                severity: DiagnosticSeverity::Error,
                message: if partial {
                    "resource pool changed but did not reach the requested result"
                } else {
                    "resource pool did not reach the requested result"
                }
                .into(),
                detail: None,
                redacted: true,
            }],
        )
    };
    ResourcePoolOutcome {
        state,
        snapshot: after,
        process: process.ok(),
        transition,
        recovery,
        diagnostics,
    }
}

fn transition(
    request: &ResourcePoolRequest,
    before: &HostSnapshot,
    after: &HostSnapshot,
) -> ResourcePoolTransition {
    let before_cpus = cpu_set(before);
    let observed_cpus = cpu_set(after);
    let requested_cpus = request
        .cpu_hardware_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    ResourcePoolTransition {
        before_cpu_hardware_ids: before_cpus.iter().copied().collect(),
        requested_cpu_hardware_ids: requested_cpus.iter().copied().collect(),
        observed_cpu_hardware_ids: observed_cpus.iter().copied().collect(),
        added_cpu_hardware_ids: observed_cpus.difference(&before_cpus).copied().collect(),
        returned_cpu_hardware_ids: before_cpus.difference(&observed_cpus).copied().collect(),
        before_memory_bytes: pool_memory(before),
        requested_memory_bytes: request.memory_bytes,
        observed_memory_bytes: pool_memory(after),
    }
}

fn cpu_set(snapshot: &HostSnapshot) -> BTreeSet<u32> {
    snapshot
        .resource_pool
        .cpu_hardware_ids
        .iter()
        .copied()
        .collect()
}

fn pool_memory(snapshot: &HostSnapshot) -> u64 {
    snapshot
        .resource_pool
        .memory_regions
        .iter()
        .map(|region| region.bytes)
        .sum()
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, convert::Infallible};

    use kernmux_api::v1::{
        Cpu, CpuTopology, HostMemory, Instance, InstanceId, InstanceState, KernelImage, KernelInfo,
        MemoryRegion, ResourceAllocation, ResourcePool,
    };

    use super::*;
    use crate::lifecycle_executor::{KerfTermination, SnapshotRefresher};

    struct SequenceSnapshots {
        snapshots: VecDeque<Result<HostSnapshot, Infallible>>,
    }

    impl SnapshotRefresher for SequenceSnapshots {
        type Error = Infallible;

        fn refresh_snapshot(&mut self) -> Result<HostSnapshot, Self::Error> {
            self.snapshots.pop_front().expect("snapshot must exist")
        }
    }

    struct FixedRunner(KerfRunResult);

    impl KerfRunner for FixedRunner {
        type Error = Infallible;

        fn run(&mut self, _invocation: &KerfInvocation) -> Result<KerfRunResult, Self::Error> {
            Ok(self.0.clone())
        }
    }

    fn snapshot(cpus: Vec<u32>, memory_bytes: u64) -> HostSnapshot {
        HostSnapshot {
            generation: Generation(1),
            health: SnapshotHealth::Healthy,
            diagnostics: Vec::new(),
            kernel: KernelInfo {
                release: "7.0.0-mk".into(),
                multikernel_enabled: true,
            },
            capabilities: Vec::new(),
            topology: CpuTopology {
                architecture: "x86_64".into(),
                cpus: (4..=7)
                    .map(|hardware_id| Cpu {
                        logical_id: hardware_id,
                        hardware_id,
                        package_id: 0,
                        core_id: hardware_id,
                        thread_index: 0,
                        numa_node: 0,
                        online: true,
                    })
                    .collect(),
                numa_nodes: Vec::new(),
            },
            memory: HostMemory {
                total_bytes: memory_bytes,
                host_reserved_bytes: 0,
                assignable_bytes: memory_bytes,
                assigned_bytes: 0,
            },
            resource_pool: ResourcePool {
                cpu_hardware_ids: cpus.clone(),
                available_cpu_hardware_ids: cpus,
                memory_regions: (memory_bytes != 0)
                    .then_some(MemoryRegion {
                        base: 0x3_ee20_0000,
                        bytes: memory_bytes,
                        numa_node: 0,
                    })
                    .into_iter()
                    .collect(),
            },
            instances: Vec::new(),
            transactions: Vec::new(),
            operations: Vec::new(),
        }
    }

    fn failed_process() -> KerfRunResult {
        KerfRunResult {
            termination: KerfTermination::Exited(1),
            stdout: Vec::new(),
            stderr: b"pool resize failed".to_vec(),
        }
    }

    #[test]
    fn reports_cpu_return_with_retained_memory_as_partial_application() {
        let before = snapshot(vec![4, 5, 6, 7], 2_147_483_648);
        let after = snapshot(Vec::new(), 2_147_483_648);
        let snapshots = SequenceSnapshots {
            snapshots: VecDeque::from([Ok(before), Ok(after)]),
        };
        let mut executor = ResourcePoolExecutor::new(FixedRunner(failed_process()), snapshots);
        let request = ResourcePoolRequest {
            expected_generation: Generation(1),
            cpu_hardware_ids: Vec::new(),
            memory_bytes: 0,
        };

        let outcome = executor.execute(&request).unwrap();

        assert_eq!(outcome.state, OperationState::Failed);
        assert!(outcome.transition.partially_applied());
        assert_eq!(outcome.transition.returned_cpu_hardware_ids, [4, 5, 6, 7]);
        assert_eq!(outcome.transition.observed_memory_bytes, 2_147_483_648);
        assert_eq!(outcome.recovery, ResourcePoolRecovery::HostRestartRequired);
        assert_eq!(
            outcome.diagnostics[0].code,
            "resource_pool_partially_applied"
        );
    }

    #[test]
    fn trusts_reached_pool_state_over_process_exit() {
        let before = snapshot(Vec::new(), 0);
        let after = snapshot(vec![4, 5], 1_073_741_824);
        let snapshots = SequenceSnapshots {
            snapshots: VecDeque::from([Ok(before), Ok(after)]),
        };
        let mut executor = ResourcePoolExecutor::new(FixedRunner(failed_process()), snapshots);
        let request = ResourcePoolRequest {
            expected_generation: Generation(1),
            cpu_hardware_ids: vec![5, 4],
            memory_bytes: 1_073_741_824,
        };

        let outcome = executor.execute(&request).unwrap();

        assert_eq!(outcome.state, OperationState::Succeeded);
        assert_eq!(outcome.recovery, ResourcePoolRecovery::None);
        assert_eq!(outcome.diagnostics[0].code, "kerf_result_reconciled");
    }

    #[test]
    fn refuses_to_exclude_assigned_instance_resources() {
        let mut current = snapshot(vec![4, 5], 1_073_741_824);
        current.instances.push(Instance {
            id: InstanceId(1),
            name: "lab".into(),
            generation: Generation(1),
            state: InstanceState::Ready,
            resources: ResourceAllocation {
                cpu_hardware_ids: vec![4],
                memory_base: None,
                memory_bytes: 536_870_912,
                memory_region: None,
                device_ids: Vec::new(),
            },
            image: KernelImage::default(),
        });
        let request = ResourcePoolRequest {
            expected_generation: Generation(1),
            cpu_hardware_ids: vec![5],
            memory_bytes: 1_073_741_824,
        };

        assert_eq!(
            plan(&request, &current).unwrap_err(),
            ResourcePoolPlanError::AssignedResourcesExcluded
        );
    }

    #[test]
    fn plans_canonical_shell_free_pool_arguments() {
        let current = snapshot(Vec::new(), 0);
        let initialize = plan(
            &ResourcePoolRequest {
                expected_generation: Generation(1),
                cpu_hardware_ids: vec![7, 4, 6, 5],
                memory_bytes: 2_147_483_648,
            },
            &current,
        )
        .unwrap();
        assert_eq!(
            initialize
                .arguments
                .iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>(),
            ["init", "--cpus=4,5,6,7", "--memory=2147483648"]
        );

        let release = plan(
            &ResourcePoolRequest {
                expected_generation: Generation(1),
                cpu_hardware_ids: Vec::new(),
                memory_bytes: 0,
            },
            &current,
        )
        .unwrap();
        assert_eq!(
            release
                .arguments
                .iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>(),
            ["init", "--cpus=none", "--memory=none"]
        );
    }
}
