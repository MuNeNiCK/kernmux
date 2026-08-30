//! CPU identity and availability checks for Multikernel placement.

use std::{collections::BTreeSet, fmt};

use kernmux_api::v1::HostSnapshot;

/// Why a requested hardware/APIC ID cannot be used for a placement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuPlacementError {
    /// No host CPU currently has the requested hardware ID.
    UnknownHardwareId(u32),
    /// More than one host CPU reports the requested hardware ID.
    AmbiguousHardwareId(u32),
    /// The host CPU is known but not online and cannot enter the pool.
    OfflineHardwareId(u32),
    /// The hardware ID is not available to the target instance.
    UnavailableHardwareId(u32),
}

impl fmt::Display for CpuPlacementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownHardwareId(id) => write!(formatter, "unknown CPU hardware ID {id}"),
            Self::AmbiguousHardwareId(id) => {
                write!(formatter, "CPU hardware ID {id} is not unique")
            }
            Self::OfflineHardwareId(id) => write!(formatter, "CPU hardware ID {id} is offline"),
            Self::UnavailableHardwareId(id) => {
                write!(
                    formatter,
                    "CPU hardware ID {id} is not available in the resource pool"
                )
            }
        }
    }
}

impl std::error::Error for CpuPlacementError {}

/// Validates only CPUs newly entering the Multikernel pool.
///
/// CPUs already in the pool may be absent from host topology because Kerf
/// offlines them and they disappear from `/proc/cpuinfo` and NUMA CPU lists.
pub(crate) fn validate_pool_cpus(
    snapshot: &HostSnapshot,
    requested_hardware_ids: &[u32],
) -> Result<(), CpuPlacementError> {
    let retained = snapshot
        .resource_pool
        .cpu_hardware_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    for hardware_id in requested_hardware_ids
        .iter()
        .copied()
        .filter(|hardware_id| !retained.contains(hardware_id))
    {
        let mut matches = snapshot
            .topology
            .cpus
            .iter()
            .filter(|cpu| cpu.hardware_id == hardware_id);
        let Some(cpu) = matches.next() else {
            return Err(CpuPlacementError::UnknownHardwareId(hardware_id));
        };
        if matches.next().is_some() {
            return Err(CpuPlacementError::AmbiguousHardwareId(hardware_id));
        }
        if !cpu.online {
            return Err(CpuPlacementError::OfflineHardwareId(hardware_id));
        }
    }
    Ok(())
}

/// Validates an instance CPU replacement against authoritative pool state.
pub(crate) fn validate_instance_cpus(
    snapshot: &HostSnapshot,
    requested_hardware_ids: &[u32],
    retained_hardware_ids: &[u32],
) -> Result<(), CpuPlacementError> {
    let mut available = snapshot
        .resource_pool
        .available_cpu_hardware_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    available.extend(retained_hardware_ids.iter().copied());

    requested_hardware_ids
        .iter()
        .copied()
        .find(|hardware_id| !available.contains(hardware_id))
        .map_or(Ok(()), |hardware_id| {
            Err(CpuPlacementError::UnavailableHardwareId(hardware_id))
        })
}

#[cfg(test)]
mod tests {
    use kernmux_api::v1::{
        Cpu, CpuTopology, Generation, HostMemory, HostSnapshot, KernelInfo, ResourcePool,
        SnapshotHealth,
    };

    use super::*;

    fn snapshot() -> HostSnapshot {
        HostSnapshot {
            generation: Generation(1),
            health: SnapshotHealth::Healthy,
            diagnostics: Vec::new(),
            kernel: KernelInfo {
                release: "test".into(),
                multikernel_enabled: true,
            },
            capabilities: Vec::new(),
            topology: CpuTopology {
                architecture: "x86_64".into(),
                cpus: vec![
                    Cpu {
                        logical_id: 1,
                        hardware_id: 8,
                        package_id: 0,
                        core_id: 0,
                        thread_index: 0,
                        numa_node: 0,
                        online: true,
                    },
                    Cpu {
                        logical_id: 4,
                        hardware_id: 9,
                        package_id: 0,
                        core_id: 0,
                        thread_index: 1,
                        numa_node: 0,
                        online: true,
                    },
                    Cpu {
                        logical_id: 2,
                        hardware_id: 10,
                        package_id: 0,
                        core_id: 0,
                        thread_index: 1,
                        numa_node: 1,
                        online: false,
                    },
                    Cpu {
                        logical_id: 3,
                        hardware_id: 16,
                        package_id: 0,
                        core_id: 1,
                        thread_index: 0,
                        numa_node: 1,
                        online: true,
                    },
                ],
                numa_nodes: Vec::new(),
            },
            memory: HostMemory {
                total_bytes: 0,
                host_reserved_bytes: 0,
                assignable_bytes: 0,
                assigned_bytes: 0,
            },
            resource_pool: ResourcePool {
                cpu_hardware_ids: vec![12],
                available_cpu_hardware_ids: vec![12, 14],
                memory_regions: Vec::new(),
                devices: Vec::new(),
                available_device_ids: Vec::new(),
            },
            instances: Vec::new(),
            transactions: Vec::new(),
            operations: Vec::new(),
        }
    }

    #[test]
    fn pool_uses_hardware_ids_and_retains_offlined_members() {
        let snapshot = snapshot();
        assert_eq!(validate_pool_cpus(&snapshot, &[12, 8]), Ok(()));
        assert_eq!(
            validate_pool_cpus(&snapshot, &[12, 1]),
            Err(CpuPlacementError::UnknownHardwareId(1))
        );
        assert_eq!(
            validate_pool_cpus(&snapshot, &[12, 10]),
            Err(CpuPlacementError::OfflineHardwareId(10))
        );
    }

    #[test]
    fn pool_rejects_ambiguous_hardware_identity() {
        let mut snapshot = snapshot();
        snapshot
            .topology
            .cpus
            .push(snapshot.topology.cpus[0].clone());
        assert_eq!(
            validate_pool_cpus(&snapshot, &[8]),
            Err(CpuPlacementError::AmbiguousHardwareId(8))
        );
    }

    #[test]
    fn pool_allows_split_smt_and_cross_numa_requests() {
        let snapshot = snapshot();

        assert_eq!(validate_pool_cpus(&snapshot, &[8]), Ok(()));
        assert_eq!(validate_pool_cpus(&snapshot, &[8, 16]), Ok(()));
    }

    #[test]
    fn instance_update_may_retain_its_own_cpu_only() {
        let snapshot = snapshot();
        assert_eq!(validate_instance_cpus(&snapshot, &[12, 14], &[]), Ok(()));
        assert_eq!(validate_instance_cpus(&snapshot, &[16, 12], &[16]), Ok(()));
        assert_eq!(
            validate_instance_cpus(&snapshot, &[18], &[16]),
            Err(CpuPlacementError::UnavailableHardwareId(18))
        );
    }
}
