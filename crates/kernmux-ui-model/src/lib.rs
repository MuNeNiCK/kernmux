//! Renderer- and transport-independent state for Kernmux management clients.

use kernmux_api::v1::{
    Generation, HostSnapshot, ImageArtifact, ImageKind, InstanceId, InstanceState, OperationId,
    OperationState,
};

/// Parses a compact hardware CPU list such as `4-7,10,12`.
///
/// # Errors
/// Returns a presentation-safe reason for malformed, reversed, or duplicate entries.
pub fn parse_cpu_hardware_ids(value: &str) -> Result<Vec<u32>, &'static str> {
    let mut ids = Vec::new();
    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if let Some((start, end)) = part.split_once('-') {
            let start = start.parse::<u32>().map_err(|_| "CPU list is invalid")?;
            let end = end.parse::<u32>().map_err(|_| "CPU list is invalid")?;
            if start > end {
                return Err("CPU range must be ascending");
            }
            ids.extend(start..=end);
        } else {
            ids.push(part.parse::<u32>().map_err(|_| "CPU list is invalid")?);
        }
    }
    if ids.is_empty() {
        return Err("at least one CPU is required");
    }
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("CPU list contains duplicates");
    }
    Ok(ids)
}

/// Parses bytes or a binary unit (`KiB`, `MiB`, `GiB`, `TiB`).
///
/// # Errors
/// Returns a presentation-safe reason for malformed, zero, or overflowing values.
pub fn parse_memory_bytes(value: &str) -> Result<u64, &'static str> {
    let value = value.trim();
    let (number, multiplier) = [
        ("TiB", 1_u64 << 40),
        ("GiB", 1_u64 << 30),
        ("MiB", 1_u64 << 20),
        ("KiB", 1_u64 << 10),
        ("B", 1),
    ]
    .into_iter()
    .find_map(|(suffix, multiplier)| {
        value
            .strip_suffix(suffix)
            .map(|number| (number.trim(), multiplier))
    })
    .unwrap_or((value, 1));
    let number = number
        .parse::<u64>()
        .map_err(|_| "memory size is invalid")?;
    let bytes = number
        .checked_mul(multiplier)
        .ok_or("memory size is too large")?;
    (bytes > 0)
        .then_some(bytes)
        .ok_or("memory size must be greater than zero")
}

/// Top-level host-management section.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Section {
    #[default]
    Overview,
    Resources,
    Instances,
    Images,
    Operations,
}

impl Section {
    pub const ALL: [Self; 5] = [
        Self::Overview,
        Self::Resources,
        Self::Instances,
        Self::Images,
        Self::Operations,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Resources => "Resources",
            Self::Instances => "Instances",
            Self::Images => "Images",
            Self::Operations => "Operations",
        }
    }
}

/// Authoritative data required by the initial management shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagementSnapshot {
    pub host: HostSnapshot,
    pub images: Vec<ImageArtifact>,
}

/// Current data-loading state without coupling to an async runtime.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum DataState {
    #[default]
    Loading,
    Ready(Box<ManagementSnapshot>),
    Failed(String),
}

/// User intent emitted for a transport adapter to authorize and execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Intent {
    Refresh,
    ConfigurePool {
        expected_generation: Generation,
        cpu_hardware_ids: Vec<u32>,
        memory_bytes: u64,
    },
    CreateInstance {
        expected_generation: Generation,
        id: InstanceId,
        name: String,
        cpu_hardware_ids: Vec<u32>,
        memory_bytes: u64,
    },
    ImportImage {
        expected_generation: Generation,
        kind: ImageKind,
        source_path: String,
        expected_id: Option<String>,
    },
    LoadInstanceImage {
        id: InstanceId,
        expected_generation: Generation,
        kernel_id: String,
        initrd_id: Option<String>,
        command_line: Option<String>,
    },
    UnloadInstance {
        id: InstanceId,
        expected_generation: Generation,
    },
    OpenConsole(InstanceId),
    StartInstance {
        id: InstanceId,
        expected_generation: Generation,
    },
    StopInstance {
        id: InstanceId,
        expected_generation: Generation,
        force: bool,
    },
    DeleteInstance {
        id: InstanceId,
        expected_generation: Generation,
    },
    CancelOperation(OperationId),
}

