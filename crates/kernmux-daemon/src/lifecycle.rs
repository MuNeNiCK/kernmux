//! Typed, shell-free Kerf lifecycle command planning.

use std::{collections::BTreeSet, ffi::OsString, fmt, path::PathBuf};

use kernmux_api::v1::{ErrorCode, Generation, HostSnapshot, Instance, InstanceId, InstanceState};

use crate::placement::{CpuPlacementError, validate_instance_cpus};

/// A lifecycle mutation accepted by the host service.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecycleRequest {
    Create(CreateRequest),
    Update(UpdateRequest),
    Load(LoadRequest),
    Start(InstanceRequest),
    Stop(StopRequest),
    Unload(InstanceRequest),
    Delete(InstanceRequest),
}

/// Common optimistic-concurrency fields for an instance mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstanceRequest {
    pub expected_generation: Generation,
    pub id: InstanceId,
}

/// Creates one peer-kernel resource domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateRequest {
    pub expected_generation: Generation,
    pub id: InstanceId,
    pub name: String,
    pub cpu_hardware_ids: Vec<u32>,
    pub memory_bytes: u64,
}

/// Replaces resources assigned to a ready instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateRequest {
    pub instance: InstanceRequest,
    pub cpu_hardware_ids: Option<Vec<u32>>,
    pub memory_bytes: Option<u64>,
    pub device_ids: Option<Vec<String>>,
    pub dry_run: bool,
}

/// Loads a kernel image into a ready instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadRequest {
    pub instance: InstanceRequest,
    pub kernel: PathBuf,
    pub initrd: Option<PathBuf>,
    pub cmdline: Option<String>,
}

/// Stops an active peer kernel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopRequest {
    pub instance: InstanceRequest,
    pub force: bool,
}

/// Expected authoritative state after a planned mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpectedState {
    Instance(InstanceId, InstanceState),
    InstanceResources {
        id: InstanceId,
        state: InstanceState,
        name: Option<String>,
        cpu_hardware_ids: Option<Vec<u32>>,
        memory_bytes: Option<u64>,
        device_ids: Option<Vec<String>>,
    },
    ResourcePool {
        cpu_hardware_ids: Vec<u32>,
        memory_bytes: u64,
    },
    Absent(InstanceId),
}

/// Executable Kerf argv and the state used to reconcile its result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KerfInvocation {
    pub arguments: Vec<OsString>,
    pub expected_state: ExpectedState,
    pub mutates_kernel: bool,
}

/// Validates a lifecycle request against an authoritative snapshot.
///
/// # Errors
///
/// Returns a stable planning error for stale generations, invalid values, or
/// incompatible lifecycle state.
pub fn plan(
    request: &LifecycleRequest,
    snapshot: &HostSnapshot,
) -> Result<KerfInvocation, LifecyclePlanError> {
    let expected_generation = match request {
        LifecycleRequest::Create(request) => request.expected_generation,
        LifecycleRequest::Update(request) => request.instance.expected_generation,
        LifecycleRequest::Load(request) => request.instance.expected_generation,
        LifecycleRequest::Start(request)
        | LifecycleRequest::Unload(request)
        | LifecycleRequest::Delete(request) => request.expected_generation,
        LifecycleRequest::Stop(request) => request.instance.expected_generation,
    };
    if expected_generation != snapshot.generation {
        return Err(LifecyclePlanError::StaleGeneration {
            expected: expected_generation,
            actual: snapshot.generation,
        });
    }

    match request {
        LifecycleRequest::Create(request) => plan_create(request, snapshot),
        LifecycleRequest::Update(request) => plan_update(request, snapshot),
        LifecycleRequest::Load(request) => plan_load(request, snapshot),
        LifecycleRequest::Start(request) => plan_state_command(
            "exec",
            *request,
            snapshot,
            InstanceState::Loaded,
            InstanceState::Active,
        ),
        LifecycleRequest::Stop(request) => plan_stop(request, snapshot),
        LifecycleRequest::Unload(request) => plan_state_command(
            "unload",
            *request,
            snapshot,
            InstanceState::Loaded,
            InstanceState::Ready,
        ),
        LifecycleRequest::Delete(request) => plan_state_command(
            "delete",
            *request,
            snapshot,
            InstanceState::Ready,
            InstanceState::Absent,
        ),
    }
}

