#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
    printf 'usage: %s OS ARCH BINARY\n' "$0" >&2
    exit 2
fi

release_os=$1
release_arch=$2
release_binary=$3
release_name=crank-$release_os-$release_arch
release_directory=${CRANK_DIST_DIR:-dist}

case "$release_name" in
    crank-linux-x86_64 | crank-linux-aarch64 | crank-macos-x86_64 | crank-macos-aarch64 | crank-freebsd-x86_64) ;;
    *)
        printf 'unsupported release target: %s\n' "$release_name" >&2
        exit 2
        ;;
esac

[ -f "$release_binary" ] || {
    printf 'release binary does not exist: %s\n' "$release_binary" >&2
    exit 1
}

mkdir -p "$release_directory"
cp "$release_binary" "$release_directory/$release_name"
chmod 755 "$release_directory/$release_name"

if command -v sha256sum >/dev/null 2>&1; then
    checksum=$(sha256sum "$release_directory/$release_name" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    checksum=$(shasum -a 256 "$release_directory/$release_name" | awk '{print $1}')
elif command -v sha256 >/dev/null 2>&1; then
    checksum=$(sha256 -q "$release_directory/$release_name")
else
    printf 'no supported SHA-256 command found\n' >&2
    exit 1
fi

printf '%s  %s\n' "$checksum" "$release_name" >"$release_directory/$release_name.sha256"
printf 'packaged %s\n' "$release_directory/$release_name"