/// Stable management-shell state shared by browser and future native adapters.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ManagementModel {
    section: Section,
    data: DataState,
    selected_instance: Option<InstanceId>,
    pending_intent: Option<Intent>,
}

impl ManagementModel {
    #[must_use]
    pub fn loading() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_snapshot(snapshot: ManagementSnapshot) -> Self {
        Self {
            data: DataState::Ready(Box::new(snapshot)),
            ..Self::default()
        }
    }

    #[must_use]
    pub const fn section(&self) -> Section {
        self.section
    }

    #[must_use]
    pub const fn data(&self) -> &DataState {
        &self.data
    }

    #[must_use]
    pub const fn selected_instance(&self) -> Option<InstanceId> {
        self.selected_instance
    }

    pub fn navigate(&mut self, section: Section) {
        self.section = section;
    }

    pub fn select_instance(&mut self, id: Option<InstanceId>) {
        self.selected_instance = id.filter(|candidate| self.instance_exists(*candidate));
    }

    pub fn replace_snapshot(&mut self, snapshot: ManagementSnapshot) {
        if self
            .selected_instance
            .is_some_and(|id| !snapshot.host.instances.iter().any(|item| item.id == id))
        {
            self.selected_instance = None;
        }
        self.data = DataState::Ready(Box::new(snapshot));
        self.pending_intent = None;
    }

    pub fn fail(&mut self, message: impl Into<String>) {
        self.data = DataState::Failed(message.into());
        self.pending_intent = None;
    }

    /// Releases an action that failed before a replacement snapshot arrived.
    ///
    /// The last authoritative snapshot remains visible so clients can present
    /// a recoverable action error without turning the whole host unavailable.
    pub fn reject_pending(&mut self) {
        self.pending_intent = None;
    }

