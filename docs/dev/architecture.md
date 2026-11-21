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
├── app.rs                  # App, ApplicationHandler, frame, input, overlay
├── demo/
│   ├── mod.rs              # DemoRuntime seam, constants, re-exports, tests
│   ├── patch.rs            # TerrainDemo, block access, LOD apply, materials
│   ├── lod.rs              # LOD0/1/2 bands, clipped LOD1, LOD2
│   ├── stream.rs           # column ring, worker pool, jobs
│   ├── edit.rs             # edit record, incremental rebuild, save queue
│   └── persist.rs          # replay, repair prompt, recovered journal
├── v0/                     # frozen legacy stack
├── renderer.rs, camera.rs, player.rs, raycast.rs, sky.rs, overlay.rs, frustum.rs
├── block.rs, config.rs, headless.rs, metrics.rs
└── *.wgsl                  # shaders stay with the renderer
```

| Path | Responsibility |
|---|---|
| `src/main.rs` | Thin binary entry point; calls `engine::run()`. |
| `src/lib.rs` | Crate table of contents: crate docs, `mod` declarations, the `pub(crate)` terrain re-exports, and the public surface (`run`, `RunError`, `RendererError`, `ChunkCoord`, `SaveError`, `v0`, and the `terrain` facade). No logic. See [Public API](public-api.md). |
| `src/startup.rs` | `run`, tracing setup, configuration and seed loading, world choice (saved, generated, or stress), the `DemoRuntime::open` hand-off, and the headless hand-off. |
| `src/app.rs` | `App`, the `winit` application handler, window/renderer lifecycle, frame and redraw order, input mapping, overlay and stats, and the demo-to-renderer upload adapter. |
| `src/demo/mod.rs` | `DemoRuntime`, the demo's one app-facing seam: it owns the patch, ring, save worker, repair prompt, pending rebuild, and edit queue, and exposes `open`, `tick`, `edit`, `shutdown`, `repair`, `stats`, and the read-only overlay/upload accessors. Also shared constants and the test fixtures. |
| `src/demo/patch.rs` | `TerrainDemo` state, resident columns, block access and collision, light/mesh installation, LOD selection, and the material/light tables. |
| `src/demo/lod.rs` | LOD0/1/2 band selection, complete and clipped LOD1 regions, and LOD2 derivation. |
| `src/demo/stream.rs` | The column ring: worker pool, generation and light/mesh jobs, the coarse job, snapshots, and in-flight bookkeeping. |
| `src/demo/edit.rs` | The edit record, incremental relight/remesh, and journal/section save queueing per edit. |
| `src/demo/persist.rs` | Save replay, the repair prompt, and the recovered-journal path that runs before the ring opens. |
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
| `src/frustum.rs` | Six-plane frustum extraction and conservative AABB tests. |
| `src/metrics.rs` | Tracy plots for frame, GPU, chunk, draw, rebuild, upload, and queue metrics; no-ops without the `tracy` feature. |
| `src/config.rs` | `config.toml` parsing for the render distance. |
| `src/headless.rs` | Windowless fixed-step run selected by `ENGINE_HEADLESS=1`; drives generation, streaming, light, and meshing, then prints one `HEADLESS` summary line. |
| `crates/terrain-core/src/save.rs`, `crates/terrain-core/src/world_save.rs` | Section files, backup rotation, journal file, replay/trimming, `WorldSave`, save worker, repair, migration/report. See [Terrain v1 runtime](terrain-v1.md) and [Save format](save-format.md). |
| `crates/terrain-core/src/light2.rs`, `crates/terrain-core/src/mesher2.rs`, `crates/terrain-core/src/lod.rs`, `crates/terrain-core/src/section.rs`, `src/terrain_draw.wgsl` | v1 column light, greedy mesher, coarse LOD, section storage, packed-quad shader. The shader stays with the renderer. |
| `src/sky.rs` | Day/night cycle, sun direction, sky/ambient/sun colours, and the CPU fog formula. |
| `src/renderer.rs` | `wgpu` device/surface setup, pipelines, block texture array, buffers, depth texture, resizing, and draw submission. |
| `src/shader.wgsl` | Chunk vertex transform, lit/fogged opaque shading, and the animated water fragment. See [Shader authoring](shaders.md). |
| `src/highlight.wgsl` | Line-list block outline. |
| `src/overlay.wgsl` | Screen-space overlay quads. |

`src/v0/` groups the frozen legacy stack. `src/lib.rs` declares `mod v0;` and re-exports
`v0::{generation, light, mesh, mesher, streaming, voxel, world}`, so paths such as `crate::world`
and `engine::world` resolve exactly as before. v0 is still the default run path when `TERRAIN_DEMO`
is unset; it receives fixes only, and the whole directory is removed when the terrain demo becomes
the default. The move is behaviour-preserving and does not change the save format.

`src/startup.rs`, `src/app.rs`, and `src/demo/` split the old crate-root logic by responsibility.
`lib.rs` is now a table of contents; `startup` owns environment switches and world choice, `app`
owns `winit`/`wgpu` and the frame loop, and `demo` owns the v1 patch, ring, LOD, edits, and saves.
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
| `shutdown()` | Flush the save worker on `Ctrl+S` and window close. |
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

## Ownership

- `App` owns the window handle, renderer, world, player, camera, sky clock, selected block, input state, mesh queue, worker pool, frame timing, and one `Option<DemoRuntime>`.
- `DemoRuntime` owns the v1 patch, column ring and workers, save worker, repair prompt, pending rebuild, and edit queue. It returns a `DemoFrame` instead of holding the renderer.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, chunk and highlight pipelines, camera uniform, per-chunk LOD buffers and origin uniforms, the block texture array, and depth attachment.
- `Camera` owns only look state; it does not read the clock or input devices.
- `Player` owns feet position, velocity, and ground state; it does not read input devices.
- `World` owns the dense chunk map and the cached light fields; every stored chunk is non-empty. Generated chunks are inserted through `World::insert_chunk`, and chunk snapshots for meshing are a direct copy of the block array.
- `mesh::build_mesh` is a pure function over a chunk snapshot and its border blocks.
- `mesher::WorkerPool` owns the worker threads for mesh and light jobs; `MeshQueue` owns revisions, pending jobs, built state, and desired LODs.

The renderer holds an `Arc<Window>` and creates a `Surface<'static>` from it. The application also holds a window clone for redraw requests and cursor capture.

