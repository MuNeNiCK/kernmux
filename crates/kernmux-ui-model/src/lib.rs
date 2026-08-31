//! Renderer- and transport-independent state for Kernmux management clients.

use kernmux_api::v1::{
    Generation, HostSnapshot, ImageArtifact, InstanceId, InstanceState, OperationId, OperationState,
};

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
    CreateInstance,
    ConfigurePool,
    ImportImage,
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
    }

    pub fn fail(&mut self, message: impl Into<String>) {
        self.data = DataState::Failed(message.into());
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

    fn validate(&self, intent: &Intent) -> Result<(), &'static str> {
        let snapshot = self.snapshot().ok_or("host data is not ready")?;
        match intent {
            Intent::Refresh
            | Intent::CreateInstance
            | Intent::ConfigurePool
            | Intent::ImportImage => Ok(()),
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
            model.request(Intent::ConfigurePool),
            Err("host data is not ready")
        );
        assert_eq!(model.take_intent(), None);
    }
}
