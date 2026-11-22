<div class="hero">
<div class="hero-copy">

# A world, one voxel at a time.

Valescent Engine is a voxel engine written in Rust. Nine milestones are shipped through **v0.0.9 Far View** - a voxel scene, a fly camera, chunked meshes, a playable world, a scaled renderer, an atmospheric one, a procedurally generated one, packed light, and a kilometre-scale LOD hierarchy - and every step after that is written down before it is built.

This is **pre-alpha**. The current world is a walkable, editable, procedurally generated set of 32x32x32 chunks with greedy meshing, three LODs, frustum culling, seed-native streaming, voxel lighting with baked AO, fog and water, worker-thread remeshing, procedural block textures, swept collision, and `VCHK` saves. The spec-driven v1 terrain stack runs as an opt-in demo with LOD1, tile streaming, and its own save format ([Terrain v1 runtime](dev/terrain-v1.md)). The 0.0.10 release is in progress: the first checkpoint adds hold-to-break, items, inventory, drops, and a versioned player save on that demo ([Gameplay core](dev/gameplay.md)).

<div class="hero-actions">

[Set up the toolchain](dev/setup.md) [Read the roadmap](roadmap.md)

</div>

<div class="hero-data">

`PRE-ALPHA` <span>/</span> `v0.0.9 SHIPPED` <span>/</span> `RUST + WGPU`

</div>

</div>
<figure class="survey">
<div class="survey-stage">
<div class="plate-fallback" aria-hidden="true">
<img src="assets/voxel-fallback.svg" alt="" width="160" height="160">
</div>
<canvas id="voxel-plate" hidden tabindex="0" role="img" aria-keyshortcuts="ArrowLeft ArrowRight" aria-label="Isometric illustration of an island of voxel cubes. Drag or use the arrow keys to orbit."></canvas>
<span class="plate-label tl">Survey 01 / Voxel model</span>
<span class="plate-label tr">Y-up / right-handed</span>
<span class="plate-label bl">Yaw <span data-yaw>028°</span></span>
<span class="plate-label br">Drag / arrow keys</span>
</div>
<figcaption>Drawn from the current data model: block voxels stored per chunk, meshed as face-culled textured indexed quads, right-handed and Y-up. The highlighted cube stands in for the block outline. Illustration, not a screenshot.</figcaption>
</figure>
</div>

<div class="home-split">
<section class="home-start">

## Start here

1. **[Set up the toolchain](dev/setup.md).** Platform prerequisites, the pinned compiler, and the commands you will run every day.
2. **[Read the conventions](dev/conventions.md).** How engine code is written: errors, logging, determinism, tests.
3. **[Walk the roadmap](roadmap.md).** What each version delivers, and what it deliberately excludes.

</section>
<section class="home-reference">

## Reference

