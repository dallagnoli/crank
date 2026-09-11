# Crank

Crank is a portable terminal application for discovering, inspecting, and
running a curated collection of shell scripts. The project is under active
development; the bundled catalog currently contains harmless fixtures used to
build and verify the platform.

## Development

```console
cargo run
cargo run -- --catalog ./catalog
cargo run -- --catalog ./catalog --validate
```

The local catalog option uses the same parser and validation rules as the
catalog embedded in release binaries.

Inside Crank, use the arrow keys and Enter to browse, `/` to search, `p` to
preview a script, and `?` for the complete key list. While an action is active,
Ctrl-C reaches its terminal session and the reserved Ctrl-X chord requests
cancellation.

See [SPEC.md](SPEC.md) for the product and portability contract.

The tested release targets and bootstrap behavior are documented in
[docs/portability.md](docs/portability.md). No public release has been
published yet.
