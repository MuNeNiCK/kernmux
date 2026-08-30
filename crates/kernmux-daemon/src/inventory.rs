//! Authoritative host snapshot assembly.

use std::{fmt, path::PathBuf};

use kernmux_api::v1::{
    Capability, Cpu, CpuTopology, Generation, HostMemory, HostSnapshot, Instance, InstanceId,
    InstanceState, KernelImage, KernelInfo, MemoryRegion, NumaNode, ResourceAllocation,
    ResourcePool, Transaction, TransactionState,
};
use kernmux_core::{
    host::{HostCapability, LinuxHostObservation, LinuxHostProbe, ProbeError},
    multikernel::{
        InstanceLifecycleState, InventoryError, MultikernelObservation, MultikernelProbe,
        TransactionStatus,
    },
};

/// A consistent pair of Linux and Multikernel observations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostObservations {
    pub linux: LinuxHostObservation,
    pub multikernel: MultikernelObservation,
}

/// Replaceable source of authoritative host observations.
pub trait InventorySource {
    type Error;

    /// Reads the current host state.
    ///
    /// # Errors
    ///
    /// Returns the source-specific error when a required observation fails.
    fn observe(&mut self) -> Result<HostObservations, Self::Error>;
}

/// Filesystem-backed inventory source for a Linux control kernel.
#[derive(Clone, Debug)]
pub struct FilesystemInventorySource {
    linux: LinuxHostProbe,
    multikernel: MultikernelProbe,
}

impl FilesystemInventorySource {
    /// Creates a source rooted at a Linux filesystem tree.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        Self {
            linux: LinuxHostProbe::new(&root),
            multikernel: MultikernelProbe::new(root),
        }
    }

    /// Creates a source for the running control kernel.
    #[must_use]
    pub fn running_host() -> Self {
        Self::new("/")
    }
}

impl InventorySource for FilesystemInventorySource {
    type Error = FilesystemInventoryError;

    fn observe(&mut self) -> Result<HostObservations, Self::Error> {
        Ok(HostObservations {
            linux: self
                .linux
                .observe()
                .map_err(FilesystemInventoryError::Linux)?,
            multikernel: self
                .multikernel
                .observe()
                .map_err(FilesystemInventoryError::Multikernel)?,
        })
    }
}

/// Failure from one of the Linux filesystem probes.
#[derive(Debug)]
pub enum FilesystemInventoryError {
    Linux(ProbeError),
    Multikernel(InventoryError),
}

impl fmt::Display for FilesystemInventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Linux(error) => error.fmt(formatter),
            Self::Multikernel(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for FilesystemInventoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Linux(error) => Some(error),
            Self::Multikernel(error) => Some(error),
        }
    }
}

/// Stateful snapshot builder that preserves generations for unchanged state.
#[derive(Debug)]
pub struct InventoryService<S> {
    source: S,
    current: Option<HostSnapshot>,
}

impl<S> InventoryService<S>
where
    S: InventorySource,
{
    /// Creates a service over an observation source.
    #[must_use]
    pub fn new(source: S) -> Self {
        Self {
            source,
            current: None,
        }
    }

    /// Refreshes kernel truth and returns a generation-stamped snapshot.
    ///
    /// An unchanged refresh preserves its generation. A changed refresh
    /// increments the generation before publishing the new snapshot.
    ///
    /// # Errors
    ///
    /// Returns the source error without replacing the last good snapshot.
    pub fn refresh(&mut self) -> Result<HostSnapshot, S::Error> {
        let observations = self.source.observe()?;
        if let Some(current) = &self.current {
            let same_generation = assemble_snapshot(current.generation, &observations);
            if &same_generation == current {
                return Ok(current.clone());
            }
            let next = Generation(current.generation.0.saturating_add(1));
            let snapshot = assemble_snapshot(next, &observations);
            self.current = Some(snapshot.clone());
            return Ok(snapshot);
        }

        let snapshot = assemble_snapshot(Generation(1), &observations);
        self.current = Some(snapshot.clone());
        Ok(snapshot)
    }

    /// Last successfully assembled snapshot.
    #[must_use]
    pub fn current(&self) -> Option<&HostSnapshot> {
        self.current.as_ref()
    }
}

