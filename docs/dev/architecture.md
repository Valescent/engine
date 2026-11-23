# Architecture

The engine is a small `winit` and `wgpu` application. This page describes the current modules, ownership, and frame flow. Update it when module boundaries or resource ownership change.

## Workspace crates

The repository is a two-package workspace rooted at the `engine` package:

| Crate | Responsibility |
|---|---|
| `engine` | `winit` application, renderer, camera, player, legacy v0 world/chunk stack, and the `engine::terrain` facade over the terrain stack. It depends on `terrain-core`. |
| `terrain-core` (`crates/terrain-core`) | Headless terrain description, generation, storage, lighting, meshing, and save stack, plus the `terrain-gen` binary. It has no window, GPU, audio, or renderer dependency. |

`terrain-core` is a normal dependency (not a dev-dependency). `engine` exposes its modules through the public `engine::terrain` facade, so external code uses paths such as `engine::terrain::pipeline::Terrain`, `engine::terrain::light2`, and `engine::terrain::section`. Internally, `src/lib.rs` keeps `pub(crate) use terrain_core::{...}`, so `crate::pipeline` and the other flat paths still resolve inside the crate; they are not public API. The `terrain-gen` binary lives in `terrain-core`; CI checks that its normal dependency tree contains no `wgpu`, `winit`, `wgpu-profiler`, or `tracy-client`.

## Modules

```text
src/
├── main.rs                 # thin binary shell
├── lib.rs                  # crate docs, mod declarations, pub use
├── startup.rs              # run, config, seed, world choice, demo open
├── app/
│   ├── mod.rs              # App, ApplicationHandler, lifecycle, RunError
│   ├── input.rs            # key/mouse mapping, capture, hotbar, edit actions
│   ├── game.rs             # targets, break progress, drops, placement, saves
│   ├── ui.rs               # egui input forwarding, frame build, platform output
│   └── frame.rs            # tick/redraw order, timing, overlay, streaming
├── game/                   # headless gameplay: items, inventory, tools, entities, save
│   ├── item.rs             # stable names, definitions, stacks
│   ├── inventory.rs        # hotbar/main/offhand slots and operations
│   ├── tools.rs            # tool classes, tiers, break rules
│   ├── drops.rs            # content-pack drop tables
│   ├── entities.rs         # dropped-item physics, pickup, records
│   ├── save.rs             # VPLY player save with CRC and atomic write
│   └── state.rs            # fixed tick, break progress, persistence
├── ui/                     # egui state and screens
│   ├── mod.rs              # UiState, capture policy, screen dispatch
│   ├── hud.rs              # crosshair, hotbar, interaction prompt
│   ├── inventory.rs        # inventory grid, drag/drop, quick move, tooltips
│   └── item_icon.rs        # shared icon, count, and durability drawing
├── demo/
│   ├── mod.rs              # DemoRuntime seam, frame/stats, constants, re-exports
│   ├── patch.rs            # TerrainDemo, block access, light/mesh apply, LOD apply
│   ├── build.rs            # incremental relight/remesh over an edit batch
│   ├── lod.rs              # LOD0/1/2 bands, clipped LOD1, LOD2
│   ├── stream.rs           # column ring, worker pool, jobs
│   ├── edit.rs             # edit queue, journal/section save queueing
│   ├── persist.rs          # replay, repair prompt, recovered journal
│   └── content.rs          # material table, collision table, texture layers
├── v0/                     # frozen legacy stack
├── renderer.rs             # Renderer, RendererError, re-exports, draw passes
├── renderer/
│   ├── chunk.rs            # legacy per-chunk LOD/water meshes and origins
│   ├── frame.rs            # frame uniform, camera bind group, depth, stats
│   ├── origin.rs           # absolute origin uniforms, camera-relative bookkeeping
│   ├── pipeline.rs         # shader modules, bind group layouts, pipelines
│   ├── terrain.rs          # packed-quad sections, quad decode, upload plan
│   ├── items.rs            # dropped-item cube pass
│   ├── ui.rs               # egui textures, buffers, and the color-only UI pass
│   └── texture.rs          # procedural block texture array
├── camera.rs, player.rs, raycast.rs, sky.rs, overlay.rs, frustum.rs
├── block.rs, config.rs, headless.rs, metrics.rs
└── *.wgsl                  # shaders stay with the renderer
```

