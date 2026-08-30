//! Read-only Multikernel resource, instance, and transaction discovery.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use fdt::{Fdt, node::NodeProperty};
use serde::{Deserialize, Serialize};

/// Root-relative reader for the Multikernel filesystem.
#[derive(Clone, Debug)]
pub struct MultikernelProbe {
    root: PathBuf,
}

impl MultikernelProbe {
    /// Creates a probe rooted at a Linux filesystem tree.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Creates a probe for the running host.
    #[must_use]
    pub fn running_host() -> Self {
        Self::new("/")
    }

    /// Reads the authoritative Multikernel device tree and sysfs state.
    ///
    /// # Errors
    ///
    /// Returns [`InventoryError`] when a required interface is unreadable or
    /// contains an invalid device tree, integer, or lifecycle state.
    pub fn observe(&self) -> Result<MultikernelObservation, InventoryError> {
        Ok(MultikernelObservation {
            pool: self.read_resource_pool()?,
            instances: self.read_instances()?,
            transactions: self.read_transactions()?,
        })
    }

    fn read_resource_pool(&self) -> Result<ResourcePoolObservation, InventoryError> {
        let relative = "sys/fs/multikernel/device_tree";
        let bytes = self.read(relative)?;
        let tree = parse_fdt(&bytes, self.path(relative))?;
        let Some(resources) = tree.find_node("/resources") else {
            return Ok(ResourcePoolObservation::default());
        };

        let mut cpu_hardware_ids = property_hardware_ids(resources.property("cpus"), "cpus")?;
        cpu_hardware_ids.sort_unstable();
        let mut available_cpu_hardware_ids =
            property_hardware_ids(resources.property("cpus-available"), "cpus-available")?;
        available_cpu_hardware_ids.sort_unstable();
        let mut memory_regions = Vec::new();
        for node in resources
            .children()
            .filter(|node| node.name.starts_with("memory@"))
        {
            let cells = property_cells(node.property("reg"), "reg")?;
            if cells.len() != 4 {
                return Err(InventoryError::invalid(
                    "device_tree",
                    format!("memory reg must contain four cells, found {}", cells.len()),
                ));
            }
            memory_regions.push(MemoryRegionObservation {
                base: join_cells(cells[0], cells[1]),
                bytes: join_cells(cells[2], cells[3]),
                numa_node: property_u32(node.property("numa-node-id"), "numa-node-id")?,
            });
        }
        memory_regions.sort_by_key(|region| region.base);
        let devices = resources
            .children()
            .find(|node| node.name == "devices")
            .map(|devices| {
                devices
                    .children()
                    .map(|node| self.read_device(node))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();

        Ok(ResourcePoolObservation {
            cpu_hardware_ids,
            available_cpu_hardware_ids,
            memory_regions,
            devices,
        })
    }

    fn read_instances(&self) -> Result<Vec<InstanceObservation>, InventoryError> {
        let relative = "sys/fs/multikernel/instances";
        let directory = self.path(relative);
        let entries =
            fs::read_dir(&directory).map_err(|source| InventoryError::io(&directory, source))?;
        let mut instances = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| InventoryError::io(&directory, source))?;
            if !entry
                .file_type()
                .map_err(|source| InventoryError::io(entry.path(), source))?
                .is_dir()
            {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let base = format!("{relative}/{name}");
            let id = self
                .read_trimmed(&format!("{base}/id"))?
                .parse()
                .map_err(|error| {
                    InventoryError::invalid(&base, format!("invalid instance id: {error}"))
                })?;
            let state =
                InstanceLifecycleState::parse(&self.read_trimmed(&format!("{base}/status"))?)?;
            let device_tree_path = format!("{base}/device_tree");
            let bytes = self.read(&device_tree_path)?;
            let tree = parse_fdt(&bytes, self.path(&device_tree_path))?;
            let resources = tree
                .find_node(&format!("/{name}/resources"))
                .or_else(|| tree.all_nodes().find(|node| node.name == "resources"))
                .ok_or_else(|| {
                    InventoryError::invalid(&device_tree_path, "missing resources node")
                })?;
            let mut cpu_hardware_ids = property_hardware_ids(resources.property("cpus"), "cpus")?;
            cpu_hardware_ids.sort_unstable();
            instances.push(InstanceObservation {
                id,
                name,
                state,
                resources: InstanceResourceObservation {
                    cpu_hardware_ids,
                    memory_base: property_u64(resources.property("memory-base"), "memory-base")?,
                    memory_bytes: property_u64(resources.property("memory-bytes"), "memory-bytes")?,
                    devices: resources
                        .children()
                        .find(|node| node.name == "devices")
                        .map(|devices| {
                            devices
                                .children()
                                .map(|node| self.read_device(node))
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .transpose()?
                        .unwrap_or_default(),
                },
                image: KernelImageObservation {
                    present: matches!(
                        state,
                        InstanceLifecycleState::Loaded | InstanceLifecycleState::Active
                    ),
                },
            });
        }
        instances.sort_by_key(|instance| instance.id);
        Ok(instances)
    }

    fn read_device(
        &self,
        node: fdt::node::FdtNode<'_, '_>,
    ) -> Result<PciDeviceObservation, InventoryError> {
        let pci_id = property_string(node.property("pci-id"), "pci-id")?;
        let iommu_group = self.read_iommu_group(&pci_id)?;
        Ok(PciDeviceObservation {
            pool_name: node.name.to_owned(),
            pci_id,
            vendor_id: property_optional_u32(node.property("vendor-id"), "vendor-id")?,
            device_id: property_optional_u32(node.property("device-id"), "device-id")?,
            iommu_group: iommu_group.as_ref().map(|(id, _)| *id),
            iommu_group_members: iommu_group.map_or_else(Vec::new, |(_, members)| members),
        })
    }

    fn read_iommu_group(&self, pci_id: &str) -> Result<Option<(u32, Vec<String>)>, InventoryError> {
        let link = self.path(&format!("sys/bus/pci/devices/{pci_id}/iommu_group"));
        let target = match fs::read_link(&link) {
            Ok(target) => target,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(InventoryError::io(link, source)),
        };
        let id = target
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| InventoryError::invalid(&link, "IOMMU group target has no numeric ID"))?
            .parse::<u32>()
            .map_err(|error| {
                InventoryError::invalid(&link, format!("invalid IOMMU group ID: {error}"))
            })?;
        let directory = self.path(&format!("sys/kernel/iommu_groups/{id}/devices"));
        let mut members = fs::read_dir(&directory)
            .map_err(|source| InventoryError::io(&directory, source))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .map_err(|source| InventoryError::io(&directory, source))
            })
            .collect::<Result<Vec<_>, _>>()?;
        members.sort();
        Ok(Some((id, members)))
    }