fn plan_create(
    request: &CreateRequest,
    snapshot: &HostSnapshot,
) -> Result<KerfInvocation, LifecyclePlanError> {
    validate_name(&request.name)?;
    validate_id(request.id)?;
    validate_cpus(&request.cpu_hardware_ids)?;
    validate_instance_cpus(snapshot, &request.cpu_hardware_ids, &[])
        .map_err(LifecyclePlanError::CpuPlacement)?;
    if request.memory_bytes == 0 {
        return Err(LifecyclePlanError::InvalidRequest(
            "memory allocation must be greater than zero",
        ));
    }
    if snapshot
        .instances
        .iter()
        .any(|instance| instance.id == request.id || instance.name == request.name)
    {
        return Err(LifecyclePlanError::AlreadyExists);
    }
    validate_create_memory(snapshot, request.memory_bytes)
        .map_err(LifecyclePlanError::MemoryPlacement)?;

    Ok(KerfInvocation {
        arguments: vec![
            "create".into(),
            request.name.clone().into(),
            option("--id=", request.id.0),
            option("--cpus=", cpu_list(&request.cpu_hardware_ids)),
            option("--memory=", request.memory_bytes),
        ],
        expected_state: ExpectedState::InstanceResources {
            id: request.id,
            state: InstanceState::Ready,
            name: Some(request.name.clone()),
            cpu_hardware_ids: Some(request.cpu_hardware_ids.clone()),
            memory_bytes: Some(request.memory_bytes),
            device_ids: None,
        },
        mutates_kernel: true,
    })
}

fn plan_update(
    request: &UpdateRequest,
    snapshot: &HostSnapshot,
) -> Result<KerfInvocation, LifecyclePlanError> {
    let instance = require_instance(snapshot, request.instance.id, InstanceState::Ready)?;
    validate_name(&instance.name)?;
    if request.cpu_hardware_ids.is_none()
        && request.memory_bytes.is_none()
        && request.device_ids.is_none()
    {
        return Err(LifecyclePlanError::InvalidRequest(
            "an update must specify CPU, memory, or device resources",
        ));
    }
    let mut arguments = vec!["update".into(), instance.name.clone().into()];
    if let Some(cpus) = &request.cpu_hardware_ids {
        validate_cpus(cpus)?;
        validate_instance_cpus(snapshot, cpus, &instance.resources.cpu_hardware_ids)
            .map_err(LifecyclePlanError::CpuPlacement)?;
        arguments.push(option("--cpus=", cpu_list(cpus)));
    }
    if let Some(memory) = request.memory_bytes {
        if memory == 0 {
            return Err(LifecyclePlanError::InvalidRequest(
                "memory allocation must be greater than zero",
            ));
        }
        validate_update_memory(snapshot, instance, memory)
            .map_err(LifecyclePlanError::MemoryPlacement)?;
        arguments.push(option("--memory=", memory));
    }
    if let Some(devices) = &request.device_ids {
        validate_update_devices(snapshot, instance, devices)
            .map_err(LifecyclePlanError::DevicePlacement)?;
        arguments.push(option("--devices=", devices.join(",")));
    }
    if request.dry_run {
        arguments.push("--dry-run".into());
    }
    Ok(KerfInvocation {
        arguments,
        expected_state: ExpectedState::InstanceResources {
            id: request.instance.id,
            state: InstanceState::Ready,
            name: None,
            cpu_hardware_ids: request.cpu_hardware_ids.clone(),
            memory_bytes: request.memory_bytes,
            device_ids: request.device_ids.clone(),
        },
        mutates_kernel: !request.dry_run,
    })
}

fn plan_load(
    request: &LoadRequest,
    snapshot: &HostSnapshot,
) -> Result<KerfInvocation, LifecyclePlanError> {
    require_state(snapshot, request.instance.id, InstanceState::Ready)?;
    if request.kernel.as_os_str().is_empty() {
        return Err(LifecyclePlanError::InvalidRequest(
            "kernel path must not be empty",
        ));
    }
    let mut arguments = vec![
        "load".into(),
        id_option(request.instance.id),
        path_option("--kernel=", &request.kernel),
    ];
    if let Some(initrd) = &request.initrd {
        if initrd.as_os_str().is_empty() {
            return Err(LifecyclePlanError::InvalidRequest(
                "initrd path must not be empty",
            ));
        }
        arguments.push(path_option("--initrd=", initrd));
    }
    if let Some(cmdline) = &request.cmdline {
        arguments.push(option("--cmdline=", cmdline));
    }
    Ok(KerfInvocation {
        arguments,
        expected_state: ExpectedState::Instance(request.instance.id, InstanceState::Loaded),
        mutates_kernel: true,
    })
}

fn plan_stop(
    request: &StopRequest,
    snapshot: &HostSnapshot,
) -> Result<KerfInvocation, LifecyclePlanError> {
    require_state(snapshot, request.instance.id, InstanceState::Active)?;
    let mut arguments = vec!["kill".into(), id_option(request.instance.id)];
    if request.force {
        arguments.push("--force".into());
    }
    Ok(KerfInvocation {
        arguments,
        expected_state: ExpectedState::Instance(request.instance.id, InstanceState::Loaded),
        mutates_kernel: true,
    })
}

