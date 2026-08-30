use kernmux_daemon::inventory::{FilesystemInventorySource, InventoryService};

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
