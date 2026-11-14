# Valescent Engine

A voxel engine written in Rust.

**Status: pre-alpha.** `0.0.1` is the foundation milestone: a window, a camera, and cubes drawn from a voxel set rather than hardcoded draw calls. Nothing in [`docs/roadmap.md`](docs/roadmap.md) past that is built yet.

This is **source-available, not open source**. The code is under the [PolyForm Shield License 1.0.0](LICENSE): you may read, run, modify, and redistribute it, but not use it to provide a product that competes with it. PolyForm Shield is not OSI-approved.

## Requirements

- Rust **1.99.1** or newer — pinned in [`rust-toolchain.toml`](rust-toolchain.toml); `rustup` installs it automatically
- A GPU and driver supporting Vulkan, Direct3D 12, or Metal (via `wgpu`)
- Windows: the MSVC build tools installed through the Visual Studio Build Tools

## Build and run

```console
git clone https://github.com/Valescent/engine
cd engine
cargo run
```

## Development

```console
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all
```

Enable the commit-message hook once per clone:

```console
git config core.hooksPath .githooks
```

See [`docs/dev/setup.md`](docs/dev/setup.md) for platform details and [`docs/dev/conventions.md`](docs/dev/conventions.md) for code and commit conventions.

## Layout

| Path | Contents |
|---|---|
| `src/` | Library and binary crate. |
| `docs/` | [Roadmap](docs/roadmap.md), [decision records](docs/README.md), development guide. |
| `scripts/` | Release tooling. |
| `.github/` | CI, issue and pull request templates, Dependabot. |

## Documentation

- [Roadmap](docs/roadmap.md) — versioned milestones, definitions of done, and non-goals.
- [Architecture decision records](docs/README.md) — why the engine uses Rust, `wgpu`, `winit`, `glam`, and `bytemuck`.
- [Changelog](CHANGELOG.md) — user-visible changes.

## Contributing

Contributions are welcome. Read [`CONTRIBUTING.md`](CONTRIBUTING.md) first: commits follow [Conventional Commits](https://www.conventionalcommits.org), user-facing changes need a changelog entry, and contributions require accepting [`CLA.md`](CLA.md).

## License

Copyright (c) 2026 Valescent.

Licensed under the [PolyForm Shield License 1.0.0](LICENSE), with the `Required Notice` line preserved.
