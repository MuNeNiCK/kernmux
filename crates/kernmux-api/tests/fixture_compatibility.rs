use std::{fs, path::Path};

use kernmux_api::v1::{InstanceState, OperationKind};
use serde_json::Value;

const FIXTURE_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/v1");
const SUPPORTED_FIXTURE_VERSION: u64 = 1;

fn read_json(relative_path: &str) -> Value {
    let path = Path::new(FIXTURE_ROOT).join(relative_path);
    let bytes = fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn require_supported_version(value: &Value) -> Result<(), String> {
    match value.get("fixture_version").and_then(Value::as_u64) {
        Some(SUPPORTED_FIXTURE_VERSION) | None => Ok(()),
        Some(version) => Err(format!("unsupported fixture version {version}")),
    }
}

fn lifecycle_state(value: &str) -> InstanceState {
    match value {
        "absent" => InstanceState::Absent,
        "ready" => InstanceState::Ready,
        "loaded" => InstanceState::Loaded,
        "active" => InstanceState::Active,
        _ => InstanceState::Unknown,
    }
}

fn operation_kind(value: &str) -> OperationKind {
    match value {
        "init" => OperationKind::InitializeResourcePool,
        "release-pool" => OperationKind::ReleaseResourcePool,
        "create" => OperationKind::CreateInstance,
        "update-dry-run" => OperationKind::UpdateInstance,
        "load" => OperationKind::LoadInstance,
        "exec" => OperationKind::StartInstance,
        "kill" => OperationKind::StopInstance,
        "unload" => OperationKind::UnloadInstance,
        "delete" => OperationKind::DeleteInstance,
        "console-probe" => OperationKind::OpenConsole,
        _ => OperationKind::Unknown,
    }
}

#[test]
fn every_json_fixture_parses_and_uses_a_supported_version() {
    for path in [
        "manifest.json",
        "host/capabilities.json",
        "host/topology.json",
        "kerf/init-report.json",
        "kerf/known-behaviors.json",
        "lifecycle/states.json",
    ] {
        let value = read_json(path);
        require_supported_version(&value)
            .unwrap_or_else(|error| panic!("{path} is incompatible: {error}"));
    }
}

#[test]
fn every_observed_lifecycle_state_maps_to_the_public_model() {
    let fixture = read_json("lifecycle/states.json");
    let states = fixture["states"]
        .as_array()
        .expect("states fixture must contain an array");
    let mapped: Vec<_> = states
        .iter()
        .map(|entry| {
            lifecycle_state(
                entry["state"]
                    .as_str()
                    .expect("every state must be a string"),
            )
        })
        .collect();

    assert_eq!(
        mapped,
        [
            InstanceState::Absent,
            InstanceState::Ready,
            InstanceState::Loaded,
            InstanceState::Active,
            InstanceState::Loaded,
            InstanceState::Ready,
            InstanceState::Absent,
        ]
    );
    assert!(!mapped.contains(&InstanceState::Unknown));
}

#[test]
fn every_observed_operation_maps_to_the_public_model() {
    let path = Path::new(FIXTURE_ROOT).join("lifecycle/operations.jsonl");
    let contents = fs::read_to_string(&path).expect("operations fixture must be readable");
    let kinds: Vec<_> = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: Value = serde_json::from_str(line).expect("each operation must be JSON");
            operation_kind(
                value["operation"]
                    .as_str()
                    .expect("every operation must have a name"),
            )
        })
        .collect();

    assert_eq!(kinds.len(), 10);
    assert!(!kinds.contains(&OperationKind::Unknown));
}

#[test]
fn unsupported_fixture_versions_are_rejected() {
    let fixture = serde_json::json!({ "fixture_version": 2 });
    assert_eq!(
        require_supported_version(&fixture),
        Err("unsupported fixture version 2".into())
    );
}
