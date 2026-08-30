//! Version 1 of the Kernmux host-management contract.

use serde::{Deserialize, Serialize};

/// Generation of an authoritative host snapshot.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Generation(pub u64);

/// Monotonic sequence number in the host event stream.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EventSequence(pub u64);

/// Stable identifier assigned to a managed kernel instance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct InstanceId(pub u32);

/// Stable identifier assigned to an asynchronous operation.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct OperationId(pub String);

/// Authoritative view of one Multikernel host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostSnapshot {
    pub generation: Generation,
    pub kernel: KernelInfo,
    pub capabilities: Vec<Capability>,
    pub topology: CpuTopology,
    pub memory: HostMemory,
    pub resource_pool: ResourcePool,
    pub instances: Vec<Instance>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transactions: Vec<Transaction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<Operation>,
}

/// Identity and compatibility information for the running control kernel.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KernelInfo {
    pub release: String,
    pub multikernel_enabled: bool,
}

/// Runtime capability advertised by the host service.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Multikernel,
    InstanceLifecycle,
    DynamicResources,
    TransactionRollback,
    Console,
    DeviceAssignment,
    SharedMemory,
    #[serde(other)]
    Unknown,
}

/// CPU and NUMA topology used for placement and conflict validation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CpuTopology {
    pub architecture: String,
    pub cpus: Vec<Cpu>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numa_nodes: Vec<NumaNode>,
}

/// One logical CPU and the hardware identifiers needed for assignment.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Cpu {
    pub logical_id: u32,
    pub hardware_id: u32,
    pub package_id: u32,
    pub core_id: u32,
    pub thread_index: u32,
    pub numa_node: u32,
    pub online: bool,
}

/// Memory and CPU membership of one NUMA node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NumaNode {
    pub id: u32,
    pub logical_cpu_ids: Vec<u32>,
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
}

/// Host-wide memory totals and the assignable Multikernel pool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostMemory {
    pub total_bytes: u64,
    pub host_reserved_bytes: u64,
    pub assignable_bytes: u64,
    pub assigned_bytes: u64,
}

/// CPU and memory resources delegated to peer kernels.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourcePool {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu_hardware_ids: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_cpu_hardware_ids: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_regions: Vec<MemoryRegion>,
}

/// One contiguous memory region delegated to the resource pool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryRegion {
    pub base: u64,
    pub bytes: u64,
    pub numa_node: u32,
}

/// One managed peer-kernel instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Instance {
    pub id: InstanceId,
    pub name: String,
    pub generation: Generation,
    pub state: InstanceState,
    pub resources: ResourceAllocation,
    #[serde(default)]
    pub image: KernelImage,
}

/// Lifecycle state observed from authoritative kernel state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    Absent,
    Ready,
    Loaded,
    Active,
    #[serde(other)]
    Unknown,
}

/// Resources assigned to an instance.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceAllocation {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu_hardware_ids: Vec<u32>,
    pub memory_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_region: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_ids: Vec<String>,
}

/// Kernel image state authoritatively reported for an instance.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct KernelImage {
    pub present: bool,
}

/// Generation precondition supplied with a mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MutationPrecondition {
    pub expected_generation: Generation,
}

/// One asynchronous host mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Operation {
    pub id: OperationId,
    pub kind: OperationKind,
    pub state: OperationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<u8>,
    pub expected_generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<Generation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affected_resources: Vec<ResourceReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Actor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_id: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// Requested class of host mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    InitializeResourcePool,
    ReleaseResourcePool,
    CreateInstance,
    UpdateInstance,
    LoadInstance,
    StartInstance,
    StopInstance,
    UnloadInstance,
    DeleteInstance,
    OpenConsole,
    #[serde(other)]
    Unknown,
}

/// Execution state of an asynchronous operation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Indeterminate,
    #[serde(other)]
    Unknown,
}

/// Resource affected by an operation or event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourceReference {
    pub kind: ResourceKind,
    pub id: String,
}

