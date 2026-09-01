# Operations

## Mutation contract

All mutations require the current host or image-catalog generation. Mutation
responses are asynchronous `accepted` envelopes, not completion proof. Extract
the operation ID, wait for a terminal state, and refresh the affected resource
before continuing.

```sh
generation=$(sudo kernmuxctl host show | jq -er '.generation')
response=$(sudo kernmuxctl pool set \
  --generation "$generation" --cpus 4-7 --memory 2GiB)
operation=$(printf '%s\n' "$response" | jq -er '.operation.id')

while :; do
  result=$(sudo kernmuxctl operation show "$operation")
  state=$(printf '%s\n' "$result" | jq -er '.data.state')
  case "$state" in
    succeeded) break ;;
    queued|running) sleep 1 ;;
    *) printf '%s\n' "$result" >&2; exit 1 ;;
  esac
done

sudo kernmuxctl --pretty pool show
```

Never treat `indeterminate` as retryable. See [Safe recovery](#safe-recovery).

## Managed-image lifecycle

Image-catalog generations are independent of host generations:

```sh
catalog_generation=$(sudo kernmuxctl image list | jq -er '.generation')
sudo kernmuxctl image import \
  --generation "$catalog_generation" \
  --kind kernel \
  --source /boot/vmlinuz-7.0.0-mk2-kernmux-mk+
```

Wait for the returned operation as shown above, refresh `image list`, and repeat
for the initrd. The catalog returns canonical `sha256:` IDs.

With a configured pool, create and load an instance using managed IDs:

```sh
generation=$(sudo kernmuxctl host show | jq -er '.generation')
sudo kernmuxctl instance create \
  --generation "$generation" --id 1 --name workload \
  --cpus 4-5 --memory 1GiB

# Wait for create, then refresh generation and substitute IDs from image list.
generation=$(sudo kernmuxctl host show | jq -er '.generation')
sudo kernmuxctl instance load-image 1 \
  --generation "$generation" \
  --kernel 'sha256:…' --initrd 'sha256:…' \
  --cmdline 'console=mktty0'
```

Each of `start`, `stop`, `unload`, `delete`, and `pool release` follows the same
wait-refresh-generation pattern. The authoritative lifecycle states are
`ready`, `loaded`, and `active`.

## Safe recovery

Start with read-only evidence:

```sh
sudo kernmuxctl --pretty host diagnose
sudo systemctl status kernmuxd --no-pager
sudo journalctl -u kernmuxd --since -30min --no-pager
sudo kernmuxctl --pretty host preflight       # when the socket is available
sudo kernmuxctl --pretty host show            # authoritative inventory
sudo kernmuxctl --pretty operation list
```

Apply these rules:

1. For `failed`, read operation diagnostics, refresh state, correct the stated
   precondition, and submit with the new generation.
2. For `indeterminate`, stop automation. Do not rerun the command and do not
   issue out-of-band Kerf mutations. Compare Multikernel sysfs/Kerf state with
   `host show` and retain the journal.
3. If diagnostics explicitly report that a host restart is required, stop
   active peer workloads and schedule a controlled reboot. A reboot is not a
   generic retry mechanism.
4. If `kernmuxd` exited, run offline `host diagnose`. Restart it only after
   prerequisites pass, then refresh host state before any mutation. Operation
   history is process-local, but kernel state is reconstructed from sysfs.
5. Do not delete `/var/lib/kernmux`, remove overlays, or purge the package as an
   attempted repair. Preserve evidence until the authoritative state is known.

When escalating a failure, retain `host diagnose`, `host preflight`, `host
show`, the relevant operation envelope, `kerf show`, kernel release, package
version, and the service journal. Redact workload command lines or device data
where required.