    fn read_transactions(&self) -> Result<Vec<TransactionObservation>, InventoryError> {
        let relative = "sys/fs/multikernel/overlays";
        let directory = self.path(relative);
        let entries =
            fs::read_dir(&directory).map_err(|source| InventoryError::io(&directory, source))?;
        let mut transactions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| InventoryError::io(&directory, source))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !entry
                .file_type()
                .map_err(|source| InventoryError::io(entry.path(), source))?
                .is_dir()
                || !name.starts_with("tx_")
            {
                continue;
            }
            let base = format!("{relative}/{name}");
            let id = self
                .read_trimmed(&format!("{base}/id"))?
                .parse()
                .map_err(|error| {
                    InventoryError::invalid(&base, format!("invalid transaction id: {error}"))
                })?;
            transactions.push(TransactionObservation {
                id,
                status: TransactionStatus::parse(&self.read_trimmed(&format!("{base}/status"))?)?,
                instance_path: self.read_optional_trimmed(&format!("{base}/instance"))?,
                resource_summary: self
                    .read_optional_trimmed(&format!("{base}/resources"))?
                    .filter(|value| value != "unknown"),
            });
        }
        transactions.sort_by_key(|transaction| transaction.id);
        Ok(transactions)
    }

    fn read(&self, relative: &str) -> Result<Vec<u8>, InventoryError> {
        let path = self.path(relative);
        fs::read(&path).map_err(|source| InventoryError::io(path, source))
    }

    fn read_trimmed(&self, relative: &str) -> Result<String, InventoryError> {
        let path = self.path(relative);
        fs::read_to_string(&path)
            .map(|value| value.trim().to_owned())
            .map_err(|source| InventoryError::io(path, source))
    }

    fn read_optional_trimmed(&self, relative: &str) -> Result<Option<String>, InventoryError> {
        let path = self.path(relative);
        match fs::read_to_string(&path) {
            Ok(value) => Ok(Some(value.trim().to_owned())),
            Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(InventoryError::io(path, source)),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

/// Normalized Multikernel observations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MultikernelObservation {
    pub pool: ResourcePoolObservation,
    pub instances: Vec<InstanceObservation>,
    pub transactions: Vec<TransactionObservation>,
}

/// Resources delegated from the control kernel to the Multikernel pool.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResourcePoolObservation {
    pub cpu_hardware_ids: Vec<u32>,
    pub available_cpu_hardware_ids: Vec<u32>,
    pub memory_regions: Vec<MemoryRegionObservation>,
    #[serde(default)]
    pub devices: Vec<PciDeviceObservation>,
}

