# Installation

## Prerequisites

The host must already have:

1. Ubuntu 24.04 x86-64;
2. the exact Multikernel kernel named in
   `packaging/release/compatibility.json`, booted and exposing
   `/sys/fs/multikernel/device_tree`;
3. the exact Kerf version named by that contract on `PATH`;
4. systemd.

The upstream [Multikernel getting-started
guide](https://multikernel.io/getting-started.html) covers kernel, Lazy CMA, and
Kerf setup. Kernmux intentionally does not automate those privileged host
changes.

## Install the package

```sh
sudo dpkg -i kernmux_0.1.0-1_amd64.deb
```

The package installs `kernmuxd`, `kernmuxctl`, the systemd unit,
`/etc/kernmux/kernmuxd.env`, and `/etc/kernmux/release.json`. It creates the
`kernmux` system group and root-owned state directories with group-restricted
permissions. Upgrades preserve the environment conffile and state. Removal
also preserves them; package purge and state removal are separate operator
decisions.

Installation enables `kernmuxd.service` but does not start it. It never invokes
`kerf init`, loads a module, runs kexec, or writes Multikernel sysfs.

## Diagnose before start

`host diagnose` is local and does not connect to the daemon:

```sh
sudo kernmuxctl --pretty host diagnose
```

It emits stable JSON and checks Multikernel sysfs, the packaged release
manifest, bounded `kerf --version`, the local OS/kernel identity, and whether
the daemon socket exists. Missing socket is a warning; a required incompatibility
returns process status 5.

Start the service only after required checks pass:

```sh
sudo systemctl start kernmuxd
sudo systemctl status kernmuxd --no-pager
sudo kernmuxctl --pretty host preflight
```

`host preflight` is the daemon's full compatibility evaluation, including
Multikernel capabilities and state-schema compatibility.

## Authorization

The defaults deny unprivileged API requests. Configure numeric UID/GID role
lists in `/etc/kernmux/kernmuxd.env`; do not rely on socket group membership as
the API authorization policy. Available roles are reader, operator, and
administrator. Restart the daemon after changing configuration.

Kernel and initrd import sources must be regular files inside one of the
administrator-controlled `KERNMUX_IMAGE_ROOTS` directories. The defaults are
`/boot` and `/var/lib/kernmux/images`.

Review the complete environment template at
`packaging/systemd/kernmuxd.env.example` before deployment.
