# Valescent Engine

A voxel engine written in Rust.

**Status: pre-alpha.** `0.0.1` and `0.0.2` are released. `0.0.3` (chunks and meshes) is implemented and awaiting release. Nothing past that is built yet.

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
- `Space` — add or remove the raised voxel on the chunk boundary

See [`docs/dev/controls.md`](docs/dev/controls.md) for the full binding table, including the
planned walking controls.

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
- [FAQ and troubleshooting](docs/faq.md) - common questions and a symptom-first index.
- [Architecture decision records](docs/decisions/index.md) - why the engine uses Rust, `wgpu`, `winit`, `glam`, `bytemuck`, and Tracy.
- [Architecture overview](docs/dev/architecture.md) - module ownership and the current frame flow.
- [Shader authoring](docs/dev/shaders.md) - WGSL conventions, pipeline interfaces, and validation.
- [Texture and asset pipeline](docs/dev/textures.md) - procedural block tiles, layer table, and upload path.
- [v0.0.3 spec](docs/specs/v0.0.3.md) - normative chunk, meshing, and renderer contract.
- [v0.0.4 spec](docs/specs/v0.0.4.md) - normative player, interaction, texture, and persistence contract.
- [v0.0.5 spec](docs/specs/v0.0.5.md) - normative greedy meshing, LOD, frustum, async, and stress-scene contract.
- [v0.0.6 spec](docs/specs/v0.0.6.md) - normative light, AO, day/night, fog, water, and VCHK v2 contract.
- [v0.0.7 spec](docs/specs/v0.0.7.md) - normative generation, biomes, streaming, modified saves, and VCHK v3 contract.
- [Configuration](docs/dev/configuration.md) - runtime inputs, the planned config file, and repository tooling.
- [World management](docs/dev/world-management.md) - backups, corruption recovery, and reset.
- [Debugging](docs/dev/debugging.md) - logging, error reporting, and the planned log file.
- [Profiling](docs/dev/profiling.md) - Tracy, wgpu-profiler, and the planned CPU/GPU timeline.
- [Benchmark results](docs/dev/benchmarks.md) - pool_sim methodology and run log.
- [CI](docs/dev/ci.md) - GitHub Actions workflows, local checks, and current gaps.
- [Testing](docs/dev/testing.md) - headless tests and manual GPU acceptance.
- [Versioning](docs/dev/versioning.md) - semantic versions, tags, the changelog, and compatibility policy.
- [Public API](docs/dev/public-api.md) - current surface and stability gate.
- [Glossary](docs/dev/glossary.md) - preferred terms for the engine, project process, and tools.
- [Release checklist](docs/dev/release-checklist.md) - versioning, tagging, and post-release verification.
- [Changelog](CHANGELOG.md) - user-visible changes.

## Contributing

Contributions are welcome. Read [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) first: commits follow [Conventional Commits](https://www.conventionalcommits.org), user-facing changes need a changelog entry, and contributions require accepting [`docs/CLA.md`](docs/CLA.md).

## License

Copyright (c) 2026 Valescent.

Licensed under the [PolyForm Shield License 1.0.0](LICENSE), with the `Required Notice` line preserved.
