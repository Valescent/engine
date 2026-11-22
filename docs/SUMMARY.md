# Summary

[Valescent Engine](index.md)

# Getting started

- [Setup](dev/setup.md)
- [Conventions](dev/conventions.md)
- [FAQ and troubleshooting](faq.md)

# Roadmap

- [Roadmap](roadmap.md)
- [0.0.10–0.0.19 plan](dev/roadmap-0.0.10-0.0.19.md)
- [Progress](progress.md)
  - [v0.0.1 — The Foundation](dev/v0.0.1-scope.md)
  - [v0.0.2 — Camera](dev/v0.0.2-scope.md)
  - [v0.0.3 — Chunks and Meshes](dev/v0.0.3-scope.md)
  - [v0.0.4 — Playable](dev/v0.0.4-scope.md)
  - [v0.0.5 — Scale and Performance](dev/v0.0.5-scope.md)
  - [v0.0.6 — Atmosphere](dev/v0.0.6-scope.md)
  - [v0.0.7 — Advanced Horizons](dev/v0.0.7-scope.md)
  - [v0.0.8 — Scale](dev/v0.0.8-scope.md)
  - [v0.0.9 – Far View](dev/v0.0.9-scope.md)
  - [v0.0.10 – Basic mechanics](dev/v0.0.10-scope.md)
  - [Milestone template](dev/milestone-template.md)

# Specs

- [Terrain](specs/terrain.md)
- [v0.0.3 - Chunks and Meshes](specs/v0.0.3.md)
- [v0.0.4 — Playable](specs/v0.0.4.md)
- [v0.0.5 — Scale and Performance](specs/v0.0.5.md)
- [v0.0.6 — Atmosphere](specs/v0.0.6.md)
- [v0.0.7 — Advanced Horizons](specs/v0.0.7.md)
- [v0.0.8 — Scale](specs/v0.0.8.md)
- [GPU-Driven Voxel Meshing and Rendering Spec v1](<GPU-Driven Voxel Meshing and Rendering Spec v1.md>)

# Design

- [Lighting pipeline](design/lighting-pipeline.md)
- [Terrain pipeline](design/terrain-pipeline.md)
- [Terrain interview notes](design/terrain-interview.md)
- [Terrain build plan](design/terrain-build-plan.md)
- [Terrain build prompt](prompt/terrain-build-prompt.md)
- [Seed-native LOD hierarchy](design/lod-hierarchy.md)
  - [LOD hierarchy design review](dev/lod-hierarchy-review.md)
- [Terrain reference notes](design/terrain-lessons.md)
- [Modular structure](design/modular-structure.md)
- [Visibility audit](design/visibility-audit.md)

# Development

- [Architecture overview](dev/architecture.md)
- [Gameplay core](dev/gameplay.md)
- [UI](dev/ui.md)
- [Terrain v1 runtime](dev/terrain-v1.md)
- [Terrain tools](dev/terrain-tools.md)
- [Shader authoring](dev/shaders.md)
- [Texture and asset pipeline](dev/textures.md)
- [Coordinates](dev/coordinates.md)
- [Controls](dev/controls.md)
- [Configuration](dev/configuration.md)
- [World management](dev/world-management.md)
- [Save format](dev/save-format.md)
- [Debugging](dev/debugging.md)
- [Profiling](dev/profiling.md)
- [Benchmark results](dev/benchmarks.md)
- [Performance backlog](dev/performance-backlog.md)
- [Culling](dev/culling.md)
- [CI](dev/ci.md)
- [Testing](dev/testing.md)
- [Versioning](dev/versioning.md)
- [Public API](dev/public-api.md)
- [Glossary](dev/glossary.md)
- [Release checklist](dev/release-checklist.md)

# Research

- [Engine research](dev/research/README.md)
  - [Memory](dev/research/memory.md)
  - [Rendering](dev/research/rendering.md)
  - [GPU-driven meshing](dev/research/gpu-driven-meshing.md)
  - [Meshing](dev/research/meshing.md)
  - [Geometry representation](dev/research/geometry-representation.md)
  - [Scheduling and streaming](dev/research/scheduling.md)
  - [World generation](dev/research/worldgen.md)
  - [Simulation and tooling](dev/research/simulation-and-tooling.md)

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
  - [0014 — Seed-native multi-level LOD sections](decisions/0014-seed-native-lod-hierarchy.md)
  - [0015 — Per-face texture references are presentation-only](decisions/0015-texture-references-are-presentation.md)
  - [0016 — Block texture mipmap and sampling policy](decisions/0016-block-texture-mipmap-policy.md)
  - [0017 – Quad pool and indirect draws](decisions/0017-quad-pool-and-indirect-draws.md)
  - [0018 – egui for game UI](decisions/0018-egui-for-game-ui.md)
  - [0019 – kira for game audio](decisions/0019-kira-for-game-audio.md)
  - [0020 – Minimal entity runtime](decisions/0020-minimal-entity-runtime.md)
  - [Template](decisions/0000-template.md)

# Contributing

- [Contributing](CONTRIBUTING.md)
- [Code of conduct](CODE_OF_CONDUCT.md)
- [Security policy](SECURITY.md)
- [Contributor licence agreement](CLA.md)