| Path | Responsibility |
|---|---|
| `src/main.rs` | Thin binary entry point; calls `engine::run()`. |
| `src/lib.rs` | Crate table of contents: crate docs, `mod` declarations, the `pub(crate)` terrain re-exports, and the public surface (`run`, `RunError`, `RendererError`, `ChunkCoord`, `SaveError`, `v0`, and the `terrain` facade). No logic. See [Public API](public-api.md). |
| `src/startup.rs` | `run`, tracing setup, configuration and seed loading, world choice (saved, generated, or stress), the `DemoRuntime::open` hand-off, and the headless hand-off. |
| `src/app/mod.rs` | `App` (fields and construction), the `winit` application handler and window-event dispatch, window/renderer lifecycle, `RunError`, and the module declarations. |
| `src/app/input.rs` | Key mapping, cursor capture/release, focus and mouse-look handling, hotbar selection, movement-key helpers, repair-key mapping, and the legacy break/place actions. |
| `src/app/game.rs` | The v1 gameplay glue: target names and hardness, hold-to-break, block removal and drops, placement rules, pickup upload, and the player save/load paths. |
| `src/app/ui.rs` | The egui/winit bridge: event forwarding, the UI-first capture decision, `take_egui_input`, `run_pass`, platform output, tessellation, `UiFrame` construction, and inventory open/close. |
| `src/app/frame.rs` | Per-frame work: tick and redraw order, frame delta and timing accumulation, `FrameAccumulator`, overlay text and stats, resize, mesh request/upload and column streaming, and the demo-to-renderer upload adapter. |
| `src/game/` | The headless gameplay core: item identity and stacks, inventory and hotbar, tool classes and break rules, drop tables, item entities, survival state, environment probes, damage and respawn, the `VPLY` save, and the fixed-tick `GameState`. It names no `winit` or `wgpu` type. |
| `src/ui/` | The UI state and screens: `UiState`, pointer/keyboard ownership, crosshair, nine-slot hotbar, survival bars and damage flash, pause/death overlays, item colours, counts, durability, interaction prompt, and the inventory and crafting screens with drag/drop, shift-click, tooltips, and keyboard focus. See [UI](ui.md). |
| `src/demo/mod.rs` | `DemoRuntime`, the demo's one app-facing seam: it owns the patch, ring, save worker, repair prompt, pending rebuild, and edit queue, and exposes `open`, `tick`, `edit`, `save`, `shutdown`, `repair`, `stats`, and the read-only overlay/upload accessors. Also `DemoFrame`, `DemoStats`, shared constants, and the runtime tests. |
| `src/demo/patch.rs` | `TerrainDemo` state, resident columns, block access and collision, light/mesh installation, LOD selection, and the light table. |
| `src/demo/build.rs` | The incremental rebuild over a batch of edits: the 3x3 light scope, the 26-neighbour remesh, `DemoEdit`/`DemoPatch`/`DemoRebuild`, and their tests. |
| `src/demo/content.rs` | The material table and face-texture layer map from the v1 pack, the collision (`solid`) table, and the material test. |
| `src/demo/lod.rs` | LOD0/1/2 band selection, complete and clipped LOD1 regions, LOD2 derivation, and LOD tests. |
| `src/demo/stream.rs` | The column ring: worker pool, generation and light/mesh jobs, the coarse job, snapshots, in-flight bookkeeping, and build/window tests. |
| `src/demo/edit.rs` | The edit queue and journal/section save queueing per edit, with its queue test. |
| `src/demo/persist.rs` | Save replay, the repair prompt, the recovered-journal path that runs before the ring opens, and replay tests. |
| `src/camera.rs` | Look state, yaw/pitch math, view-projection, and tests. Position comes from the player eye. |
| `src/block.rs` | Block types, stable IDs, hotbar mapping, and face texture layers. |
| `src/player.rs` | Player physics, gravity, jumping, swept AABB collision, and tests. |
| `src/overlay.rs` | Screen-space debug text geometry and font. |
| `src/raycast.rs` | DDA voxel traversal, hit result, tie-breaking, and tests. |
| `src/v0/mod.rs` | The frozen v0 stack: module list and docs. Removed once the terrain demo becomes the default run path. |
| `src/v0/world.rs` | Chunk and block storage, coordinate conversion, dirty-chunk tracking, the modified set, `VCHK` save/load, and seed file handling. |
| `src/v0/light.rs` | Four-bit packed sky/block light, BFS propagation, padded `LightField` accessors, and direct-light snapshots. |
| `src/v0/mesh.rs` | Greedy meshing, the 2x2x2 LOD1 and 4x4x4 LOD2 downsamples, LOD selection, chunk snapshots, border blocks, and tests. |
| `src/v0/mesher.rs` | Mesh, light, and generation jobs, revisions, the bounded micropool `WorkerPool`, finished-result collection, and tests. |
| `src/v0/generation.rs` | Seeded noise fields, height, biomes, caves, water fill, trees, and the spawn column. |
| `src/v0/streaming.rs` | Player-centred load rings, forward-first ordering, unload hysteresis, readiness, and the movement guard. |
| `src/v0/voxel.rs` | The packed 12-byte chunk vertex, the highlight mesh, and the v0.0.4 face-culled mesher kept as a test reference. |
| `src/section.rs` | `SectionKey` level/extent/origin/parent/child math, seed-native coarse generation, the modified-chunk edit overlay, coarse light, and section meshes. |
| `src/hierarchy.rs` | Projected section selection, readiness, hysteresis, parent retention, hidden children, build order, and eviction. |
| `src/frustum.rs` | Six-plane frustum extraction and conservative AABB tests. |
| `src/horizon.rs` | Column-top horizon bins and the conservative occlusion test for L0 chunks and hierarchy sections. |
| `src/metrics.rs` | Tracy plots for frame, GPU, chunk, draw, rebuild, upload, and queue metrics; no-ops without the `tracy` feature. |
| `src/config.rs` | `config.toml` parsing for the render distance. |
| `src/headless.rs` | Windowless fixed-step run selected by `ENGINE_HEADLESS=1`; drives generation, streaming, light, and meshing, then prints one `HEADLESS` summary line. |
| `crates/terrain-core/src/save.rs`, `crates/terrain-core/src/world_save.rs` | Section files, backup rotation, journal file, replay/trimming, `WorldSave`, save worker, repair, migration/report. See [Terrain v1 runtime](terrain-v1.md) and [Save format](save-format.md). |
| `crates/terrain-core/src/light2.rs`, `crates/terrain-core/src/mesher2.rs`, `crates/terrain-core/src/lod.rs`, `crates/terrain-core/src/section.rs`, `src/terrain_draw.wgsl` | v1 column light, greedy mesher, coarse LOD, section storage, packed-quad shader. The shader stays with the renderer. |
| `src/sky.rs` | Day/night cycle, sun direction, sky/ambient/sun colours, and the CPU fog formula. |
| `src/renderer.rs`, `src/renderer/` | `renderer.rs`: `wgpu` device/surface setup, `Renderer` and its methods, re-exports, and draw submission. `renderer/chunk.rs`: legacy chunk LOD/water buffers and origin uniforms. `renderer/frame.rs`: frame uniform, camera bind group, depth texture, present mode, and stats. `renderer/origin.rs`: absolute origin uniforms and camera-relative bookkeeping. `renderer/pipeline.rs`: shader modules, bind group layouts, and pipelines. `renderer/terrain.rs`: packed-quad sections, quad decode, and the upload plan. `renderer/items.rs`: the dropped-item cube pass. `renderer/ui.rs`: egui textures, buffers, and the color-only UI pass. `renderer/texture.rs`: the procedural block texture array. |
| `src/quad.rs` | The 12-byte quad record and 48-byte chunk-info layout. |
| `src/quad_renderer.rs` | Quad-pool uploads keyed by `NodeKey`, the chunk-info table, indirect arguments, visibility, and the two draws. |
| `src/pool.rs` | Flat 64-quad pages with a coalescing free list and frames-in-flight retirement. |
| `src/shader.wgsl` | Chunk vertex transform, lit/fogged opaque shading, and the animated water fragment. See [Shader authoring](shaders.md). |
| `src/quad.wgsl` | Vertex pulling, indexed quads, and the pooled opaque/water fragment shaders. |
| `src/highlight.wgsl` | Line-list block outline with break-progress colour. |
| `src/items.wgsl` | Flat-coloured dropped-item cubes. |
| `src/overlay.wgsl` | Screen-space overlay quads. |

