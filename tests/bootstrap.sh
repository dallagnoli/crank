#!/bin/sh
set -eu

project_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
bootstrap=$project_root/bootstrap/crank.sh
test_root=$(mktemp -d "${TMPDIR:-/tmp}/crank-bootstrap-test.XXXXXXXX")
trap 'rm -rf "$test_root"' 0 1 2 15

fail() {
    printf 'bootstrap test failed: %s\n' "$*" >&2
    exit 1
}

assert_contains() {
    case "$1" in
        *"$2"*) ;;
        *) fail "expected [$1] to contain [$2]" ;;
    esac
}

help_output=$(/bin/sh "$bootstrap" --help)
assert_contains "$help_output" 'Arguments after -- are passed unchanged'

case_root=$test_root/success
mkdir -p "$case_root/bin" "$case_root/tmp"
: >"$case_root/tty"

cat >"$case_root/artifact" <<'ARTIFACT'
#!/bin/sh
printf '%s\n' "$@" >"$CRANK_TEST_RESULT"
exit 7
ARTIFACT
chmod 700 "$case_root/artifact"

cat >"$case_root/bin/uname" <<'UNAME'
#!/bin/sh
case "$1" in
    -s) printf '%s\n' "${CRANK_TEST_OS:-Linux}" ;;
    -m) printf '%s\n' "${CRANK_TEST_ARCH:-x86_64}" ;;
esac
UNAME
chmod 700 "$case_root/bin/uname"

cat >"$case_root/bin/curl" <<'CURL'
#!/bin/sh
output=
url=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output)
            output=$2
            shift 2
            ;;
        *)
            url=$1
            shift
            ;;
    esac
done
printf '%s\n' "$url" >>"$CRANK_TEST_URLS"
case "$url" in
    *.sha256)
        checksum=$(sha256sum "$CRANK_TEST_ARTIFACT" | awk '{print $1}')
        if [ "${CRANK_TEST_BAD_CHECKSUM:-0}" = 1 ]; then
            checksum=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        fi
        printf '%s  crank-linux-x86_64\n' "$checksum" >"$output"
        ;;
    *) cp "$CRANK_TEST_ARTIFACT" "$output" ;;
esac
CURL
chmod 700 "$case_root/bin/curl"

set +e
PATH="$case_root/bin:$PATH" \
TMPDIR="$case_root/tmp" \
CRANK_TEST_ARTIFACT="$case_root/artifact" \
CRANK_TEST_RESULT="$case_root/result" \
CRANK_TEST_URLS="$case_root/urls" \
CRANK_TTY="$case_root/tty" \
CRANK_RELEASE_BASE_URL=https://releases.example.invalid/crank \
    /bin/sh "$bootstrap" --version 9.8.7 -- --label 'space value' 'semi;colon'
status=$?
set -e
[ "$status" -eq 7 ] || fail "expected child exit 7, got $status"
expected_arguments=$(printf '%s\n' --label 'space value' 'semi;colon')
actual_arguments=$(cat "$case_root/result")
[ "$actual_arguments" = "$expected_arguments" ] || fail 'binary arguments changed'
urls=$(cat "$case_root/urls")
assert_contains "$urls" '/v9.8.7/crank-linux-x86_64'
[ -z "$(find "$case_root/tmp" -mindepth 1 -print -quit)" ] || fail 'temporary files were not cleaned up'

set +e
mismatch_output=$(PATH="$case_root/bin:$PATH" \
    TMPDIR="$case_root/tmp" \
    CRANK_TEST_ARTIFACT="$case_root/artifact" \
    CRANK_TEST_RESULT="$case_root/result" \
    CRANK_TEST_URLS="$case_root/urls" \
    CRANK_TEST_BAD_CHECKSUM=1 \
    CRANK_TTY="$case_root/tty" \
    CRANK_RELEASE_BASE_URL=https://releases.example.invalid/crank \
    /bin/sh "$bootstrap" 2>&1)
mismatch_status=$?
set -e
[ "$mismatch_status" -ne 0 ] || fail 'checksum mismatch unexpectedly succeeded'
assert_contains "$mismatch_output" 'does not match its release checksum'

set +e
unsupported_output=$(CRANK_OS=SunOS CRANK_ARCH=sparc /bin/sh "$bootstrap" 2>&1)
unsupported_status=$?
set -e
[ "$unsupported_status" -ne 0 ] || fail 'unsupported target unexpectedly succeeded'
assert_contains "$unsupported_output" 'unsupported operating system: SunOS'

printf 'bootstrap tests passed\n'
