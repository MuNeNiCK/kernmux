#!/bin/sh
set -eu

package=${1:?usage: vm-smoke.sh PACKAGE.deb [UPGRADE.deb]}
upgrade_package=${2:-}
test -f "$package"
test ! -e /sys/fs/multikernel/device_tree

dpkg -i "$package"
test "$(getent group kernmux | cut -d: -f1)" = kernmux
test "$(id -gn kernmux-web)" = kernmux
test "$(stat -c %U:%G:%a /etc/kernmux/kernmuxd.env)" = root:kernmux:640
test "$(stat -c %U:%G:%a /etc/kernmux/gateway.token)" = kernmux-web:kernmux:400
test "$(stat -c %U:%G:%a /var/lib/kernmux)" = root:kernmux:750
test "$(stat -c %U:%G:%a /var/lib/kernmux/images)" = root:kernmux:750
test "$(systemctl is-enabled kernmuxd.service)" = enabled
test "$(systemctl is-enabled kernmux-gateway.service)" = enabled
systemctl start kernmuxd.service
test "$(systemctl show kernmuxd.service -P ConditionResult)" = no
test "$(systemctl is-active kernmuxd.service)" = inactive
test ! -e /run/kernmux/kernmuxd.sock
systemctl start kernmux-gateway.service
test "$(systemctl is-active kernmux-gateway.service)" = active
curl --fail --silent http://127.0.0.1:9443/ >/dev/null
gateway_token=$(cat /etc/kernmux/gateway.token)
test "${#gateway_token}" -ge 32
gateway_token_digest=$(sha256sum /etc/kernmux/gateway.token | cut -d' ' -f1)
http_status=$(curl --silent --output /dev/null --write-out '%{http_code}' \
    -H "Authorization: Bearer $gateway_token" http://127.0.0.1:9443/api/1.0)
test "$http_status" = 502
kernmuxctl --version
if diagnostic=$(kernmuxctl host diagnose); then
    diagnostic_status=0
else
    diagnostic_status=$?
fi
test "$diagnostic_status" -eq 5
printf '%s\n' "$diagnostic" | grep -F '"compatible":false' >/dev/null

printf '%s\n' '# administrator-marker' >>/etc/kernmux/kernmuxd.env
printf '%s\n' persistent >/var/lib/kernmux/state-sentinel
if [ -n "$upgrade_package" ]; then
    test -f "$upgrade_package"
    dpkg -i "$upgrade_package"
    test "$(sha256sum /etc/kernmux/gateway.token | cut -d' ' -f1)" = "$gateway_token_digest"
fi
grep -Fx '# administrator-marker' /etc/kernmux/kernmuxd.env >/dev/null
test "$(cat /var/lib/kernmux/state-sentinel)" = persistent
test ! -e /run/kernmux/kernmuxd.sock

dpkg --remove kernmux
test -f /etc/kernmux/gateway.token
test -f /etc/kernmux/kernmuxd.env
test "$(cat /var/lib/kernmux/state-sentinel)" = persistent
test ! -e /sys/fs/multikernel/device_tree
