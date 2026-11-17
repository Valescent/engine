<div class="hero">
<div class="hero-copy">

# A world, one voxel at a time.

Valescent Engine is a voxel engine written in Rust. Four milestones are shipped - a voxel scene, a fly camera, chunked meshes, and a playable world - and every step after that is written down before it is built.

This is **pre-alpha**. The current world is a walkable, editable set of 32×32×32 chunks with procedural block textures, swept collision, and `VCHK` saves. Lighting and larger draw distances are planned in public.

<div class="hero-actions">

[Set up the toolchain](dev/setup.md) [Read the roadmap](roadmap.md)

</div>

<div class="hero-data">

`PRE-ALPHA` <span>/</span> `v0.0.4 SHIPPED` <span>/</span> `RUST + WGPU`

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
<figcaption>Drawn from the current data model: block voxels stored per chunk, meshed as face-culled textured non-indexed cubes, right-handed and Y-up. The highlighted cube stands in for the block outline. Illustration, not a screenshot.</figcaption>
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
- [Benchmark results](dev/benchmarks.md) - pool_sim methodology and run log.
- [Changelog](https://github.com/Valescent/engine/blob/main/CHANGELOG.md) — user-visible changes.
- [Contributing](CONTRIBUTING.md) — commits, changelog entries, pull requests, and the CLA.
- [Security policy](SECURITY.md) — how to report a vulnerability.

<p class="licence-note">Source-available, not open source. The code is under the <a href="https://github.com/Valescent/engine/blob/main/LICENSE">PolyForm Shield License 1.0.0</a>: read it, run it, modify it, and redistribute it, but do not use it to provide a competing product.</p>

</section>
</div>

<section class="home-next">

<div>

## Next on the bench

<span class="version">v0.0.5</span>

### Scale and Performance

The playable world is in place. The next milestone adds greedy meshing, two LOD levels, frustum culling, and async chunk rebuilds, so draw distance grows without losing frame rate.

[Read the v0.0.5 scope](dev/v0.0.5-scope.md) · [Read the v0.0.5 spec](specs/v0.0.5.md)

</div>
<div>

> **Definition of done.** A vast draw distance with a smooth frame rate, and block edits without hitches.

The work after that is already scoped: voxel lighting, day/night, fog, and water in [v0.0.6 — Atmosphere](dev/v0.0.6-scope.md).

</div>
</section>

<section class="home-decisions">

## Decisions, on the record

Six records explain the stack, including the alternatives that lost and the reasons they lost. They are superseded, never rewritten.

| ID | Decision | Status |
|---|---|---|
| [0001](decisions/0001-rust-for-engine-core.md) | Rust for the engine core | Accepted |
| [0002](decisions/0002-wgpu-for-graphics.md) | wgpu for graphics | Accepted |
| [0003](decisions/0003-winit-for-windowing.md) | winit for windowing | Accepted |
| [0004](decisions/0004-glam-for-math.md) | glam for math | Accepted |
| [0005](decisions/0005-bytemuck-for-gpu-layout.md) | bytemuck for GPU data layout | Accepted |
| [0006](decisions/0006-tracy-for-profiling.md) | Tracy for profiling | Accepted |

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
