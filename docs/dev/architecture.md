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
| `src/world.rs` | Chunk and block storage, coordinate conversion, dirty-chunk tracking, `VCHK` save/load, and tests. |
| `src/mesh.rs` | Greedy meshing, the 2x2x2 LOD1 downsample, LOD selection, chunk snapshots, border blocks, and tests. |
| `src/mesher.rs` | Mesh jobs, revisions, the bounded micropool `WorkerPool`, finished-result collection, and tests. |
| `src/frustum.rs` | Six-plane frustum extraction and conservative AABB tests. |
| `src/metrics.rs` | Tracy plots for frame, GPU, chunk, draw, rebuild, upload, and queue metrics; no-ops without the `tracy` feature. |
| `src/voxel.rs` | Texture vertices, the highlight mesh, and the v0.0.4 face-culled mesher kept as a test reference. |
| `src/renderer.rs` | `wgpu` device/surface setup, pipelines, block texture array, buffers, depth texture, resizing, and draw submission. |
| `src/shader.wgsl` | Chunk vertex transform and textured fragment shading. See [Shader authoring](shaders.md). |
| `src/highlight.wgsl` | Line-list block outline. |
| `src/overlay.wgsl` | Screen-space overlay quads. |

## Ownership

- `App` owns the window handle, renderer, world, player, camera, selected block, input state, mesh queue, worker pool, and frame timing.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, chunk and highlight pipelines, camera uniform, per-chunk LOD buffers and origin uniforms, the block texture array, and depth attachment.
- `Camera` owns only look state; it does not read the clock or input devices.
- `Player` owns feet position, velocity, and ground state; it does not read input devices.
- `World` owns the chunk map; every stored chunk is non-empty.
- `mesh::build_mesh` is a pure function over a chunk snapshot and its border blocks.
- `mesher::WorkerPool` owns the worker threads; `MeshQueue` owns revisions, pending jobs, and desired LODs.

The renderer holds an `Arc<Window>` and creates a `Surface<'static>` from it. The application also holds a window clone for redraw requests and cursor capture.

## Startup

1. `main` calls `engine::run`.
2. `run` initialises tracing, then loads `world/chunks/` or seeds the demo world when the directory is missing. `ENGINE_STRESS_SCENE=1` builds the in-memory stress scene instead and skips world I/O.
3. `run` creates the `winit` event loop and `App`.
4. `App::resumed` creates the window and renderer.
5. The renderer selects an adapter, requests a device and queue, configures the surface, builds the pipelines and the texture array, and starts no chunk mesh work itself.
6. `App` starts the micropool worker pool and queues an urgent LOD0 job for every chunk. Meshes appear as workers finish.
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
  -> recalculate each chunk's desired LOD and request missing meshes
  -> collect up to two finished meshes from the worker pool
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
- Breaking and placing call `World::set_block`, invalidate the edited chunks, and queue an urgent LOD0 rebuild plus a background LOD1 rebuild. The dirty count, total vertices, and enqueue time are logged.

## Coordinates and data

See [Coordinates](coordinates.md) for the full conventions, including chunk and mesh spaces.

- Right-handed, Y-up.
- A voxel at `(x, y, z)` occupies `[x, x + 1] × [y, y + 1] × [z, z + 1]`.
- The world stores 32³ chunks of `Block` values. Missing chunks are air, and stored chunks never contain `Air`.
- Chunk meshes use chunk-local positions, 36-byte textured vertices, and merge coplanar faces whose world-space neighbour is air, including across chunk borders.
- LOD0 is the full-resolution greedy mesh; LOD1 is the same mesher over a 2x2x2 downsample at half the cell count. LOD1 emits every chunk-boundary face so LOD transitions cannot crack.
- Chunks at or within four chunk units of the player use LOD0, chunks at eight or more use LOD1, and the range between keeps the current LOD.
- Greedy quads UVs are in block units, so the sampler repeats the texture tile across merged faces.
- Chunk data, collision, editing, and saving stay full-resolution; LOD1 is rendering-only.
- Meshes are built from immutable chunk snapshots and a one-cell border on worker threads; the main thread only uploads finished results.
- Chunks are saved as `VCHK` v1 files; the format and validation rules are in the [v0.0.4 spec](../specs/v0.0.4.md) and [ADR-0008](../decisions/0008-chunk-save-format.md).

## Rendering resources

- Surface format comes from `Surface::get_default_config`; the default is typically `Bgra8UnormSrgb`.
- The chunk pipeline uses `Depth32Float`, depth write enabled, and `Less` depth comparison.
- The camera uniform is a 64-byte `mat4x4<f32>` in bind group 0.
- Each non-empty chunk owns an origin uniform in bind group 1 and up to two vertex buffers, one per LOD. The vertex shader adds the origin to the chunk-local vertex position.
- Bind group 2 holds one ten-layer `texture_2d_array` and a nearest-filtering sampler that repeats in U and V and clamps in W. The table is in [Texture and asset pipeline](textures.md).
- A chunk's previous buffer stays visible until a newer mesh for its LOD is uploaded. Edits invalidate both LODs; a chunk with an empty mesh releases that buffer.
- Draw calls are frustum-culled per chunk AABB and counted with visible chunks, visible vertices, draw calls, worker rebuild time, and upload time in the once-per-second debug log.
- With the local `tracy` feature, the renderer requests timestamp queries when the adapter supports them, wraps the pass in a `wgpu-profiler` GPU zone, and ends the profiler frame after present. See [Profiling](profiling.md).
- The surface starts in FIFO (vsync) presentation. `V` reconfigures it to immediate or mailbox presentation when supported, and back.
- The highlight is a 24-vertex line list drawn after chunk meshes with `LessEqual` depth testing and no depth writes.
- The debug overlay is a screen-space triangle list with per-vertex colour, alpha blending, and an `Always` depth compare. It refreshes from the one-second frame sample.
- The depth texture is recreated on resize.
- `Outdated` and `Suboptimal` surfaces reconfigure; `Lost` surfaces are recreated; zero-sized resize dimensions are clamped to `1×1`.

## Current limitations

- No occlusion culling and no third LOD level.
- No terrain generation or streaming. World management and backups are described in
  [World management](world-management.md).
- No lighting, fog, or transparency.
- One block palette; no inventory UI.

These are deliberate milestone boundaries. See `docs/roadmap.md` and the scope pages under `docs/dev/` for what replaces them.