/// One PCI device managed by the Multikernel resource hierarchy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PciDeviceObservation {
    pub pool_name: String,
    pub pci_id: String,
    pub vendor_id: Option<u32>,
    pub device_id: Option<u32>,
    pub iommu_group: Option<u32>,
    #[serde(default)]
    pub iommu_group_members: Vec<String>,
}

/// One contiguous memory region in the resource pool.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MemoryRegionObservation {
    pub base: u64,
    pub bytes: u64,
    pub numa_node: u32,
}

/// One peer-kernel instance discovered from sysfs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstanceObservation {
    pub id: u32,
    pub name: String,
    pub state: InstanceLifecycleState,
    pub resources: InstanceResourceObservation,
    pub image: KernelImageObservation,
}

/// Resources assigned to one instance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstanceResourceObservation {
    pub cpu_hardware_ids: Vec<u32>,
    pub memory_base: u64,
    pub memory_bytes: u64,
    #[serde(default)]
    pub devices: Vec<PciDeviceObservation>,
}

/// Image state that can be authoritatively observed from the kernel.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct KernelImageObservation {
    pub present: bool,
}

/// Lifecycle state reported by Multikernel sysfs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InstanceLifecycleState {
    Ready,
    Loaded,
    Active,
}

impl InstanceLifecycleState {
    fn parse(value: &str) -> Result<Self, InventoryError> {
        match value {
            "ready" => Ok(Self::Ready),
            "loaded" => Ok(Self::Loaded),
            "active" => Ok(Self::Active),
            _ => Err(InventoryError::invalid(
                "status",
                format!("unknown instance state {value:?}"),
            )),
        }
    }
}

/// Transaction retained by the Multikernel overlay filesystem.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TransactionObservation {
    pub id: u64,
    pub status: TransactionStatus,
    pub instance_path: Option<String>,
    pub resource_summary: Option<String>,
}

/// Kernel transaction status.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum TransactionStatus {
    Pending,
    Applied,
    RolledBack,
    Failed,
}

impl TransactionStatus {
    fn parse(value: &str) -> Result<Self, InventoryError> {
        match value {
            "pending" => Ok(Self::Pending),
            "applied" => Ok(Self::Applied),
            "rolled_back" | "rolled-back" => Ok(Self::RolledBack),
            "failed" => Ok(Self::Failed),
            _ => Err(InventoryError::invalid(
                "transaction status",
                format!("unknown transaction status {value:?}"),
            )),
        }
    }
}

