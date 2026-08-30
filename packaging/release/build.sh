#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
release_version=${KERNMUX_RELEASE_VERSION:-0.1.0}
package_revision=${KERNMUX_PACKAGE_REVISION:-1}
output_dir=${KERNMUX_OUTPUT_DIR:-"$repo_root/dist"}

case "$release_version" in
    ''|*[!0-9A-Za-z.+~_-]*)
        echo "invalid release version" >&2
        exit 2
        ;;
esac
case "$package_revision" in
    ''|*[!0-9]*)
        echo "invalid package revision" >&2
        exit 2
        ;;
esac

if [ -n "$(git -C "$repo_root" status --porcelain --untracked-files=all)" ]; then
    echo "release artifacts must be built from a clean source tree" >&2
    exit 3
fi

source_date_epoch=${SOURCE_DATE_EPOCH:-$(git -C "$repo_root" log -1 --format=%ct)}
export SOURCE_DATE_EPOCH=$source_date_epoch
export KERNMUX_PACKAGE_VERSION="${release_version}-${package_revision}"
export KERNMUX_OUTPUT_DIR=$output_dir

install -d "$output_dir"
package="$output_dir/kernmux_${KERNMUX_PACKAGE_VERSION}_amd64.deb"
source_archive="$output_dir/kernmux-${release_version}.tar.gz"
compatibility="$output_dir/kernmux-${release_version}-compatibility.json"
checksums="$output_dir/SHA256SUMS"

rm -f -- "$package" "$source_archive" "$compatibility" "$checksums"
"$repo_root/packaging/debian/build.sh" >/dev/null
git -C "$repo_root" archive \
    --format=tar.gz \
    --prefix="kernmux-${release_version}/" \
    --output="$source_archive" \
    HEAD
install -m 0644 "$repo_root/packaging/release/compatibility.json" "$compatibility"

(
    cd "$output_dir"
    sha256sum \
        "$(basename "$package")" \
        "$(basename "$source_archive")" \
        "$(basename "$compatibility")" >"$(basename "$checksums")"
    sha256sum --check "$(basename "$checksums")"
)

printf '%s\n' "$package" "$source_archive" "$compatibility" "$checksums"