    /// Queues one intent after checking it against the current authoritative state.
    ///
    /// # Errors
    /// Returns a presentation-safe reason when the action is unavailable.
    pub fn request(&mut self, intent: Intent) -> Result<(), &'static str> {
        if self.pending_intent.is_some() {
            return Err("another action is pending");
        }
        self.validate(&intent)?;
        self.pending_intent = Some(intent);
        Ok(())
    }

    #[must_use]
    pub fn take_intent(&mut self) -> Option<Intent> {
        self.pending_intent.take()
    }

    #[must_use]
    pub fn active_instances(&self) -> usize {
        self.snapshot().map_or(0, |snapshot| {
            snapshot
                .host
                .instances
                .iter()
                .filter(|instance| instance.state == InstanceState::Active)
                .count()
        })
    }

    #[must_use]
    pub fn busy_operations(&self) -> usize {
        self.snapshot().map_or(0, |snapshot| {
            snapshot
                .host
                .operations
                .iter()
                .filter(|operation| {
                    matches!(
                        operation.state,
                        OperationState::Queued | OperationState::Running
                    )
                })
                .count()
        })
    }

    fn snapshot(&self) -> Option<&ManagementSnapshot> {
        match &self.data {
            DataState::Ready(snapshot) => Some(snapshot),
            DataState::Loading | DataState::Failed(_) => None,
        }
    }

    fn instance_exists(&self, id: InstanceId) -> bool {
        self.snapshot()
            .is_some_and(|snapshot| snapshot.host.instances.iter().any(|item| item.id == id))
    }

    #[allow(clippy::too_many_lines)]
    fn validate(&self, intent: &Intent) -> Result<(), &'static str> {
        let snapshot = self.snapshot().ok_or("host data is not ready")?;
        match intent {
            Intent::Refresh => Ok(()),
            Intent::ConfigurePool {
                expected_generation,
                cpu_hardware_ids,
                memory_bytes,
            } => {
                current_generation(snapshot, *expected_generation)?;
                (!cpu_hardware_ids.is_empty() && *memory_bytes > 0)
                    .then_some(())
                    .ok_or("resource pool requires CPU and memory")
            }
            Intent::CreateInstance {
                expected_generation,
                id,
                name,
                cpu_hardware_ids,
                memory_bytes,
            } => {
                current_generation(snapshot, *expected_generation)?;
                (!name.trim().is_empty()
                    && !cpu_hardware_ids.is_empty()
                    && *memory_bytes > 0
                    && !self.instance_exists(*id))
                .then_some(())
                .ok_or("instance configuration is incomplete or conflicts")
            }
            Intent::ImportImage {
                expected_generation,
                source_path,
                ..
            } => {
                current_generation(snapshot, *expected_generation)?;
                (!source_path.trim().is_empty())
                    .then_some(())
                    .ok_or("image source path is required")
            }
            Intent::LoadInstanceImage {
                id,
                expected_generation,
                kernel_id,
                ..
            } => {
                current_generation(snapshot, *expected_generation)?;
                (!kernel_id.trim().is_empty())
                    .then_some(())
                    .ok_or("kernel image is required")?;
                instance(snapshot, *id).and_then(|item| {
                    (item.state == InstanceState::Ready)
                        .then_some(())
                        .ok_or("load requires a ready instance")
                })
            }
            Intent::UnloadInstance {
                id,
                expected_generation,
            } => {
                current_generation(snapshot, *expected_generation)?;
                instance(snapshot, *id).and_then(|item| {
                    (item.state == InstanceState::Loaded)
                        .then_some(())
                        .ok_or("unload requires a loaded instance")
                })
            }
            Intent::OpenConsole(id) => instance(snapshot, *id).and_then(|item| {
                (item.state == InstanceState::Active)
                    .then_some(())
                    .ok_or("console requires an active instance")
            }),
            Intent::StartInstance {
                id,
                expected_generation,
            } => {
                current_generation(snapshot, *expected_generation)?;
                instance(snapshot, *id).and_then(|item| {
                    (item.state == InstanceState::Loaded)
                        .then_some(())
                        .ok_or("start requires a loaded instance")
                })
            }
            Intent::StopInstance {
                id,
                expected_generation,
                ..
            } => {
                current_generation(snapshot, *expected_generation)?;
                instance(snapshot, *id).and_then(|item| {
                    (item.state == InstanceState::Active)
                        .then_some(())
                        .ok_or("stop requires an active instance")
                })
            }
            Intent::DeleteInstance {
                id,
                expected_generation,
            } => {
                current_generation(snapshot, *expected_generation)?;
                instance(snapshot, *id).and_then(|item| {
                    (item.state == InstanceState::Ready)
                        .then_some(())
                        .ok_or("delete requires a ready instance")
                })
            }
            Intent::CancelOperation(id) => snapshot
                .host
                .operations
                .iter()
                .find(|operation| &operation.id == id)
                .ok_or("operation no longer exists")
                .and_then(|operation| {
                    matches!(
                        operation.state,
                        OperationState::Queued | OperationState::Running
                    )
                    .then_some(())
                    .ok_or("operation is already terminal")
                }),
        }
    }
}

fn instance(
    snapshot: &ManagementSnapshot,
    id: InstanceId,
) -> Result<&kernmux_api::v1::Instance, &'static str> {
    snapshot
        .host
        .instances
        .iter()
        .find(|instance| instance.id == id)
        .ok_or("instance no longer exists")
}