fn assemble_snapshot(generation: Generation, observed: &HostObservations) -> HostSnapshot {
    let pool_memory_bytes = observed
        .multikernel
        .pool
        .memory_regions
        .iter()
        .map(|region| region.bytes)
        .sum::<u64>();
    let assigned_memory_bytes = observed
        .multikernel
        .instances
        .iter()
        .map(|instance| instance.resources.memory_bytes)
        .sum::<u64>();

    HostSnapshot {
        generation,
        kernel: KernelInfo {
            release: observed.linux.kernel_release.clone(),
            multikernel_enabled: observed
                .linux
                .capabilities
                .supports(HostCapability::Multikernel),
        },
        capabilities: observed
            .linux
            .capabilities
            .iter()
            .map(map_capability)
            .collect(),
        topology: map_topology(&observed.linux),
        memory: HostMemory {
            total_bytes: observed
                .linux
                .memory
                .total_bytes
                .saturating_add(pool_memory_bytes),
            host_reserved_bytes: observed.linux.memory.total_bytes,
            assignable_bytes: pool_memory_bytes,
            assigned_bytes: assigned_memory_bytes,
        },
        resource_pool: map_resource_pool(&observed.multikernel),
        instances: map_instances(generation, &observed.multikernel),
        transactions: map_transactions(&observed.multikernel),
        operations: Vec::new(),
    }
}

fn map_topology(observed: &LinuxHostObservation) -> CpuTopology {
    CpuTopology {
        architecture: observed.architecture.clone(),
        cpus: observed
            .cpus
            .iter()
            .map(|cpu| Cpu {
                logical_id: cpu.logical_id,
                hardware_id: cpu.hardware_id,
                package_id: cpu.package_id,
                core_id: cpu.core_id,
                thread_index: cpu.thread_index,
                numa_node: cpu.numa_node,
                online: true,
            })
            .collect(),
        numa_nodes: observed
            .numa_nodes
            .iter()
            .map(|node| NumaNode {
                id: node.id,
                logical_cpu_ids: node.logical_cpu_ids.clone(),
                total_memory_bytes: node.total_memory_bytes,
                available_memory_bytes: node.available_memory_bytes,
            })
            .collect(),
    }
}

fn map_resource_pool(observed: &MultikernelObservation) -> ResourcePool {
    ResourcePool {
        cpu_hardware_ids: observed.pool.cpu_hardware_ids.clone(),
        available_cpu_hardware_ids: observed.pool.available_cpu_hardware_ids.clone(),
        memory_regions: observed
            .pool
            .memory_regions
            .iter()
            .map(|region| MemoryRegion {
                base: region.base,
                bytes: region.bytes,
                numa_node: region.numa_node,
            })
            .collect(),
    }
}

fn map_instances(generation: Generation, observed: &MultikernelObservation) -> Vec<Instance> {
    observed
        .instances
        .iter()
        .map(|instance| Instance {
            id: InstanceId(instance.id),
            name: instance.name.clone(),
            generation,
            state: map_instance_state(instance.state),
            resources: ResourceAllocation {
                cpu_hardware_ids: instance.resources.cpu_hardware_ids.clone(),
                memory_bytes: instance.resources.memory_bytes,
                memory_region: None,
                device_ids: Vec::new(),
            },
            image: KernelImage {
                present: instance.image.present,
            },
        })
        .collect()
}

fn map_transactions(observed: &MultikernelObservation) -> Vec<Transaction> {
    observed
        .transactions
        .iter()
        .map(|transaction| Transaction {
            id: transaction.id.to_string(),
            state: map_transaction_state(transaction.status),
            generation_before: None,
            generation_after: None,
            diagnostics: Vec::new(),
        })
        .collect()
}

const fn map_capability(capability: HostCapability) -> Capability {
    match capability {
        HostCapability::Multikernel => Capability::Multikernel,
        HostCapability::InstanceLifecycle => Capability::InstanceLifecycle,
        HostCapability::Transactions => Capability::TransactionRollback,
        HostCapability::DynamicResources => Capability::DynamicResources,
        HostCapability::Console => Capability::Console,
        HostCapability::SharedMemory => Capability::SharedMemory,
    }
}

