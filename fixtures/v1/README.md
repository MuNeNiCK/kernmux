# Kernmux contract fixtures v1

These fixtures are sanitized observations from the disposable KVM lab. They
describe upstream Multikernel Linux and Kerf behavior; they are not the public
Kernmux API.

Normalization rules:

- Host names, wall-clock timestamps, usernames, and source checkout paths are
  omitted.
- Kernel and upstream Git revisions are retained because they define the
  compatibility matrix.
- Physical addresses are replaced by symbolic region names in JSON fixtures.
  Representative DTS fixtures use a fixed synthetic address.
- Kernel and initramfs paths in commands use `<VMLINUX>` and `<INITRAMFS>`.
- Console output excludes terminal control sequences.

Consumers must reject an unsupported `fixture_version`. Event consumers should
treat `observed_state` as authoritative and command output as diagnostic data.
In particular, the create fixture records a real case where Kerf exited with an
error after the kernel mutation had succeeded.

The device-tree directory includes source DTS files for review and their
deterministically compiled DTB counterparts for parser tests.
