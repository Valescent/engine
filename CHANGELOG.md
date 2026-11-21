# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.0.9] - 2026-11-21

### Added

- The LOD hierarchy foundation: `SectionKey` level math, a level-1 coarse generator with a modified-chunk overlay, per-column coarse light, and a `32³` sample-space mesh path. One level-1 section uses 32 KiB of samples + 19.7 KiB of light for 2,193 quads; the eight LOD1 chunks covering the same volume use 288 KiB of samples + light for 3,613 quads.
- The quad pool accepts coarse section nodes through `NodeKey`. `ENGINE_HIERARCHY=1` stops L0 at `ENGINE_DETAIL_CHUNKS` (default 16), selects level-1/2 sections out to 2 km with projected-size hysteresis, retains parents until all eight children are ready, keeps ready children hidden until the swap, meshes coarse water, and rebuilds sections from the modified/saved chunk overlay on edits. The settled west view is 0.466 ms / 0.235 ms GPU, 3.58 M vertices, 2 draws, and 30.94 MB of allocated quads; L0 at detail 16 is 85.9 MB resident.
- Levels 3-4 extend the hierarchy to 8 km with recursive projected refinement, parent retention, 256 m and 512 m sections, and an 8M-quad (96 MB) hierarchy pool. Children above the 128-block world are filtered; coarse BFS light replaces the per-column 0/15 field (about 120 µs per section) and water stays lit through the seabed top. With the section horizon, the settled 8 km west view is 0.577 ms / 0.323 ms GPU, 5.15 M vertices, 1,259 visible nodes, 2 draws, 74.62 MB allocated quads, and 94.24 MB geometry reservation.
- The 12-byte quad record from ADR-0015 is implemented as pack/unpack with unit tests.

### Changed

- Chunk geometry now renders from a 12-byte quad pool with vertex pulling and indirect draws: at distance 32, draw calls fall from 3,409 to 2, resident geometry from 100.9 MB to 37.6 MiB, visible geometry from 30.1 MB to 7.52 MiB, and frame CPU from 4.3 ms to 1.9 ms. The per-chunk vertex and index path remains as the fallback.
- Chunk-top horizon culling is now on by default (`ENGINE_HORIZON_CULL=0` disables it). Occluders use the minimum column top, so spawn captures on seeds 42 and 7 and a strafe view match the horizon-off image. It removes two-thirds of visible chunks and cuts the GPU scene from 0.25 ms to 0.14 ms.
- The horizon pass now covers hierarchy sections. `Section::bounds` reports the lowest solid column top and the highest visible top; water is visible but not an occluder. At the spawn 8 km view it removes 646 of 1,330 in-frustum sections (-49%), 4.25 M of 9.40 M visible vertices (-45%), and 8.10 MB of visible geometry, cutting the GPU scene from 0.780 ms to 0.323 ms with an identical image. `bench_horizon_sections` reproduces the counts without a window.
- Coarse section generation, light, and meshing now run as background jobs on the shared worker pool, with up to eight in flight and uploads on the main thread. The settled 8 km hierarchy (4,908 sections) fills in 6.4-6.9 s instead of 22.9 s, and the first logged frame falls from 4.14 ms to 2.36 ms; the steady 0.58-0.69 ms frame and 0.323 ms GPU scene are unchanged. The frame log reports `sections_in_flight`.
- Streaming keeps a cached candidate list until the player crosses a column or a column unloads, and marks a column ready from mesh acceptance instead of scanning every loaded column each frame. The settled distance-32 frame falls from 2.5 ms to 0.67 ms, and headless wall time falls to 7.6 s at distance 24 and 12.2 s at 32.
- The mesher gathers each face's 3x3 corner neighbourhood once instead of reading it for every corner and every light candidate. LOD0 ground meshing falls from 1.72 ms to 1.13 ms, LOD1 ground from 0.53 ms to 0.28 ms, and flat slab LOD1 from 0.29 ms to 0.14 ms with identical output.
- Streaming lists the saved-chunk directory once at startup instead of probing the filesystem for every generated chunk, and records the chunks written by unloads and `Ctrl+S`. A distance-32 load no longer makes about 9,300 failed reads.
- The light job finds lamps during the block fill and seeds sky light only above each column ceiling, removing a full `96³` pass; snapshot construction shares chunk storage behind an `Arc`. Ground-chunk light falls from 4.37 ms to 3.55 ms and the distance-32 settle from 12.1-12.8 s to 11.9 s.
- Quad-pool slots are stored densely with a chunk-to-position map instead of a per-chunk `HashMap`, and the two indirect-argument lists are reused each frame. The frame log reports `stream_ms`, `levels_ms`, and `render_ms`; at distance 32 the render call is 0.47-0.60 ms of a 0.59-0.60 ms frame and streaming/level work is below 0.001 ms.
- The frame log reports reserved and peak quad-pool bytes. The settled distance-32 peak is 2.36 M quads (28.32 MB) against the 3.2 M-quad reservation, so the cap already carries the planned 35% slack.
- GPU timing is always available when the adapter supports timestamp queries, and the frame log reports `gpu_ms`. The quad pipeline uses a shared `u16` index pattern for meshes up to 16,383 quads, cutting vertex invocations from six to four per quad: the settled distance-32 GPU scene falls from 0.38-0.59 ms to 0.253 ms and the frame from 0.62-0.65 ms to 0.48 ms, with identical visible chunks and vertices. Larger meshes keep the non-indexed path.