/// Kind of managed resource.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Host,
    ResourcePool,
    Instance,
    Device,
    Console,
    #[serde(other)]
    Unknown,
}

/// Local identity that requested a privileged operation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Actor {
    pub uid: u32,
    pub gid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Result of an atomic resource transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Transaction {
    pub id: String,
    pub state: TransactionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_before: Option<Generation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_after: Option<Generation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

/// State of a resource transaction.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionState {
    Planned,
    Applied,
    RolledBack,
    Failed,
    #[serde(other)]
    Unknown,
}

/// Diagnostic suitable for presentation after redaction by the service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default)]
    pub redacted: bool,
}

/// Severity of a diagnostic.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
    #[serde(other)]
    Unknown,
}

/// Event indicating that clients may need to refresh authoritative state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Event {
    pub sequence: EventSequence,
    pub snapshot_generation: Generation,
    pub kind: EventKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceReference>,
}

impl Event {
    /// Returns true when this event directly follows `previous` without a gap
    /// and does not move the snapshot generation backwards.
    #[must_use]
    pub const fn is_contiguous_after(&self, previous: &Self) -> bool {
        self.sequence.0 == previous.sequence.0.saturating_add(1)
            && self.snapshot_generation.0 >= previous.snapshot_generation.0
    }
}

/// Kind of state invalidation event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    SnapshotChanged,
    CapabilitiesChanged,
    InstanceChanged,
    OperationChanged,
    StreamOverflow,
    #[serde(other)]
    Unknown,
}

/// Stable error returned by the management API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_generation: Option<Generation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
}

/// Stable category of API failure.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    PreconditionFailed,
    Unsupported,
    BackendUnavailable,
    Timeout,
    Internal,
    #[serde(other)]
    Unknown,
}

