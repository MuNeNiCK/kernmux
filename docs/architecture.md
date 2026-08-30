# Architecture

Kernmux v0.1 is a single-node control plane for Multikernel Linux. It manages
peer kernels; it does not introduce host/guest virtualization semantics.

## Process boundary

`kernmuxctl` connects to `kernmuxd` through an HTTP/1.1 JSON API on a Unix
domain socket. The socket controls filesystem admission, while the daemon uses
kernel-provided peer credentials and an explicit UID/GID role policy for API
authorization. Root is always an administrator. Unprivileged access is denied
unless configured in `/etc/kernmux/kernmuxd.env`.

`kernmuxd` is the only privileged Kernmux process. It validates topology,
resource ownership, lifecycle state, image identity, request size, concurrency,
and expected generation before invoking Kerf. Commands are constructed as
argument vectors without a shell, have deadlines, and have bounded output.

## Authoritative state and operations

Multikernel sysfs is authoritative for host, pool, instance, and transaction
state. Kerf performs mutations. Kernmux observes state again after every Kerf
invocation; a process exit code alone is never treated as proof that a mutation
succeeded.

Mutations are asynchronous and return an operation ID. Clients must wait for a
terminal operation state and then refresh authoritative state. Every mutation
also carries an expected generation so stale automation fails closed.

An `indeterminate` operation means Kernmux could not prove the resulting kernel
state. It is not safe to retry automatically. The operator must inspect the
operation diagnostics and refreshed host state first.

## Managed images

Kernel and initrd files are admitted into an immutable, content-addressed local
store. Their public IDs are canonical `sha256:` identifiers. Lifecycle requests
can refer to those IDs with `instance load-image`; clients never need the
daemon's private storage paths.

Bundle construction, OCI transport, registry authentication, and download
policy belong to an unprivileged acquisition layer outside `kernmuxd`. That
layer may later preserve a bundle identity and OCI digest as metadata while
Kernmux continues to own host-local verification, admission, cache integrity,
and lifecycle references. Raw file import remains the v0.1 bootstrap path.

## Service hardening

The packaged systemd unit runs as root with group `kernmux`, a private runtime
directory, a restrictive umask, read-only system paths, a small writable path
set, Unix-only networking, namespace restrictions, bounded file descriptors,
and restart-on-failure. Package lifecycle scripts do not run Kerf mutations,
load modules, use kexec, or write Multikernel sysfs.

Kernmux does not protect an operator from concurrent out-of-band Kerf commands.
Treat `kernmuxd` as the sole writer while it is managing the host.
