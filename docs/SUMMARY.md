# Summary

[Valescent Engine](index.md)

# Getting started

- [Setup](dev/setup.md)
- [Conventions](dev/conventions.md)
- [FAQ and troubleshooting](faq.md)

# Roadmap

- [Roadmap](roadmap.md)
  - [v0.0.1 — The Foundation](dev/v0.0.1-scope.md)
  - [v0.0.2 — Camera](dev/v0.0.2-scope.md)
  - [v0.0.3 — Chunks and Meshes](dev/v0.0.3-scope.md)
  - [v0.0.4 — Playable](dev/v0.0.4-scope.md)
  - [v0.0.5 — Scale and Performance](dev/v0.0.5-scope.md)
  - [v0.0.6 — Atmosphere](dev/v0.0.6-scope.md)
  - [v0.0.7 — Advanced Horizons](dev/v0.0.7-scope.md)
  - [Milestone template](dev/milestone-template.md)

# Specs

- [v0.0.3 — Chunks and Meshes](specs/v0.0.3.md)
- [v0.0.4 — Playable](specs/v0.0.4.md)
- [v0.0.5 — Scale and Performance](specs/v0.0.5.md)
- [v0.0.6 — Atmosphere](specs/v0.0.6.md)
- [v0.0.7 — Advanced Horizons](specs/v0.0.7.md)

# Design

- [Terrain pipeline](design/terrain-pipeline.md)

# Development

- [Architecture overview](dev/architecture.md)
- [Shader authoring](dev/shaders.md)
- [Texture and asset pipeline](dev/textures.md)
- [Coordinates](dev/coordinates.md)
- [Controls](dev/controls.md)
- [Configuration](dev/configuration.md)
- [World management](dev/world-management.md)
- [Debugging](dev/debugging.md)
- [Profiling](dev/profiling.md)
- [Benchmark results](dev/benchmarks.md)
- [CI](dev/ci.md)
- [Testing](dev/testing.md)
- [Versioning](dev/versioning.md)
- [Public API](dev/public-api.md)
- [Glossary](dev/glossary.md)
- [Release checklist](dev/release-checklist.md)

# Decisions

- [Decision records](decisions/index.md)
  - [0001 — Rust for the engine core](decisions/0001-rust-for-engine-core.md)
  - [0002 — wgpu for graphics](decisions/0002-wgpu-for-graphics.md)
  - [0003 — winit for windowing](decisions/0003-winit-for-windowing.md)
  - [0004 — glam for math](decisions/0004-glam-for-math.md)
  - [0005 — bytemuck for GPU data layout](decisions/0005-bytemuck-for-gpu-layout.md)
  - [0006 — Tracy for profiling](decisions/0006-tracy-for-profiling.md)
  - [0007 — Texture array for block textures](decisions/0007-texture-array-for-block-textures.md)
  - [0008 — Chunk save format (VCHK)](decisions/0008-chunk-save-format.md)
  - [0009 — micropool for chunk workers](decisions/0009-micropool-for-chunk-workers.md)
  - [0010 — Two LOD levels for chunks](decisions/0010-two-level-lod-for-chunks.md)
  - [0011 — Voxel lighting with baked AO](decisions/0011-voxel-light-and-baked-ao.md)
  - [0012 — fastnoise-lite with OpenSimplex2](decisions/0012-fastnoise-lite-and-opensimplex2.md)
  - [0013 — Finite player-centred chunk streaming](decisions/0013-player-centred-chunk-streaming.md)
  - [Template](decisions/0000-template.md)

# Contributing

- [Contributing](CONTRIBUTING.md)
- [Code of conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md)
- [Contributor licence agreement](CLA.md)