`src/v0/` groups the frozen legacy stack. `src/lib.rs` declares `mod v0;` and re-exports
`v0::{generation, light, mesh, mesher, streaming, voxel, world}`, so paths such as `crate::world`
and `engine::world` resolve exactly as before. v0 is still the default run path when `TERRAIN_DEMO`
is unset; it receives fixes only, and the whole directory is removed when the terrain demo becomes
the default. The move is behaviour-preserving and does not change the save format.

`src/startup.rs`, `src/app/`, `src/game/`, `src/ui/`, and `src/demo/` split the old crate-root logic by responsibility.
`lib.rs` is now a table of contents; `startup` owns environment switches and world choice, `app`
owns `winit`/`wgpu` and the frame loop, `game` owns the headless gameplay state, `ui` owns the
egui state and HUD, and `demo` owns the v1 patch, ring, LOD, edits, and saves.
`demo` does not name `winit`, `wgpu`, or `renderer`; `app` translates its state into renderer
submissions. These modules are private, so the public surface is unchanged.

### The `DemoRuntime` seam

`App` holds one `demo: Option<DemoRuntime>` and no demo state of its own. `DemoRuntime` (in
`demo/mod.rs`) owns the thirty-field `TerrainDemo` patch, the `RingStream`, the `SaveWorker`, the
`RepairPrompt`, the in-flight `DemoRebuild`, and the queued `DemoEdit`s. Its interface is:

