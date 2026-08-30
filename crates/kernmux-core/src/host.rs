//! Read-only Linux host discovery.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Root-relative reader for Linux host interfaces.
#[derive(Clone, Debug)]
pub struct LinuxHostProbe {
    root: PathBuf,
}

impl LinuxHostProbe {
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

    /// Reads kernel, capability, topology, and memory observations.
    ///
    /// # Errors
    ///
    /// Returns [`ProbeError`] when a required procfs or sysfs interface cannot
    /// be read or contains an invalid value.
    pub fn observe(&self) -> Result<LinuxHostObservation, ProbeError> {
        let kernel_release = self.read_trimmed("proc/sys/kernel/osrelease")?;
        let capabilities = self.capabilities();
        let numa_nodes = self.read_numa_nodes()?;
        let cpus = self.read_cpus(&numa_nodes)?;
        let memory = self.read_host_memory()?;

        Ok(LinuxHostObservation {
            kernel_release,
            architecture: std::env::consts::ARCH.to_owned(),
            capabilities,
            cpus,
            numa_nodes,
            memory,
        })
    }

    fn capabilities(&self) -> HostCapabilities {
        let candidates = [
            (
                HostCapability::Multikernel,
                "sys/fs/multikernel/device_tree",
            ),
            (
                HostCapability::InstanceLifecycle,
                "sys/fs/multikernel/instances",
            ),
            (HostCapability::Transactions, "sys/fs/multikernel/overlays"),
            (HostCapability::DynamicResources, "dev/lazy_cma"),
            (HostCapability::Console, "dev/mktty"),
            (HostCapability::SharedMemory, "dev/dma_heap/multikernel"),
        ];
        HostCapabilities {
            supported: candidates
                .into_iter()
                .filter_map(|(capability, path)| self.exists(path).then_some(capability))
                .collect(),
        }
    }

    fn read_cpus(
        &self,
        numa_nodes: &[NumaNodeObservation],
    ) -> Result<Vec<CpuObservation>, ProbeError> {
        let online = parse_cpu_list(&self.read_trimmed("sys/devices/system/cpu/online")?)?;
        let hardware_ids = self.read_hardware_cpu_ids()?;
        let mut cpus = Vec::with_capacity(online.len());

        for logical_id in online {
            let base = format!("sys/devices/system/cpu/cpu{logical_id}/topology");
            let package_id = self.read_u32(&format!("{base}/physical_package_id"))?;
            let core_id = self.read_u32(&format!("{base}/core_id"))?;
            let numa_node = numa_nodes
                .iter()
                .find(|node| node.logical_cpu_ids.contains(&logical_id))
                .map_or(0, |node| node.id);
            cpus.push(CpuObservation {
                logical_id,
                hardware_id: hardware_ids.get(&logical_id).copied().unwrap_or(logical_id),
                package_id,
                core_id,
                thread_index: 0,
                numa_node,
            });
        }

        let mut next_thread = BTreeMap::<(u32, u32), u32>::new();
        for cpu in &mut cpus {
            let index = next_thread
                .entry((cpu.package_id, cpu.core_id))
                .or_default();
            cpu.thread_index = *index;
            *index = index.saturating_add(1);
        }
        Ok(cpus)
    }

    fn read_hardware_cpu_ids(&self) -> Result<BTreeMap<u32, u32>, ProbeError> {
        let contents = self.read_trimmed("proc/cpuinfo")?;
        let mut result = BTreeMap::new();
        for block in contents.split("\n\n") {
            let fields: BTreeMap<_, _> = block
                .lines()
                .filter_map(|line| line.split_once(':'))
                .map(|(key, value)| (key.trim(), value.trim()))
                .collect();
            let Some(logical) = fields.get("processor").and_then(|value| value.parse().ok()) else {
                continue;
            };
            let hardware = fields
                .get("apicid")
                .or_else(|| fields.get("initial apicid"))
                .and_then(|value| value.parse().ok())
                .unwrap_or(logical);
            result.insert(logical, hardware);
        }
        Ok(result)
    }

