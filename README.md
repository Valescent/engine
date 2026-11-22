# Valescent Engine

A voxel engine written in Rust.

**Status: pre-alpha.** `0.0.1` through `0.0.7` are released; `0.0.8` is implemented and in acceptance. The world is procedurally generated from a saved seed, streams in and out around the player, and keeps player edits. It has voxel lighting, ambient occlusion, a day/night cycle, distance fog, transparent animated water, and three LOD levels out to a configurable render distance. The spec-driven v1 terrain stack runs as an opt-in demo with LOD1, tile streaming, and its own save format.

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

- `W` / `S` — walk forward or back using the camera yaw
- `A` / `D` — walk left or right
- Mouse — look around while captured; click the window to capture the cursor
- Left click — capture the cursor, then break the targeted block
- Right click — place the selected block
- `Space` — jump
- `1`–`5` — select Stone, Dirt, Grass, Wood, or Leaves
- `Ctrl+S` — save the world
- `V` — toggle vsync
- `F3` — toggle the FPS/frame-time overlay
- `Escape` — release the cursor

Water is drawn after opaque geometry without per-face sorting, so overlapping water can blend out of order. Generated worlds fill basins below sea level.

The world lives in `world/`: `world/seed` holds the generation seed and `world/chunks/` holds only
modified chunks. Unmodified chunks are regenerated from the seed. Chunks save when they unload,
on `Ctrl+S`, and on exit; a modified chunk is applied over its generated blocks on the next load.
`config.toml` sets `world.render_distance_chunks` (default `12`, range `8..=32`). `ENGINE_SEED=<u64>`
chooses the seed of a new world. Set `ENGINE_STRESS_SCENE=1` to load the deterministic 64-chunk
stress scene instead; it never touches `world/`.

See [`docs/dev/controls.md`](docs/dev/controls.md) for the full binding table.

### Terrain v1 demo

The v1 terrain stack runs as an opt-in demo on top of the legacy scene:

```console
$env:TERRAIN_DEMO = "1"
$env:ENGINE_STRESS_SCENE = "1"   # only when world/seed is absent
cargo run --bin engine
```

It builds a 128x128 patch in about 0.9 s, streams neighbouring tiles with prefetch, draws distance
LOD1, and saves edits under `world/terrain-demo/` (header, description, sections, journal). Editing
is asynchronous with an incremental light/mesh rebuild (~24 ms release). A corrupt section shows an
in-app `R`/`Q`/`S` repair prompt. See [`docs/dev/terrain-v1.md`](docs/dev/terrain-v1.md).

## Development

```console
cargo test
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all
```

Profile a local build with Tracy (CPU spans, memory events, GPU timer zones, and plots).
Debug builds profile by default and save a trace to `reports/tracy/`:

```console
cargo run --bin engine
```

Open a trace with the [Tracy GUI](https://github.com/wolfpld/tracy/releases/tag/v0.13.1). Set
`TRACY_CAPTURE_SECONDS=0` to keep profiling on without capturing, or build with
`--no-default-features` to turn profiling off. CI always disables the feature. See
[`docs/dev/profiling.md`](docs/dev/profiling.md).

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
| `scripts/` | Release, changelog, and progress tooling. |
| `.github/` | CI, issue and pull request templates, Dependabot. |

## Documentation

The documentation site is published at **https://valescent.github.io/engine/**.

- [Roadmap](docs/roadmap.md) - versioned milestones, definitions of done, and non-goals.
- [Progress](docs/progress.md) - the computed scoreboard: level, streaks, roadmap bars, and achievements.
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
- [Configuration](docs/dev/configuration.md) - runtime inputs, the config file, and repository tooling.
- [World management](docs/dev/world-management.md) - backups, corruption recovery, and reset.
- [Debugging](docs/dev/debugging.md) - logging, error reporting, and the planned log file.
- [Profiling](docs/dev/profiling.md) - Tracy CPU, memory, and GPU timelines behind the local `tracy` feature.
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