| Method | Purpose |
|---|---|
| `open(seed, column, view_radius)` | Open or create the saved world, replay recovered edits, start the worker pool and ring. |
| `tick(now, player_column, focused)` | Poll the rebuild, drain ring jobs, plan the bounded generate/build/evict batch, run the coarse and upload throttles, and return a `DemoFrame`. |
| `edit(cell, id)` | Apply one block change, queue its journal/section save, and start the coalescing rebuild. |
| `flush()` | Queue a terrain journal flush without stopping the save worker. `Ctrl+S` uses it. |
| `shutdown()` | Flush and join the save worker on window close. |
| `block_at(cell)` | The resident block id at a cell, or `None` when the column is not loaded. |
| `repair(action, player_column)` | Apply the repair choice, reload sections, and restart the save worker. |
| `stats()` | The once-per-second overlay counters and the ring-complete edge. |
| `is_ready`, `is_solid`, `sections`, `air`, `repair_key`, `advance_lod` | Read-only or LOD accessors for movement, raycasts, the overlay, and the upload adapter. |

`DemoFrame` carries the two side effects the runtime cannot perform itself: `spawn` (the app places
the player) and `upload` (the app calls `upload_terrain_demo`). The demo never stores a renderer, so
the dependency edge stays `app -> demo` and `app -> renderer`; `demo` sees only `terrain-core`, the
player's `BlockSource`, and the content pack.

### `terrain-core` modules

| Path | Responsibility |
|---|---|
| `description.rs`, `header.rs`, `section.rs`, `content.rs` | World description, header bytes, section records, and the v1 content pack. |
| `hash.rs`, `error.rs`, `metrics2.rs` | Deterministic hashing, typed errors, and pipeline counters/overlays. |
| `lattice.rs`, `noise.rs`, `curve.rs`, `graph/` | Macro lattice, noise fields, curves, and the generation graph. |
| `climate.rs`, `height.rs`, `surface.rs`, `strata.rs`, `volume.rs`, `features/`, `hydro/` | Column generation from climate through surface, strata, caves, features, and water. |
| `pipeline.rs` | The fixed-order column pipeline and its world hydrology handle. |
| `light2.rs`, `padded.rs`, `mesher2.rs`, `lod.rs`, `revisions.rs`, `residency.rs` | Column light, padded scratch, greedy meshing, coarse LOD, revisions, and residency policy. |
| `editlog.rs`, `save.rs`, `world_save.rs` | Edit journal, section store, backup/repair, migration, and save workers. |
| `src/terrain_gen.rs` (binary `terrain-gen`) | Headless acceptance harness; goldens live in `crates/terrain-core/tests/golden/`. |