- [Architecture overview](dev/architecture.md) — module ownership and the frame flow.
- [Coordinates](dev/coordinates.md) — axes, units, and voxel addresses.
- [Controls](dev/controls.md) - current bindings and the planned ones.
- [FAQ and troubleshooting](faq.md) - common questions and a symptom-first index.
- [Configuration](dev/configuration.md) — environment variables, constants, and tooling files.
- [Decision records](decisions/index.md) — why the engine is built the way it is.
- [Testing](dev/testing.md) - headless tests and manual GPU acceptance.
- [Public API](dev/public-api.md) - current unstable surface and the reference gate.
- [Benchmark results](dev/benchmarks.md) - pool_sim, engine, and headless runs.
- [Changelog](https://github.com/Valescent/engine/blob/main/CHANGELOG.md) — user-visible changes.
- [Contributing](CONTRIBUTING.md) — commits, changelog entries, pull requests, and the CLA.
- [Security policy](SECURITY.md) — how to report a vulnerability.

<p class="licence-note">Source-available, not open source. The code is under the <a href="https://github.com/Valescent/engine/blob/main/LICENSE">PolyForm Shield License 1.0.0</a>: read it, run it, modify it, and redistribute it, but do not use it to provide a competing product.</p>

</section>
</div>

<section class="home-next">

<div>

## Next on the bench

<span class="version">v0.0.10</span>

### Basic mechanics

The v1 terrain demo becomes a playable loop: hold-to-break with hardness and tool tiers, item drops and pickup, an inventory and hotbar, crafting, health and stamina, and a versioned player save. The items, HUD, and inventory are on `main`; crafting, survival, and audio follow.

[Read the v0.0.10 scope](dev/v0.0.10-scope.md) · [Read the gameplay core](dev/gameplay.md)

</div>
<div>

> **Definition of done.** A new player can spawn, gather wood and stone, craft a tool, place blocks, survive a fall, and reopen the run with inventory, player state, dropped items, and edits intact.

The work after that is already drawn up: the target slice and terrain scale in [v0.0.11](dev/roadmap-0.0.10-0.0.19.md), then water, lighting, creatures, caves, environment, and fishing.

</div>
</section>

<section class="home-decisions">

## Decisions, on the record

Twenty records explain the stack, including the alternatives that lost and the reasons they lost. They are superseded, never rewritten.

| ID | Decision | Status |
|---|---|---|
| [0001](decisions/0001-rust-for-engine-core.md) | Rust for the engine core | Accepted |
| [0002](decisions/0002-wgpu-for-graphics.md) | wgpu for graphics | Accepted |
| [0003](decisions/0003-winit-for-windowing.md) | winit for windowing | Accepted |
| [0004](decisions/0004-glam-for-math.md) | glam for math | Accepted |
| [0005](decisions/0005-bytemuck-for-gpu-layout.md) | bytemuck for GPU data layout | Accepted |
| [0006](decisions/0006-tracy-for-profiling.md) | Tracy for profiling | Accepted |
| [0007](decisions/0007-texture-array-for-block-textures.md) | Texture array for block textures | Accepted |
| [0008](decisions/0008-chunk-save-format.md) | Chunk save format (VCHK) | Accepted |
| [0009](decisions/0009-micropool-for-chunk-workers.md) | micropool for chunk workers | Accepted |
| [0010](decisions/0010-two-level-lod-for-chunks.md) | Two LOD levels with conservative downsampling | Accepted; extended by v0.0.8 LOD2 |
| [0011](decisions/0011-voxel-light-and-baked-ao.md) | Voxel lighting with baked AO | Accepted |
| [0012](decisions/0012-fastnoise-lite-and-opensimplex2.md) | fastnoise-lite with OpenSimplex2 | Accepted |
| [0013](decisions/0013-player-centred-chunk-streaming.md) | Finite player-centred streaming | Accepted |
| [0014](decisions/0014-seed-native-lod-hierarchy.md) | Seed-native multi-level LOD sections | Accepted |
| [0015](decisions/0015-texture-references-are-presentation.md) | Per-face texture references are presentation-only | Accepted |
| [0016](decisions/0016-block-texture-mipmap-policy.md) | Block texture mipmap and sampling policy | Accepted |
| [0017](decisions/0017-quad-pool-and-indirect-draws.md) | Quad pool with vertex pulling and indirect draws | Accepted; implemented in v0.0.9 |
| [0018](decisions/0018-egui-for-game-ui.md) | egui for the gameplay UI | Accepted; scoped for v0.0.10 |
| [0019](decisions/0019-kira-for-game-audio.md) | kira with procedural placeholders for audio | Accepted; scoped for v0.0.10 |
| [0020](decisions/0020-minimal-entity-runtime.md) | Minimal entity runtime instead of an ECS | Accepted; scoped for v0.0.10 |

</section>

<div class="closing">

## Source-available, not open source.

You may read, run, modify, and redistribute Valescent Engine. You may not use it to provide a product that competes with it. PolyForm Shield is not OSI-approved.

<div class="closing-links">

[Read the licence](https://github.com/Valescent/engine/blob/main/LICENSE) [Contributing](CONTRIBUTING.md) [GitHub](https://github.com/Valescent/engine)

</div>
</div>

<div class="home-foot">
<span>Valescent Engine · pre-alpha</span>
<a href="https://github.com/Valescent/engine">Source on GitHub</a>
</div>
