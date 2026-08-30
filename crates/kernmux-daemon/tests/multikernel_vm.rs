use std::{convert::Infallible, env, ffi::OsString, path::PathBuf, time::Duration};

use kernmux_api::v1::{Capability, InstanceId, OperationState};
use kernmux_daemon::inventory::{
    FilesystemInventorySource, InventoryService, ProcessInventorySource,
};
use kernmux_daemon::{
    lifecycle::{
        CreateRequest, DevicePlacementError, ExpectedState, InstanceRequest, KerfInvocation,
        LifecyclePlanError, LifecycleRequest, LoadRequest, MemoryPlacementError, StopRequest,
        UpdateRequest,
    },
    lifecycle_executor::{
        KerfRunner, KerfTermination, LifecycleExecutionError, LifecycleExecutor, LifecycleOutcome,
        ProcessKerfRunner,
    },
};

#[test]
#[ignore = "requires a Multikernel Linux VM"]
fn assembles_running_host_snapshot() {
    let mut inventory = InventoryService::new(FilesystemInventorySource::running_host());
    let snapshot = inventory
        .refresh()
        .expect("running host snapshot must assemble");

    assert!(snapshot.kernel.multikernel_enabled);
    assert!(!snapshot.topology.cpus.is_empty());
    assert!(!snapshot.topology.numa_nodes.is_empty());
    println!(
        "generation={} pool_cpus={} instances={} transactions={}",
        snapshot.generation.0,
        snapshot.resource_pool.cpu_hardware_ids.len(),
        snapshot.instances.len(),
        snapshot.transactions.len()
    );
}

#[test]
#[ignore = "requires a Multikernel Linux VM and KERNMUXD_BINARY"]
fn assembles_snapshot_through_isolated_probe() {
    let binary = env::var_os("KERNMUXD_BINARY").expect("KERNMUXD_BINARY must identify kernmuxd");
    let source = ProcessInventorySource::new(
        binary,
        [OsString::from("--inventory-helper")],
        Duration::from_secs(5),
    );
    let mut inventory = InventoryService::new(source);
    let snapshot = inventory
        .refresh()
        .expect("isolated running host snapshot must assemble");

    assert!(snapshot.kernel.multikernel_enabled);
    assert!(!snapshot.topology.cpus.is_empty());
    println!(
        "generation={} health={:?} pool_cpus={} instances={} transactions={}",
        snapshot.generation.0,
        snapshot.health,
        snapshot.resource_pool.cpu_hardware_ids.len(),
        snapshot.instances.len(),
        snapshot.transactions.len()
    );
}

#[test]
#[ignore = "requires a Multikernel Linux VM with Kerf"]
fn runs_kerf_through_bounded_process_backend() {
    let invocation = KerfInvocation {
        arguments: vec![OsString::from("show")],
        expected_state: ExpectedState::Absent(InstanceId(511)),
        mutates_kernel: false,
    };
    let mut runner = ProcessKerfRunner::system(Duration::from_secs(5), 1024 * 1024);

    let result = runner.run(&invocation).expect("Kerf show must execute");

    assert_eq!(result.termination, KerfTermination::Exited(0));
    assert!(String::from_utf8_lossy(&result.stdout).contains("No instances found"));
}