### Gameplay core

`src/game/` is a headless state machine: items, inventory, tools, drops, item entities, the fixed
tick, and the `VPLY` player save. It names no `winit` or `wgpu` type. `App` holds a cached
`ContentPack`, an `ItemRegistry`, a `GameState`, and a `break_held` flag. `src/app/game.rs` is the
only world-aware glue: it resolves a raycast to a stable block name and hardness, drives the break
target, applies the completed edit through `DemoRuntime::edit`, spawns drops, places items, probes
survival hazards, respawns the player, and loads or saves the player file. The renderer reads only
the entity list and the break fraction.
See [Gameplay core](gameplay.md) for the item table, break rules, save layout, and edge cases.

`0.0.10.3` implements crafting Phase 1 in `game/recipe/`, `game/crafting.rs`, and
`ui/crafting.rs`; `ui/inventory.rs` hosts both the inventory and workbench layouts and the shared
crafting panel. The full grammar and later phases are in
[Crafting system](../design/crafting-system.md). The engine-facing architecture notes are in
[Engine architecture notes](research/engine-architecture.md).

## Ownership

- `App` owns the window handle, renderer, world, player, camera, sky clock, selected block, input state, mesh queue, worker pool, hierarchy selection and uploaded-section set, horizon bins, frame timing, one `Option<DemoRuntime>`, the cached content pack, the item registry, the compiled recipe book, the `GameState` (inventory, survival, item entities, break progress, and the fixed tick), and the egui context, winit adapter, and `UiState`.
- `UiState` owns the open station, cursor stack, focused slot, transient crafting grid, recipe search and selection, and the last craft error. Closing returns the grid and cursor to the inventory.
- `DemoRuntime` owns the v1 patch, column ring and workers, save worker, repair prompt, pending rebuild, and edit queue. It returns a `DemoFrame` instead of holding the renderer.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, chunk and highlight pipelines, camera uniform, the quad-pool renderer, the per-chunk fallback buffers and origin uniforms, the block texture array, the depth attachment, and the egui textures, buffers, and color-only UI pass.
- `Camera` owns only look state; it does not read the clock or input devices.
- `Player` owns feet position, velocity, and ground state; it does not read input devices.
- `World` owns the packed chunk map and the cached light fields; every stored chunk is non-empty. Generated chunks are inserted through `World::insert_chunk`, and chunk snapshots for meshing share the chunk's packed `Arc<Vec<u8>>`; a write copies the storage first (copy-on-write), so a snapshot stays immutable.
- `hierarchy::Hierarchy` owns the section target, readiness, build queue, hysteresis memory, and eviction decisions. Coarse sections are derived from the seed plus the modified-chunk overlay; they live outside `World` and are never saved.
- `mesh::build_mesh` is a pure function over a chunk snapshot and its border blocks; `section::generate_section` is a pure function of the seed, level, coordinate, and edit overlay.
- `mesher::WorkerPool` owns the worker threads for chunk generation, light, mesh, and coarse section jobs; `MeshQueue` owns revisions, pending jobs, built state, and desired LODs.

The renderer holds an `Arc<Window>` and creates a `Surface<'static>` from it. The application also holds a window clone for redraw requests and cursor capture.

## Startup

1. `main` calls `engine::run`.
2. `startup::run` initialises tracing and reads `config.toml`. It then loads `world/seed` (creating one when the world is new) and generates the spawn column; saved chunk files are applied over the generated blocks. `ENGINE_STRESS_SCENE=1` builds the in-memory stress scene instead and skips world I/O. `TERRAIN_DEMO=1` opens or creates the v1 world, replays recovered edits, and starts the column ring. Once the seed and world choice are known, a windowed run loads the player sidecar; headless and stress runs skip it.
3. `startup::run` creates the `winit` event loop and the `App` from `app.rs`.
4. `App::resumed` creates the window, the egui/winit adapter, and the renderer.
5. The renderer selects an adapter, requests a device and queue, configures the surface, builds the pipelines, the egui renderer, and the texture array, and starts no chunk mesh work itself.
6. `App` starts the micropool worker pool and queues an urgent LOD0 job for every loaded chunk. Meshes appear as workers finish.
7. The application requests the first redraw.

