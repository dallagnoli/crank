# Portability and releases

Crank's release workflow builds one raw executable and one SHA-256 file for
each target below. A target is published only when its native test job and
packaging job succeed.

| Artifact | Build and execution environment | Linkage |
| --- | --- | --- |
| `crank-linux-x86_64` | Ubuntu 24.04 x86_64; also launched on Alpine musl | Static musl |
| `crank-linux-aarch64` | Ubuntu 24.04 arm64; also launched on Alpine musl | Static musl |
| `crank-macos-x86_64` | GitHub macOS 15 Intel runner | Apple system libraries |
| `crank-macos-aarch64` | GitHub macOS 15 Apple Silicon runner | Apple system libraries |
| `crank-freebsd-x86_64` | FreeBSD 14.x virtual machine | FreeBSD system libraries |

These are the current tested baselines. Older OS and kernel releases are not
yet advertised. Linux builds use Rust's musl targets and avoid a glibc runtime
dependency. macOS and FreeBSD builds remain native executables with their
platform system-library requirements.

## Bootstrap

The ephemeral launch command for the current stable release is:

```sh
curl -fsSL https://raw.githubusercontent.com/dallagnoli/crank/main/bootstrap/crank.sh | sh
```

Pin a release with:

```sh
curl -fsSL https://raw.githubusercontent.com/dallagnoli/crank/main/bootstrap/crank.sh | sh -s -- --version 0.1.1
```

Pass binary arguments after a second `--`:

```sh
curl -fsSL https://raw.githubusercontent.com/dallagnoli/crank/main/bootstrap/crank.sh | sh -s -- --version 0.1.1 -- --catalog /path/to/catalog
```

The bootstrap selects an artifact from an explicit target map, downloads the
binary and its checksum from the same immutable tagged release, verifies it,
and reconnects all child streams to the controlling terminal. It removes its
private temporary directory and returns the child's exit status.

Checksums detect incomplete or mismatched assets. Publisher authentication is
provided by HTTPS delivery from this repository; release signing has not been
selected yet.
