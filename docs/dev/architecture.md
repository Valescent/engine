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
| `src/mesh.rs` | Greedy meshing, the 2x2x2 LOD1 and 4x4x4 LOD2 downsamples, LOD selection, chunk snapshots, border blocks, and tests. |
| `src/mesher.rs` | Mesh, light, and generation jobs, revisions, the bounded micropool `WorkerPool`, finished-result collection, and tests. |
| `src/frustum.rs` | Six-plane frustum extraction and conservative AABB tests. |
| `src/metrics.rs` | Tracy plots for frame, GPU, chunk, draw, rebuild, upload, and queue metrics; no-ops without the `tracy` feature. |
| `src/light.rs` | Four-bit packed sky/block light, BFS propagation, padded `LightField` accessors, and direct-light snapshots. |
| `src/generation.rs` | Seeded noise fields, height, biomes, caves, water fill, trees, and the spawn column. |
| `src/streaming.rs` | Player-centred load rings, forward-first ordering, unload hysteresis, readiness, and the movement guard. |
| `src/config.rs` | `config.toml` parsing for the render distance. |
| `src/headless.rs` | Windowless fixed-step run selected by `ENGINE_HEADLESS=1`; drives generation, streaming, light, and meshing, then prints one `HEADLESS` summary line. |
| `src/sky.rs` | Day/night cycle, sun direction, sky/ambient/sun colours, and the CPU fog formula. |
| `src/voxel.rs` | The packed 12-byte chunk vertex, the highlight mesh, and the v0.0.4 face-culled mesher kept as a test reference. |
| `src/renderer.rs` | `wgpu` device/surface setup, pipelines, block texture array, buffers, depth texture, resizing, and draw submission. |
| `src/shader.wgsl` | Chunk vertex transform, lit/fogged opaque shading, and the animated water fragment. See [Shader authoring](shaders.md). |
| `src/highlight.wgsl` | Line-list block outline. |
| `src/overlay.wgsl` | Screen-space overlay quads. |

## Ownership

- `App` owns the window handle, renderer, world, player, camera, sky clock, selected block, input state, mesh queue, worker pool, and frame timing.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, chunk and highlight pipelines, camera uniform, per-chunk LOD buffers and origin uniforms, the block texture array, and depth attachment.
- `Camera` owns only look state; it does not read the clock or input devices.
- `Player` owns feet position, velocity, and ground state; it does not read input devices.
- `World` owns the dense chunk map and the cached light fields; every stored chunk is non-empty. Generated chunks are inserted through `World::insert_chunk`, and chunk snapshots for meshing are a direct copy of the block array.
- `mesh::build_mesh` is a pure function over a chunk snapshot and its border blocks.
- `mesher::WorkerPool` owns the worker threads for mesh and light jobs; `MeshQueue` owns revisions, pending jobs, built state, and desired LODs.

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
  -> extract the frustum from the view-projection
  -> write camera uniform
  -> renderer.render(&frustum)
       -> acquire surface texture
       -> encode clear + depth clear + frustum-culled chunk draws + outline
       -> submit
       -> present
  -> request the next redraw
```

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
- The world stores 32³ chunks as four-bit packed block IDs (16 KiB per chunk, two cells per byte) with
  a non-air counter and the highest solid local y for horizon culling; `VCHK` still stores one byte per
  block. Light lives beside the chunk map in a padded `LightField`; the sky plane is always stored, and
  the block-light plane only when a lamp is in range. Missing chunks are air, and stored chunks never
  contain `Air`. Light lives beside the chunk map in a padded `LightField`, not inside `Chunk`.
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
- Each non-empty chunk owns an origin uniform in bind group 1 and up to four vertex meshes: LOD0, LOD1, LOD2, and an optional transparent water mesh. Index buffers are shared: one `u16` quad pattern serves every chunk that fits a `u16` index, and only larger chunks allocate their own `u32` buffer. The vertex shader adds the origin to the chunk-local vertex position. This is the fallback path.
- When the adapter exposes storage buffers, `INDIRECT_FIRST_INSTANCE`, and indirect execution, chunk geometry uses the quad path from [ADR-0015](../decisions/0015-quad-pool-and-indirect-draws.md): bind group 1 is a 48-byte-per-chunk storage table plus the 12-byte quad pool, the vertex shader pulls six vertices per record, and the CPU writes one indirect argument list for opaque and one for water. Draws are one `multi_draw_indirect` per pass.
- The clear colour follows the interpolated sky colour, so fog blends into the distance instead of ending against a fixed backdrop.
- A chunk's previous mesh stays visible until a newer mesh for its LOD is uploaded. Edits invalidate both LODs; a chunk with an empty mesh releases its range or buffer.
- The CPU frustum-culls chunk AABBs in the quad path and writes indirect arguments for each visible chunk; visible chunks, visible vertices, resident and visible geometry bytes, draw calls, worker rebuild time, and upload time are logged once per second.
- With the local `tracy` feature, the renderer requests timestamp queries when the adapter supports them, wraps the pass in a `wgpu-profiler` GPU zone, and ends the profiler frame after present. See [Profiling](profiling.md).
- The surface starts in FIFO (vsync) presentation. `V` reconfigures it to immediate or mailbox presentation when supported, and back.
- Water is a separate full-resolution mesh, drawn after all opaque chunks in ascending chunk order with blending `SrcAlpha`/`OneMinusSrcAlpha`, alpha `0.65`, `LessEqual` depth testing, and no depth writes. Faces are not sorted, so overlapping water can blend out of order.
- The highlight is a 24-vertex line list drawn after chunk and water draws with `LessEqual` depth testing and no depth writes.
- The debug overlay is a screen-space triangle list with per-vertex colour, alpha blending, and an `Always` depth compare. It refreshes from the one-second frame sample.
- The depth texture is recreated on resize.
- `Outdated` and `Suboptimal` surfaces reconfigure; `Lost` surfaces are recreated; zero-sized resize dimensions are clamped to `1×1`.

## Current limitations

- No occlusion culling; horizon culling is opt-in behind `ENGINE_HORIZON_CULL`.
- No shadow maps or sorted transparency; water faces blend in mesh order.
- One block palette; no inventory UI.

These are deliberate milestone boundaries. See `docs/roadmap.md` and the scope pages under `docs/dev/` for what replaces them.