World load errors return through `RunError::Save` before the window opens. Startup errors are stored on `App`, logged, and returned through `RunError` after the event loop exits. The engine does not panic on adapter, device, surface, shader, or malformed save failures.

## Frame flow

```
RedrawRequested
  -> read frame delta
  -> build walking input from held keys and camera yaw
  -> player.update() with gravity, jump, and swept collision
  -> resolve the demo break target and run the fixed gameplay tick
  -> move the camera to the player eye
  -> raycast and update the block outline and break progress
  -> upload dropped-item cubes
  -> run egui: take input, draw the HUD or inventory, tessellate, and build a UiFrame
  -> advance the day/night cycle
  -> unload columns past the margin, then stream missing columns and mark ready ones
  -> request light and mesh work only for chunks in the level work set
  -> recalculate each chunk's desired LOD and request missing meshes
  -> drain finished light, mesh, section, and generation results (up to sixteen mesh/section uploads per frame)
  -> upload them and select the active LOD per chunk
  -> when the hierarchy is enabled, select sections by projected size, submit up to eleven
     generation/light/mesh jobs, upload finished sections, then update visibility and evict
  -> rebuild horizon bins every 250 ms when horizon culling is enabled
  -> extract the frustum from the view-projection
  -> write camera uniform
  -> upload UI textures and buffers
  -> renderer.render(&frustum, &horizon, ui_frame)
       -> acquire surface texture
       -> encode clear + depth clear + frustum/horizon-culled draws + outline
       -> encode the color-only egui pass over the scene
       -> submit
       -> present
  -> request the next redraw
```

With `TERRAIN_DEMO=1`, `DemoRuntime::tick` runs before the player step and in place of the v0
streaming phases: it applies a landed rebuild, drains at most 16 ring jobs, plans the next batch
through `residency::RingBudget::v1`, then runs the coarse (250 ms) and upload (100 ms) throttles. It
reports the spawn and upload through a `DemoFrame`; the app performs `set_spawn` and
`upload_terrain_demo`. The v0 world still streams underneath, but the demo path is what draws.

The demo also runs `tick_game` while focused: it resolves the target, sets the break duration,
advances the fixed tick, applies a completed break through `DemoRuntime::edit`, spawns drops,
picks up nearby entities, and passes the break fraction and item list to the renderer. A loaded
player save suppresses the demo's initial spawn override, so a restored position is not moved back
to the centre column.

- Continuous redraw runs while the window is visible, including when it is unfocused, so profilers and external tools keep receiving frames. Input stays gated on focus.
- The loop pauses only when the window is occluded.
- Frame time is logged at debug level about once per second.
- `ENGINE_CAPTURE=<path>` arms one surface readback after `ENGINE_CAPTURE_DELAY` seconds (default `20`) and writes a 24-bit BMP. It is a development hook, not a runtime feature.
- `ENGINE_YAW`, `ENGINE_AUTO_WALK=1`, and `ENGINE_AUTO_STRAFE=1` give captures a reproducible camera and movement without changing the normal input path.

## Input flow

See [Controls](controls.md) for binding tables and capture rules.

- Every window event goes to `egui_winit::State` first. `UiState::captures_pointer` and
  `captures_keyboard` decide whether gameplay ignores it; the HUD captures nothing and the
  inventory captures both. See [UI](ui.md).
- `E` opens the inventory, releases the cursor, and clears held movement keys. `E`, `Escape`, or
  the close button closes it and recaptures the cursor. The cursor stack is returned first.
- `W`, `A`, `S`, and `D` move the player horizontally using yaw only; pitch does not affect walking.
- `Space` requests a jump for the next frame; key repeat is ignored.
- Mouse look is applied only while the cursor is captured and the window is focused.
- A left click captures the cursor. Later left presses break in the legacy path and start a
  hold-to-break in the demo. Right click places the selected item.
- Number keys `1`-`9` select a demo hotbar slot; the legacy path accepts `1`-`5` for its block
  list.
