#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
package_version=${KERNMUX_PACKAGE_VERSION:-0.1.0-1}
package_arch=${KERNMUX_PACKAGE_ARCH:-amd64}
rust_target=${KERNMUX_RUST_TARGET:-x86_64-unknown-linux-musl}
output_dir=${KERNMUX_OUTPUT_DIR:-"$repo_root/dist"}
source_date_epoch=${SOURCE_DATE_EPOCH:-$(git -C "$repo_root" log -1 --format=%ct)}
export SOURCE_DATE_EPOCH=$source_date_epoch
daemon_binary="$repo_root/target/$rust_target/release/kernmuxd"
client_binary="$repo_root/target/$rust_target/release/kernmuxctl"

case "$package_version" in
    ''|*[!0-9A-Za-z.+:~_-]*)
        echo "invalid package version" >&2
        exit 2
        ;;
esac

if ! command -v dpkg-deb >/dev/null 2>&1; then
    echo "dpkg-deb is required" >&2
    exit 3
fi

if [ ! -x "$daemon_binary" ] || [ ! -x "$client_binary" ]; then
    cargo build --locked --release --target "$rust_target" \
        --manifest-path "$repo_root/Cargo.toml" \
        -p kernmux-daemon -p kernmux-cli
fi

stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT HUP INT TERM
chmod 0755 "$stage"

install -d "$stage/DEBIAN" "$stage/usr/bin" "$stage/lib/systemd/system"
install -d "$stage/etc/kernmux" "$stage/usr/share/doc/kernmux"
install -m 0755 "$daemon_binary" "$stage/usr/bin/kernmuxd"
install -m 0755 "$client_binary" "$stage/usr/bin/kernmuxctl"
install -m 0644 "$repo_root/packaging/systemd/kernmuxd.service" \
    "$stage/lib/systemd/system/kernmuxd.service"
install -m 0640 "$repo_root/packaging/systemd/kernmuxd.env.example" \
    "$stage/etc/kernmux/kernmuxd.env"
install -m 0644 "$repo_root/packaging/release/compatibility.json" \
    "$stage/etc/kernmux/release.json"
install -m 0644 "$repo_root/packaging/debian/copyright" \
    "$stage/usr/share/doc/kernmux/copyright"

cat >"$stage/DEBIAN/control" <<EOF
Package: kernmux
Version: $package_version
Section: admin
Priority: optional
Architecture: $package_arch
Maintainer: Kernmux Contributors
Depends: adduser, init-system-helpers, systemd
Description: Multikernel Linux host management plane
 Kernmux provides a privileged local daemon and an unprivileged automation
 client for managing resources and peer-kernel instances on Multikernel Linux.
EOF

cat >"$stage/DEBIAN/conffiles" <<'EOF'
/etc/kernmux/kernmuxd.env
EOF

cat >"$stage/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if ! getent group kernmux >/dev/null; then
    addgroup --system kernmux
fi
chown root:kernmux /etc/kernmux/kernmuxd.env
chmod 0640 /etc/kernmux/kernmuxd.env
install -d -o root -g kernmux -m 0750 /var/lib/kernmux
install -d -o root -g kernmux -m 0750 /var/lib/kernmux/images
if command -v deb-systemd-helper >/dev/null; then
    deb-systemd-helper unmask kernmuxd.service >/dev/null || true
    deb-systemd-helper enable kernmuxd.service >/dev/null || true
fi
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null || true
fi
exit 0
EOF

cat >"$stage/DEBIAN/prerm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = remove ]; then
    if [ -d /run/systemd/system ]; then
        systemctl stop kernmuxd.service >/dev/null || true
    fi
    if command -v deb-systemd-helper >/dev/null; then
        deb-systemd-helper disable kernmuxd.service >/dev/null || true
    fi
fi
exit 0
EOF

cat >"$stage/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if [ "$1" = purge ] && command -v deb-systemd-helper >/dev/null; then
    deb-systemd-helper purge kernmuxd.service >/dev/null || true
fi
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload >/dev/null || true
fi
exit 0
EOF

chmod 0755 "$stage/DEBIAN/postinst" "$stage/DEBIAN/prerm" "$stage/DEBIAN/postrm"
find "$stage" -exec touch -h -d "@$source_date_epoch" {} +
install -d "$output_dir"
artifact="$output_dir/kernmux_${package_version}_${package_arch}.deb"
dpkg-deb --root-owner-group --build "$stage" "$artifact"
"$repo_root/packaging/debian/verify.sh" "$artifact"
printf '%s\n' "$artifact"