/// Failure to read or normalize a Multikernel interface.
#[derive(Debug)]
pub struct InventoryError {
    location: PathBuf,
    detail: String,
    source: Option<io::Error>,
}

impl InventoryError {
    fn io(location: impl Into<PathBuf>, source: io::Error) -> Self {
        Self {
            location: location.into(),
            detail: source.to_string(),
            source: Some(source),
        }
    }

    fn invalid(location: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            detail: detail.into(),
            source: None,
        }
    }

    /// Interface location associated with the failure.
    #[must_use]
    pub fn location(&self) -> &Path {
        &self.location
    }
}

impl fmt::Display for InventoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to inventory {}: {}",
            self.location.display(),
            self.detail
        )
    }
}

impl std::error::Error for InventoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

fn parse_fdt(bytes: &[u8], location: impl Into<PathBuf>) -> Result<Fdt<'_>, InventoryError> {
    Fdt::new(bytes).map_err(|error| InventoryError::invalid(location, error.to_string()))
}

fn property_cells(
    property: Option<NodeProperty<'_>>,
    name: &str,
) -> Result<Vec<u32>, InventoryError> {
    let property = property
        .ok_or_else(|| InventoryError::invalid("device_tree", format!("missing {name}")))?;
    if !property.value.len().is_multiple_of(4) {
        return Err(InventoryError::invalid(
            "device_tree",
            format!("{name} is not a sequence of 32-bit cells"),
        ));
    }
    Ok(property
        .value
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_be_bytes(*chunk))
        .collect())
}

fn property_u32(property: Option<NodeProperty<'_>>, name: &str) -> Result<u32, InventoryError> {
    let cells = property_cells(property, name)?;
    match cells.as_slice() {
        [value] => Ok(*value),
        _ => Err(InventoryError::invalid(
            "device_tree",
            format!("{name} must contain one cell"),
        )),
    }
}

fn property_optional_u32(
    property: Option<NodeProperty<'_>>,
    name: &str,
) -> Result<Option<u32>, InventoryError> {
    property
        .map(|property| property_u32(Some(property), name))
        .transpose()
}

fn property_string(
    property: Option<NodeProperty<'_>>,
    name: &str,
) -> Result<String, InventoryError> {
    property
        .and_then(NodeProperty::as_str)
        .map(str::to_owned)
        .ok_or_else(|| InventoryError::invalid("device_tree", format!("missing or invalid {name}")))
}

fn property_u64(property: Option<NodeProperty<'_>>, name: &str) -> Result<u64, InventoryError> {
    let cells = property_cells(property, name)?;
    match cells.as_slice() {
        [high, low] => Ok(join_cells(*high, *low)),
        _ => Err(InventoryError::invalid(
            "device_tree",
            format!("{name} must contain two cells"),
        )),
    }
}

fn property_hardware_ids(
    property: Option<NodeProperty<'_>>,
    name: &str,
) -> Result<Vec<u32>, InventoryError> {
    let Some(property) = property else {
        return Ok(Vec::new());
    };
    let cells = property_cells(Some(property), name)?;
    if cells.as_slice() == [0, 0] {
        return Ok(Vec::new());
    }
    if !cells.len().is_multiple_of(2) {
        return Err(InventoryError::invalid(
            "device_tree",
            format!("{name} must contain pairs of cells"),
        ));
    }
    cells
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            u32::try_from(join_cells(pair[0], pair[1])).map_err(|_| {
                InventoryError::invalid(
                    "device_tree",
                    format!("{name} contains a hardware ID larger than u32"),
                )
            })
        })
        .collect()
}