fn current_generation(
    snapshot: &ManagementSnapshot,
    expected: Generation,
) -> Result<(), &'static str> {
    (snapshot.host.generation == expected)
        .then_some(())
        .ok_or("host state changed; refresh before retrying")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernmux_api::v1::*;

    fn snapshot() -> ManagementSnapshot {
        ManagementSnapshot {
            host: HostSnapshot {
                generation: Generation(7),
                health: SnapshotHealth::Healthy,
                diagnostics: Vec::new(),
                kernel: KernelInfo {
                    release: "7.0.0-mk".into(),
                    multikernel_enabled: true,
                },
                capabilities: vec![Capability::Multikernel, Capability::InstanceLifecycle],
                topology: CpuTopology {
                    architecture: "x86_64".into(),
                    cpus: Vec::new(),
                    numa_nodes: Vec::new(),
                },
                memory: HostMemory {
                    total_bytes: 32,
                    host_reserved_bytes: 8,
                    assignable_bytes: 24,
                    assigned_bytes: 8,
                },
                resource_pool: ResourcePool::default(),
                instances: vec![Instance {
                    id: InstanceId(2),
                    name: "build".into(),
                    generation: Generation(7),
                    state: InstanceState::Active,
                    resources: ResourceAllocation::default(),
                    image: KernelImage { present: true },
                }],
                transactions: Vec::new(),
                operations: Vec::new(),
            },
            images: Vec::new(),
        }
    }

    #[test]
    fn navigation_and_selection_survive_refresh_when_resources_exist() {
        let mut model = ManagementModel::from_snapshot(snapshot());
        model.navigate(Section::Instances);
        model.select_instance(Some(InstanceId(2)));
        model.replace_snapshot(snapshot());
        assert_eq!(model.section(), Section::Instances);
        assert_eq!(model.selected_instance(), Some(InstanceId(2)));

        let mut removed = snapshot();
        removed.host.instances.clear();
        model.replace_snapshot(removed);
        assert_eq!(model.selected_instance(), None);
    }

    #[test]
    fn lifecycle_intents_fail_closed_against_state_and_generation() {
        let mut model = ManagementModel::from_snapshot(snapshot());
        assert_eq!(
            model.request(Intent::StartInstance {
                id: InstanceId(2),
                expected_generation: Generation(7)
            }),
            Err("start requires a loaded instance")
        );
        assert_eq!(
            model.request(Intent::StopInstance {
                id: InstanceId(2),
                expected_generation: Generation(6),
                force: false
            }),
            Err("host state changed; refresh before retrying")
        );
        model
            .request(Intent::StopInstance {
                id: InstanceId(2),
                expected_generation: Generation(7),
                force: false,
            })
            .unwrap();
        assert!(matches!(
            model.take_intent(),
            Some(Intent::StopInstance { .. })
        ));
    }

    #[test]
    fn unavailable_data_never_emits_a_mutation() {
        let mut model = ManagementModel::loading();
        assert_eq!(
            model.request(Intent::ConfigurePool {
                expected_generation: Generation(1),
                cpu_hardware_ids: vec![2],
                memory_bytes: 1,
            }),
            Err("host data is not ready")
        );
        assert_eq!(model.take_intent(), None);
    }

    #[test]
    fn authoritative_refresh_releases_the_pending_action() {
        let mut model = ManagementModel::from_snapshot(snapshot());
        let intent = Intent::StopInstance {
            id: InstanceId(2),
            expected_generation: Generation(7),
            force: false,
        };
        model.request(intent.clone()).unwrap();
        assert_eq!(
            model.request(intent.clone()),
            Err("another action is pending")
        );

        model.replace_snapshot(snapshot());
        model.request(intent).unwrap();
    }

    #[test]
    fn rejected_action_keeps_authoritative_data_available() {
        let mut model = ManagementModel::from_snapshot(snapshot());
        let intent = Intent::StopInstance {
            id: InstanceId(2),
            expected_generation: Generation(7),
            force: false,
        };
        model.request(intent.clone()).unwrap();

        model.reject_pending();

        assert!(matches!(model.data(), DataState::Ready(_)));
        model.request(intent).unwrap();
    }

    #[test]
    fn setup_values_are_parsed_strictly() {
        assert_eq!(parse_cpu_hardware_ids("4-6, 9"), Ok(vec![4, 5, 6, 9]));
        assert_eq!(
            parse_cpu_hardware_ids("4,4"),
            Err("CPU list contains duplicates")
        );
        assert_eq!(
            parse_cpu_hardware_ids("7-4"),
            Err("CPU range must be ascending")
        );
        assert_eq!(parse_memory_bytes("2 GiB"), Ok(2_u64 << 30));
        assert_eq!(parse_memory_bytes("512MiB"), Ok(512_u64 << 20));
        assert_eq!(
            parse_memory_bytes("0"),
            Err("memory size must be greater than zero")
        );
    }
}
