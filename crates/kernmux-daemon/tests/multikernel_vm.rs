use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use kernmux_api::v1::{InstanceId, OperationState};
use kernmux_daemon::inventory::{
    FilesystemInventorySource, InventoryService, ProcessInventorySource,
};
use kernmux_daemon::{
    lifecycle::{
        CreateRequest, ExpectedState, InstanceRequest, KerfInvocation, LifecycleRequest,
        LoadRequest, StopRequest, UpdateRequest,
    },
    lifecycle_executor::{
        KerfRunner, KerfTermination, LifecycleExecutor, LifecycleOutcome, ProcessKerfRunner,
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
