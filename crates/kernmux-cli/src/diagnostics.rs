use std::{
    fs,
    io::Read,
    os::unix::fs::FileTypeExt,
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use kernmux_api::v1::ReleaseCompatibilityManifest;
use serde::Serialize;

const MAX_VERSION_OUTPUT: u64 = 64 * 1024;

#[derive(Debug)]
pub(crate) struct DiagnosticConfig {
    sysfs_device_tree: PathBuf,
    release_manifest: PathBuf,
    os_release: PathBuf,
    kernel_release: PathBuf,
    kerf_program: PathBuf,
    socket: PathBuf,
    architecture: String,
    timeout: Duration,
}

impl DiagnosticConfig {
    pub(crate) fn system(socket: PathBuf) -> Self {
        Self {
            sysfs_device_tree: configured_path(
                "KERNMUX_DIAGNOSTIC_SYSFS",
                "/sys/fs/multikernel/device_tree",
            ),
            release_manifest: configured_path(
                "KERNMUX_DIAGNOSTIC_MANIFEST",
                "/etc/kernmux/release.json",
            ),
            os_release: configured_path("KERNMUX_DIAGNOSTIC_OS_RELEASE", "/etc/os-release"),
            kernel_release: configured_path(
                "KERNMUX_DIAGNOSTIC_KERNEL_RELEASE",
                "/proc/sys/kernel/osrelease",
            ),
            kerf_program: configured_path("KERNMUX_DIAGNOSTIC_KERF", "kerf"),
            socket,
            architecture: std::env::consts::ARCH.to_owned(),
            timeout: Duration::from_secs(3),
        }
    }
}

fn configured_path(name: &str, default: &str) -> PathBuf {
    std::env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

#[derive(Debug, Serialize)]
pub(crate) struct DiagnosticReport {
    schema_version: u32,
    pub(crate) compatible: bool,
    daemon_socket_available: bool,
    checks: Vec<DiagnosticCheck>,
}

#[derive(Debug, Serialize)]
struct DiagnosticCheck {
    name: &'static str,
    required: bool,
    status: CheckStatus,
    message: String,
    remediation: &'static str,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum CheckStatus {
    Passed,
    Failed,
    Warning,
}

pub(crate) fn diagnose(config: &DiagnosticConfig) -> DiagnosticReport {
    let mut checks = Vec::new();
    let sysfs_present = config
        .sysfs_device_tree
        .metadata()
        .is_ok_and(|metadata| metadata.is_file());
    checks.push(required_check(
        "multikernel_sysfs",
        sysfs_present,
        if sysfs_present {
            format!("{} is present", config.sysfs_device_tree.display())
        } else {
            format!("{} is missing", config.sysfs_device_tree.display())
        },
        "Boot the supported Multikernel kernel before starting kernmuxd.",
    ));

    let manifest = read_manifest(&config.release_manifest);
    checks.push(required_check(
        "release_manifest",
        manifest.is_ok(),
        match &manifest {
            Ok(manifest) => format!("loaded release contract {}", manifest.release_id),
            Err(error) => error.clone(),
        },
        "Reinstall the matching Kernmux package and do not hand-edit release.json.",
    ));

    let kerf = kerf_version(&config.kerf_program, config.timeout);
    checks.push(required_check(
        "kerf",
        kerf.is_ok(),
        match &kerf {
            Ok(version) => format!("Kerf reports version {version}"),
            Err(error) => error.clone(),
        },
        "Install the Kerf version required by the packaged release contract.",
    ));

    let release_matches = match (&manifest, &kerf) {
        (Ok(manifest), Ok(kerf_version)) => release_match(config, manifest, kerf_version),
        _ => Err("release compatibility cannot be evaluated until prerequisite checks pass".into()),
    };
    checks.push(required_check(
        "release_compatibility",
        release_matches.is_ok(),
        release_matches.unwrap_or_else(|error| error),
        "Install a Kernmux package built for this OS, architecture, kernel, and Kerf release.",
    ));

    let socket_available = config
        .socket
        .metadata()
        .is_ok_and(|metadata| metadata.file_type().is_socket());
    checks.push(DiagnosticCheck {
        name: "daemon_socket",
        required: false,
        status: if socket_available {
            CheckStatus::Passed
        } else {
            CheckStatus::Warning
        },
        message: if socket_available {
            format!("{} is available", config.socket.display())
        } else {
            format!("{} is not available", config.socket.display())
        },
        remediation: "Inspect systemctl status kernmuxd and journalctl -u kernmuxd before retrying.",
    });

    let compatible = checks
        .iter()
        .filter(|check| check.required)
        .all(|check| matches!(check.status, CheckStatus::Passed));
    DiagnosticReport {
        schema_version: 1,
        compatible,
        daemon_socket_available: socket_available,
        checks,
    }
}

fn required_check(
    name: &'static str,
    passed: bool,
    message: String,
    remediation: &'static str,
) -> DiagnosticCheck {
    DiagnosticCheck {
        name,
        required: true,
        status: if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        },
        message,
        remediation,
    }
}

fn read_manifest(path: &PathBuf) -> Result<ReleaseCompatibilityManifest, String> {
    let bytes =
        fs::read(path).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is invalid: {error}", path.display()))
}

fn kerf_version(program: &PathBuf, timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to execute {}: {error}", program.display()))?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{} --version exceeded {} ms",
                    program.display(),
                    timeout.as_millis()
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed to wait for {}: {error}", program.display()));
            }
        }
    };
    let mut stdout = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        pipe.take(MAX_VERSION_OUTPUT)
            .read_to_end(&mut stdout)
            .map_err(|error| format!("failed to read Kerf version: {error}"))?;
    }
    if !status.success() {
        return Err(format!(
            "{} --version exited with {status}",
            program.display()
        ));
    }
    let output = String::from_utf8(stdout)
        .map_err(|_| "Kerf version output is not valid UTF-8".to_owned())?;
    parse_kerf_version(&output)
}