fn join_cells(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::TempDir;

    use super::*;

    const BASELINE_DTB: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/v1/device-tree/baseline.dtb"
    ));
    const INSTANCE_DTB: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/v1/device-tree/instance-lab.dtb"
    ));

    fn write(root: &Path, relative: &str, contents: impl AsRef<[u8]>) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture path must have a parent"))
            .expect("fixture directory must be created");
        fs::write(path, contents).expect("fixture must be written");
    }

    fn iommu_device(root: &Path, pci_id: &str, group: u32, members: &[&str]) {
        let device = root.join(format!("sys/bus/pci/devices/{pci_id}"));
        fs::create_dir_all(&device).expect("PCI fixture directory must be created");
        symlink(
            format!("../../../../kernel/iommu_groups/{group}"),
            device.join("iommu_group"),
        )
        .expect("IOMMU group fixture link must be created");
        for member in members {
            write(
                root,
                &format!("sys/kernel/iommu_groups/{group}/devices/{member}"),
                "",
            );
        }
    }

    #[test]
    fn normalizes_resource_instance_and_transaction_state() {
        let root = TempDir::new().expect("temporary root must be created");
        write(root.path(), "sys/fs/multikernel/device_tree", BASELINE_DTB);
        iommu_device(root.path(), "0000:05:00.0", 13, &["0000:05:00.0"]);
        iommu_device(root.path(), "0000:06:00.0", 14, &["0000:06:00.0"]);
        write(root.path(), "sys/fs/multikernel/instances/lab/id", "1\n");
        write(
            root.path(),
            "sys/fs/multikernel/instances/lab/status",
            "loaded\n",
        );
        write(
            root.path(),
            "sys/fs/multikernel/instances/lab/device_tree",
            INSTANCE_DTB,
        );
        write(
            root.path(),
            "sys/fs/multikernel/overlays/tx_101/id",
            "101\n",
        );
        write(
            root.path(),
            "sys/fs/multikernel/overlays/tx_101/status",
            "applied\n",
        );
        write(
            root.path(),
            "sys/fs/multikernel/overlays/tx_101/instance",
            "/instances/lab\n",
        );
        write(
            root.path(),
            "sys/fs/multikernel/overlays/tx_101/resources",
            "unknown\n",
        );

        let observed = MultikernelProbe::new(root.path())
            .observe()
            .expect("fixture inventory must normalize");

        assert_eq!(observed.pool.cpu_hardware_ids, [4, 5, 6, 7]);
        assert_eq!(observed.pool.available_cpu_hardware_ids, [4, 5, 6, 7]);
        assert_eq!(observed.pool.devices[0].pci_id, "0000:05:00.0");
        assert_eq!(observed.pool.devices[0].iommu_group, Some(13));
        assert_eq!(
            observed.pool.devices[0].iommu_group_members,
            ["0000:05:00.0"]
        );
        assert_eq!(
            observed.pool.memory_regions,
            [MemoryRegionObservation {
                base: 0x4_0000_0000,
                bytes: 0x8000_0000,
                numa_node: 0,
            }]
        );
        assert_eq!(observed.instances.len(), 1);
        assert_eq!(observed.instances[0].id, 1);
        assert_eq!(observed.instances[0].state, InstanceLifecycleState::Loaded);
        assert!(observed.instances[0].image.present);
        assert_eq!(observed.instances[0].resources.cpu_hardware_ids, [4, 5]);
        assert_eq!(observed.instances[0].resources.memory_bytes, 0x4000_0000);
        assert_eq!(
            observed.instances[0].resources.devices[0].pci_id,
            "0000:06:00.0"
        );
        assert_eq!(
            observed.instances[0].resources.devices[0].iommu_group,
            Some(14)
        );
        assert_eq!(observed.transactions.len(), 1);
        assert_eq!(observed.transactions[0].status, TransactionStatus::Applied);
        assert_eq!(observed.transactions[0].resource_summary, None);
    }

    #[test]
    fn rejects_unknown_lifecycle_state() {
        assert!(InstanceLifecycleState::parse("starting").is_err());
    }

    #[test]
    fn normalizes_empty_cpu_pool_sentinel() {
        let value = [0_u8; 8];
        let property = NodeProperty {
            name: "cpus",
            value: &value,
        };
        assert_eq!(
            property_hardware_ids(Some(property), "cpus").unwrap(),
            Vec::<u32>::new()
        );
    }
}