    fn read_numa_nodes(&self) -> Result<Vec<NumaNodeObservation>, ProbeError> {
        let directory = self.path("sys/devices/system/node");
        let entries =
            fs::read_dir(&directory).map_err(|source| ProbeError::new(&directory, source))?;
        let mut nodes = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| ProbeError::new(&directory, source))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(id) = name
                .strip_prefix("node")
                .and_then(|value| value.parse().ok())
            else {
                continue;
            };
            let relative = format!("sys/devices/system/node/{name}");
            let logical_cpu_ids =
                parse_cpu_list(&self.read_trimmed(&format!("{relative}/cpulist"))?)?;
            let memory = self.read_trimmed(&format!("{relative}/meminfo"))?;
            nodes.push(NumaNodeObservation {
                id,
                logical_cpu_ids,
                total_memory_bytes: parse_kib_field(&memory, "MemTotal")?,
                available_memory_bytes: parse_kib_field(&memory, "MemFree")?,
            });
        }
        nodes.sort_by_key(|node| node.id);
        Ok(nodes)
    }

    fn read_host_memory(&self) -> Result<HostMemoryObservation, ProbeError> {
        let contents = self.read_trimmed("proc/meminfo")?;
        let total_bytes = parse_kib_field(&contents, "MemTotal")?;
        let available_bytes = parse_kib_field(&contents, "MemAvailable")
            .or_else(|_| parse_kib_field(&contents, "MemFree"))?;
        Ok(HostMemoryObservation {
            total_bytes,
            available_bytes,
        })
    }

    fn read_u32(&self, relative: &str) -> Result<u32, ProbeError> {
        let value = self.read_trimmed(relative)?;
        value.parse().map_err(|error| {
            ProbeError::invalid(
                self.path(relative),
                format!("invalid integer {value:?}: {error}"),
            )
        })
    }

    fn read_trimmed(&self, relative: &str) -> Result<String, ProbeError> {
        let path = self.path(relative);
        fs::read_to_string(&path)
            .map(|value| value.trim().to_owned())
            .map_err(|source| ProbeError::new(path, source))
    }

    fn exists(&self, relative: &str) -> bool {
        self.path(relative).exists()
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

/// Normalized Linux observations before Multikernel inventory is merged.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinuxHostObservation {
    pub kernel_release: String,
    pub architecture: String,
    pub capabilities: HostCapabilities,
    pub cpus: Vec<CpuObservation>,
    pub numa_nodes: Vec<NumaNodeObservation>,
    pub memory: HostMemoryObservation,
}

/// Host interfaces discovered at runtime.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostCapabilities {
    supported: BTreeSet<HostCapability>,
}

impl HostCapabilities {
    /// Returns whether the host advertises a capability.
    #[must_use]
    pub fn supports(&self, capability: HostCapability) -> bool {
        self.supported.contains(&capability)
    }

    /// Iterates over supported capabilities in stable order.
    pub fn iter(&self) -> impl Iterator<Item = HostCapability> + '_ {
        self.supported.iter().copied()
    }
}

impl FromIterator<HostCapability> for HostCapabilities {
    fn from_iter<T: IntoIterator<Item = HostCapability>>(iter: T) -> Self {
        Self {
            supported: iter.into_iter().collect(),
        }
    }
}

/// Runtime feature discovered from Linux host interfaces.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum HostCapability {
    Multikernel,
    InstanceLifecycle,
    Transactions,
    DynamicResources,
    Console,
    SharedMemory,
}

/// One online logical CPU.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CpuObservation {
    pub logical_id: u32,
    pub hardware_id: u32,
    pub package_id: u32,
    pub core_id: u32,
    pub thread_index: u32,
    pub numa_node: u32,
}

/// One NUMA node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NumaNodeObservation {
    pub id: u32,
    pub logical_cpu_ids: Vec<u32>,
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
}

/// Host memory totals observed from procfs.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostMemoryObservation {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

/// Failure to read or normalize a host interface.
#[derive(Debug)]
pub struct ProbeError {
    path: PathBuf,
    source: io::Error,
}

impl ProbeError {
    fn new(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self {
            path: path.into(),
            source,
        }
    }

    fn invalid(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::new(
            path,
            io::Error::new(io::ErrorKind::InvalidData, message.into()),
        )
    }

