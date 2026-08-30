use std::{env, ffi::OsString, time::Duration};

use kernmux_daemon::inventory::{
    FilesystemInventorySource, InventoryService, ProcessInventorySource,
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