## Startup

1. `main` calls `engine::run`.
2. `startup::run` initialises tracing and reads `config.toml`. It then loads `world/seed` (creating one when the world is new) and generates the spawn column; saved chunk files are applied over the generated blocks. `ENGINE_STRESS_SCENE=1` builds the in-memory stress scene instead and skips world I/O. `TERRAIN_DEMO=1` opens or creates the v1 world, replays recovered edits, and starts the column ring.
3. `startup::run` creates the `winit` event loop and the `App` from `app.rs`.
4. `App::resumed` creates the window and renderer.
5. The renderer selects an adapter, requests a device and queue, configures the surface, builds the pipelines and the texture array, and starts no chunk mesh work itself.
6. `App` starts the micropool worker pool and queues an urgent LOD0 job for every loaded chunk. Meshes appear as workers finish.
7. The application requests the first redraw.

World load errors return through `RunError::Save` before the window opens. Startup errors are stored on `App`, logged, and returned through `RunError` after the event loop exits. The engine does not panic on adapter, device, surface, shader, or malformed save failures.

## Frame flow

```
RedrawRequested
  -> read frame delta
  -> build walking input from held keys and camera yaw
  -> player.update() with gravity, jump, and swept collision
  -> move the camera to the player eye
  -> raycast and update the block outline
  -> advance the day/night cycle
  -> unload columns past the margin, then stream missing columns and mark ready ones
  -> request light and mesh work only for chunks in the level work set
  -> recalculate each chunk's desired LOD and request missing meshes
  -> drain finished light, mesh, and generation results (two mesh uploads per frame)
  -> upload them and select the active LOD per chunk
  -> extract the frustum from the view-projection
  -> write camera uniform
  -> renderer.render(&frustum)
       -> acquire surface texture
       -> encode clear + depth clear + frustum-culled chunk draws + outline
       -> submit
       -> present
  -> request the next redraw
```

With `TERRAIN_DEMO=1`, `DemoRuntime::tick` runs before the player step and in place of the v0
streaming phases: it applies a landed rebuild, drains at most 16 ring jobs, plans the next batch
through `residency::RingBudget::v1`, then runs the coarse (250 ms) and upload (100 ms) throttles. It
reports the spawn and upload through a `DemoFrame`; the app performs `set_spawn` and
`upload_terrain_demo`. The v0 world still streams underneath, but the demo path is what draws.

- Continuous redraw runs while the window is visible, including when it is unfocused, so profilers and external tools keep receiving frames. Input stays gated on focus.
- The loop pauses only when the window is occluded.
- Frame time is logged at debug level about once per second.

## Input flow

See [Controls](controls.md) for binding tables and capture rules.

- `W`, `A`, `S`, and `D` move the player horizontally using yaw only; pitch does not affect walking.
- `Space` requests a jump for the next frame; key repeat is ignored.
- Mouse look is applied only while the cursor is captured and the window is focused.
- A left click captures the cursor. Later left clicks break the targeted block; right clicks place the selected block.
- Number keys `1`–`5` select Stone, Dirt, Grass, Wood, or Leaves.
- `Ctrl+S` saves the world; closing the window saves before exit.
- `Escape` releases the cursor; losing focus clears held keys and releases the cursor.
- Breaking and placing call `World::set_block`, then invalidate the owning chunk, its 3x3x3 neighbourhood, and every loaded chunk below it. Urgent light jobs run first; each chunk is remeshed only after its new light field is accepted. The dirty count, total vertices, and enqueue time are logged.

