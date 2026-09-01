# Kernmux

Kernmux is an open-source, headless management plane for a single
[Multikernel Linux](https://multikernel.io/getting-started.html) host. It aims
at the host-management problem addressed by ESXi and Proxmox while managing
peer Linux kernels directly on hardware instead of virtual machines behind a
hypervisor.

The v0.1 host service provides:

- resource-pool and kernel-instance lifecycle management;
- CPU, NUMA, memory, device, transaction, and operation inventory;
- immutable, content-addressed kernel and initrd admission;
- asynchronous operations with generation preconditions and reconciliation;
- a privileged daemon, an unprivileged automation client, and a versioned
  local API;
- release compatibility preflight and daemon-independent host diagnostics.

Kernmux is not a desktop application, a GUI toolkit product, a hypervisor, a
cluster manager, or a Multikernel kernel installer. v0.1 is an early host MVP,
not a production-ready release.

## Architecture

```text
kernmuxctl ───────────────┐
                         │ HTTP/JSON over Unix socket
local API client ─ gateway┤
                         ▼
                    kernmuxd (root)
                         │ validated, bounded execution
                         ├──────── Kerf
                         └──────── /sys/fs/multikernel
                                        │
                                   peer kernels
```

The CLI and any future management clients use the same management
plane. The privileged daemon has no registry credentials and does not pull
images from the network. See [Architecture](docs/architecture.md) for the
security and bundle-acquisition boundaries.

## Supported v0.1 host

The release package contains a strict compatibility contract. The current
contract supports Ubuntu 24.04 on x86-64 with kernel
`7.0.0-mk2-kernmux-mk+`, Kerf `0.2.0`, Kernmux `0.1.0`, and API v1. The file
[`packaging/release/compatibility.json`](packaging/release/compatibility.json)
is authoritative.

Kernmux assumes the compatible Multikernel kernel and
[Kerf](https://github.com/multikernel/kerf) are already installed. The `.deb`
does not replace the running kernel, load kernel modules, initialize a resource
pool, or create instances.

## Install

```sh
sudo dpkg -i kernmux_0.1.0-1_amd64.deb
sudo kernmuxctl --pretty host diagnose
sudo systemctl start kernmuxd
sudo kernmuxctl --pretty host preflight
```

Installation enables the service but deliberately does not force-start it.
systemd starts it only when Multikernel sysfs and the release contract are
present and Kerf is executable. API authorization is deny-by-default; root is
the initial administrator.

See [Installation](docs/installation.md) for prerequisites and configuration,
and [Operations](docs/operations.md) for the complete managed-image lifecycle
and safe recovery procedure.

## Build and test

Rust 1.98.0 is pinned by `rust-toolchain.toml`.

```sh
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

On an Ubuntu build environment with `dpkg-deb` and the
`x86_64-unknown-linux-musl` target available:

```sh
packaging/debian/build.sh
```

Release artifacts must be assembled from a clean commit:

```sh
packaging/release/build.sh
```

## v0.1 limitations

- one local host; no clustering, HA, live migration, or remote API listener;
- no web management client, multi-host inventory, or remote identity provider;
- no kernel, Lazy CMA, DAXFS, or Kerf installation automation;
- no registry acquisition in the privileged daemon;
- imported image garbage collection is not yet exposed;
- operation history is bounded and local to the running daemon.

## License

Apache License 2.0. See [LICENSE](LICENSE).
