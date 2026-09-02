//! Fail-closed Linux block-device inventory for OS image deployment.

use std::{
    collections::BTreeSet,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use kernmux_api::v1::{Generation, StorageDevice, StorageInventory, StorageRejectionReason};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct LinuxStorageInventory {
    root: PathBuf,
}

impl LinuxStorageInventory {
    #[must_use]
    pub fn running_host() -> Self {
        Self::new("/")
    }

    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Observes block devices without opening a device for reading or writing.
    ///
    /// # Errors
    /// Returns an error when safety-relevant kernel state cannot be read reliably.
    pub fn observe(
        &self,
        peer_pci_ids: &BTreeSet<String>,
    ) -> Result<StorageInventory, StorageInventoryError> {
        let mounts = self.mounts()?;
        let root_devices = mounts
            .iter()
            .filter_map(|(dev, point)| (point == "/").then_some(dev.clone()))
            .collect::<BTreeSet<_>>();
        let mounted = mounts
            .into_iter()
            .map(|(dev, _)| dev)
            .collect::<BTreeSet<_>>();
        let swaps = self.swaps()?;
        let class = self.path("sys/class/block");
        let mut names = fs::read_dir(&class)
            .map_err(|source| StorageInventoryError::io(&class, source))?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|source| StorageInventoryError::io(&class, source))
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        let mut devices = Vec::new();
        for name in names {
            let Some(name) = name.to_str() else {
                return Err(StorageInventoryError::Invalid(
                    "non-UTF-8 block device name".into(),
                ));
            };
            devices.push(self.device(name, &mounted, &root_devices, &swaps, peer_pci_ids)?);
        }
        devices.sort_by(|left, right| left.name.cmp(&right.name));
        let encoded = serde_json::to_vec(&devices).map_err(StorageInventoryError::Serialize)?;
        let digest = Sha256::digest(encoded);
        let mut generation_bytes = [0_u8; 8];
        generation_bytes.copy_from_slice(&digest[..8]);
        let generation = Generation(u64::from_be_bytes(generation_bytes));
        Ok(StorageInventory {
            generation,
            devices,
        })
    }

    fn device(
        &self,
        name: &str,
        mounted: &BTreeSet<String>,
        roots: &BTreeSet<String>,
        swaps: &BTreeSet<String>,
        peers: &BTreeSet<String>,
    ) -> Result<StorageDevice, StorageInventoryError> {
        if name.is_empty() || name.contains('/') {
            return Err(StorageInventoryError::Invalid(
                "invalid block device name".into(),
            ));
        }
        let node = self.path(&format!("sys/class/block/{name}"));
        let target =
            fs::canonicalize(&node).map_err(|source| StorageInventoryError::io(&node, source))?;
        let major_minor = read_trimmed(node.join("dev"))?;
        valid_major_minor(&major_minor)?;
        let sectors = read_trimmed(node.join("size"))?
            .parse::<u64>()
            .map_err(|_| StorageInventoryError::Invalid(format!("invalid size for {name}")))?;
        let size_bytes = sectors
            .checked_mul(512)
            .ok_or_else(|| StorageInventoryError::Invalid(format!("size overflow for {name}")))?;
        let read_only = read_trimmed(node.join("ro"))?.parse::<u8>().map_err(|_| {
            StorageInventoryError::Invalid(format!("invalid read-only state for {name}"))
        })? != 0;
        let whole_device = !node.join("partition").exists();
        let (descendants, descendant_paths) = self.descendants(&target)?;
        let device_path = format!("/dev/{name}");
        let pci_id = target
            .components()
            .filter_map(|part| part.as_os_str().to_str())
            .find(|part| valid_pci_id(part))
            .map(str::to_owned);
        let mut reasons = BTreeSet::new();
        if !whole_device {
            reasons.insert(StorageRejectionReason::NotWholeDevice);
        }
        if target.to_string_lossy().contains("/devices/virtual/") {
            reasons.insert(StorageRejectionReason::VirtualDevice);
        }
        if read_only {
            reasons.insert(StorageRejectionReason::ReadOnly);
        }
        if size_bytes == 0 {
            reasons.insert(StorageRejectionReason::ZeroSized);
        }
        if descendants.iter().any(|id| mounted.contains(id)) {
            reasons.insert(StorageRejectionReason::Mounted);
        }
        if descendants.iter().any(|id| roots.contains(id)) {
            reasons.insert(StorageRejectionReason::HostRoot);
        }
        if swaps.contains(&device_path) || descendant_paths.iter().any(|path| swaps.contains(path))
        {
            reasons.insert(StorageRejectionReason::Swap);
        }
        if self.descendant_has_holders(&target)? {
            reasons.insert(StorageRejectionReason::HasHolders);
        }
        if pci_id.as_ref().is_some_and(|id| peers.contains(id)) {
            reasons.insert(StorageRejectionReason::PeerAssigned);
        }
        if major_minor.is_empty() || device_path.len() <= 5 {
            reasons.insert(StorageRejectionReason::IncompleteIdentity);
        }
        Ok(StorageDevice {
            name: name.into(),
            device_path,
            major_minor,
            size_bytes,
            whole_device,
            read_only,
            serial: optional_trimmed(node.join("device/serial"))?,
            wwn: optional_trimmed(node.join("wwid"))?
                .or(optional_trimmed(node.join("device/wwid"))?),
            transport: optional_trimmed(node.join("device/transport"))?,
            pci_id,
            eligible: reasons.is_empty(),
            rejection_reasons: reasons.into_iter().collect(),
        })
    }

    fn descendants(
        &self,
        target: &Path,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>), StorageInventoryError> {
        let class = self.path("sys/class/block");
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for entry in
            fs::read_dir(&class).map_err(|source| StorageInventoryError::io(&class, source))?
        {
            let entry = entry.map_err(|source| StorageInventoryError::io(&class, source))?;
            let child = fs::canonicalize(entry.path())
                .map_err(|source| StorageInventoryError::io(entry.path(), source))?;
            if child == target || child.starts_with(target) {
                ids.insert(read_trimmed(entry.path().join("dev"))?);
                let name = entry.file_name();
                let name = name.to_str().ok_or_else(|| {
                    StorageInventoryError::Invalid("non-UTF-8 block device name".into())
                })?;
                paths.insert(format!("/dev/{name}"));
            }
        }
        Ok((ids, paths))
    }

    fn descendant_has_holders(&self, target: &Path) -> Result<bool, StorageInventoryError> {
        let class = self.path("sys/class/block");
        for entry in
            fs::read_dir(&class).map_err(|source| StorageInventoryError::io(&class, source))?
        {
            let entry = entry.map_err(|source| StorageInventoryError::io(&class, source))?;
            let child = fs::canonicalize(entry.path())
                .map_err(|source| StorageInventoryError::io(entry.path(), source))?;
            if (child == target || child.starts_with(target))
                && has_entries(&entry.path().join("holders"))?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn mounts(&self) -> Result<Vec<(String, String)>, StorageInventoryError> {
        let path = self.path("proc/self/mountinfo");
        let text =
            fs::read_to_string(&path).map_err(|source| StorageInventoryError::io(&path, source))?;
        text.lines()
            .map(|line| {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                if fields.len() < 5 {
                    return Err(StorageInventoryError::Invalid("malformed mountinfo".into()));
                }
                valid_major_minor(fields[2])?;
                Ok((fields[2].into(), fields[4].into()))
            })
            .collect()
    }

    fn swaps(&self) -> Result<BTreeSet<String>, StorageInventoryError> {
        let path = self.path("proc/swaps");
        let text =
            fs::read_to_string(&path).map_err(|source| StorageInventoryError::io(&path, source))?;
        let mut lines = text.lines();
        let header = lines
            .next()
            .ok_or_else(|| StorageInventoryError::Invalid("missing swaps header".into()))?;
        if !header.starts_with("Filename") {
            return Err(StorageInventoryError::Invalid(
                "malformed swaps header".into(),
            ));
        }
        Ok(lines
            .filter_map(|line| line.split_whitespace().next())
            .map(str::to_owned)
            .collect())
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative.trim_start_matches('/'))
    }
}

fn read_trimmed(path: impl AsRef<Path>) -> Result<String, StorageInventoryError> {
    let path = path.as_ref();
    fs::read_to_string(path)
        .map(|value| value.trim().to_owned())
        .map_err(|source| StorageInventoryError::io(path, source))
}

fn optional_trimmed(path: impl AsRef<Path>) -> Result<Option<String>, StorageInventoryError> {
    let path = path.as_ref();
    match fs::read_to_string(path) {
        Ok(value) => {
            let value = value.trim();
            Ok((!value.is_empty()).then(|| value.to_owned()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(StorageInventoryError::io(path, source)),
    }
}

fn has_entries(path: &Path) -> Result<bool, StorageInventoryError> {
    match fs::read_dir(path) {
        Ok(mut entries) => Ok(entries
            .next()
            .transpose()
            .map_err(|source| StorageInventoryError::io(path, source))?
            .is_some()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(
            StorageInventoryError::Invalid(format!("missing holders state: {}", path.display())),
        ),
        Err(source) => Err(StorageInventoryError::io(path, source)),
    }
}

fn valid_major_minor(value: &str) -> Result<(), StorageInventoryError> {
    let Some((major, minor)) = value.split_once(':') else {
        return Err(StorageInventoryError::Invalid(
            "invalid major:minor identity".into(),
        ));
    };
    if major.parse::<u32>().is_err() || minor.parse::<u32>().is_err() {
        return Err(StorageInventoryError::Invalid(
            "invalid major:minor identity".into(),
        ));
    }
    Ok(())
}

fn valid_pci_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 12
        && bytes[4] == b':'
        && bytes[7] == b':'
        && bytes[10] == b'.'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10) || byte.is_ascii_hexdigit())
}

#[derive(Debug)]
pub enum StorageInventoryError {
    Io { context: String, source: io::Error },
    Invalid(String),
    Serialize(serde_json::Error),
}

impl StorageInventoryError {
    fn io(path: impl AsRef<Path>, source: io::Error) -> Self {
        Self::Io {
            context: path.as_ref().display().to_string(),
            source,
        }
    }
}
impl fmt::Display for StorageInventoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { context, source } => write!(f, "failed to read {context}: {source}"),
            Self::Invalid(detail) => f.write_str(detail),
            Self::Serialize(error) => write!(f, "failed to fingerprint storage inventory: {error}"),
        }
    }
}
impl std::error::Error for StorageInventoryError {}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::symlink, path::Path};

    use tempfile::tempdir;

    use super::*;

    fn write(root: &Path, relative: impl AsRef<Path>, value: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    fn disk(root: &Path, name: &str, target: &str, dev: &str, read_only: bool) {
        let target = root.join(target);
        fs::create_dir_all(target.join("holders")).unwrap();
        let relative = target.strip_prefix(root).unwrap();
        write(root, relative.join("dev"), dev);
        write(root, relative.join("size"), "2048\n");
        write(
            root,
            relative.join("ro"),
            if read_only { "1\n" } else { "0\n" },
        );
        fs::create_dir_all(root.join("sys/class/block")).unwrap();
        symlink(target, root.join(format!("sys/class/block/{name}"))).unwrap();
    }

    fn partition(root: &Path, disk_target: &str, name: &str, dev: &str) {
        let target = root.join(disk_target).join(name);
        fs::create_dir_all(target.join("holders")).unwrap();
        let relative = target.strip_prefix(root).unwrap();
        for (file, value) in [
            ("dev", dev),
            ("size", "1024\n"),
            ("ro", "0\n"),
            ("partition", "1\n"),
        ] {
            write(root, relative.join(file), value);
        }
        symlink(target, root.join(format!("sys/class/block/{name}"))).unwrap();
    }

    #[test]
    fn rejects_root_partition_swap_read_only_and_peer_assigned_storage() {
        let root = tempdir().unwrap();
        let vda = "sys/devices/pci0000:00/0000:00:01.0/virtio0/block/vda";
        disk(root.path(), "vda", vda, "8:0\n", false);
        partition(root.path(), vda, "vda1", "8:1\n");
        write(root.path(), "sys/class/block/vda1/holders/dm-0", "");
        disk(
            root.path(),
            "vdb",
            "sys/devices/pci0000:00/0000:00:02.0/virtio1/block/vdb",
            "8:16\n",
            false,
        );
        disk(
            root.path(),
            "vdc",
            "sys/devices/pci0000:00/0000:00:03.0/virtio2/block/vdc",
            "8:32\n",
            true,
        );
        write(
            root.path(),
            "proc/self/mountinfo",
            "20 1 8:1 / / rw - ext4 /dev/vda1 rw\n",
        );
        write(
            root.path(),
            "proc/swaps",
            "Filename Type Size Used Priority\n/dev/vdb partition 1 0 -2\n",
        );
        let observed = LinuxStorageInventory::new(root.path())
            .observe(&BTreeSet::from(["0000:00:02.0".into()]))
            .unwrap();

        let find = |name| {
            observed
                .devices
                .iter()
                .find(|item| item.name == name)
                .unwrap()
        };
        assert!(
            find("vda")
                .rejection_reasons
                .contains(&StorageRejectionReason::HostRoot)
        );
        assert!(
            find("vda")
                .rejection_reasons
                .contains(&StorageRejectionReason::Mounted)
        );
        assert!(
            find("vda")
                .rejection_reasons
                .contains(&StorageRejectionReason::HasHolders)
        );
        assert!(
            find("vda1")
                .rejection_reasons
                .contains(&StorageRejectionReason::NotWholeDevice)
        );
        assert!(
            find("vdb")
                .rejection_reasons
                .contains(&StorageRejectionReason::Swap)
        );
        assert!(
            find("vdb")
                .rejection_reasons
                .contains(&StorageRejectionReason::PeerAssigned)
        );
        assert!(
            find("vdc")
                .rejection_reasons
                .contains(&StorageRejectionReason::ReadOnly)
        );
        assert!(observed.devices.iter().all(|item| !item.eligible));
    }

    #[test]
    fn accepts_unused_physical_disk_and_changes_generation_with_safety_state() {
        let root = tempdir().unwrap();
        disk(
            root.path(),
            "vdd",
            "sys/devices/pci0000:00/0000:00:04.0/virtio3/block/vdd",
            "8:48\n",
            false,
        );
        write(
            root.path(),
            "sys/class/block/vdd/device/serial",
            "KERNMUX-DISK-1\n",
        );
        write(root.path(), "sys/class/block/vdd/wwid", "naa.1234\n");
        write(
            root.path(),
            "sys/class/block/vdd/device/transport",
            "virtio\n",
        );
        write(
            root.path(),
            "proc/self/mountinfo",
            "1 0 0:1 / /proc rw - proc proc rw\n",
        );
        write(
            root.path(),
            "proc/swaps",
            "Filename Type Size Used Priority\n",
        );
        let source = LinuxStorageInventory::new(root.path());
        let before = source.observe(&BTreeSet::new()).unwrap();
        assert!(before.devices[0].eligible);
        assert_eq!(before.devices[0].serial.as_deref(), Some("KERNMUX-DISK-1"));
        assert_eq!(before.devices[0].wwn.as_deref(), Some("naa.1234"));
        assert_eq!(before.devices[0].transport.as_deref(), Some("virtio"));
        fs::write(root.path().join("sys/class/block/vdd/ro"), "1\n").unwrap();
        let after = source.observe(&BTreeSet::new()).unwrap();
        assert_ne!(before.generation, after.generation);
        assert!(!after.devices[0].eligible);
    }

    #[test]
    fn rejects_virtual_devices_and_malformed_mount_identity() {
        let root = tempdir().unwrap();
        disk(
            root.path(),
            "loop0",
            "sys/devices/virtual/block/loop0",
            "7:0\n",
            false,
        );
        write(
            root.path(),
            "proc/self/mountinfo",
            "1 0 0:1 / /proc rw - proc proc rw\n",
        );
        write(
            root.path(),
            "proc/swaps",
            "Filename Type Size Used Priority\n",
        );
        let observed = LinuxStorageInventory::new(root.path())
            .observe(&BTreeSet::new())
            .unwrap();
        assert!(
            observed.devices[0]
                .rejection_reasons
                .contains(&StorageRejectionReason::VirtualDevice)
        );

        write(root.path(), "proc/self/mountinfo", "malformed\n");
        assert!(
            LinuxStorageInventory::new(root.path())
                .observe(&BTreeSet::new())
                .is_err()
        );
    }
}
