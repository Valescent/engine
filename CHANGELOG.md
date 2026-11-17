# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Optional `tracy` feature for local profiling: CPU spans for the frame, player update, highlight, mesh request, mesh upload, encode, and chunk worker jobs; allocation events from a global allocator; GPU timer zones from `wgpu-profiler`; and eight per-frame plots (frame, GPU, chunks, draws, vertices, rebuild, upload, queued jobs).
- `tracy-profiler` thread lanes are named `chunk worker` once per pool thread.

### Changed

- CI runs Clippy, tests, and rustdoc without `--all-features`, so the profiling feature's C++ client stays out of the runners.
- The engine keeps rendering while the window is unfocused, so a live profiler keeps receiving frames instead of freezing the capture. Input remains focus-gated and occlusion still pauses the loop.

## [0.0.5] - 2026-11-17

### Added

- Greedy meshing: adjacent coplanar block faces merge into one quad with UVs in block units; a flat 32x32 chunk now emits far fewer vertices than the v0.0.4 face-culled mesher.
- Distance LOD: chunks within four chunk units render at full resolution, chunks at eight or more render a 2x2x2-downsampled mesh, and the gap keeps the current level so boundaries do not flicker.
- Frustum culling per chunk AABB, so chunks behind or beside the camera issue no draw calls.
- Asynchronous meshing on a bounded micropool worker pool, with per-chunk revisions, urgent edit rebuilds, background LOD work, and a two-mesh-per-frame upload cap.
- A deterministic 64-chunk stress scene on `ENGINE_STRESS_SCENE=1`; it never reads or writes `world/chunks/`.
- Per-second debug metrics for frame time, visible and total chunks, visible vertices, draw calls, worker rebuild time, and upload time.

### Changed

- The texture sampler repeats in U and V so merged greedy quads tile correctly instead of clamping.
- The camera far plane moved from 100 m to 512 m so the 10-chunk stress draw distance is visible.
- `micropool` moved from a development dependency to a runtime dependency.

## [0.0.4] - 2026-11-17

### Added

- Walking player with gravity, jumping, and swept AABB collision against voxel terrain.
- Block types: stone, dirt, grass, wood, and leaves, stored per chunk in a block map.
- Eight-layer procedural texture array with per-face block mapping and nearest sampling.
- DDA voxel raycasting, a wireframe block outline, and break/place interaction.
- Debug FPS/frame-time overlay on `F3`; it refreshes once per second.
- `VCHK` v1 chunk persistence under `world/chunks/`, load on startup, save on exit and `Ctrl+S`.
- A demo world with grass over dirt, stone, wood, and leaves, plus a safe spawn column.

### Changed

- WASD now walks on the ground instead of flying; `Space` jumps instead of toggling a demo voxel.
- Number keys `1`–`5` select the block to place; the cursor still captures on left click.
- Chunk meshes use the 36-byte textured vertex layout.
- `V` toggles vsync by reconfiguring the surface between FIFO and immediate or mailbox presentation.

### Removed

- The v0.0.2 fly-camera movement and the v0.0.3 boundary-voxel toggle.

## [0.0.3] - 2026-11-17

### Added

- Chunked world storage: 32×32×32 chunks, Euclidean chunk coordinates, and dirty-chunk tracking.
- Face culling through world coordinates, including across chunk borders.
- One GPU vertex buffer and one draw call per non-empty chunk, with a per-chunk origin uniform.
- Debug logging for dirty chunk count, total vertices, and rebuild time after an edit.

### Changed

- The demo scene is a 4×4 floor spanning four chunks with a raised voxel on the chunk boundary.
- `Space` toggles that boundary voxel and rebuilds only the affected chunks.
- The camera starts near `(37, 4.5, 38)` and is aimed at `(32, 0.5, 32)`.

### Fixed

- Chunk meshing treats neighbours outside the `i32` world bounds as air instead of overflowing.

## [0.0.2] - 2026-11-14

### Added

- Fly camera: WASD movement, mouse look with cursor capture, delta-time updates, and frame-time logging.

## [0.0.1] - 2026-11-14

### Added

- Repository scaffold: Cargo manifest, roadmap, architecture decision records, and PolyForm Shield 1.0.0 license.
- Conventional Commits and changelog enforcement in continuous integration.
- Release automation: one command bumps the version, tags it, and publishes a GitHub release.
- Documentation site built with mdBook and published to GitHub Pages.
- v0.0.1 foundation: a `winit` window with a `wgpu` renderer, depth buffering, and a static perspective camera.
- Voxel scene stored in a `HashSet<IVec3>` and rebuilt as a single mesh on change.
- `Space` toggles the raised centre voxel at runtime.