    /// Interface path associated with the failure.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "failed to probe {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for ProbeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn parse_cpu_list(value: &str) -> Result<Vec<u32>, ProbeError> {
    let mut cpus = Vec::new();
    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if let Some((start, end)) = part.split_once('-') {
            let start = parse_u32_value(start, "cpu list")?;
            let end = parse_u32_value(end, "cpu list")?;
            if start > end {
                return Err(ProbeError::invalid("cpu-list", "CPU range is reversed"));
            }
            cpus.extend(start..=end);
        } else {
            cpus.push(parse_u32_value(part, "cpu list")?);
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

fn parse_kib_field(contents: &str, field: &str) -> Result<u64, ProbeError> {
    for line in contents.lines() {
        let Some((prefix, rest)) = line.split_once(':') else {
            continue;
        };
        if prefix.split_whitespace().last() != Some(field) {
            continue;
        }
        let kib = rest
            .split_whitespace()
            .next()
            .ok_or_else(|| ProbeError::invalid("meminfo", format!("missing value for {field}")))?;
        let kib = parse_u64_value(kib, field)?;
        return kib
            .checked_mul(1024)
            .ok_or_else(|| ProbeError::invalid("meminfo", format!("{field} overflows bytes")));
    }
    Err(ProbeError::invalid(
        "meminfo",
        format!("missing field {field}"),
    ))
}

fn parse_u32_value(value: &str, context: &str) -> Result<u32, ProbeError> {
    value.parse().map_err(|error| {
        ProbeError::invalid(context, format!("invalid integer {value:?}: {error}"))
    })
}

fn parse_u64_value(value: &str, context: &str) -> Result<u64, ProbeError> {
    value.parse().map_err(|error| {
        ProbeError::invalid(context, format!("invalid integer {value:?}: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("fixture path must have a parent"))
            .expect("fixture directory must be created");
        fs::write(path, contents).expect("fixture file must be written");
    }

    fn fixture_root() -> TempDir {
        let root = TempDir::new().expect("temporary root must be created");
        write(root.path(), "proc/sys/kernel/osrelease", "7.0.0-mk\n");
        write(root.path(), "sys/devices/system/cpu/online", "0-1,4\n");
        write(
            root.path(),
            "proc/cpuinfo",
            "processor: 0\napicid: 0\n\nprocessor: 1\napicid: 2\n\nprocessor: 4\napicid: 8\n",
        );
        for (cpu, package, core) in [(0, 0, 0), (1, 0, 0), (4, 1, 0)] {
            write(
                root.path(),
                &format!("sys/devices/system/cpu/cpu{cpu}/topology/physical_package_id"),
                &package.to_string(),
            );
            write(
                root.path(),
                &format!("sys/devices/system/cpu/cpu{cpu}/topology/core_id"),
                &core.to_string(),
            );
        }
        write(
            root.path(),
            "sys/devices/system/node/node0/cpulist",
            "0-1\n",
        );
        write(
            root.path(),
            "sys/devices/system/node/node0/meminfo",
            "Node 0 MemTotal: 2048 kB\nNode 0 MemFree: 1024 kB\n",
        );
        write(root.path(), "sys/devices/system/node/node1/cpulist", "4\n");
        write(
            root.path(),
            "sys/devices/system/node/node1/meminfo",
            "Node 1 MemTotal: 4096 kB\nNode 1 MemFree: 3072 kB\n",
        );
        write(
            root.path(),
            "proc/meminfo",
            "MemTotal: 6144 kB\nMemAvailable: 4096 kB\n",
        );
        for path in [
            "sys/fs/multikernel/device_tree",
            "dev/lazy_cma",
            "dev/mktty",
            "dev/dma_heap/multikernel",
        ] {
            write(root.path(), path, "");
        }
        fs::create_dir_all(root.path().join("sys/fs/multikernel/instances"))
            .expect("instances directory must be created");
        fs::create_dir_all(root.path().join("sys/fs/multikernel/overlays"))
            .expect("overlays directory must be created");
        root
    }

    #[test]
    fn parses_sparse_cpu_lists() {
        assert_eq!(parse_cpu_list("0-2,4,6-7").unwrap(), [0, 1, 2, 4, 6, 7]);
    }

    #[test]
    fn observes_a_root_relative_linux_host() {
        let root = fixture_root();
        let observed = LinuxHostProbe::new(root.path())
            .observe()
            .expect("fixture host must be observed");

        assert_eq!(observed.kernel_release, "7.0.0-mk");
        for capability in [
            HostCapability::Multikernel,
            HostCapability::InstanceLifecycle,
            HostCapability::Transactions,
            HostCapability::DynamicResources,
            HostCapability::Console,
            HostCapability::SharedMemory,
        ] {
            assert!(observed.capabilities.supports(capability));
        }
        assert_eq!(observed.memory.total_bytes, 6_291_456);
        assert_eq!(observed.memory.available_bytes, 4_194_304);
        assert_eq!(observed.numa_nodes.len(), 2);
        assert_eq!(observed.cpus.len(), 3);
        assert_eq!(observed.cpus[1].hardware_id, 2);
        assert_eq!(observed.cpus[1].thread_index, 1);
        assert_eq!(observed.cpus[2].numa_node, 1);
    }

    #[test]
    fn rejects_reversed_cpu_ranges() {
        assert!(parse_cpu_list("4-2").is_err());
    }
}
