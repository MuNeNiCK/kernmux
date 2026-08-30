use kernmux_core::host::{HostCapability, LinuxHostProbe};

#[test]
#[ignore = "requires a Multikernel Linux VM"]
fn observes_running_multikernel_host() {
    let observed = LinuxHostProbe::running_host()
        .observe()
        .expect("running Multikernel host must be observable");

    assert!(observed.kernel_release.contains("-mk"));
    assert!(observed.capabilities.supports(HostCapability::Multikernel));
    assert!(
        observed
            .capabilities
            .supports(HostCapability::InstanceLifecycle)
    );
    assert!(!observed.cpus.is_empty());
    assert!(!observed.numa_nodes.is_empty());
    assert!(observed.memory.total_bytes > 0);

    println!(
        "kernel={} cpus={} numa_nodes={} memory_bytes={} capabilities={}",
        observed.kernel_release,
        observed.cpus.len(),
        observed.numa_nodes.len(),
        observed.memory.total_bytes,
        observed.capabilities.iter().count()
    );
}
