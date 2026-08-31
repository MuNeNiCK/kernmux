#!/bin/sh
set -eu

artifact=${1:?usage: verify.sh PACKAGE.deb}
test -f "$artifact"
command -v dpkg-deb >/dev/null 2>&1

contents=$(dpkg-deb --contents "$artifact")
for path in \
    ./usr/bin/kernmuxd \
    ./usr/bin/kernmuxctl \
    ./usr/bin/kernmux-gateway \
    ./lib/systemd/system/kernmuxd.service \
    ./lib/systemd/system/kernmux-gateway.service \
    ./usr/share/kernmux/web/index.html \
    ./usr/share/kernmux/web/bootstrap.js \
    ./usr/share/kernmux/web/app.js \
    ./usr/share/kernmux/web/app_bg.wasm \
    ./etc/kernmux/kernmuxd.env \
    ./etc/kernmux/release.json \
    ./usr/share/doc/kernmux/copyright
do
    printf '%s\n' "$contents" | grep -F " $path" >/dev/null
done

control_dir=$(mktemp -d)
trap 'rm -rf -- "$control_dir"' EXIT HUP INT TERM
dpkg-deb --control "$artifact" "$control_dir"
grep -Fx '/etc/kernmux/kernmuxd.env' "$control_dir/conffiles" >/dev/null
if grep -F '/etc/kernmux/gateway.token' "$control_dir/conffiles"; then
    echo "generated gateway credential must not be a conffile" >&2
    exit 4
fi
if printf '%s\n' "$contents" | grep -E '(/| )\.beads(/|$)'; then
    echo "internal project metadata must not be packaged" >&2
    exit 5
fi

if grep -E '(kerf[[:space:]]+init|modprobe|insmod|kexec|/sys/fs/multikernel)' \
    "$control_dir/postinst" "$control_dir/prerm" "$control_dir/postrm"
then
    echo "package lifecycle scripts contain forbidden host mutations" >&2
    exit 6
fi