fn parse_kerf_version(output: &str) -> Result<String, String> {
    let version = output
        .trim()
        .strip_prefix("kerf, version ")
        .filter(|version| !version.is_empty() && !version.chars().any(char::is_whitespace))
        .ok_or_else(|| "Kerf version output has an unsupported format".to_owned())?;
    Ok(version.to_owned())
}

fn release_match(
    config: &DiagnosticConfig,
    manifest: &ReleaseCompatibilityManifest,
    kerf_version: &str,
) -> Result<String, String> {
    let os_release = fs::read_to_string(&config.os_release)
        .map_err(|error| format!("failed to read {}: {error}", config.os_release.display()))?;
    let os_id = os_release_value(&os_release, "ID")
        .ok_or_else(|| format!("{} has no ID", config.os_release.display()))?;
    let os_version = os_release_value(&os_release, "VERSION_ID")
        .ok_or_else(|| format!("{} has no VERSION_ID", config.os_release.display()))?;
    let kernel_release = fs::read_to_string(&config.kernel_release).map_err(|error| {
        format!(
            "failed to read {}: {error}",
            config.kernel_release.display()
        )
    })?;
    let kernel_release = kernel_release.trim();
    let observed = [
        ("os_id", os_id.as_str(), manifest.os_id.as_str()),
        (
            "os_version_id",
            os_version.as_str(),
            manifest.os_version_id.as_str(),
        ),
        (
            "architecture",
            config.architecture.as_str(),
            manifest.architecture.as_str(),
        ),
        (
            "kernel_release",
            kernel_release,
            manifest.kernel_release.as_str(),
        ),
        ("kerf_version", kerf_version, manifest.kerf_version.as_str()),
    ];
    let mismatches = observed
        .into_iter()
        .filter(|(_, actual, expected)| actual != expected)
        .map(|(name, actual, expected)| format!("{name}: observed {actual}, expected {expected}"))
        .collect::<Vec<_>>();
    if mismatches.is_empty() {
        Ok(format!(
            "host identity matches release contract {}",
            manifest.release_id
        ))
    } else {
        Err(mismatches.join("; "))
    }
}

fn os_release_value(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name == key).then(|| value.trim_matches('"').to_owned())
    })
}

#[cfg(test)]
mod tests {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt, os::unix::net::UnixListener};

    use tempfile::TempDir;

    use super::*;

    fn fixture() -> (TempDir, DiagnosticConfig) {
        let root = TempDir::new().unwrap();
        let sysfs = root.path().join("device_tree");
        let manifest = root.path().join("release.json");
        let os_release = root.path().join("os-release");
        let kernel_release = root.path().join("kernel-release");
        let kerf = root.path().join("kerf");
        let socket = root.path().join("kernmuxd.sock");
        fs::write(&sysfs, []).unwrap();
        fs::write(
            &manifest,
            include_bytes!("../../../packaging/release/compatibility.json"),
        )
        .unwrap();
        fs::write(&os_release, "ID=ubuntu\nVERSION_ID=\"24.04\"\n").unwrap();
        fs::write(&kernel_release, "7.0.0-mk2-kernmux-mk+\n").unwrap();
        write_program(&kerf, "#!/bin/sh\nprintf 'kerf, version 0.2.0\\n'\n");
        (
            root,
            DiagnosticConfig {
                sysfs_device_tree: sysfs,
                release_manifest: manifest,
                os_release,
                kernel_release,
                kerf_program: kerf,
                socket,
                architecture: "x86_64".into(),
                timeout: Duration::from_secs(1),
            },
        )
    }

    fn write_program(path: &PathBuf, body: &str) {
        fs::write(path, body).unwrap();
        fs::set_permissions(path, Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn healthy_prerequisites_do_not_require_a_running_daemon() {
        let (_root, config) = fixture();
        let report = diagnose(&config);
        assert!(report.compatible);
        assert!(!report.daemon_socket_available);
        assert!(matches!(report.checks[4].status, CheckStatus::Warning));
    }

    #[test]
    fn observes_the_daemon_socket_without_connecting() {
        let (_root, config) = fixture();
        let _listener = UnixListener::bind(&config.socket).unwrap();
        let report = diagnose(&config);
        assert!(report.compatible);
        assert!(report.daemon_socket_available);
    }

    #[test]
    fn incompatible_release_fails_closed() {
        let (_root, config) = fixture();
        fs::write(&config.kernel_release, "unsupported\n").unwrap();
        let report = diagnose(&config);
        assert!(!report.compatible);
        assert!(matches!(report.checks[3].status, CheckStatus::Failed));
    }

    #[test]
    fn malformed_manifest_and_missing_sysfs_are_reported() {
        let (_root, config) = fixture();
        fs::remove_file(&config.sysfs_device_tree).unwrap();
        fs::write(&config.release_manifest, b"not json").unwrap();
        let report = diagnose(&config);
        assert!(!report.compatible);
        assert!(matches!(report.checks[0].status, CheckStatus::Failed));
        assert!(matches!(report.checks[1].status, CheckStatus::Failed));
    }

    #[test]
    fn kerf_probe_is_bounded() {
        let (_root, mut config) = fixture();
        config.timeout = Duration::from_millis(20);
        write_program(&config.kerf_program, "#!/bin/sh\nsleep 2\n");
        let started = Instant::now();
        let report = diagnose(&config);
        assert!(!report.compatible);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(matches!(report.checks[2].status, CheckStatus::Failed));
    }
}
