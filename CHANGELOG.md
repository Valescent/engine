# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `terrain-gen calibrate-blend` measures the biome blend width `W_blocks` per biome pair over the sweep seeds (w 0.5 to 0.95 along transects perpendicular to each border, doubled) and checks the spec bounds; the definition is in [Terrain tools](docs/dev/terrain-tools.md). Measured medians are 43-72 blocks against the 20-28 target, so the command reports the gap rather than passing, and it is not a CI gate.
- `terrain-gen counters --tier pr|nightly` pins the deterministic counters for the PR seeds, and a CI step fails when any baseline drifts by more than 5%.
- Column-level streaming replaces whole-tile paging: the demo keeps the spec's 16-column view radius resident (1,089 columns), generates nearest-first within a bounded per-frame budget, relights only a new column and its eight neighbours, remeshes only the sections whose border light or blocks changed, evicts farthest-first beyond radius + 2, and freezes the player until the centre column is ready. LOD0 covers the inner two columns with one transition ring and coarse LOD1 covers the rest; steady state is ~126 MB of section bytes and ~590 MB peak working set.
- Camera-relative rendering on both draw paths: one per-frame render origin (the camera's block position) is subtracted from every submitted section and chunk origin and from the view matrix, and fog, culling, the highlight, and the water wave phase all use the same relative frame. Picking, collision, and streaming stay world-space, with a precision test at 2^20 blocks.
- A LOD2 far band: columns beyond radius 7 draw as complete 4x4-aligned regions with four blocks per cell and 8-block skirts, sharing the throttled coarse rebuild. At radius 16 this cuts quads from 1.96M to 862k (-55.9%) and section bytes by 6.9%, with no change to generation.
- The demo ring now follows `render_distance_chunks` (clamped 4..16) instead of a fixed 16, so the streamed and visible extents agree. At the default 12 it is 625 columns, 71.4 MB of section bytes and ~491 MB peak; `render_distance_chunks = 16` still gives the spec's 1,089-column ring.
- Camera-relative origin uniforms are static world-space integers, and the shaders subtract the frame origin in integer space. Crossing a block no longer rewrites any section or chunk origin (1,760 writes avoided per crossing on the full demo ring).
- Ring fill relights only the changed border of an already-lit neighbour (narrow `relight_span_view` through a borrowed `ColumnView`) instead of the whole neighbour: build light drops from ~83 ms to ~42 ms median, the default ring fills in ~29 s (was ~59 s) and the 16-column cap in ~46 s (was ~51 s), with cell-identical light and meshes proven by a narrow-vs-full equality test.
- Each column's 1,024 base columns are computed once and reused across its twelve sections through a bounded per-worker memo, instead of being regenerated twelve times: a full 12-section column drops from ~300 ms to ~58 ms (5.2x), a 9x9 patch from 17.2 s to 4.3 s, and cold fill from ~32 s to ~21 s at the default radius. Output is byte-identical (pinned column and section hashes, unchanged goldens).
- A ring build now waits for its dependency window and validates only what it reads, so cold fill issues 70-72% fewer build jobs and discards none (was 28-31%), with unchanged steady bytes and quads.
- Every resident column in the visible ring is now covered by exactly one LOD band: leftover rim and LOD0-boundary columns get clipped, column-masked LOD1 regions instead of being left undrawn. That adds ~122k quads (+16.9%) at radius 12 and ~156k (+18.0%) at 16, with no generation change.
- Biome coverage is guarded over all 16 PR seeds with a 5% land floor per biome, and `calibrate-blend --seeds 16` runs on a nightly schedule (it stays out of the PR gates because pair 2-3 has a 63.32 p90 against a 64 limit).
- Generation version 3: every column's biome is the per-block keyed dither pick, so biome borders blend over ~24 blocks instead of snapping, and the surface rules and feature footprint always agree. The dither weight is normalised by the landscape gradient, which brings `terrain-gen calibrate-blend --seeds 16` inside the spec bounds (medians 23.3-27.2, p10 14.7-18.8, p90 32.6-63.3) where it previously measured roughly twice the target. Version 1 and 2 worlds are refused rather than regenerated.

- LOD2, a 4x4x4 downsample for chunks beyond 16 chunk units, and `render_distance_chunks` up to `32`; LOD2 emits every chunk-boundary face like LOD1.
- Light fields store one nibble per cell, halving resident light memory, and a light snapshot is built from 27 direct chunk lookups instead of a world scan.
- Chunk level work is tracked in a set fed by generated columns, edits, and player-chunk changes, so the steady-state frame no longer scans every chunk. The queue checks pool admission before building a snapshot.
- A windowless fixed-step run (`ENGINE_HEADLESS=1`) reports frame work, generated columns, ready columns, and resident bytes, and a Perf workflow gates it on every main push.
- Chunk vertices pack into 12 bytes (three cell bytes plus face, two UV bytes, layer, and light/AO) and quads use four vertices plus six indices (16-bit where a chunk fits, 32-bit otherwise), cutting vertex buffer memory by about 75 percent.
- Back faces are culled for terrain and water; greedy quads wind counter-clockwise around their outward normal on all six faces.
- The spec-driven v1 terrain stack ([`docs/specs/terrain.md`](docs/specs/terrain.md)): a hashed world description and header, region-graph generation, column light, greedy section meshing with skirts and LOD, section residency and revisions, and an edit journal.
- Section persistence under a v2 world header: per-section files with one-generation backup rotation, an append-only journal replayed on open, prefix trimming after a save, a save worker on its own IO thread, `migrate` with a JSON report, and `repair` (`restore`, `quarantine`, `salvage`).
- `terrain-gen`: `gen`, `golden check|update`, `seams`, `sweep`, `describe`, `describe-diff`, `verify --report`, `replay`, `map`, `repair`, and `migrate`, with pinned `--json` output.
- `TERRAIN_DEMO=1`: a 128x128 tile with real column light, collision, asynchronous incremental light and mesh repair (~24 ms in release), distance LOD1, tile paging with prefetch, and saves under `world/terrain-demo/`.
- Per-face texture references on content-pack records (`textures.top|side|bottom`, `ADR-0015`): the mesher writes the layer of each face, grass and wood draw distinct top/side/bottom tiles, and a separate `render_hash` tracks appearance without changing `record_hash` or `pack_hash`.

### Changed

- Generation version 2: hydrology solves the whole finite world once (512×512 macro cells) instead of per 512-block tile, and every worker shares that solve, so a lake or river that crosses a tile seam has one level. Version 1 worlds are refused rather than regenerated with different water. Cost is ~165 ms once per seed; demo startup is ~0.97 s.
- `tracy` is a default feature, so a debug run captures to `reports/tracy/` unless `TRACY_CAPTURE_SECONDS=0`; CI builds every cargo command with `--no-default-features`.
- The headless terrain stack now lives in a `terrain-core` workspace crate (generation through persistence), and `terrain-gen` moved with its tests and goldens. `engine` re-exports the moved modules, so existing paths still work, and CI proves the tool resolves no `wgpu`, `winit`, `wgpu-profiler`, or `tracy-client`.
- Generation version 4: the desert centre moved to `(0.84, 0.15, weight 1.15)`, lifting the thinnest coverage (seed 9 desert 5.09% -> 6.89%) and bringing the desert/tundra blend from p90 63.32 to 57.68 while every pair's median stays in 20-28. Two golden section hashes move; the counter baseline is unchanged.
- The frozen v0 stack (`world`, `light`, `mesh`, `mesher`, `generation`, `streaming`, `voxel`) now lives under `src/v0/` with crate-root re-exports, so every existing path keeps working; `terrain-gen` is split into `src/bin/terrain-gen/` with one module per subcommand, with byte-identical goldens, counters, and calibration output.
- The crate root is now 34 lines: `startup.rs` owns `run`/config/seed/world choice, `app.rs` owns the winit app/frame/input, and `src/demo/` owns the terrain runtime (`patch`, `lod`, `stream`, `edit`, `persist`). Pure moves with all 59 tests preserved and no public-surface change.
- The headless terrain stack is exposed through one `engine::terrain::<module>` facade; the 28 flat `engine::<module>` paths are `pub(crate)` only, proven by two `compile_fail` doctests and a facade integration test.
- Coarse LOD water is now all-or-nothing, so a distant water surface can never sit above the fine surface (the floating translucent sheets are gone), and water sections draw far-to-near from the camera with depth writes still off. The renderer terrain tests moved to `renderer/terrain/tests.rs` to keep every file under the hard size budget.
- The block texture array now carries a five-level mip chain generated in-engine by 2x2 box filtering, with nearest magnification and linear min/mip filtering ([ADR-0016](docs/decisions/0016-block-texture-mipmap-policy.md)); level 0 is byte-identical to the old tiles and layers stay isolated.
- The size check is now a hard gate: a counted Rust file over 1,000 lines fails CI, while 600-1,000 stays advisory and the frozen `src/v0/` stack is excluded. The whole tree is under the hard budget.
- The demo now has one app-facing seam: `DemoRuntime` owns the patch, ring, save worker, repair prompt, pending rebuild, and edit queue behind `open/tick/edit/shutdown/repair/stats`, and `App` holds one `demo` field instead of six. The demo stores no renderer and names no `winit`/`wgpu` in production code; behaviour, log lines, and budgets are unchanged.
- `app.rs` is split into `app/mod.rs` (App, handler, lifecycle, `RunError`), `app/input.rs` (keys, mouse, capture, hotbar, edits), and `app/frame.rs` (frame order, timing, overlay, uploads, streaming), with the frame sequence unchanged.
- The demo module is split further: `demo/build.rs` owns the incremental rebuild (3x3 light scope, 26-neighbour remesh) and `demo/content.rs` owns the material/light/texture-layer tables, leaving `demo/mod.rs` as the `DemoRuntime` seam. All 32 demo tests are preserved.
- `mesher2` and `light2` are split into submodules behind unchanged parent paths: `mesher2/{record,material,lod,greedy,cross,skirt,tests}` and `light2/{table,column,bfs,repair,tests}`. Both add a pinned output hash (mesh records and column light) taken before the split and unchanged after it.
- `pipeline` and `volume` are split the same way: `pipeline/{terrain,stages,hydro,tests}` and `volume/{detail,caves,spans,tests}`, with a pinned column/section digest added for pipeline and the existing golden columns covering volume. Files over 1,000 lines are down from 22 to 17.
- `features` is split into `discover`/`apply`/`masks` behind an unchanged parent, and `renderer` into `chunk`/`frame`/`origin`/`pipeline`/`terrain`/`texture` with the shader `include` paths adjusted and every `renderer::` item re-exported; all 19 renderer tests moved with their subjects. The size checker now excludes the frozen `src/v0/**` from its warnings, leaving 14 files over 1,000 lines.
- A non-blocking CI size-budget check reports Rust files over 1,000 lines (currently 21) instead of failing; it tightens into a gate once the tree is under budget.

### Fixed

- Columns whose chunks are built only at LOD2 now count as ready; the readiness check previously ignored the coarsest level, leaving the outer ring unready.

## [0.0.7] - 2026-11-18

### Added

- Deterministic procedural generation: five seeded noise fields (continentalness, erosion, temperature, humidity, and 3D caves) drive a 128-block-tall world with hills, mountains, sea level 32, three biomes, surface palettes, caves, water fill, and deterministic trees on plains and hills.
- `Sand` (block ID 8, texture layer 10); the texture array grows to eleven layers.
- Player-centred chunk streaming: columns inside `render_distance_chunks` are generated on the worker pool and committed two per frame, nearest and forward first, and chunks are meshed once their light is ready. Columns beyond `render_distance + 2` unload.
- A movement guard stops the player entering a column that is not yet generated, lit, and meshed.
- `config.toml` with `[world] render_distance_chunks` (`8..=16`, default `12`); the camera far plane follows the render distance.
- `world/seed`: one decimal `u64` written when a world is created and reused by every launch. `ENGINE_SEED=<u64>` chooses the seed of a new world.
- Modified-chunk persistence: `set_block` marks the owning chunk, unload and `Ctrl+S` write only modified chunks, and saved files are applied over regenerated blocks. An emptied chunk is a valid `VCHK` v3 file with zero records.
- Unloading saves, then releases chunk blocks, light, GPU meshes, water meshes, and queued jobs; a late result for the unloaded column is discarded.
- Generation metrics: the per-second debug frame line reports committed columns and their worker time, and `generated_columns` and `generation_ms` are Tracy plots.

### Fixed

- The streaming guard no longer treats an air-only chunk as unready. Columns above the terrain store no blocks, so requiring light and a mesh for them made every neighbouring column permanently unready and trapped the player in the spawn column. Absent chunks are now ignored by the readiness check; unloaded columns are still refused.

### Changed

- Fog runs from 70% to 100% of the render distance (`0.7 * r * 32` to `r * 32` metres) instead of percentages of the camera far plane, which stays two chunks beyond the fog end.
- Chunks store blocks in one dense `[Block; 32768]` array (32 KiB) with a non-air count, replacing the `HashMap` and the duplicate per-chunk light arrays; light lives only beside the chunk map. Chunk snapshots for light and meshing are a direct array copy, and borders sample only the shared face. A 1434-chunk world at `render_distance_chunks = 12` now runs at 60 FPS with a ~350 MB working set.
- `VCHK` writes format version 3 and still loads versions 1 and 2; v3 accepts block IDs `1..=8` and a zero record count for an emptied modified chunk.
- A world is always generated from `world/seed`. A `world/chunks/` directory without a seed file is a typed `MissingSeed` error instead of an eager load, so a pre-v0.0.7 world needs a seed added deliberately.
- Saves no longer delete `.vchk` files. Unmodified chunks have no file, and an emptied chunk keeps a zero-record file.

## [0.0.6] - 2026-11-18

### Added

- `docs/progress.md`, a progress dashboard generated by `scripts/progress.sh`: XP and level, roadmap completion bars, commit streaks and a 14-day activity strip, achievements, and the open boxes in the current milestone. Every number is derived from git, the roadmap, the changelog, and the source tree.
- Optional `tracy` feature for local profiling: CPU spans for the frame, player update, highlight, mesh request, mesh upload, encode, and chunk worker jobs; allocation events from a global allocator; GPU timer zones from `wgpu-profiler`; and eight per-frame plots (frame, GPU, chunks, draws, vertices, rebuild, upload, queued jobs).
- `tracy-profiler` thread lanes are named `chunk worker` once per pool thread.
- `ENGINE_NO_VSYNC=1` starts with vsync off for benchmark runs, and an ignored `bench_hot_paths` test measures snapshot preparation, LOD meshing, and collision in release. Results are recorded in [Benchmark results](docs/dev/benchmarks.md).
- Optional `rayon-pool` feature plus `ENGINE_MESH_POOL=rayon` to run the same binary with a rayon mesh pool instead of micropool for real-engine A/B comparisons.
- 4-bit sky and block light per chunk with BFS propagation across a 3x3x3 chunk neighbourhood, including the one-cell border field meshing will sample. Light is derived state and is recomputed after loading.
- `Lamp` (solid, emits block light 14) and `Water` (non-solid, passes light without attenuation) blocks, with lamp and water texture layers; the array grows to ten layers.
- Ambient occlusion shading and per-corner sky/block light in chunk meshes: vertices carry a packed `light_ao` field from a 40-byte layout, quads split at light and AO discontinuities, and corners darken in crevices.
- A 600-second day/night cycle with dawn, day, dusk, and night keyframes; the sun and moon light the world, ambient light follows the sky, and lamps emit warm block light.
- Distance fog from 70% to 100% of the camera far plane, blended to the current sky colour; the clear colour follows the sky.
- `T` skips one hour and `F4` toggles the v0.0.4 unlit debug shading.
- Water (`Water`, block ID 7) renders as a separate transparent full-resolution mesh with animated normals, alpha `0.65`, depth writes off, and blending after all opaque chunks.
- The demo world has a small pond; water is not on the hotbar.
- Edits recompute sky and block light on the worker pool: the edited chunk, its 3x3x3 neighbourhood, and loaded chunks below are invalidated, and each is remeshed after its new light field is accepted.

### Fixed

- Player collision no longer scans every block in the world. The swept AABB query visits only the blocks inside its bounds, cutting `Player::update` from 1.15 ms to 0.9 us per frame on the 64-chunk stress scene and the uncapped frame time from 1.28 ms to 0.23 ms.

### Changed

- `VCHK` writes format version 2 and still loads version 1; v2 accepts block IDs `1..=7` and v1 `1..=5`.
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
- Number keys `1`ΓÇô`5` select the block to place; the cursor still captures on left click.
- Chunk meshes use the 36-byte textured vertex layout.
- `V` toggles vsync by reconfiguring the surface between FIFO and immediate or mailbox presentation.

### Removed

- The v0.0.2 fly-camera movement and the v0.0.3 boundary-voxel toggle.

## [0.0.3] - 2026-11-17

### Added

- Chunked world storage: 32├ù32├ù32 chunks, Euclidean chunk coordinates, and dirty-chunk tracking.
- Face culling through world coordinates, including across chunk borders.
- One GPU vertex buffer and one draw call per non-empty chunk, with a per-chunk origin uniform.
- Debug logging for dirty chunk count, total vertices, and rebuild time after an edit.

### Changed

- The demo scene is a 4├ù4 floor spanning four chunks with a raised voxel on the chunk boundary.
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