fn plan_state_command(
    command: &str,
    request: InstanceRequest,
    snapshot: &HostSnapshot,
    required: InstanceState,
    expected: InstanceState,
) -> Result<KerfInvocation, LifecyclePlanError> {
    require_state(snapshot, request.id, required)?;
    let expected_state = if expected == InstanceState::Absent {
        ExpectedState::Absent(request.id)
    } else {
        ExpectedState::Instance(request.id, expected)
    };
    Ok(KerfInvocation {
        arguments: vec![command.into(), id_option(request.id)],
        expected_state,
        mutates_kernel: true,
    })
}

fn require_state(
    snapshot: &HostSnapshot,
    id: InstanceId,
    required: InstanceState,
) -> Result<(), LifecyclePlanError> {
    require_instance(snapshot, id, required).map(|_| ())
}

fn require_instance(
    snapshot: &HostSnapshot,
    id: InstanceId,
    required: InstanceState,
) -> Result<&kernmux_api::v1::Instance, LifecyclePlanError> {
    validate_id(id)?;
    let instance = snapshot
        .instances
        .iter()
        .find(|instance| instance.id == id)
        .ok_or(LifecyclePlanError::NotFound)?;
    if instance.state != required {
        return Err(LifecyclePlanError::InvalidState {
            required,
            actual: instance.state,
        });
    }
    Ok(instance)
}

fn validate_id(id: InstanceId) -> Result<(), LifecyclePlanError> {
    if !(1..=511).contains(&id.0) {
        return Err(LifecyclePlanError::InvalidRequest(
            "instance ID must be between 1 and 511",
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), LifecyclePlanError> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && name.as_bytes()[0].is_ascii_alphanumeric();
    if !valid {
        return Err(LifecyclePlanError::InvalidRequest(
            "instance name contains unsupported characters",
        ));
    }
    Ok(())
}

fn validate_cpus(cpus: &[u32]) -> Result<(), LifecyclePlanError> {
    if cpus.is_empty() || cpus.iter().copied().collect::<BTreeSet<_>>().len() != cpus.len() {
        return Err(LifecyclePlanError::InvalidRequest(
            "CPU allocation must be non-empty and contain no duplicates",
        ));
    }
    Ok(())
}

fn validate_create_memory(
    snapshot: &HostSnapshot,
    requested_bytes: u64,
) -> Result<(), MemoryPlacementError> {
    let pool_bytes = checked_sum(
        snapshot
            .resource_pool
            .memory_regions
            .iter()
            .map(|region| region.bytes),
    )?;
    let assigned_bytes = checked_sum(
        snapshot
            .instances
            .iter()
            .map(|instance| instance.resources.memory_bytes),
    )?;
    let available_bytes = pool_bytes.saturating_sub(assigned_bytes);
    if requested_bytes > available_bytes {
        return Err(MemoryPlacementError::InsufficientAvailable {
            requested_bytes,
            available_bytes,
        });
    }
    if !snapshot
        .resource_pool
        .memory_regions
        .iter()
        .any(|region| region.bytes >= requested_bytes)
    {
        return Err(MemoryPlacementError::NoContiguousPoolChunk { requested_bytes });
    }
    Ok(())
}

fn validate_update_memory(
    snapshot: &HostSnapshot,
    instance: &Instance,
    requested_bytes: u64,
) -> Result<(), MemoryPlacementError> {
    if requested_bytes <= instance.resources.memory_bytes {
        return Ok(());
    }
    let base = instance
        .resources
        .memory_base
        .ok_or(MemoryPlacementError::MissingInstanceBase { id: instance.id })?;
    let requested_end = base
        .checked_add(requested_bytes)
        .ok_or(MemoryPlacementError::AddressOverflow)?;
    let remains_in_chunk = snapshot.resource_pool.memory_regions.iter().any(|region| {
        region
            .base
            .checked_add(region.bytes)
            .is_some_and(|chunk_end| region.base <= base && requested_end <= chunk_end)
    });
    if !remains_in_chunk {
        return Err(MemoryPlacementError::LeavesPoolChunk {
            base,
            requested_bytes,
        });
    }
    for peer in snapshot
        .instances
        .iter()
        .filter(|peer| peer.id != instance.id)
    {
        let Some(peer_base) = peer.resources.memory_base else {
            if peer.resources.memory_bytes == 0 {
                continue;
            }
            return Err(MemoryPlacementError::MissingInstanceBase { id: peer.id });
        };
        let peer_end = peer_base
            .checked_add(peer.resources.memory_bytes)
            .ok_or(MemoryPlacementError::AddressOverflow)?;
        if base < peer_end && peer_base < requested_end {
            return Err(MemoryPlacementError::OverlapsInstance {
                peer_name: peer.name.clone(),
            });
        }
    }
    Ok(())
}

fn checked_sum(mut values: impl Iterator<Item = u64>) -> Result<u64, MemoryPlacementError> {
    values.try_fold(0_u64, |total, value| {
        total
            .checked_add(value)
            .ok_or(MemoryPlacementError::AddressOverflow)
    })
}

fn validate_update_devices(
    snapshot: &HostSnapshot,
    instance: &Instance,
    requested: &[String],
) -> Result<(), DevicePlacementError> {
    if requested.is_empty() {
        return Err(DevicePlacementError::EmptySelection);
    }
    let requested_set = requested.iter().cloned().collect::<BTreeSet<_>>();
    if requested_set.len() != requested.len() {
        return Err(DevicePlacementError::Duplicate);
    }
    if let Some(pci_id) = requested.iter().find(|pci_id| !is_canonical_pci_id(pci_id)) {
        return Err(DevicePlacementError::InvalidPciId((*pci_id).clone()));
    }
    for peer in snapshot
        .instances
        .iter()
        .filter(|peer| peer.id != instance.id)
    {
        if let Some(pci_id) = peer
            .resources
            .device_ids
            .iter()
            .find(|pci_id| requested_set.contains(*pci_id))
        {
            return Err(DevicePlacementError::OwnedByPeer {
                pci_id: pci_id.clone(),
                peer_name: peer.name.clone(),
            });
        }
    }
    let managed = snapshot
        .resource_pool
        .devices
        .iter()
        .map(|device| (device.pci_id.as_str(), device))
        .collect::<std::collections::BTreeMap<_, _>>();
    let available = snapshot
        .resource_pool
        .available_device_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let owned = instance
        .resources
        .device_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    for pci_id in requested {
        let device = managed
            .get(pci_id.as_str())
            .ok_or_else(|| DevicePlacementError::NotManaged(pci_id.clone()))?;
        if !available.contains(pci_id.as_str()) && !owned.contains(pci_id.as_str()) {
            return Err(DevicePlacementError::Unavailable(pci_id.clone()));
        }
        let group = device
            .iommu_group
            .ok_or_else(|| DevicePlacementError::MissingIommuGroup(pci_id.clone()))?;
        let missing = device
            .iommu_group_members
            .iter()
            .filter(|member| {
                !requested_set.contains(*member) || !managed.contains_key(member.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(DevicePlacementError::IncompleteIommuGroup {
                pci_id: pci_id.clone(),
                group,
                missing,
            });
        }
    }
    Ok(())
}

fn is_canonical_pci_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 12
        && bytes[4] == b':'
        && bytes[7] == b':'
        && bytes[10] == b'.'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10) || byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)
        })
}

