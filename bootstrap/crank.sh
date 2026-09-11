#!/bin/sh

# This file must parse completely before crank_main runs. That matters when the
# script is supplied to a shell through standard input.

CRANK_DEFAULT_VERSION=0.1.1
CRANK_DEFAULT_RELEASE_BASE=https://github.com/dallagnoli/crank/releases/download

crank_usage() {
    cat <<'USAGE'
Download and launch Crank without installing it.

Usage:
  crank.sh [--version VERSION] [--] [CRANK_ARGUMENTS...]

Options:
  --version VERSION  Download one immutable release (default: 0.1.1)
  -h, --help         Show this help without downloading Crank

Arguments after -- are passed unchanged to the Crank binary.
USAGE
}

crank_error() {
    printf 'crank bootstrap: %s\n' "$*" >&2
}

crank_cleanup() {
    if [ -n "${crank_temp_directory:-}" ] && [ -d "$crank_temp_directory" ]; then
        rm -rf "$crank_temp_directory"
    fi
}

crank_normalize_target() {
    crank_detected_os=${CRANK_OS:-$(uname -s 2>/dev/null)}
    crank_detected_arch=${CRANK_ARCH:-$(uname -m 2>/dev/null)}

    case "$crank_detected_os" in
        Linux | linux) crank_os=linux ;;
        Darwin | darwin | macOS | macos) crank_os=macos ;;
        FreeBSD | freebsd) crank_os=freebsd ;;
        *)
            crank_error "unsupported operating system: ${crank_detected_os:-unknown}"
            return 1
            ;;
    esac

    case "$crank_detected_arch" in
        x86_64 | amd64 | x64) crank_arch=x86_64 ;;
        aarch64 | arm64) crank_arch=aarch64 ;;
        *)
            crank_error "unsupported architecture: ${crank_detected_arch:-unknown}"
            return 1
            ;;
    esac

    case "$crank_os-$crank_arch" in
        linux-x86_64 | linux-aarch64 | macos-x86_64 | macos-aarch64 | freebsd-x86_64)
            crank_artifact=crank-$crank_os-$crank_arch
            ;;
        *)
            crank_error "no Crank release is published for $crank_os-$crank_arch"
            return 1
            ;;
    esac
}

crank_download() {
    crank_download_url=$1
    crank_download_path=$2
    curl --fail --silent --show-error --location \
        --proto '=https' --proto-redir '=https' \
        --output "$crank_download_path" "$crank_download_url"
}

crank_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    elif command -v sha256 >/dev/null 2>&1; then
        sha256 -q "$1"
    else
        crank_error 'no supported SHA-256 command found (sha256sum, shasum, or sha256)'
        return 1
    fi
}

crank_verify() {
    crank_verify_binary=$1
    crank_verify_checksum=$2
    crank_expected=$(sed -n '1{s/[[:space:]].*$//;p;}' "$crank_verify_checksum")
    if [ "${#crank_expected}" -ne 64 ] || \
        [ -n "$(printf '%s' "$crank_expected" | tr -d '0123456789abcdefABCDEF')" ]; then
        crank_error 'release checksum is malformed'
        return 1
    fi
    crank_actual=$(crank_sha256 "$crank_verify_binary") || return 1
    crank_expected=$(printf '%s' "$crank_expected" | tr 'ABCDEF' 'abcdef')
    crank_actual=$(printf '%s' "$crank_actual" | tr 'ABCDEF' 'abcdef')
    if [ "$crank_expected" != "$crank_actual" ]; then
        crank_error 'downloaded binary does not match its release checksum'
        return 1
    fi
}

crank_main() {
    crank_version=${CRANK_VERSION:-$CRANK_DEFAULT_VERSION}
    while [ "$#" -gt 0 ]; do
        case "$1" in
            -h | --help)
                crank_usage
                return 0
                ;;
            --version)
                if [ "$#" -lt 2 ] || [ -z "$2" ]; then
                    crank_error '--version requires a value'
                    return 2
                fi
                crank_version=$2
                shift 2
                ;;
            --)
                shift
                break
                ;;
            *)
                crank_error "unknown bootstrap option: $1"
                crank_error 'use -- before arguments intended for the Crank binary'
                return 2
                ;;
        esac
    done

    case "$crank_version" in
        *[!A-Za-z0-9._-]* | '')
            crank_error "invalid release version: $crank_version"
            return 2
            ;;
    esac

    crank_normalize_target || return 1
    crank_release_base=${CRANK_RELEASE_BASE_URL:-$CRANK_DEFAULT_RELEASE_BASE}
    case "$crank_release_base" in
        https://*) ;;
        *)
            crank_error 'release base URL must use HTTPS'
            return 1
            ;;
    esac
    crank_release_url=$crank_release_base/v$crank_version

    crank_temp_directory=$(mktemp -d "${TMPDIR:-/tmp}/crank.XXXXXXXX") || {
        crank_error 'could not create a private temporary directory'
        return 1
    }
    chmod 700 "$crank_temp_directory" || {
        crank_error 'could not secure the temporary directory'
        crank_cleanup
        return 1
    }
    trap crank_cleanup 0
    trap 'exit 129' 1
    trap 'exit 130' 2
    trap 'exit 143' 15

    crank_binary=$crank_temp_directory/$crank_artifact
    crank_checksum=$crank_binary.sha256
    crank_download "$crank_release_url/$crank_artifact" "$crank_binary" || {
        crank_error "could not download $crank_artifact from release v$crank_version"
        return 1
    }
    crank_download "$crank_release_url/$crank_artifact.sha256" "$crank_checksum" || {
        crank_error "could not download the checksum for $crank_artifact"
        return 1
    }
    crank_verify "$crank_binary" "$crank_checksum" || return 1
    chmod 700 "$crank_binary" || {
        crank_error 'could not make the verified binary executable'
        return 1
    }

    crank_terminal=${CRANK_TTY:-/dev/tty}
    if [ ! -r "$crank_terminal" ] || [ ! -w "$crank_terminal" ]; then
        crank_error "no accessible controlling terminal at $crank_terminal"
        return 1
    fi

    # All three descriptors intentionally reconnect to the same controlling terminal.
    # shellcheck disable=SC2094
    "$crank_binary" "$@" <"$crank_terminal" >"$crank_terminal" 2>"$crank_terminal"
}

crank_main "$@"
