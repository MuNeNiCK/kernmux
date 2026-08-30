//! Typed, shell-free Kerf lifecycle command planning.

use std::{collections::BTreeSet, ffi::OsString, fmt, path::PathBuf};

use kernmux_api::v1::{ErrorCode, Generation, HostSnapshot, InstanceId, InstanceState};

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
    if request.cpu_hardware_ids.is_none() && request.memory_bytes.is_none() {
        return Err(LifecyclePlanError::InvalidRequest(
            "an update must specify CPU or memory resources",
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
        arguments.push(option("--memory=", memory));
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
            Self::CpuPlacement(_) | Self::AlreadyExists | Self::InvalidState { .. } => {
                ErrorCode::Conflict
            }
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
        CpuTopology, HostMemory, KernelImage, KernelInfo, ResourceAllocation, ResourcePool,
        SnapshotHealth,
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
                .map(|state| kernmux_api::v1::Instance {
                    id: InstanceId(1),
                    name: "lab".into(),
                    generation: Generation(7),
                    state,
                    resources: ResourceAllocation::default(),
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
}