/// A device replacement that cannot be applied safely.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DevicePlacementError {
    EmptySelection,
    InvalidPciId(String),
    Duplicate,
    NotManaged(String),
    Unavailable(String),
    OwnedByPeer {
        pci_id: String,
        peer_name: String,
    },
    MissingIommuGroup(String),
    IncompleteIommuGroup {
        pci_id: String,
        group: u32,
        missing: Vec<String>,
    },
}

impl DevicePlacementError {
    const fn error_code(&self) -> ErrorCode {
        match self {
            Self::EmptySelection | Self::InvalidPciId(_) | Self::Duplicate => {
                ErrorCode::InvalidRequest
            }
            Self::NotManaged(_)
            | Self::Unavailable(_)
            | Self::OwnedByPeer { .. }
            | Self::MissingIommuGroup(_)
            | Self::IncompleteIommuGroup { .. } => ErrorCode::Conflict,
        }
    }
}

impl fmt::Display for DevicePlacementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySelection => formatter.write_str(
                "an empty device replacement is unsupported; delete the owner or request a non-empty replacement",
            ),
            Self::InvalidPciId(value) => write!(
                formatter,
                "PCI device ID '{value}' is not a canonical lowercase domain:bus:slot.function address"
            ),
            Self::Duplicate => formatter.write_str("device replacement contains duplicate PCI IDs"),
            Self::NotManaged(pci_id) => {
                write!(formatter, "PCI device {pci_id} is not managed by the Multikernel pool")
            }
            Self::Unavailable(pci_id) => {
                write!(formatter, "PCI device {pci_id} is not available to this instance")
            }
            Self::OwnedByPeer { pci_id, peer_name } => {
                write!(formatter, "PCI device {pci_id} is owned by instance '{peer_name}'")
            }
            Self::MissingIommuGroup(pci_id) => {
                write!(formatter, "PCI device {pci_id} has no authoritative IOMMU group")
            }
            Self::IncompleteIommuGroup {
                pci_id,
                group,
                missing,
            } => write!(
                formatter,
                "PCI device {pci_id} requires every member of IOMMU group {group}; missing {}",
                missing.join(",")
            ),
        }
    }
}

impl std::error::Error for DevicePlacementError {}