- `Ctrl+S` saves the world and the player file. Closing the window performs a final terrain flush
  and then writes the player file.
- `Escape` releases the cursor and cancels a break; losing focus clears held keys, releases the
  cursor, and cancels a break.
- In the demo, breaking and placing go through `GameState` and `DemoRuntime::edit`, so inventory,
  drops, and the player save stay consistent. The legacy path calls `World::set_block`, then
  invalidates the owning chunk, its 3x3x3 neighbourhood, and every loaded chunk below it. Urgent
  light jobs run first; each chunk is remeshed only after its new light field is accepted.

## Coordinates and data

See [Coordinates](coordinates.md) for the full conventions, including chunk and mesh spaces.

- Right-handed, Y-up.
- A voxel at `(x, y, z)` occupies `[x, x + 1] × [y, y + 1] × [z, z + 1]`.
- The world stores 32³ chunks as four-bit packed block IDs (16 KiB per chunk, two cells per byte) with
  a non-air counter and cached terrain/water bounds (highest solid, highest water, and lowest column
  top) for horizon culling; `VCHK` v3 stores sparse four-byte records (three local coordinates and
  one block id), not one byte per voxel. Light lives beside the chunk map in a padded `LightField`;
  the sky plane is always stored, and the block-light plane only when a lamp is in range. Missing
  chunks are air, and stored chunks never contain `Air`.
- Chunk meshes use chunk-local cells, 12-byte packed vertices (three cell bytes plus the face index, two UV bytes, the texture layer, and packed light/AO), and merge coplanar faces whose world-space neighbour is air, including across chunk borders. Quads emit four vertices and six indices, wound counter-clockwise around the outward normal, so the terrain and water pipelines cull back faces. The shader unpacks cells and normals.
- LOD0 is the full-resolution greedy mesh; LOD1 and LOD2 are the same mesher over 2x2x2 and 4x4x4 downsamples. Both emit every chunk-boundary face so LOD transitions cannot crack. This per-chunk path is the fallback; its fixed distance bands apply only when the hierarchy is off.
- Chunks at or within four chunk units of the player use LOD0, chunks at eight or more use LOD1, chunks at sixteen or more use LOD2, and the ranges between keep the current LOD.
- With `ENGINE_HIERARCHY=1`, L0 stops at `ENGINE_DETAIL_CHUNKS` chunks around the player (default `16`, 512 m). Outside it, `SectionKey { level, coord }` names a 32³ sample section at a spacing of `2^level`: levels 1-4 cover 64/128/256/512 m. Level 0 is a chunk. Sections come from the seed plus the modified-chunk overlay, are never saved, and mesh in sample space with the same greedy contract. See the [hierarchy design](../design/lod-hierarchy.md), [design review](lod-hierarchy-review.md), and [ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md).
- Greedy quads UVs are in block units, so the sampler repeats the texture tile across merged faces.
- Chunk data, collision, editing, and saving stay full-resolution; LOD1, LOD2, and coarse sections are rendering-only.
- Chunk meshes are built from immutable snapshots and a one-cell border on worker threads; the main thread only uploads finished results. Coarse sections run as background jobs on the same pool (up to eleven in flight); the main thread builds the edit overlay, uploads the finished meshes, and records horizon bounds. Section meshing scans only the non-air y range, and unedited columns fill packed nibbles in bulk.
- Chunks are saved as `VCHK` v3 files; light is derived and never saved. Only modified chunks are written, and an emptied chunk is a v3 file with zero records. The format and validation rules are in the [v0.0.4 spec](../specs/v0.0.4.md), [v0.0.6 spec](../specs/v0.0.6.md), [v0.0.7 spec](../specs/v0.0.7.md), and [ADR-0008](../decisions/0008-chunk-save-format.md).

## Rendering resources

