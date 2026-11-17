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
| `src/voxel.rs` | Mesh vertices, textured chunk meshing, highlight mesh, and tests. |
| `src/renderer.rs` | `wgpu` device/surface setup, pipelines, block texture array, buffers, depth texture, resizing, and draw submission. |
| `src/shader.wgsl` | Chunk vertex transform and textured fragment shading. See [Shader authoring](shaders.md). |
| `src/highlight.wgsl` | Line-list block outline. |
| `src/overlay.wgsl` | Screen-space overlay quads. |

## Ownership

- `App` owns the window handle, renderer, world, player, camera, selected block, input state, and frame timing.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, chunk and highlight pipelines, camera uniform, per-chunk buffers and origin uniforms, the block texture array, and depth attachment.
- `Camera` owns only look state; it does not read the clock or input devices.
- `Player` owns feet position, velocity, and ground state; it does not read input devices.
- `World` owns the chunk map; every stored chunk is non-empty.
- `voxel::build_chunk_mesh` is a pure function over the world and one chunk.

The renderer holds an `Arc<Window>` and creates a `Surface<'static>` from it. The application also holds a window clone for redraw requests and cursor capture.

## Startup

1. `main` calls `engine::run`.
2. `run` initialises tracing, then loads `world/chunks/` or seeds the demo world when the directory is missing.
3. `run` creates the `winit` event loop and `App`.
4. `App::resumed` creates the window and renderer.
5. The renderer selects an adapter, requests a device and queue, configures the surface, builds both pipelines and the texture array, and uploads every non-empty chunk once.
6. The application requests the first redraw.

World load errors return through `RunError::Save` before the window opens. Startup errors are stored on `App`, logged, and returned through `RunError` after the event loop exits. The engine does not panic on adapter, device, surface, shader, or malformed save failures.

## Frame flow

```
RedrawRequested
  -> read frame delta
  -> build walking input from held keys and camera yaw
  -> player.update() with gravity, jump, and swept collision
  -> move the camera to the player eye
  -> raycast and update the block outline
  -> write camera uniform
  -> renderer.render()
       -> acquire surface texture
       -> encode clear + depth clear + chunk draws + outline
       -> submit
       -> present
  -> request next redraw while focused
```

- The loop pauses continuous redraw when the window loses focus or becomes occluded.
- A single frame is still rendered on demand while unfocused, so the window is not left blank.
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
- Breaking and placing call `World::set_block`, then re-mesh and upload only the dirty chunks. The dirty count, total vertices, and rebuild time are logged.

## Coordinates and data

See [Coordinates](coordinates.md) for the full conventions, including chunk and mesh spaces.

- Right-handed, Y-up.
- A voxel at `(x, y, z)` occupies `[x, x + 1] × [y, y + 1] × [z, z + 1]`.
- The world stores 32³ chunks of `Block` values. Missing chunks are air, and stored chunks never contain `Air`.
- Chunk meshes use chunk-local positions, 36-byte textured vertices, and cull faces whose world-space neighbour is solid, including across chunk borders.
- Visible voxel faces are emitted as two non-indexed triangles with position, normal, UV, and texture-layer data.
- Chunk voxels are sorted by local coordinate before meshing so output is independent of `HashMap` iteration order.
- Chunks are saved as `VCHK` v1 files; the format and validation rules are in the [v0.0.4 spec](../specs/v0.0.4.md) and [ADR-0008](../decisions/0008-chunk-save-format.md).

## Rendering resources

- Surface format comes from `Surface::get_default_config`; the default is typically `Bgra8UnormSrgb`.
- The chunk pipeline uses `Depth32Float`, depth write enabled, and `Less` depth comparison.
- The camera uniform is a 64-byte `mat4x4<f32>` in bind group 0.
- Each non-empty chunk owns a vertex buffer and a 16-byte origin uniform in bind group 1. The vertex shader adds the origin to the chunk-local vertex position.
- Bind group 2 holds one eight-layer `texture_2d_array` and a nearest-filtering sampler. The table is in [Texture and asset pipeline](textures.md).
- Chunk vertex buffers are created or replaced only when their chunk is dirty. A chunk with an empty mesh releases its buffer.
- The surface starts in FIFO (vsync) presentation. `V` reconfigures it to immediate or mailbox presentation when supported, and back.
- The highlight is a 24-vertex line list drawn after chunk meshes with `LessEqual` depth testing and no depth writes.
- The debug overlay is a screen-space triangle list with per-vertex colour, alpha blending, and an `Always` depth compare. It refreshes from the one-second frame sample.
- The depth texture is recreated on resize.
- `Outdated` and `Suboptimal` surfaces reconfigure; `Lost` surfaces are recreated; zero-sized resize dimensions are clamped to `1×1`.

## Current limitations

- No greedy meshing, LOD, or frustum culling.
- No terrain generation or streaming. World management and backups are described in
  [World management](world-management.md).
- No lighting, fog, or transparency.
- No worker threads or async meshing.
- One block palette; no inventory UI.

These are deliberate milestone boundaries. See `docs/roadmap.md` and the scope pages under `docs/dev/` for what replaces them.
