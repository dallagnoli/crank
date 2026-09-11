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

See [SPEC.md](SPEC.md) for the product and portability contract.