#[test]
#[ignore = "mutates the configured Multikernel Linux VM"]
fn reconciles_complete_kerf_lifecycle() {
    let daemon = env::var_os("KERNMUXD_BINARY").expect("KERNMUXD_BINARY must identify kernmuxd");
    let kernel = PathBuf::from(
        env::var_os("KERNMUX_KERNEL").expect("KERNMUX_KERNEL must identify the spawn kernel"),
    );
    let initrd = PathBuf::from(
        env::var_os("KERNMUX_INITRD").expect("KERNMUX_INITRD must identify the spawn initrd"),
    );
    let source = ProcessInventorySource::new(
        daemon,
        [OsString::from("--inventory-helper")],
        Duration::from_secs(10),
    );
    let inventory = InventoryService::new(source);
    let runner = ProcessKerfRunner::system(Duration::from_secs(120), 4 * 1024 * 1024);
    let mut executor = LifecycleExecutor::new(runner, inventory);

    let generation = executor.refresh_snapshot().unwrap().generation;
    assert_succeeded(executor.execute(&LifecycleRequest::Create(CreateRequest {
        expected_generation: generation,
        id: InstanceId(1),
        name: "kernmux-validation".into(),
        cpu_hardware_ids: vec![4, 5],
        memory_bytes: 1_073_741_824,
    })));

    let generation = executor.refresh_snapshot().unwrap().generation;
    assert_succeeded(executor.execute(&LifecycleRequest::Update(UpdateRequest {
        instance: request(generation),
        cpu_hardware_ids: Some(vec![4, 5, 6]),
        memory_bytes: None,
        device_ids: None,
        dry_run: false,
    })));

    let generation = executor.refresh_snapshot().unwrap().generation;
    assert_succeeded(executor.execute(&LifecycleRequest::Load(LoadRequest {
        instance: request(generation),
        kernel,
        initrd: Some(initrd),
        cmdline: Some("kerf.entrypoint=/bin/sh console=mktty0".into()),
    })));

    let generation = executor.refresh_snapshot().unwrap().generation;
    assert_succeeded(executor.execute(&LifecycleRequest::Start(request(generation))));

    let generation = executor.refresh_snapshot().unwrap().generation;
    assert_succeeded(executor.execute(&LifecycleRequest::Stop(StopRequest {
        instance: request(generation),
        force: false,
    })));

    let generation = executor.refresh_snapshot().unwrap().generation;
    assert_succeeded(executor.execute(&LifecycleRequest::Unload(request(generation))));

    let generation = executor.refresh_snapshot().unwrap().generation;
    assert_succeeded(executor.execute(&LifecycleRequest::Delete(request(generation))));
    assert!(executor.refresh_snapshot().unwrap().instances.is_empty());
}

#[test]
#[ignore = "requires a prepared fragmented Multikernel memory pool VM"]
fn rejects_peer_memory_overlap_before_running_kerf() {
    struct NeverRunner;

    impl KerfRunner for NeverRunner {
        type Error = Infallible;

        fn run(
            &mut self,
            _invocation: &KerfInvocation,
        ) -> Result<kernmux_daemon::lifecycle_executor::KerfRunResult, Self::Error> {
            panic!("memory overlap must be rejected before Kerf is run")
        }
    }

    const GIB: u64 = 1 << 30;
    let daemon = env::var_os("KERNMUXD_BINARY").expect("KERNMUXD_BINARY must identify kernmuxd");
    let source = ProcessInventorySource::new(
        daemon,
        [OsString::from("--inventory-helper")],
        Duration::from_secs(10),
    );
    let inventory = InventoryService::new(source);
    let mut executor = LifecycleExecutor::new(NeverRunner, inventory);
    let snapshot = executor.refresh_snapshot().unwrap();
    let alpha = snapshot
        .instances
        .iter()
        .find(|instance| instance.name == "alpha")
        .expect("prepared VM must contain alpha");
    let beta = snapshot
        .instances
        .iter()
        .find(|instance| instance.name == "beta")
        .expect("prepared VM must contain beta");
    let alpha_base = alpha
        .resources
        .memory_base
        .expect("alpha must expose its authoritative memory base");
    assert_eq!(alpha.resources.memory_bytes, GIB + GIB / 2);
    assert_eq!(beta.resources.memory_base, Some(alpha_base + GIB + GIB / 2));
    assert_eq!(beta.resources.memory_bytes, GIB / 2);

    let outcome = executor.execute(&LifecycleRequest::Update(UpdateRequest {
        instance: InstanceRequest {
            expected_generation: snapshot.generation,
            id: alpha.id,
        },
        cpu_hardware_ids: None,
        memory_bytes: Some(2 * GIB),
        device_ids: None,
        dry_run: false,
    }));

    assert!(matches!(
        outcome,
        Err(LifecycleExecutionError::Plan(
            LifecyclePlanError::MemoryPlacement(MemoryPlacementError::OverlapsInstance {
                peer_name
            })
        )) if peer_name == "beta"
    ));
}