/// Common response envelope for synchronous, asynchronous, and failed calls.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response<T> {
    Result { generation: Generation, data: T },
    Accepted { operation: Operation },
    Error { error: ApiError },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    fn round_trip<T>(value: &T)
    where
        T: Serialize + for<'de> Deserialize<'de> + Eq + std::fmt::Debug,
    {
        let json = serde_json::to_string(value).expect("value must serialize");
        let decoded = serde_json::from_str(&json).expect("value must deserialize");
        assert_eq!(value, &decoded);
    }

    fn operation(state: OperationState) -> Operation {
        Operation {
            id: OperationId("op-1".into()),
            kind: OperationKind::CreateInstance,
            state,
            progress_percent: Some(50),
            expected_generation: Generation(4),
            observed_generation: Some(Generation(5)),
            affected_resources: vec![ResourceReference {
                kind: ResourceKind::Instance,
                id: "lab".into(),
            }],
            error: None,
            actor: Some(Actor {
                uid: 1000,
                gid: 1000,
                label: None,
            }),
            audit_id: Some("audit-1".into()),
            created_at: "2026-08-30T00:00:00Z".into(),
            completed_at: None,
        }
    }

    #[test]
    fn all_lifecycle_states_round_trip() {
        for state in [
            InstanceState::Absent,
            InstanceState::Ready,
            InstanceState::Loaded,
            InstanceState::Active,
            InstanceState::Unknown,
        ] {
            round_trip(&state);
        }
    }

    #[test]
    fn all_operation_states_round_trip() {
        for state in [
            OperationState::Queued,
            OperationState::Running,
            OperationState::Succeeded,
            OperationState::Failed,
            OperationState::Cancelled,
            OperationState::Indeterminate,
            OperationState::Unknown,
        ] {
            round_trip(&state);
        }
    }

    #[test]
    fn response_variants_round_trip() {
        round_trip(&Response::Result {
            generation: Generation(5),
            data: InstanceState::Ready,
        });
        round_trip(&Response::<InstanceState>::Accepted {
            operation: operation(OperationState::Queued),
        });
        round_trip(&Response::<InstanceState>::Error {
            error: ApiError {
                code: ErrorCode::PreconditionFailed,
                message: "host state changed; refresh and retry".into(),
                retryable: true,
                current_generation: Some(Generation(6)),
                diagnostics: Vec::new(),
            },
        });
    }

    #[test]
    fn host_snapshot_and_transaction_round_trip() {
        let snapshot = HostSnapshot {
            generation: Generation(7),
            kernel: KernelInfo {
                release: "7.0.0-mk".into(),
                multikernel_enabled: true,
            },
            capabilities: vec![
                Capability::InstanceLifecycle,
                Capability::TransactionRollback,
                Capability::Console,
            ],
            topology: CpuTopology {
                architecture: "x86_64".into(),
                cpus: vec![Cpu {
                    logical_id: 4,
                    hardware_id: 4,
                    package_id: 0,
                    core_id: 2,
                    thread_index: 0,
                    numa_node: 0,
                    online: true,
                }],
                numa_nodes: vec![NumaNode {
                    id: 0,
                    logical_cpu_ids: vec![4],
                    total_memory_bytes: 2_147_483_648,
                    available_memory_bytes: 1_073_741_824,
                }],
            },
            memory: HostMemory {
                total_bytes: 17_179_869_184,
                host_reserved_bytes: 15_032_385_536,
                assignable_bytes: 2_147_483_648,
                assigned_bytes: 1_073_741_824,
            },
            resource_pool: ResourcePool {
                cpu_hardware_ids: vec![4],
                available_cpu_hardware_ids: Vec::new(),
                memory_regions: vec![MemoryRegion {
                    base: 0x4_0000_0000,
                    bytes: 2_147_483_648,
                    numa_node: 0,
                }],
            },
            instances: vec![Instance {
                id: InstanceId(1),
                name: "lab".into(),
                generation: Generation(3),
                state: InstanceState::Ready,
                resources: ResourceAllocation {
                    cpu_hardware_ids: vec![4],
                    memory_bytes: 1_073_741_824,
                    memory_region: Some("instance-memory-0".into()),
                    device_ids: Vec::new(),
                },
                image: KernelImage { present: false },
            }],
            transactions: Vec::new(),
            operations: Vec::new(),
        };
        round_trip(&snapshot);

        let transaction = Transaction {
            id: "transaction-1".into(),
            state: TransactionState::RolledBack,
            generation_before: Some(Generation(7)),
            generation_after: Some(Generation(8)),
            diagnostics: vec![Diagnostic {
                code: "placement_changed".into(),
                severity: DiagnosticSeverity::Warning,
                message: "requested placement was rolled back".into(),
                detail: None,
                redacted: false,
            }],
        };
        round_trip(&transaction);
    }

    #[test]
    fn unknown_additive_fields_are_ignored() {
        let state: Instance = serde_json::from_str(
            r#"{
                "id": 1,
                "name": "lab",
                "generation": 2,
                "state": "ready",
                "resources": {"memory_bytes": 0},
                "image": {"present": false},
                "future_field": {"enabled": true}
            }"#,
        )
        .expect("unknown fields must be tolerated");
        assert_eq!(state.state, InstanceState::Ready);
    }

    #[test]
    fn event_gaps_and_generation_regressions_require_refresh() {
        let first = Event {
            sequence: EventSequence(10),
            snapshot_generation: Generation(4),
            kind: EventKind::SnapshotChanged,
            resource: None,
        };
        let next = Event {
            sequence: EventSequence(11),
            snapshot_generation: Generation(5),
            kind: EventKind::InstanceChanged,
            resource: None,
        };
        assert!(next.is_contiguous_after(&first));

        let gap = Event {
            sequence: EventSequence(13),
            ..next.clone()
        };
        assert!(!gap.is_contiguous_after(&first));

        let regression = Event {
            sequence: EventSequence(11),
            snapshot_generation: Generation(3),
            ..next
        };
        assert!(!regression.is_contiguous_after(&first));
    }
}
