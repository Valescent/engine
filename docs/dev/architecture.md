# Architecture

The engine is a small `winit` and `wgpu` application. This page describes the current modules, ownership, and frame flow. Update it when module boundaries or resource ownership change.

## Modules

| Path | Responsibility |
|---|---|
| `src/main.rs` | Thin binary entry point; calls `engine::run()`. |
| `src/lib.rs` | Public entry point, `winit` application handler, input, frame loop, and error aggregation. |
| `src/camera.rs` | Fly-camera state, yaw/pitch math, movement, view-projection, and tests. |
| `src/voxel.rs` | Voxel mesh vertices, cube faces, mesh generation, and tests. |
| `src/renderer.rs` | `wgpu` device/surface setup, pipeline, buffers, depth texture, resizing, and draw submission. |
| `src/shader.wgsl` | Vertex transform and flat-shaded fragment colour. |

## Ownership

- `App` owns the window handle, renderer, voxel set, camera, input state, and frame timing.
- `Renderer` owns the `wgpu` instance, surface, device, queue, surface configuration, pipeline, uniform buffer, bind group, vertex buffer, and depth attachment.
- `Camera` owns only camera state; it does not read the clock or input devices.
- `voxel::build_mesh` is a pure function over the voxel set.

The renderer holds an `Arc<Window>` and creates a `Surface<'static>` from it. The application also holds a window clone for redraw requests and cursor capture.

## Startup

1. `main` calls `engine::run`.
2. `run` initialises tracing and creates the `winit` event loop.
3. `App::resumed` creates the window, builds the initial voxel mesh, and creates the renderer.
4. The renderer selects an adapter, requests a device and queue, configures the surface, and builds the pipeline and GPU buffers.
5. The application requests the first redraw.

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

- `Space` toggles one demo voxel, rebuilds the mesh, uploads the new vertex buffer, and requests a redraw.
- `W`, `A`, `S`, and `D` are tracked as pressed physical key codes.
- Mouse look is applied only while the cursor is captured and the window is focused.
- A left click captures the cursor; `Escape` releases it.
- Losing focus clears held keys and releases the cursor.

## Coordinates and data

- Right-handed, Y-up.
- A voxel at `(x, y, z)` occupies `[x, x + 1] × [y, y + 1] × [z, z + 1]`.
- Voxel cubes are emitted as 36 non-indexed vertices with position and face normal.
- Voxel coordinates are sorted before meshing so output is independent of `HashSet` iteration order.
- The current world is one global `HashSet<IVec3>` and one vertex buffer. Chunking, face culling, and per-chunk rebuilds arrive in v0.0.3.

## Rendering resources

- Surface format comes from `Surface::get_default_config`; the default is typically `Bgra8UnormSrgb`.
- The pipeline uses `Depth32Float`, depth write enabled, and `Less` depth comparison.
- The camera uniform is a 64-byte `mat4x4<f32>` in bind group 0.
- The vertex buffer is recreated when the voxel set changes.
- The depth texture is recreated on resize.
- `Outdated` and `Suboptimal` surfaces reconfigure; `Lost` surfaces are recreated; zero-sized resize dimensions are clamped to `1×1`.

## Current limitations

- No chunking or face culling.
- No collision, block interaction, or persistence.
- No lighting, fog, or transparency.
- No worker threads or async meshing.
- One pipeline and one block colour.

These are deliberate milestone boundaries. See `docs/roadmap.md` and the scope pages under `docs/dev/` for what replaces them.
