# Valescent Engine

A voxel engine written in Rust.

**Status: pre-alpha.** `0.0.1` and `0.0.2` are complete: a voxel scene and a fly camera. Nothing in [`docs/roadmap.md`](docs/roadmap.md) past that is built yet.

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

## Controls

- `W` / `S` — move forward or back along the view direction
- `A` / `D` — strafe left or right
- Mouse — look around; click the window to capture the cursor
- `Escape` — release the cursor
- `Space` — add or remove the raised centre voxel

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
| `docs/` | [Roadmap](docs/roadmap.md), [decision records](docs/decisions/index.md), development guide. |
| `scripts/` | Release tooling. |
| `.github/` | CI, issue and pull request templates, Dependabot. |

## Documentation

The documentation site is published at **https://valescent.github.io/engine/**.

- [Roadmap](docs/roadmap.md) - versioned milestones, definitions of done, and non-goals.
- [Architecture decision records](docs/decisions/index.md) - why the engine uses Rust, `wgpu`, `winit`, `glam`, `bytemuck`, and Tracy.
- [Architecture overview](docs/dev/architecture.md) - module ownership and the current frame flow.
- [Testing](docs/dev/testing.md) - headless tests and manual GPU acceptance.
- [Release checklist](docs/dev/release-checklist.md) - versioning, tagging, and post-release verification.
- [Changelog](CHANGELOG.md) - user-visible changes.

## Contributing

Contributions are welcome. Read [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) first: commits follow [Conventional Commits](https://www.conventionalcommits.org), user-facing changes need a changelog entry, and contributions require accepting [`docs/CLA.md`](docs/CLA.md).

## License

Copyright (c) 2026 Valescent.

Licensed under the [PolyForm Shield License 1.0.0](LICENSE), with the `Required Notice` line preserved.
