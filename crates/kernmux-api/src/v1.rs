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
    #[serde(default)]
    pub health: SnapshotHealth,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<Diagnostic>,
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

/// Confidence in the freshness of an authoritative host snapshot.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotHealth {
    #[default]
    Healthy,
    Indeterminate,
    #[serde(other)]
    Unknown,
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

/// Resources delegated to peer kernels.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourcePool {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu_hardware_ids: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_cpu_hardware_ids: Vec<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub memory_regions: Vec<MemoryRegion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<PciDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub available_device_ids: Vec<String>,
}

/// One PCI device delegated to the Multikernel resource hierarchy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PciDevice {
    pub pci_id: String,
    pub pool_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iommu_group: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub iommu_group_members: Vec<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_base: Option<u64>,
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

/// Semantic use of one immutable image artifact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageKind {
    Kernel,
    Initrd,
    #[serde(other)]
    Unknown,
}

/// One verified content-addressed image artifact.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImageArtifact {
    pub schema_version: u32,
    pub kind: ImageKind,
    pub id: String,
    pub bytes: u64,
}

/// Generation precondition supplied with a mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MutationPrecondition {
    pub expected_generation: Generation,
}

/// Replaces the Multikernel CPU and memory resource pool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourcePoolMutation {
    pub expected_generation: Generation,
    pub cpu_hardware_ids: Vec<u32>,
    pub memory_bytes: u64,
}

/// Creates one peer-kernel instance from resources already in the pool.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateInstanceMutation {
    pub expected_generation: Generation,
    pub id: InstanceId,
    pub name: String,
    pub cpu_hardware_ids: Vec<u32>,
    pub memory_bytes: u64,
}

/// Replaces selected resources of a ready instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct UpdateInstanceMutation {
    pub expected_generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_hardware_ids: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_ids: Option<Vec<String>>,
    #[serde(default)]
    pub dry_run: bool,
}

/// Loads a kernel and optional initrd into a ready instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LoadInstanceMutation {
    pub expected_generation: Generation,
    pub kernel_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initrd_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_line: Option<String>,
}

/// Imports one administrator-controlled file into immutable image storage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ImportImageMutation {
    pub expected_generation: Generation,
    pub kind: ImageKind,
    pub source_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_id: Option<String>,
}

/// Applies a lifecycle transition to an existing instance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstanceLifecycleMutation {
    pub expected_generation: Generation,
}

/// Stops an active instance, optionally requesting a forced transition.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StopInstanceMutation {
    pub expected_generation: Generation,
    #[serde(default)]
    pub force: bool,
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
    ImportImage,
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
    Image,
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

/// Console features negotiated when a client attaches to an instance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConsoleCapabilities {
    /// Console payloads are transported without text transcoding.
    pub binary: bool,
    /// Whether terminal dimensions can be forwarded to the instance.
    pub resize: bool,
}

/// Metadata returned before the binary console stream begins.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConsoleAttachment {
    pub instance_id: InstanceId,
    pub capabilities: ConsoleCapabilities,
    pub max_frame_bytes: u32,
}

/// Terminal dimensions requested by an interactive client.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConsoleSize {
    pub columns: u16,
    pub rows: u16,
}

/// Stable reason why a console stream became terminal.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleCloseReason {
    ClientDetached,
    EndOfStream,
    InstanceStopped,
    DeviceUnavailable,
    TransportError,
    #[serde(other)]
    Unknown,
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

/// Bounded page of events after a client-provided cursor.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventPage {
    pub events: Vec<Event>,
    pub overflowed: bool,
    pub latest_sequence: EventSequence,
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
    fn console_contract_round_trips_without_implying_resize_support() {
        round_trip(&ConsoleAttachment {
            instance_id: InstanceId(1),
            capabilities: ConsoleCapabilities {
                binary: true,
                resize: false,
            },
            max_frame_bytes: 65_536,
        });
        round_trip(&ConsoleSize {
            columns: 160,
            rows: 48,
        });
        for reason in [
            ConsoleCloseReason::ClientDetached,
            ConsoleCloseReason::EndOfStream,
            ConsoleCloseReason::InstanceStopped,
            ConsoleCloseReason::DeviceUnavailable,
            ConsoleCloseReason::TransportError,
            ConsoleCloseReason::Unknown,
        ] {
            round_trip(&reason);
        }
    }

    #[test]
    fn mutation_contracts_round_trip_with_generation_preconditions() {
        round_trip(&ResourcePoolMutation {
            expected_generation: Generation(4),
            cpu_hardware_ids: vec![4, 5],
            memory_bytes: 2_147_483_648,
        });
        round_trip(&CreateInstanceMutation {
            expected_generation: Generation(5),
            id: InstanceId(1),
            name: "lab".into(),
            cpu_hardware_ids: vec![4],
            memory_bytes: 1_073_741_824,
        });
        round_trip(&UpdateInstanceMutation {
            expected_generation: Generation(6),
            cpu_hardware_ids: Some(vec![4, 5]),
            memory_bytes: None,
            device_ids: None,
            dry_run: false,
        });
        round_trip(&LoadInstanceMutation {
            expected_generation: Generation(7),
            kernel_path: "/var/lib/kernmux/images/vmlinux".into(),
            initrd_path: Some("/var/lib/kernmux/images/initrd".into()),
            command_line: Some("console=mktty0".into()),
        });
        round_trip(&StopInstanceMutation {
            expected_generation: Generation(8),
            force: false,
        });
        round_trip(&ImportImageMutation {
            expected_generation: Generation(9),
            kind: ImageKind::Kernel,
            source_path: "/var/lib/kernmux/import/vmlinux".into(),
            expected_id: Some(format!("sha256:{}", "a".repeat(64))),
        });
        round_trip(&ImageArtifact {
            schema_version: 1,
            kind: ImageKind::Initrd,
            id: format!("sha256:{}", "b".repeat(64)),
            bytes: 4096,
        });
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
            health: SnapshotHealth::Healthy,
            diagnostics: Vec::new(),
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
                devices: Vec::new(),
                available_device_ids: Vec::new(),
            },
            instances: vec![Instance {
                id: InstanceId(1),
                name: "lab".into(),
                generation: Generation(3),
                state: InstanceState::Ready,
                resources: ResourceAllocation {
                    cpu_hardware_ids: vec![4],
                    memory_base: Some(0x4_0000_a000),
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
    fn resource_allocation_accepts_payload_without_memory_base() {
        let allocation: ResourceAllocation = serde_json::from_value(serde_json::json!({
            "memory_bytes": 1_073_741_824
        }))
        .unwrap();

        assert_eq!(allocation.memory_base, None);
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