/// A memory request that cannot be placed safely in authoritative pool state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MemoryPlacementError {
    InsufficientAvailable {
        requested_bytes: u64,
        available_bytes: u64,
    },
    NoContiguousPoolChunk {
        requested_bytes: u64,
    },
    MissingInstanceBase {
        id: InstanceId,
    },
    LeavesPoolChunk {
        base: u64,
        requested_bytes: u64,
    },
    OverlapsInstance {
        peer_name: String,
    },
    AddressOverflow,
}

impl fmt::Display for MemoryPlacementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsufficientAvailable {
                requested_bytes,
                available_bytes,
            } => write!(
                formatter,
                "requested {requested_bytes} bytes, but only {available_bytes} pool bytes are unassigned"
            ),
            Self::NoContiguousPoolChunk { requested_bytes } => write!(
                formatter,
                "requested {requested_bytes} bytes do not fit in one contiguous pool chunk"
            ),
            Self::MissingInstanceBase { id } => write!(
                formatter,
                "instance {} has no authoritative memory base; refresh inventory before growing it",
                id.0
            ),
            Self::LeavesPoolChunk {
                base,
                requested_bytes,
            } => write!(
                formatter,
                "memory range at {base:#x} with {requested_bytes} bytes leaves its current pool chunk"
            ),
            Self::OverlapsInstance { peer_name } => write!(
                formatter,
                "requested memory range overlaps instance '{peer_name}'"
            ),
            Self::AddressOverflow => {
                formatter.write_str("memory placement address or capacity overflows u64")
            }
        }
    }
}

impl std::error::Error for MemoryPlacementError {}

fn cpu_list(cpus: &[u32]) -> String {
    cpus.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

fn id_option(id: InstanceId) -> OsString {
    option("--id=", id.0)
}

fn option(prefix: &str, value: impl fmt::Display) -> OsString {
    format!("{prefix}{value}").into()
}

fn path_option(prefix: &str, value: &std::path::Path) -> OsString {
    let mut option = OsString::from(prefix);
    option.push(value);
    option
}

/// Stable category for a rejected lifecycle plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LifecyclePlanError {
    StaleGeneration {
        expected: Generation,
        actual: Generation,
    },
    InvalidRequest(&'static str),
    CpuPlacement(CpuPlacementError),
    MemoryPlacement(MemoryPlacementError),
    DevicePlacement(DevicePlacementError),
    NotFound,
    AlreadyExists,
    InvalidState {
        required: InstanceState,
        actual: InstanceState,
    },
}

impl LifecyclePlanError {
    /// Maps the rejection to the public API error contract.
    #[must_use]
    pub const fn error_code(&self) -> ErrorCode {
        match self {
            Self::StaleGeneration { .. } => ErrorCode::PreconditionFailed,
            Self::InvalidRequest(_) => ErrorCode::InvalidRequest,
            Self::NotFound => ErrorCode::NotFound,
            Self::DevicePlacement(error) => error.error_code(),
            Self::CpuPlacement(_)
            | Self::MemoryPlacement(_)
            | Self::AlreadyExists
            | Self::InvalidState { .. } => ErrorCode::Conflict,
        }
    }
}

impl fmt::Display for LifecyclePlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleGeneration { expected, actual } => write!(
                formatter,
                "snapshot generation changed from {} to {}",
                expected.0, actual.0
            ),
            Self::InvalidRequest(message) => formatter.write_str(message),
            Self::CpuPlacement(error) => write!(formatter, "CPU placement was rejected: {error}"),
            Self::MemoryPlacement(error) => {
                write!(formatter, "memory placement was rejected: {error}")
            }
            Self::DevicePlacement(error) => {
                write!(formatter, "device placement was rejected: {error}")
            }
            Self::NotFound => formatter.write_str("instance was not found"),
            Self::AlreadyExists => formatter.write_str("instance ID or name already exists"),
            Self::InvalidState { required, actual } => {
                write!(
                    formatter,
                    "instance must be {required:?}, but is {actual:?}"
                )
            }
        }
    }
}

impl std::error::Error for LifecyclePlanError {}

#[cfg(test)]
mod tests {
    use kernmux_api::v1::{
        CpuTopology, HostMemory, KernelImage, KernelInfo, MemoryRegion, PciDevice,
        ResourceAllocation, ResourcePool, SnapshotHealth,
    };

    use super::*;