## Coordinates and data

See [Coordinates](coordinates.md) for the full conventions, including chunk and mesh spaces.

- Right-handed, Y-up.
- A voxel at `(x, y, z)` occupies `[x, x + 1] × [y, y + 1] × [z, z + 1]`.
- The world stores 32³ chunks of `Block` values in one dense `[Block; 32768]` array per chunk (32 KiB); a chunk also counts its non-air blocks. Missing chunks are air, and stored chunks never contain `Air`. Light lives beside the chunk map in a padded `LightField`, not inside `Chunk`.
- Chunk meshes use chunk-local cells, 12-byte packed vertices (three cell bytes plus the face index, two UV bytes, the texture layer, and packed light/AO), and merge coplanar faces whose world-space neighbour is air, including across chunk borders. Quads emit four vertices and six indices, wound counter-clockwise around the outward normal, so the terrain and water pipelines cull back faces. The shader unpacks cells and normals.
- LOD0 is the full-resolution greedy mesh; LOD1 and LOD2 are the same mesher over 2x2x2 and 4x4x4 downsamples. Both emit every chunk-boundary face so LOD transitions cannot crack.
- Chunks at or within four chunk units of the player use LOD0, chunks at eight or more use LOD1, chunks at sixteen or more use LOD2, and the ranges between keep the current LOD.
- Greedy quads UVs are in block units, so the sampler repeats the texture tile across merged faces.
- Chunk data, collision, editing, and saving stay full-resolution; LOD1 is rendering-only.
- Meshes are built from immutable chunk snapshots and a one-cell border on worker threads; the main thread only uploads finished results.
- Chunks are saved as `VCHK` v3 files; light is derived and never saved. Only modified chunks are written, and an emptied chunk is a v3 file with zero records. The format and validation rules are in the [v0.0.4 spec](../specs/v0.0.4.md), [v0.0.6 spec](../specs/v0.0.6.md), [v0.0.7 spec](../specs/v0.0.7.md), and [ADR-0008](../decisions/0008-chunk-save-format.md).

## Rendering resources

- Surface format comes from `Surface::get_default_config`; the default is typically `Bgra8UnormSrgb`.
- The chunk pipeline uses `Depth32Float`, depth write enabled, and `Less` depth comparison.
- Bind group 0 holds a 160-byte frame uniform: view-projection, sky, ambient, and sun colours, sun direction, an unlit flag, the far plane, time of day, and the camera position.
- Each non-empty chunk owns an origin uniform in bind group 1 and up to four vertex/index meshes: LOD0, LOD1, LOD2, and an optional transparent water mesh. The vertex shader adds the origin to the chunk-local vertex position.
- Bind group 2 holds one ten-layer `texture_2d_array` and a nearest-filtering sampler that repeats in U and V and clamps in W. The table is in [Texture and asset pipeline](textures.md).
- The clear colour follows the interpolated sky colour, so fog blends into the distance instead of ending against a fixed backdrop.
- A chunk's previous buffer stays visible until a newer mesh for its LOD is uploaded. Edits invalidate both LODs; a chunk with an empty mesh releases that buffer.
- Draw calls are frustum-culled per chunk AABB and counted with visible chunks, visible vertices, draw calls, worker rebuild time, and upload time in the once-per-second debug log.
- With the local `tracy` feature, the renderer requests timestamp queries when the adapter supports them, wraps the pass in a `wgpu-profiler` GPU zone, and ends the profiler frame after present. See [Profiling](profiling.md).
- The surface starts in FIFO (vsync) presentation. `V` reconfigures it to immediate or mailbox presentation when supported, and back.
- Water is a separate full-resolution mesh, drawn after all opaque chunks in ascending chunk order with blending `SrcAlpha`/`OneMinusSrcAlpha`, alpha `0.65`, `LessEqual` depth testing, and no depth writes. Faces are not sorted, so overlapping water can blend out of order.
- The highlight is a 24-vertex line list drawn after chunk and water draws with `LessEqual` depth testing and no depth writes.
- The debug overlay is a screen-space triangle list with per-vertex colour, alpha blending, and an `Always` depth compare. It refreshes from the one-second frame sample.
- The depth texture is recreated on resize.
- `Outdated` and `Suboptimal` surfaces reconfigure; `Lost` surfaces are recreated; zero-sized resize dimensions are clamped to `1×1`.

## Current limitations

- No occlusion culling, horizon culling, or batched indirect drawing.
- No shadow maps or sorted transparency; water faces blend in mesh order.
- One block palette; no inventory UI.

These are deliberate milestone boundaries. See `docs/roadmap.md` and the scope pages under `docs/dev/` for what replaces them.
