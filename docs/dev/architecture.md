# Architecture

The engine is a small `winit` and `wgpu` application. This page describes the current modules, ownership, and frame flow. Update it when module boundaries or resource ownership change.

## Modules

| Path | Responsibility |
|---|---|
| `src/main.rs` | Thin binary entry point; calls `engine::run()`. |
| `src/lib.rs` | Public entry point, `winit` application handler, input, interaction, frame loop, save-on-exit, and error aggregation. See [Public API](public-api.md). |
| `src/camera.rs` | Look state, yaw/pitch math, view-projection, and tests. Position comes from the player eye. |
| `src/block.rs` | Block types, stable IDs, hotbar mapping, and face texture layers. |
| `src/player.rs` | Player physics, gravity, jumping, swept AABB collision, and tests. |
| `src/overlay.rs` | Screen-space debug text geometry and font. |
| `src/raycast.rs` | DDA voxel traversal, hit result, tie-breaking, and tests. |
| `src/world.rs` | Chunk and block storage, coordinate conversion, dirty-chunk tracking, the modified set, `VCHK` save/load, and seed file handling. |
| `src/mesh.rs` | Greedy meshing, the 2x2x2 LOD1 and 4x4x4 LOD2 downsamples, sample-space section meshing, LOD selection, chunk snapshots, border blocks, and tests. |
| `src/mesher.rs` | Mesh, light, and generation jobs, revisions, the bounded micropool `WorkerPool`, finished-result collection, and tests. |
| `src/section.rs` | `SectionKey` level/extent/origin/parent/child math, seed-native coarse generation, the modified-chunk edit overlay, coarse light, and section meshes. |
| `src/hierarchy.rs` | Projected section selection, readiness, hysteresis, parent retention, hidden children, build order, and eviction. |
| `src/frustum.rs` | Six-plane frustum extraction and conservative AABB tests. |
| `src/horizon.rs` | Column-top horizon bins and the conservative occlusion test for L0 chunks and hierarchy sections. |
| `src/metrics.rs` | Tracy plots for frame, GPU, chunk, draw, rebuild, upload, and queue metrics; no-ops without the `tracy` feature. |
| `src/light.rs` | Four-bit packed sky/block light, BFS propagation, padded `LightField` accessors, and direct-light snapshots. |
| `src/generation.rs` | Seeded noise fields, height, biomes, caves, water fill, trees, and the spawn column. |
| `src/streaming.rs` | Player-centred load rings, forward-first ordering, unload hysteresis, readiness, and the movement guard. |
| `src/config.rs` | `config.toml` parsing for the render distance. |
| `src/headless.rs` | Windowless fixed-step run selected by `ENGINE_HEADLESS=1`; drives generation, streaming, light, and meshing, then prints one `HEADLESS` summary line. |
| `src/sky.rs` | Day/night cycle, sun direction, sky/ambient/sun colours, and the CPU fog formula. |
| `src/voxel.rs` | The packed 12-byte chunk vertex, the highlight mesh, and the v0.0.4 face-culled mesher kept as a test reference. |
| `src/renderer.rs` | `wgpu` device/surface setup, pipelines, block texture array, buffers, depth texture, resizing, capture readback, and draw submission. |
| `src/quad.rs` | The 12-byte quad record and 48-byte chunk-info layout. |
| `src/quad_renderer.rs` | Quad-pool uploads keyed by `NodeKey`, the chunk-info table, indirect arguments, visibility, and the two draws. |
| `src/pool.rs` | Flat 64-quad pages with a coalescing free list and frames-in-flight retirement. |
| `src/shader.wgsl` | Chunk vertex transform, lit/fogged opaque shading, and the animated water fragment. See [Shader authoring](shaders.md). |
| `src/quad.wgsl` | Vertex pulling, indexed quads, and the pooled opaque/water fragment shaders. |
| `src/highlight.wgsl` | Line-list block outline. |
| `src/overlay.wgsl` | Screen-space overlay quads. |

## Ownership