### Fixed

- A chunk no longer stays at its coarsest LOD after the player walks near and away: `select_active` pruned the other LOD ranges while the queue still reported them as built, and selection only ran on upload. Active LOD and the queue's built entries are now kept in sync.
- The indirect arguments buffer covers both the opaque and water batches; the water write previously ran past the buffer once more than 16,384 chunks had geometry.
- The chunk slot table holds 20,480 entries, covering the 19,044 chunks reachable at render distance 32, and a failed upload stops after eight attempts instead of requesting the same mesh every frame.
- Loading or unloading a column invalidates the light and meshes of its eight neighbours, so the bright or dark band left where a missing column was treated as open sky is corrected. Light work waits until the 3x3 column neighbourhood is settled, which keeps headless settle at 11.1 s at distance 24 and 20.4 s at 32 instead of 26.4 s and 47.4 s.
- Water faces are no longer emitted twice at a shared chunk border; the mesher's border snapshot now carries water as well as solids.
- Horizon culling uses the water surface height, so a coastal chunk is not culled while its water is still visible.
- Saving clears the modified set and loading no longer marks saved chunks modified, so an unchanged world is not rewritten on exit.
- The fallback spawn scans solids only, so a water column spawns on the ground.
- The v2 `VCHK` fixture builds a real version-2 stream and covers the lamp and water ids.

## [0.0.8] - 2026-11-20

### Added

- LOD2, a 4x4x4 downsample for chunks beyond 16 chunk units, and `render_distance_chunks` up to `32`; LOD2 emits every chunk-boundary face like LOD1.
- Light fields store one nibble per cell, halving resident light memory, and a light snapshot is built from 27 direct chunk lookups instead of a world scan.
- Chunk level work is tracked in a set fed by generated columns, edits, and player-chunk changes, so the steady-state frame no longer scans every chunk. The queue checks pool admission before building a snapshot.
- A windowless fixed-step run (`ENGINE_HEADLESS=1`) reports frame work, generated columns, ready columns, and resident bytes, and a Perf workflow gates it on every main push.
- Chunk vertices pack into 12 bytes (three cell bytes plus face, two UV bytes, layer, and light/AO) and quads use four vertices plus six indices (16-bit where a chunk fits, 32-bit otherwise), cutting vertex buffer memory by about 75 percent.
- Back faces are culled for terrain and water; greedy quads wind counter-clockwise around their outward normal on all six faces.

### Changed

- Chunk block storage is four-bit packed (16 KiB per chunk) with `VCHK` unchanged. Blocks plus light drop from 381 MB to 191 MB at `render_distance_chunks = 24` and from 669 MB to 335 MB at `32`; meshing costs about 15-25% more per chunk because of the nibble decode.
- Light fields store the sky plane for every chunk and the block-light plane only when a lamp is in range, cutting typical resident light from 39 KiB to 19 KiB per chunk. A lamp restores the full field for the 27 chunks around it.
- Water meshes are built only for chunks that contain water; dry chunks skip the full-grid water scan (about 0.37 ms each). Water mesh jobs are still full resolution and are not distance-limited.
- The renderer caches resident geometry bytes, tests each chunk's frustum once per frame for both opaque and water draws, and drops non-active LOD meshes after a replacement is uploaded. Settled frame time at distance 32 falls from 9.06 ms to about 5 ms on the reference machine; distance 24 is unchanged at about 2.3 ms.
- Light jobs skip the block-light working grid when no lamp is in range and use a flat ceiling array instead of a hash map. Headless settle time falls from 15.1 s to 12.2 s at distance 24 and from 29.1 s to 24.4 s at 32.
- Chunk level work drains under a 2 ms per-frame budget in a deterministic order, a chunk leaves the queue while its job runs and returns when the result is accepted, and active-LOD selection happens on upload instead of scanning every loaded chunk each frame. Settled distance-32 frame time falls to about 4.3 ms and no frame logs a slow work phase; headless wall time falls to 11.7 s at distance 24 and 21.6 s at 32.
- Index buffers are shared: one `u16` quad pattern replaces the per-chunk index buffers, so resident geometry at distance 32 falls from 125.9 MB to 100.9 MB and visible geometry from 37.6 MB to 30.1 MB. Chunks above 65,535 vertices keep a `u32` fallback buffer.
- Mesh uploads rose from 2 to 8 per frame; the old cap limited first-load streaming, and the new value keeps distance-32 settle at the worker rate.

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
