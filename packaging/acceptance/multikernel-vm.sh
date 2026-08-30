#!/bin/sh
set -eu

package=${1:?usage: multikernel-vm.sh PACKAGE.deb KERNEL}
kernel=${2:?usage: multikernel-vm.sh PACKAGE.deb KERNEL}
pool_cpus=${KERNMUX_ACCEPT_POOL_CPUS:-4-7}
pool_memory=${KERNMUX_ACCEPT_POOL_MEMORY:-2GiB}
pool_memory_bytes=${KERNMUX_ACCEPT_POOL_MEMORY_BYTES:-2147483648}
instance_cpus=${KERNMUX_ACCEPT_INSTANCE_CPUS:-4-5}
instance_memory=${KERNMUX_ACCEPT_INSTANCE_MEMORY:-1GiB}
instance_memory_bytes=${KERNMUX_ACCEPT_INSTANCE_MEMORY_BYTES:-1073741824}
instance_id=${KERNMUX_ACCEPT_INSTANCE_ID:-1}
instance_name=${KERNMUX_ACCEPT_INSTANCE_NAME:-kernmux-v01-acceptance}
busybox=${KERNMUX_ACCEPT_BUSYBOX:-/usr/bin/busybox}
operation_timeout=${KERNMUX_ACCEPT_OPERATION_TIMEOUT:-180}
active_soak=${KERNMUX_ACCEPT_ACTIVE_SOAK:-5}
socket=/run/kernmux/kernmuxd.sock
instance_owned=false
pool_owned=false
indeterminate=false
staging_initrd=
work=

if [ "${KERNMUX_ACCEPT_MUTATION:-}" != 1 ]; then
    echo "refusing to mutate a host without KERNMUX_ACCEPT_MUTATION=1" >&2
    exit 2
fi
if [ "$(id -u)" -ne 0 ]; then
    echo "the Multikernel acceptance harness must run as root" >&2
    exit 2
fi
for command in cpio dpkg file find gzip install jq sha256sum sort stat systemctl; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "$command is required" >&2
        exit 3
    }
