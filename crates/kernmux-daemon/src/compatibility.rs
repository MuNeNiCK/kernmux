//! Read-only release compatibility evidence collection.

use std::{collections::BTreeMap, ffi::OsString, fs, path::Path, time::Duration};

use crate::lifecycle_executor::ProcessKerfRunner;
use kernmux_api::{
    API_MAJOR_VERSION,
    v1::{
        HostCompatibilityEvidence, HostCompatibilityReport, HostSnapshot,
        ReleaseCompatibilityManifest,
    },
};

pub(crate) const STATE_SCHEMA_VERSION: u32 = 1;

pub(crate) fn evaluate(
    manifest_path: &Path,
    root: &Path,
    kerf_executable: &Path,
    kerf_deadline: Duration,
    kerf_output_limit: u64,
    snapshot: &HostSnapshot,
) -> Result<HostCompatibilityReport, String> {
    let manifest: ReleaseCompatibilityManifest = serde_json::from_slice(
        &fs::read(manifest_path)
            .map_err(|error| format!("compatibility manifest is unavailable: {error}"))?,
    )
    .map_err(|error| format!("compatibility manifest is invalid: {error}"))?;
    let os = parse_os_release(
        &fs::read_to_string(root.join("etc/os-release"))
            .map_err(|error| format!("os-release is unavailable: {error}"))?,
    )?;
    let kerf = ProcessKerfRunner::new(kerf_executable, kerf_deadline, kerf_output_limit)
        .run_raw(&[OsString::from("--version")])
        .map_err(|error| format!("Kerf version is unavailable: {error}"))?;
    if !kerf.process_succeeded() {
        return Err("Kerf version command failed".into());
    }
    let kerf_version = parse_kerf_version(
        &String::from_utf8(kerf.stdout).map_err(|_| "Kerf version is not UTF-8")?,
    )?;
    let daxfs_format = fs::read_to_string(root.join("sys/module/daxfs/parameters/format_version"))
        .ok()
        .and_then(|value| value.trim().parse().ok());
    Ok(manifest.evaluate(&HostCompatibilityEvidence {
        os_id: required(&os, "ID")?,
        os_version_id: required(&os, "VERSION_ID")?,
        architecture: snapshot.topology.architecture.clone(),
        kernel_release: snapshot.kernel.release.clone(),
        capabilities: snapshot.capabilities.clone(),
        kerf_version,
        kernmux_release: env!("CARGO_PKG_VERSION").into(),
        kernmux_api_major: u32::from(API_MAJOR_VERSION),
        daxfs_format,
        state_schema: STATE_SCHEMA_VERSION,
    }))
}

fn parse_os_release(contents: &str) -> Result<BTreeMap<String, String>, String> {
    let mut fields = BTreeMap::new();
    for line in contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let (key, raw) = line
            .split_once('=')
            .ok_or_else(|| "os-release contains a malformed line".to_string())?;
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte == b'_')
        {
            return Err("os-release contains an invalid key".into());
        }
        let value = if raw.starts_with('"') {
            raw.strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
                .ok_or_else(|| "os-release contains an unterminated quote".to_string())?
        } else {
            raw
        };
        fields.insert(key.into(), value.into());
    }
    Ok(fields)
}

fn required(fields: &BTreeMap<String, String>, key: &str) -> Result<String, String> {
    fields
        .get(key)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| format!("os-release is missing {key}"))
}

fn parse_kerf_version(output: &str) -> Result<String, String> {
    output
        .split_whitespace()
        .last()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| "Kerf version output is empty".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_os_identity_and_kerf_version() {
        let fields = parse_os_release("ID=ubuntu\nVERSION_ID=\"24.04\"\n").unwrap();
        assert_eq!(required(&fields, "ID").unwrap(), "ubuntu");
        assert_eq!(required(&fields, "VERSION_ID").unwrap(), "24.04");
        assert_eq!(
            parse_kerf_version("kerf, version 0.2.0\n").unwrap(),
            "0.2.0"
        );
        assert!(parse_os_release("bad line").is_err());
    }
}