- Surface format comes from `Surface::get_default_config`; the default is typically `Bgra8UnormSrgb`.
- The chunk pipeline uses `Depth32Float`, depth write enabled, and `Less` depth comparison.
- Bind group 0 holds a 160-byte frame uniform: view-projection, sky, ambient, and sun colours, sun direction, an unlit flag, the far plane, time of day, and the camera position.
- The per-chunk fallback gives each non-empty chunk an origin uniform in bind group 1 and up to four vertex meshes: LOD0, LOD1, LOD2, and an optional transparent water mesh. Index buffers are shared until a larger chunk allocates its own. The vertex shader adds the origin to the chunk-local vertex position. The renderer uses this path when the adapter lacks storage buffers, `INDIRECT_FIRST_INSTANCE`, or indirect execution.
- The primary path is the quad pool from [ADR-0017](../decisions/0017-quad-pool-and-indirect-draws.md). Bind group 1 is a 48-byte-per-node info table plus the 12-byte quad pool. The vertex shader pulls one record per four indexed vertices through a shared `u16` index pattern; meshes above 16,383 quads keep the six-vertex non-indexed path. The CPU writes the opaque and water argument lists and normally records one `multi_draw_indexed_indirect` per pass, for two draws total; a mesh above 16,383 quads adds a non-indexed draw in that pass. Slots are dense and swap-removed; the renderer reports resident and visible geometry bytes, draw calls, and the pool high-water mark once per second.
- `NodeKey::{Chunk, Section}` shares one pool. Section slots stay resident while a retained parent is drawn but are hidden (`visible = false`) until the selection swaps them in. The hierarchy uses a larger pool reservation (8M quads) than the distance-32 path (3.2M). See the [hierarchy design](../design/lod-hierarchy.md), [design review](lod-hierarchy-review.md), and [ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md).
- Horizon culling builds column-top bins every 250 ms and is on by default (`ENGINE_HORIZON_CULL=0` disables it). It rejects L0 chunks and, with the hierarchy on, uploaded sections. Section occluders use the lowest solid column top and candidates the highest visible top, so water never hides land behind it. An occluder only marks an azimuth bin it fully contains; a candidate is tested against every bin its footprint intersects. The occluded sets are cached at build time, so a settled frame does hash lookups; the quad renderer iterates only slots with geometry.
- The clear colour follows the interpolated sky colour, so fog blends into the distance instead of ending against a fixed backdrop.
- A chunk's previous mesh stays visible until a newer mesh for its LOD is uploaded. Edits invalidate all per-chunk LODs and every overlapping coarse section; a chunk or section with an empty mesh releases its range or buffer.
- The CPU frustum-culls chunk and section AABBs in the quad path and writes indirect arguments for each visible node; visible chunks, visible vertices, resident and visible geometry bytes, draw calls, worker rebuild time, and upload time are logged once per second.
- The renderer requests GPU timestamp queries when the adapter supports them and reports the scene-pass time through `wgpu-profiler`; with the local `tracy` feature the same zones appear in Tracy. See [Profiling](profiling.md).
- The surface starts in FIFO (vsync) presentation. `V` reconfigures it to immediate or mailbox presentation when supported, and back.
- Water is a separate mesh, full-resolution for L0 chunks and coarse for sections. It is drawn after all opaque geometry in slot order with blending `SrcAlpha`/`OneMinusSrcAlpha`, alpha `0.65`, `LessEqual` depth testing, and no depth writes. Faces are not sorted, so overlapping water can blend out of order.
- The highlight is a 24-vertex line list drawn after chunk and water draws with `LessEqual` depth testing and no depth writes.
- The debug overlay is a screen-space triangle list with per-vertex colour, alpha blending, and an `Always` depth compare. It refreshes from the one-second frame sample.
- The depth texture is recreated on resize.
- `Outdated` and `Suboptimal` surfaces reconfigure; `Lost` surfaces are recreated; zero-sized resize dimensions are clamped to `1×1`.

## Current limitations

- Horizon culling is the only occlusion test and applies to L0 chunks and uploaded sections; Hi-Z and GPU traversal remain deferred.
- The hierarchy is opt-in behind `ENGINE_HIERARCHY=1`; the per-chunk LOD path remains the default and the fallback. Levels 3-4 use the same recursive projected refinement and parent retention as levels 1-2, with 256/512 m sections out to 8 km.
- No shadow maps or sorted transparency; water faces blend in slot order.
- One block palette; no inventory UI.

These are deliberate milestone boundaries. See `docs/roadmap.md` and the scope pages under `docs/dev/` for what replaces them.