- `App` owns the window handle, renderer, world, player, camera, sky clock, selected block, input state, mesh queue, worker pool, hierarchy selection and uploaded-section set, horizon bins, and frame timing.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, chunk and highlight pipelines, camera uniform, the quad-pool renderer, the per-chunk fallback buffers and origin uniforms, the block texture array, and depth attachment.
- `Camera` owns only look state; it does not read the clock or input devices.
- `Player` owns feet position, velocity, and ground state; it does not read input devices.
- `World` owns the packed chunk map and the cached light fields; every stored chunk is non-empty. Generated chunks are inserted through `World::insert_chunk`, and chunk snapshots for meshing share the chunk's packed `Arc<Vec<u8>>`; a write copies the storage first (copy-on-write), so a snapshot stays immutable.
- `hierarchy::Hierarchy` owns the section target, readiness, build queue, hysteresis memory, and eviction decisions. Coarse sections are derived from the seed plus the modified-chunk overlay; they live outside `World` and are never saved.
- `mesh::build_mesh` is a pure function over a chunk snapshot and its border blocks; `section::generate_section` is a pure function of the seed, level, coordinate, and edit overlay.
- `mesher::WorkerPool` owns the worker threads for chunk generation, light, mesh, and coarse section jobs; `MeshQueue` owns revisions, pending jobs, built state, and desired LODs.

The renderer holds an `Arc<Window>` and creates a `Surface<'static>` from it. The application also holds a window clone for redraw requests and cursor capture.

## Startup

1. `main` calls `engine::run`.
2. `run` initialises tracing and reads `config.toml`. It then loads `world/seed` (creating one when the world is new) and generates the spawn column; saved chunk files are applied over the generated blocks. `ENGINE_STRESS_SCENE=1` builds the in-memory stress scene instead and skips world I/O.
3. `run` creates the `winit` event loop and `App`.
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
  -> when the hierarchy is enabled, select sections by projected size, submit up to eleven
     generation/light/mesh jobs, upload finished sections, then update visibility and evict
  -> rebuild horizon bins every 250 ms when horizon culling is enabled
  -> extract the frustum from the view-projection
  -> write camera uniform
  -> renderer.render(&frustum, &horizon)
       -> acquire surface texture
       -> encode clear + depth clear + frustum/horizon-culled draws + outline
       -> submit
       -> present
  -> request the next redraw
```

- Continuous redraw runs while the window is visible, including when it is unfocused, so profilers and external tools keep receiving frames. Input stays gated on focus.
- The loop pauses only when the window is occluded.
- Frame time is logged at debug level about once per second.
- `ENGINE_CAPTURE=<path>` arms one surface readback after `ENGINE_CAPTURE_DELAY` seconds (default `20`) and writes a 24-bit BMP. It is a development hook, not a runtime feature.
- `ENGINE_YAW`, `ENGINE_AUTO_WALK=1`, and `ENGINE_AUTO_STRAFE=1` give captures a reproducible camera and movement without changing the normal input path.

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
- The primary path is the quad pool from [ADR-0015](../decisions/0015-quad-pool-and-indirect-draws.md). Bind group 1 is a 48-byte-per-node info table plus the 12-byte quad pool. The vertex shader pulls one record per four indexed vertices through a shared `u16` index pattern; meshes above 16,383 quads keep the six-vertex non-indexed path. The CPU writes the opaque and water argument lists and normally records one `multi_draw_indexed_indirect` per pass, for two draws total; a mesh above 16,383 quads adds a non-indexed draw in that pass. Slots are dense and swap-removed; the renderer reports resident and visible geometry bytes, draw calls, and the pool high-water mark once per second.
- `NodeKey::{Chunk, Section}` shares one pool. Section slots stay resident while a retained parent is drawn but are hidden (`visible = false`) until the selection swaps them in. The hierarchy uses a larger pool reservation (8M quads) than the distance-32 path (3.2M). See the [hierarchy design](../design/lod-hierarchy.md), [design review](lod-hierarchy-review.md), and [ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md).
- Horizon culling builds column-top bins every 250 ms and is on by default (`ENGINE_HORIZON_CULL=0` disables it). It rejects L0 chunks and, with the hierarchy on, uploaded sections. Section occluders use the lowest solid column top and candidates the highest visible top, so water never hides land behind it. The occluded sets are cached at build time, so a settled frame does hash lookups; the quad renderer iterates only slots with geometry.
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