    fn snapshot(state: Option<InstanceState>) -> HostSnapshot {
        HostSnapshot {
            generation: Generation(7),
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
                total_bytes: 3_221_225_472,
                host_reserved_bytes: 0,
                assignable_bytes: 3_221_225_472,
                assigned_bytes: u64::from(state.is_some()) * 1_073_741_824,
            },
            resource_pool: ResourcePool {
                cpu_hardware_ids: vec![4, 5, 6],
                available_cpu_hardware_ids: vec![4, 5, 6],
                memory_regions: vec![MemoryRegion {
                    base: 0x1_0000_0000,
                    bytes: 3_221_225_472,
                    numa_node: 0,
                }],
                devices: Vec::new(),
                available_device_ids: Vec::new(),
            },
            instances: state
                .map(|state| kernmux_api::v1::Instance {
                    id: InstanceId(1),
                    name: "lab".into(),
                    generation: Generation(7),
                    state,
                    resources: ResourceAllocation {
                        memory_base: Some(0x1_0000_b000),
                        memory_bytes: 1_073_741_824,
                        ..ResourceAllocation::default()
                    },
                    image: KernelImage {
                        present: matches!(state, InstanceState::Loaded | InstanceState::Active),
                    },
                })
                .into_iter()
                .collect(),
            transactions: Vec::new(),
            operations: Vec::new(),
        }
    }

    fn instance_request() -> InstanceRequest {
        InstanceRequest {
            expected_generation: Generation(7),
            id: InstanceId(1),
        }
    }

    fn pci_device(pci_id: &str, group: Option<u32>, members: &[&str]) -> PciDevice {
        PciDevice {
            pci_id: pci_id.into(),
            pool_name: pci_id.replace([':', '.'], "_"),
            vendor_id: Some(0x1af4),
            device_id: Some(0x1044),
            iommu_group: group,
            iommu_group_members: members.iter().map(|member| (*member).into()).collect(),
        }
    }

    fn arguments(invocation: &KerfInvocation) -> Vec<String> {
        invocation
            .arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn plans_create_and_update_without_a_shell() {
        let create = plan(
            &LifecycleRequest::Create(CreateRequest {
                expected_generation: Generation(7),
                id: InstanceId(1),
                name: "lab".into(),
                cpu_hardware_ids: vec![4, 5],
                memory_bytes: 1_073_741_824,
            }),
            &snapshot(None),
        )
        .unwrap();
        assert_eq!(
            arguments(&create),
            [
                "create",
                "lab",
                "--id=1",
                "--cpus=4,5",
                "--memory=1073741824"
            ]
        );
        assert!(matches!(
            create.expected_state,
            ExpectedState::InstanceResources { .. }
        ));

        let update = plan(
            &LifecycleRequest::Update(UpdateRequest {
                instance: instance_request(),
                cpu_hardware_ids: Some(vec![4, 5, 6]),
                memory_bytes: Some(1_610_612_736),
                device_ids: None,
                dry_run: true,
            }),
            &snapshot(Some(InstanceState::Ready)),
        )
        .unwrap();
        assert_eq!(
            arguments(&update),
            [
                "update",
                "lab",
                "--cpus=4,5,6",
                "--memory=1610612736",
                "--dry-run"
            ]
        );
        assert!(!update.mutates_kernel);
        assert!(matches!(
            update.expected_state,
            ExpectedState::InstanceResources { .. }
        ));
    }

    #[test]
    fn plans_image_and_state_transitions() {
        let load = plan(
            &LifecycleRequest::Load(LoadRequest {
                instance: instance_request(),
                kernel: "/boot/vmlinux".into(),
                initrd: Some("/boot/initramfs".into()),
                cmdline: Some("kerf.entrypoint=/bin/sh console=mktty0".into()),
            }),
            &snapshot(Some(InstanceState::Ready)),
        )
        .unwrap();
        assert_eq!(
            arguments(&load),
            [
                "load",
                "--id=1",
                "--kernel=/boot/vmlinux",
                "--initrd=/boot/initramfs",
                "--cmdline=kerf.entrypoint=/bin/sh console=mktty0"
            ]
        );

        let cases = [
            (
                LifecycleRequest::Start(instance_request()),
                InstanceState::Loaded,
                vec!["exec", "--id=1"],
                ExpectedState::Instance(InstanceId(1), InstanceState::Active),
            ),
            (
                LifecycleRequest::Stop(StopRequest {
                    instance: instance_request(),
                    force: true,
                }),
                InstanceState::Active,
                vec!["kill", "--id=1", "--force"],
                ExpectedState::Instance(InstanceId(1), InstanceState::Loaded),
            ),
            (
                LifecycleRequest::Unload(instance_request()),
                InstanceState::Loaded,
                vec!["unload", "--id=1"],
                ExpectedState::Instance(InstanceId(1), InstanceState::Ready),
            ),
            (
                LifecycleRequest::Delete(instance_request()),
                InstanceState::Ready,
                vec!["delete", "--id=1"],
                ExpectedState::Absent(InstanceId(1)),
            ),
        ];
        for (request, state, expected_arguments, expected_state) in cases {
            let invocation = plan(&request, &snapshot(Some(state))).unwrap();
            assert_eq!(arguments(&invocation), expected_arguments);
            assert_eq!(invocation.expected_state, expected_state);
        }
    }

    #[test]
    fn plans_an_isolated_available_device_replacement() {
        let mut host = snapshot(Some(InstanceState::Ready));
        host.resource_pool.devices = vec![pci_device("0000:06:00.0", Some(14), &["0000:06:00.0"])];
        host.resource_pool.available_device_ids = vec!["0000:06:00.0".into()];

        let invocation = plan(
            &LifecycleRequest::Update(UpdateRequest {
                instance: instance_request(),
                cpu_hardware_ids: None,
                memory_bytes: None,
                device_ids: Some(vec!["0000:06:00.0".into()]),
                dry_run: false,
            }),
            &host,
        )
        .unwrap();

        assert_eq!(
            arguments(&invocation),
            ["update", "lab", "--devices=0000:06:00.0"]
        );
    }

    #[test]
    fn rejects_peer_owned_and_incomplete_iommu_groups() {
        let mut host = snapshot(Some(InstanceState::Ready));
        host.resource_pool.devices = vec![pci_device("0000:06:00.0", Some(14), &["0000:06:00.0"])];
        host.instances.push(kernmux_api::v1::Instance {
            id: InstanceId(2),
            name: "beta".into(),
            generation: Generation(7),
            state: InstanceState::Ready,
            resources: ResourceAllocation {
                device_ids: vec!["0000:06:00.0".into()],
                ..ResourceAllocation::default()
            },
            image: KernelImage::default(),
        });
        let request = UpdateRequest {
            instance: instance_request(),
            cpu_hardware_ids: None,
            memory_bytes: None,
            device_ids: Some(vec!["0000:06:00.0".into()]),
            dry_run: false,
        };

        assert!(matches!(
            plan(&LifecycleRequest::Update(request.clone()), &host),
            Err(LifecyclePlanError::DevicePlacement(
                DevicePlacementError::OwnedByPeer { peer_name, .. }
            )) if peer_name == "beta"
        ));

        host.instances.pop();
        host.resource_pool.available_device_ids = vec!["0000:06:00.0".into()];
        host.resource_pool.devices[0].iommu_group_members = vec![
            "0000:00:1f.0".into(),
            "0000:00:1f.2".into(),
            "0000:06:00.0".into(),
        ];
        assert!(matches!(
            plan(&LifecycleRequest::Update(request), &host),
            Err(LifecyclePlanError::DevicePlacement(
                DevicePlacementError::IncompleteIommuGroup { group: 14, .. }
            ))
        ));
    }

    #[test]
    fn rejects_ambiguous_or_unverifiable_device_replacements() {
        let mut host = snapshot(Some(InstanceState::Ready));
        host.resource_pool.devices = vec![pci_device("0000:06:00.0", None, &[])];
        host.resource_pool.available_device_ids = vec!["0000:06:00.0".into()];
        let error = |device_ids| {
            plan(
                &LifecycleRequest::Update(UpdateRequest {
                    instance: instance_request(),
                    cpu_hardware_ids: None,
                    memory_bytes: None,
                    device_ids: Some(device_ids),
                    dry_run: false,
                }),
                &host,
            )
            .unwrap_err()
        };

        assert_eq!(error(Vec::new()).error_code(), ErrorCode::InvalidRequest);
        assert_eq!(
            error(vec!["0000:06:00.0".into(), "0000:06:00.0".into()]).error_code(),
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            error(vec!["0000:06:00.A".into()]).error_code(),
            ErrorCode::InvalidRequest
        );
        assert!(matches!(
            error(vec!["0000:07:00.0".into()]),
            LifecyclePlanError::DevicePlacement(DevicePlacementError::NotManaged(_))
        ));
        assert!(matches!(
            error(vec!["0000:06:00.0".into()]),
            LifecyclePlanError::DevicePlacement(DevicePlacementError::MissingIommuGroup(_))
        ));
    }

    #[test]
    fn rejects_stale_unsafe_and_state_incompatible_requests() {
        let stale = LifecycleRequest::Start(InstanceRequest {
            expected_generation: Generation(6),
            id: InstanceId(1),
        });
        assert_eq!(
            plan(&stale, &snapshot(Some(InstanceState::Loaded)))
                .unwrap_err()
                .error_code(),
            ErrorCode::PreconditionFailed
        );

        let unsafe_name = LifecycleRequest::Create(CreateRequest {
            expected_generation: Generation(7),
            id: InstanceId(1),
            name: "--debug".into(),
            cpu_hardware_ids: vec![4],
            memory_bytes: 1024,
        });
        assert_eq!(
            plan(&unsafe_name, &snapshot(None))
                .unwrap_err()
                .error_code(),
            ErrorCode::InvalidRequest
        );

        let wrong_state = LifecycleRequest::Unload(instance_request());
        assert!(matches!(
            plan(&wrong_state, &snapshot(Some(InstanceState::Active))),
            Err(LifecyclePlanError::InvalidState { .. })
        ));
    }

    #[test]
    fn rejects_growth_into_a_peer_instance() {
        const GIB: u64 = 1 << 30;
        let mut host = snapshot(Some(InstanceState::Ready));
        let base = host.instances[0].resources.memory_base.unwrap();
        host.instances[0].resources.memory_bytes = GIB + GIB / 2;
        host.instances.push(kernmux_api::v1::Instance {
            id: InstanceId(2),
            name: "beta".into(),
            generation: Generation(7),
            state: InstanceState::Ready,
            resources: ResourceAllocation {
                memory_base: Some(base + GIB + GIB / 2),
                memory_bytes: GIB / 2,
                ..ResourceAllocation::default()
            },
            image: KernelImage::default(),
        });

        let error = plan(
            &LifecycleRequest::Update(UpdateRequest {
                instance: instance_request(),
                cpu_hardware_ids: None,
                memory_bytes: Some(2 * GIB),
                device_ids: None,
                dry_run: false,
            }),
            &host,
        )
        .unwrap_err();

        assert_eq!(error.error_code(), ErrorCode::Conflict);
        assert!(matches!(
            error,
            LifecyclePlanError::MemoryPlacement(MemoryPlacementError::OverlapsInstance {
                peer_name
            }) if peer_name == "beta"
        ));

        host.instances[1].resources.memory_base = None;
        assert!(matches!(
            plan(
                &LifecycleRequest::Update(UpdateRequest {
                    instance: instance_request(),
                    cpu_hardware_ids: None,
                    memory_bytes: Some(2 * GIB),
                    device_ids: None,
                    dry_run: false,
                }),
                &host,
            ),
            Err(LifecyclePlanError::MemoryPlacement(
                MemoryPlacementError::MissingInstanceBase { id: InstanceId(2) }
            ))
        ));
    }

    #[test]
    fn accepts_safe_memory_shrink_and_same_chunk_growth() {
        const GIB: u64 = 1 << 30;
        let host = snapshot(Some(InstanceState::Ready));

        for requested in [GIB / 2, GIB + GIB / 2] {
            let invocation = plan(
                &LifecycleRequest::Update(UpdateRequest {
                    instance: instance_request(),
                    cpu_hardware_ids: None,
                    memory_bytes: Some(requested),
                    device_ids: None,
                    dry_run: true,
                }),
                &host,
            )
            .unwrap();
            assert!(!invocation.mutates_kernel);
        }
    }

    #[test]
    fn rejects_growth_across_pool_chunks_and_address_overflow() {
        const GIB: u64 = 1 << 30;
        let mut host = snapshot(Some(InstanceState::Ready));
        let base = host.instances[0].resources.memory_base.unwrap();
        host.instances[0].resources.memory_bytes = GIB / 2;
        host.resource_pool.memory_regions = vec![
            MemoryRegion {
                base: base - 0xb000,
                bytes: GIB,
                numa_node: 0,
            },
            MemoryRegion {
                base: 0x4_0000_0000,
                bytes: GIB,
                numa_node: 1,
            },
        ];
        let request = |memory_bytes| {
            LifecycleRequest::Update(UpdateRequest {
                instance: instance_request(),
                cpu_hardware_ids: None,
                memory_bytes: Some(memory_bytes),
                device_ids: None,
                dry_run: false,
            })
        };

        assert!(matches!(
            plan(&request(GIB + GIB / 2), &host),
            Err(LifecyclePlanError::MemoryPlacement(
                MemoryPlacementError::LeavesPoolChunk { .. }
            ))
        ));

        host.instances[0].resources.memory_base = Some(u64::MAX - 8);
        host.instances[0].resources.memory_bytes = 1;
        assert!(matches!(
            plan(&request(16), &host),
            Err(LifecyclePlanError::MemoryPlacement(
                MemoryPlacementError::AddressOverflow
            ))
        ));
    }

    #[test]
    fn rejects_unplaceable_create_memory() {
        const GIB: u64 = 1 << 30;
        let mut host = snapshot(None);
        host.resource_pool.memory_regions = vec![
            MemoryRegion {
                base: 0x1_0000_0000,
                bytes: GIB,
                numa_node: 0,
            },
            MemoryRegion {
                base: 0x4_0000_0000,
                bytes: GIB,
                numa_node: 1,
            },
        ];
        let request = |memory_bytes| {
            LifecycleRequest::Create(CreateRequest {
                expected_generation: Generation(7),
                id: InstanceId(2),
                name: "beta".into(),
                cpu_hardware_ids: vec![4],
                memory_bytes,
            })
        };

        assert!(matches!(
            plan(&request(GIB + GIB / 2), &host),
            Err(LifecyclePlanError::MemoryPlacement(
                MemoryPlacementError::NoContiguousPoolChunk { .. }
            ))
        ));
        assert!(matches!(
            plan(&request(3 * GIB), &host),
            Err(LifecyclePlanError::MemoryPlacement(
                MemoryPlacementError::InsufficientAvailable { .. }
            ))
        ));
    }
}
