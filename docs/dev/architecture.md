# Architecture

The engine is a small `winit` and `wgpu` application. This page describes the current modules, ownership, and frame flow. Update it when module boundaries or resource ownership change.

## Modules

| Path | Responsibility |
|---|---|
| `src/main.rs` | Thin binary entry point; calls `engine::run()`. |
| `src/lib.rs` | Public entry point, `winit` application handler, input, frame loop, and error aggregation. See [Public API](public-api.md). |
| `src/camera.rs` | Fly-camera state, yaw/pitch math, movement, view-projection, and tests. |
| `src/world.rs` | Chunk and world storage, coordinate conversion, dirty-chunk tracking, and tests. |
| `src/voxel.rs` | Mesh vertices, cube faces, face-culled chunk meshing, and tests. |
| `src/renderer.rs` | `wgpu` device/surface setup, pipeline, buffers, depth texture, resizing, and draw submission. |
| `src/shader.wgsl` | Vertex transform and flat-shaded fragment colour. See [Shader authoring](shaders.md). |

## Ownership

- `App` owns the window handle, renderer, world, camera, input state, and frame timing.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, pipeline, camera uniform, per-chunk vertex buffers and origin uniforms, and depth attachment.
- `Camera` owns only camera state; it does not read the clock or input devices.
- `World` owns the chunk map; every stored chunk is non-empty.
- `voxel::build_chunk_mesh` is a pure function over the world and one chunk.

The renderer holds an `Arc<Window>` and creates a `Surface<'static>` from it. The application also holds a window clone for redraw requests and cursor capture.

## Startup

1. `main` calls `engine::run`.
2. `run` initialises tracing and creates the `winit` event loop.
3. `App::resumed` creates the window, builds the initial world, and creates the renderer.
4. The renderer selects an adapter, requests a device and queue, configures the surface, and builds the pipeline.
5. The renderer meshes and uploads every non-empty chunk once at startup.
6. The application requests the first redraw.

Startup errors are stored on `App`, logged, and returned through `RunError` after the event loop exits. The engine does not panic on adapter, device, surface, or shader validation failures.

## Frame flow

```
RedrawRequested
  -> read frame delta
  -> update camera from held keys
  -> write camera uniform
  -> renderer.render()
       -> acquire surface texture
       -> encode clear + depth clear + draw
       -> submit
       -> present
  -> request next redraw while focused
```

- The loop pauses continuous redraw when the window loses focus or becomes occluded.
- A single frame is still rendered on demand while unfocused, so the window is not left blank.
- Frame time is logged at debug level about once per second.

## Input flow

See [Controls](controls.md) for binding tables, capture rules, and planned changes.

- `Space` toggles the demo voxel on a chunk boundary. Only the dirty chunks are re-meshed and uploaded, and the dirty count, total vertices, and rebuild time are logged.
- `W`, `A`, `S`, and `D` are tracked as pressed physical key codes.
- Mouse look is applied only while the cursor is captured and the window is focused.
- A left click captures the cursor; `Escape` releases it.
- Losing focus clears held keys and releases the cursor.

## Coordinates and data

See [Coordinates](coordinates.md) for the full conventions, including chunk and mesh spaces.

- Right-handed, Y-up.
- A voxel at `(x, y, z)` occupies `[x, x + 1] × [y, y + 1] × [z, z + 1]`.
- The world stores 32³ chunks. Missing chunks are air, and coordinates outside a chunk are never stored.
- Chunk meshes use chunk-local positions and cull faces whose world-space neighbour is solid, including across chunk borders.
- Visible voxel faces are emitted as two non-indexed triangles with position and face normal.
- Chunk voxels are sorted by local coordinate before meshing so output is independent of `HashSet` iteration order.

## Rendering resources

- Surface format comes from `Surface::get_default_config`; the default is typically `Bgra8UnormSrgb`.
- The pipeline uses `Depth32Float`, depth write enabled, and `Less` depth comparison.
- The camera uniform is a 64-byte `mat4x4<f32>` in bind group 0.
- Each non-empty chunk owns a vertex buffer and a 16-byte origin uniform in bind group 1. The vertex shader adds the origin to the chunk-local vertex position.
- Block textures are not implemented. The planned texture array, layer table, and procedural tile
  upload are in [Texture and asset pipeline](textures.md).
- Chunk vertex buffers are created or replaced only when their chunk is dirty. A chunk with an empty mesh releases its buffer.
- The depth texture is recreated on resize.
- `Outdated` and `Suboptimal` surfaces reconfigure; `Lost` surfaces are recreated; zero-sized resize dimensions are clamped to `1×1`.

## Current limitations

- No greedy meshing, LOD, or frustum culling.
- No collision, block interaction, or persistence. The planned world directory, backups, and
  recovery procedure are in [World management](world-management.md).
- No lighting, fog, or transparency.
- No worker threads or async meshing.
- One pipeline and one block colour.

These are deliberate milestone boundaries. See `docs/roadmap.md` and the scope pages under `docs/dev/` for what replaces them.