done
test -f "$package"
test -f "$kernel"
test -f /sys/fs/multikernel/device_tree
test -x "$busybox"
file "$busybox" | grep -F 'statically linked' >/dev/null || {
    echo "$busybox must be a statically linked busybox" >&2
    exit 3
}
case "$kernel" in
    /boot/*|/var/lib/kernmux/images/*) ;;
    *)
        echo "kernel must be inside a configured KERNMUX_IMAGE_ROOTS directory" >&2
        exit 3
        ;;
esac

wait_operation() {
    operation=$1
    elapsed=0
    while [ "$elapsed" -lt "$operation_timeout" ]; do
        result=$(kernmuxctl operation show "$operation") || return 1
        state=$(printf '%s\n' "$result" | jq -er '.data.state') || return 1
        case "$state" in
            succeeded)
                return 0
                ;;
            failed|cancelled|unknown)
                printf '%s\n' "$result" >&2
                return 1
                ;;
            indeterminate)
                indeterminate=true
                printf '%s\n' "$result" >&2
                echo "operation is indeterminate; refusing blind recovery" >&2
                return 1
                ;;
            queued|running) ;;
            *)
                printf '%s\n' "$result" >&2
                return 1
                ;;
        esac
        elapsed=$((elapsed + 1))
        sleep 1
    done
    echo "operation $operation did not finish within ${operation_timeout}s" >&2
    return 1
}

submit() {
    response=$("$@")
    operation=$(printf '%s\n' "$response" | jq -er '.operation.id')
    wait_operation "$operation"
}

host_generation() {
    kernmuxctl host show | jq -er '.generation'
}

image_generation() {
    kernmuxctl image list | jq -er '.generation'
}

cleanup() {
    original_status=$1
    trap - EXIT HUP INT TERM
    set +e
    if [ -n "$staging_initrd" ]; then
        rm -f -- "$staging_initrd"
    fi
    if [ -n "$work" ]; then
        rm -rf -- "$work"
    fi
    if [ "$indeterminate" = true ]; then
        echo "cleanup stopped: inspect authoritative state and reboot only if diagnostics require it" >&2
        exit "$original_status"
    fi
    if [ "$instance_owned" = true ] && [ -S "$socket" ]; then
        state=$(kernmuxctl instance show "$instance_id" 2>/dev/null | jq -r '.data.state // empty')
        if [ "$state" = active ]; then
            generation=$(host_generation)
            submit kernmuxctl instance stop "$instance_id" --generation "$generation" || true
            state=$(kernmuxctl instance show "$instance_id" 2>/dev/null | jq -r '.data.state // empty')
        fi
        if [ "$indeterminate" = true ]; then
            echo "cleanup stopped after an indeterminate stop" >&2
            exit "$original_status"
        fi
        if [ "$state" = loaded ]; then
            generation=$(host_generation)
            submit kernmuxctl instance unload "$instance_id" --generation "$generation" || true
            state=$(kernmuxctl instance show "$instance_id" 2>/dev/null | jq -r '.data.state // empty')
        fi
        if [ "$indeterminate" = true ]; then
            echo "cleanup stopped after an indeterminate unload" >&2
            exit "$original_status"
        fi
        if [ "$state" = ready ]; then
            generation=$(host_generation)
            submit kernmuxctl instance delete "$instance_id" --generation "$generation" || true
        fi
    fi
    if [ "$pool_owned" = true ] && [ "$indeterminate" = false ] && [ -S "$socket" ]; then
        remaining=$(kernmuxctl instance list 2>/dev/null | jq -r '.data | length')
        if [ "$remaining" = 0 ]; then
            generation=$(host_generation)
            submit kernmuxctl pool release --generation "$generation" || true
        else
            echo "pool retained because instances remain; inspect the VM manually" >&2
        fi
    fi
    exit "$original_status"
}
trap 'cleanup $?' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

dpkg -i "$package"
systemctl restart kernmuxd.service
elapsed=0
while [ "$elapsed" -lt 30 ]; do
    if [ -S "$socket" ] && kernmuxctl host show >/dev/null 2>&1; then
        break
    fi
    elapsed=$((elapsed + 1))
    sleep 1
done
test -S "$socket"
kernmuxctl host diagnose | jq -e '.compatible == true' >/dev/null
kernmuxctl host preflight | jq -e '.data.compatible == true' >/dev/null

snapshot=$(kernmuxctl host show)
test "$(printf '%s\n' "$snapshot" | jq -er '.data.instances | length')" = 0
test "$(printf '%s\n' "$snapshot" | jq -er '.data.resource_pool.cpu_hardware_ids | length')" = 0
test "$(printf '%s\n' "$snapshot" | jq -er '.data.resource_pool.memory_regions | length')" = 0

work=$(mktemp -d /var/lib/kernmux/kernmux-acceptance.XXXXXX)
mkdir -p "$work/root/bin" "$work/root/proc" "$work/root/sys" "$work/root/dev"
install -m 0755 "$busybox" "$work/root/bin/busybox"
ln -s busybox "$work/root/bin/sh"
ln -s busybox "$work/root/bin/sleep"
ln -s busybox "$work/root/bin/mount"
printf '%s\n' \
    '#!/bin/sh' \
    'mount -t proc proc /proc' \
    'mount -t sysfs sysfs /sys' \
    'while :; do sleep 60; done' >"$work/root/init"
chmod 0755 "$work/root/init"
find "$work/root" -exec touch -h -d '@0' {} +
staging_initrd=/var/lib/kernmux/images/kernmux-acceptance-$$.initrd.gz
(
    cd "$work/root"
    find . -print0 | sort -z | cpio --null -o --format=newc 2>/dev/null | gzip -n -9
) >"$staging_initrd"
chmod 0644 "$staging_initrd"

kernel_id=sha256:$(sha256sum "$kernel" | cut -d' ' -f1)
initrd_id=sha256:$(sha256sum "$staging_initrd" | cut -d' ' -f1)
generation=$(image_generation)
submit kernmuxctl image import --generation "$generation" --kind kernel \
    --source "$kernel" --expected-id "$kernel_id"
generation=$(image_generation)
submit kernmuxctl image import --generation "$generation" --kind initrd \
    --source "$staging_initrd" --expected-id "$initrd_id"
kernmuxctl image show kernel "$kernel_id" | jq -e --arg id "$kernel_id" '.data.id == $id' >/dev/null
kernmuxctl image show initrd "$initrd_id" | jq -e --arg id "$initrd_id" '.data.id == $id' >/dev/null
rm -f -- "$staging_initrd"
staging_initrd=

generation=$(host_generation)
submit kernmuxctl pool set --generation "$generation" --cpus "$pool_cpus" --memory "$pool_memory"
pool_owned=true
kernmuxctl pool show | jq -e \
    --argjson bytes "$pool_memory_bytes" \
    '([.data.memory_regions[].bytes] | add // 0) == $bytes' >/dev/null

generation=$(host_generation)
submit kernmuxctl instance create --generation "$generation" --id "$instance_id" \
    --name "$instance_name" --cpus "$instance_cpus" --memory "$instance_memory"
instance_owned=true
kernmuxctl instance show "$instance_id" | jq -e \
    --argjson bytes "$instance_memory_bytes" \
    '.data.state == "ready" and .data.resources.memory_bytes == $bytes' >/dev/null

generation=$(host_generation)
submit kernmuxctl instance load-image "$instance_id" --generation "$generation" \
    --kernel "$kernel_id" --initrd "$initrd_id" --cmdline 'console=mktty0'
kernmuxctl instance show "$instance_id" | jq -e '.data.state == "loaded" and .data.image.present' >/dev/null

generation=$(host_generation)
submit kernmuxctl instance start "$instance_id" --generation "$generation"
elapsed=0
while [ "$elapsed" -lt "$active_soak" ]; do
    kernmuxctl instance show "$instance_id" | jq -e '.data.state == "active"' >/dev/null
    elapsed=$((elapsed + 1))
    sleep 1
done

generation=$(host_generation)
submit kernmuxctl instance stop "$instance_id" --generation "$generation"
kernmuxctl instance show "$instance_id" | jq -e '.data.state == "loaded"' >/dev/null
generation=$(host_generation)
submit kernmuxctl instance unload "$instance_id" --generation "$generation"
kernmuxctl instance show "$instance_id" | jq -e '.data.state == "ready"' >/dev/null
generation=$(host_generation)
submit kernmuxctl instance delete "$instance_id" --generation "$generation"
instance_owned=false
test "$(kernmuxctl instance list | jq -er '.data | length')" = 0
generation=$(host_generation)
submit kernmuxctl pool release --generation "$generation"
pool_owned=false
test "$(kernmuxctl pool show | jq -er '.data.cpu_hardware_ids | length')" = 0

rm -rf -- "$work"
work=
trap - EXIT HUP INT TERM
echo "v0.1_multikernel_acceptance=ok"