#[test]
#[ignore = "requires a prepared Multikernel VM with an assigned PCI device"]
fn rejects_peer_device_conflict_before_running_kerf() {
    struct NeverRunner;

    impl KerfRunner for NeverRunner {
        type Error = Infallible;

        fn run(
            &mut self,
            _invocation: &KerfInvocation,
        ) -> Result<kernmux_daemon::lifecycle_executor::KerfRunResult, Self::Error> {
            panic!("device ownership conflict must be rejected before Kerf is run")
        }
    }

    const PCI_ID: &str = "0000:06:00.0";
    let daemon = env::var_os("KERNMUXD_BINARY").expect("KERNMUXD_BINARY must identify kernmuxd");
    let source = ProcessInventorySource::new(
        daemon,
        [OsString::from("--inventory-helper")],
        Duration::from_secs(10),
    );
    let inventory = InventoryService::new(source);
    let mut executor = LifecycleExecutor::new(NeverRunner, inventory);
    let snapshot = executor.refresh_snapshot().unwrap();
    let alpha = snapshot
        .instances
        .iter()
        .find(|instance| instance.name == "alpha")
        .expect("prepared VM must contain alpha");
    let beta = snapshot
        .instances
        .iter()
        .find(|instance| instance.name == "beta")
        .expect("prepared VM must contain beta");
    assert!(
        snapshot
            .capabilities
            .contains(&Capability::DeviceAssignment)
    );
    let device = snapshot
        .resource_pool
        .devices
        .iter()
        .find(|device| device.pci_id == PCI_ID)
        .expect("prepared VM must expose the managed PCI device");

    assert_eq!(alpha.resources.device_ids, [PCI_ID]);
    assert!(beta.resources.device_ids.is_empty());
    assert!(
        !snapshot
            .resource_pool
            .available_device_ids
            .iter()
            .any(|pci_id| pci_id == PCI_ID)
    );
    assert_eq!(device.iommu_group, Some(14));
    assert_eq!(device.iommu_group_members, [PCI_ID]);

    let outcome = executor.execute(&LifecycleRequest::Update(UpdateRequest {
        instance: InstanceRequest {
            expected_generation: snapshot.generation,
            id: beta.id,
        },
        cpu_hardware_ids: None,
        memory_bytes: None,
        device_ids: Some(vec![PCI_ID.into()]),
        dry_run: false,
    }));

    assert!(matches!(
        outcome,
        Err(LifecycleExecutionError::Plan(
            LifecyclePlanError::DevicePlacement(DevicePlacementError::OwnedByPeer {
                pci_id,
                peer_name
            })
        )) if pci_id == PCI_ID && peer_name == "alpha"
    ));
}

#[test]
#[ignore = "requires a prepared Multikernel VM with beta and an available isolated PCI device"]
fn reconciles_isolated_device_assignment() {
    const PCI_ID: &str = "0000:06:00.0";
    let daemon = env::var_os("KERNMUXD_BINARY").expect("KERNMUXD_BINARY must identify kernmuxd");
    let source = ProcessInventorySource::new(
        daemon,
        [OsString::from("--inventory-helper")],
        Duration::from_secs(10),
    );
    let inventory = InventoryService::new(source);
    let runner = ProcessKerfRunner::system(Duration::from_secs(30), 1024 * 1024);
    let mut executor = LifecycleExecutor::new(runner, inventory);
    let snapshot = executor.refresh_snapshot().unwrap();
    let beta = snapshot
        .instances
        .iter()
        .find(|instance| instance.name == "beta")
        .expect("prepared VM must contain beta");
    let device = snapshot
        .resource_pool
        .devices
        .iter()
        .find(|device| device.pci_id == PCI_ID)
        .expect("prepared VM must expose the managed PCI device");

    assert!(beta.resources.device_ids.is_empty());
    assert!(
        snapshot
            .resource_pool
            .available_device_ids
            .iter()
            .any(|pci_id| pci_id == PCI_ID)
    );
    assert_eq!(device.iommu_group, Some(14));
    assert_eq!(device.iommu_group_members, [PCI_ID]);

    assert_succeeded(executor.execute(&LifecycleRequest::Update(UpdateRequest {
        instance: InstanceRequest {
            expected_generation: snapshot.generation,
            id: beta.id,
        },
        cpu_hardware_ids: None,
        memory_bytes: None,
        device_ids: Some(vec![PCI_ID.into()]),
        dry_run: false,
    })));

    let after = executor.refresh_snapshot().unwrap();
    let beta = after
        .instances
        .iter()
        .find(|instance| instance.name == "beta")
        .unwrap();
    assert_eq!(beta.resources.device_ids, [PCI_ID]);
    assert!(
        !after
            .resource_pool
            .available_device_ids
            .iter()
            .any(|pci_id| pci_id == PCI_ID)
    );
}

fn request(expected_generation: kernmux_api::v1::Generation) -> InstanceRequest {
    InstanceRequest {
        expected_generation,
        id: InstanceId(1),
    }
}

fn assert_succeeded<E: std::fmt::Debug>(outcome: Result<LifecycleOutcome, E>) {
    let outcome = outcome.expect("lifecycle operation must produce an outcome");
    assert_eq!(
        outcome.state,
        OperationState::Succeeded,
        "lifecycle diagnostics: {:?}; process: {:?}",
        outcome.diagnostics,
        outcome.process
    );
}