const fn map_instance_state(state: InstanceLifecycleState) -> InstanceState {
    match state {
        InstanceLifecycleState::Ready => InstanceState::Ready,
        InstanceLifecycleState::Loaded => InstanceState::Loaded,
        InstanceLifecycleState::Active => InstanceState::Active,
    }
}

const fn map_transaction_state(status: TransactionStatus) -> TransactionState {
    match status {
        TransactionStatus::Pending => TransactionState::Planned,
        TransactionStatus::Applied => TransactionState::Applied,
        TransactionStatus::RolledBack => TransactionState::RolledBack,
        TransactionStatus::Failed => TransactionState::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, convert::Infallible};

    use kernmux_core::{
        host::{
            CpuObservation, HostCapabilities, HostMemoryObservation, LinuxHostObservation,
            NumaNodeObservation,
        },
        multikernel::{
            InstanceObservation, InstanceResourceObservation, KernelImageObservation,
            MemoryRegionObservation, ResourcePoolObservation,
        },
    };

    use super::*;

    struct SequenceSource {
        observations: VecDeque<HostObservations>,
    }

    impl InventorySource for SequenceSource {
        type Error = Infallible;

        fn observe(&mut self) -> Result<HostObservations, Self::Error> {
            Ok(self
                .observations
                .pop_front()
                .expect("test source must contain an observation"))
        }
    }

    fn observations(state: InstanceLifecycleState) -> HostObservations {
        HostObservations {
            linux: LinuxHostObservation {
                kernel_release: "7.0.0-mk".into(),
                architecture: "x86_64".into(),
                capabilities: HostCapabilities::from_iter([
                    HostCapability::Multikernel,
                    HostCapability::InstanceLifecycle,
                ]),
                cpus: vec![CpuObservation {
                    logical_id: 4,
                    hardware_id: 4,
                    package_id: 0,
                    core_id: 2,
                    thread_index: 0,
                    numa_node: 0,
                }],
                numa_nodes: vec![NumaNodeObservation {
                    id: 0,
                    logical_cpu_ids: vec![4],
                    total_memory_bytes: 2_147_483_648,
                    available_memory_bytes: 1_073_741_824,
                }],
                memory: HostMemoryObservation {
                    total_bytes: 4_294_967_296,
                    available_bytes: 2_147_483_648,
                },
            },
            multikernel: MultikernelObservation {
                pool: ResourcePoolObservation {
                    cpu_hardware_ids: vec![4],
                    available_cpu_hardware_ids: Vec::new(),
                    memory_regions: vec![MemoryRegionObservation {
                        base: 0x4_0000_0000,
                        bytes: 2_147_483_648,
                        numa_node: 0,
                    }],
                },
                instances: vec![InstanceObservation {
                    id: 1,
                    name: "lab".into(),
                    state,
                    resources: InstanceResourceObservation {
                        cpu_hardware_ids: vec![4],
                        memory_base: 0x4_0000_a000,
                        memory_bytes: 1_073_741_824,
                    },
                    image: KernelImageObservation {
                        present: state != InstanceLifecycleState::Ready,
                    },
                }],
                transactions: Vec::new(),
            },
        }
    }

    #[test]
    fn preserves_generation_until_kernel_truth_changes() {
        let ready = observations(InstanceLifecycleState::Ready);
        let loaded = observations(InstanceLifecycleState::Loaded);
        let source = SequenceSource {
            observations: VecDeque::from([ready.clone(), ready, loaded]),
        };
        let mut inventory = InventoryService::new(source);

        let first = inventory.refresh().unwrap();
        let unchanged = inventory.refresh().unwrap();
        let changed = inventory.refresh().unwrap();

        assert_eq!(first.generation, Generation(1));
        assert_eq!(unchanged.generation, Generation(1));
        assert_eq!(changed.generation, Generation(2));
        assert_eq!(changed.instances[0].state, InstanceState::Loaded);
        assert!(changed.instances[0].image.present);
        assert_eq!(changed.memory.assignable_bytes, 2_147_483_648);
        assert_eq!(changed.memory.assigned_bytes, 1_073_741_824);
        assert_eq!(changed.memory.total_bytes, 6_442_450_944);
        assert_eq!(changed.memory.host_reserved_bytes, 4_294_967_296);
    }
}
