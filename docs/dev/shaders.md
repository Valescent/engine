# Shader authoring

Valescent Engine renders with `wgpu` and writes its shaders in WGSL. This page covers the current
shader, the interface it shares with the Rust pipeline, authoring conventions, validation, and the
shader changes planned for later milestones.

| Area | Status |
|---|---|
| Shader source and embedding | Current |
| Camera uniform and vertex/normal interface | Current |
| Textured block shader | Planned (v0.0.4) |
| Greedy-quad and LOD shader inputs | Planned (v0.0.5) |
| Light, AO, fog, and water | Planned (v0.0.6) |
| Eleven texture layers and Sand | Planned (v0.0.7) |
| Shader hot reload | Not implemented |
| Automated WGSL tests | Not implemented; startup validation and manual GPU checks |

## Current shader

The only shader is `src/shader.wgsl`. `create_pipeline` embeds it with
`wgpu::include_wgsl!("shader.wgsl")` and creates one render pipeline with the `vs_main` and
`fs_main` entry points. `include_wgsl!` embeds the source at build time, but WGSL is parsed and
validated when the shader module is created at startup. Editing the file requires a rebuild and
restart; there is no hot reload.

Keep shader source beside the renderer and embed it with `include_wgsl!`. A second independent
pipeline gets its own `src/<name>.wgsl`; small variants can share a module with distinct entry
points. There is no runtime shader loader or asset path.

| Stage | Interface |
|---|---|
| Vertex input | `@location(0) position: vec3<f32>`, `@location(1) normal: vec3<f32>` |
| Uniform | `@group(0) @binding(0) camera.view_projection: mat4x4<f32>` |
| Vertex output | `@builtin(position) clip_position`, `@location(0) normal` |
| Fragment output | `@location(0) vec4<f32>` |
| Pipeline state | Triangle list, no culling, `Depth32Float`, `Less`, depth write enabled, one color target |

The vertex stage computes `clip_position = view_projection * vec4(position, 1.0)` and forwards the
normal. The fragment stage uses a fixed light direction `(0.35, 0.85, 0.45)`, a `0.45..1.0` shade,
and a fixed green base color. There are no textures, lighting uniforms, fog, or transparency in
the current shader.

The interface, in brief:

```wgsl
struct Camera {
    view_projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
};
```

## The Rust/WGSL contract

WGSL and Rust are linked by numbers and entry-point names, not by a shared header. Four things
must match:

| WGSL | Rust | Failure when they differ |
|---|---|---|
| `@group` and `@binding` | `BindGroupLayoutEntry` and `BindGroupEntry` | Pipeline validation error or a missing resource |
| `@location` | `VertexAttribute.shader_location` | Vertex input cannot be matched |
| Vertex field type | `VertexAttribute.format` and offset | Wrong bytes read, or validation failure |
| Entry point name | `VertexState::entry_point` or `FragmentState::entry_point` | Entry point not found |

### Bindings

- The current layout has one entry: group `0`, binding `0`, `ShaderStages::VERTEX`, uniform,
  `min_binding_size` 64.
- The shader declares `var<uniform> camera: Camera` with the same group and binding. Changing
  either number without the other fails pipeline creation.
- The uniform is visible to the vertex stage only. If a fragment function reads it, add
  `ShaderStages::FRAGMENT` to the layout entry.
- The buffer is 64 bytes, matching one `mat4x4<f32>`. If the struct grows, update the buffer size,
  `min_binding_size`, and the Rust write together.
- WGSL matrices are column-major. `Mat4::to_cols_array()` writes them column by column, so
  `view_projection * vec4(position, 1.0)` is correct. Do not transpose the matrix on the CPU or in
  the shader.

### Uniform layout

WGSL uniform structs do not use C packing. The rules that matter for engine uniforms:

| Type | Size | Alignment |
|---|---:|---:|
| `f32` | 4 | 4 |
| `vec2<f32>` | 8 | 8 |
| `vec3<f32>` | 12 | 16 |
| `vec4<f32>` | 16 | 16 |
| `mat4x4<f32>` | 64 | 16 |

A struct's alignment is its largest member alignment, and its size is rounded up to that
alignment. For a uniform buffer, keep the struct size a multiple of 16 bytes. Add explicit padding
fields; do not assume the Rust `#[repr(C)]` size matches the WGSL size. Check `size_of`,
`offset_of!`, and the WGSL struct together. The current camera struct is one matrix, so it has no
padding.

### Device features and limits

- The renderer requests `wgpu::Features::empty()` and downlevel WebGL2 limits resolved against
  the adapter's limits. Stay within baseline WGSL and those limits.
- An optional shader feature, such as `f16`, requires adding the device feature and handling
  adapters that do not support it. Do not use it speculatively.
- A new binding or uniform that exceeds a limit fails device or pipeline validation, not WGSL
  parsing. Check the limits before growing the interface.

### Vertex inputs

- `Vertex` is `#[repr(C)]`, derives `Pod` and `Zeroable`, and is 24 bytes: `position` at offset
  `0`, `normal` at offset `12`.
- The buffer layout uses `array_stride = size_of::<Vertex>()` and `Float32x3` attributes at those
  offsets. `shader_location` `0` and `1` are the `@location` numbers in `VertexIn`.
- Every `@location` in `VertexIn` needs exactly one matching `VertexAttribute`, and the WGSL type
  must match the format. `Float32x3` is a `vec3<f32>`; `Uint32` is a `u32`.
- If you add a field, use `std::mem::offset_of!` for the attribute offset instead of guessing.
  Keep the stride as `size_of::<Vertex>()`; do not hard-code a second size.
- The mesher and the vertex layout are coupled. A layout change updates the mesher, its tests,
  the Rust attribute list, and the WGSL struct in the same change.

### Entry points

- `VertexState::entry_point` and `FragmentState::entry_point` are strings. A typo fails pipeline
  creation, not WGSL parsing.
- The single-pipeline module uses `vs_main` and `fs_main`. If one module serves more than one
  pipeline, use distinct, descriptive entry points so the pipeline descriptor makes the choice
  obvious.
- Unused entry points are still validated. Do not leave a broken debug entry point in the file.

### Render target and depth

- The fragment output `@location(0)` matches the single color target in `FragmentState::targets`.
- The color target format comes from the surface configuration, not the shader.
- Depth testing is pipeline state. The shader writes clip-space `@builtin(position)`; it does not
  choose the depth comparison or whether depth is written.
- The current pipeline uses `Depth32Float`, `Less`, and depth write enabled.

## Coordinates, depth, and color

- The world is right-handed and Y-up. The camera's default forward is `-Z`.
- The projection is right-handed with a `0..1` depth range
  (`glam::camera::rh::proj::directx`). WGSL clip space is `x, y` in `-1..1` and `z` in `0..1`.
  Do not add an OpenGL-style `z` remap.
- Do not flip Y in the shader. The projection and viewport already produce the expected
  orientation.
- Normals are world-space unit axis vectors in the current mesh. There is no model transform, so
  the shader does not need a normal matrix. From v0.0.3, chunk meshes are chunk-local and the
  renderer adds the chunk origin; a translation changes positions, not normals. Normalize a normal
  after interpolation in the fragment stage; interpolation shortens non-unit vectors.
- Winding is counter-clockwise from outside the voxel. The current pipeline uses
  `PrimitiveState::default()`, whose cull mode is `None`. If backface culling is enabled, keep
  counter-clockwise as the front face and verify the mesher's vertex order.
- Shader colors are linear RGB. The surface format from `Surface::get_default_config` is usually
  `Bgra8UnormSrgb`; the target encodes linear values for display. Do not apply gamma manually.
  If a non-sRGB target is introduced, decide where encoding happens instead of adding `pow`
  calls to arbitrary shaders.
- Opaque fragments return alpha `1.0`. The planned water pipeline uses blending; do not use
  `discard` to fake transparency.
- Positions are `f32` metres. The current far plane is `100 m`. Moving large world offsets into
  the shader needs a coordinate change, not just a wider type.

## WGSL style

- `PascalCase` for structs, `snake_case` for functions and variables, and
  `SCREAMING_SNAKE_CASE` for constants.
- Declare the uniform struct and its binding first, then the vertex and interstage structs, then
  functions, matching the current file.
- Prefer `let`. Use `var` only when a value is mutated, such as building a `VertexOut` field by
  field.
- Use explicit `f32` literals (`0.0`, `1.0`) and keep integer and floating-point math separate.
- Comment units, spaces, and non-obvious formulas. Do not leave commented-out code.
- Keep entry points small. Factor reusable math into helper functions.
- Use `@interpolate(flat)` on integer varyings such as a texture layer, and on the matching
  fragment input when it is a separate struct. Do not pass packed light/AO across the stage
  boundary as a flat integer; unpack it to floats in the vertex stage so the per-corner values
  interpolate.
- Do not call `textureSample` in non-uniform control flow. Use `textureSampleLevel` or
  `textureLoad` when derivatives are unavailable or the sample must not depend on neighbouring
  fragments.
- Do not add per-fragment loops or heavy math without measuring. A shader change is a rendering
  change; validate it on the target GPU.
- Do not create a second source of truth for the vertex layout. `Vertex`, the `VertexBufferLayout`,
  and the WGSL struct must describe the same bytes.

## Changing the vertex layout

1. Update `Vertex` in `src/voxel.rs`. Keep `#[repr(C)]`, `Pod`, and `Zeroable`.
2. Update the mesher and its tests. The v0.0.3, v0.0.4, and v0.0.6 specs pin the field order and
   byte sizes for their milestones.
3. Update `VertexBufferLayout` in `src/renderer.rs`: stride, attribute formats, offsets, and
   `shader_location` values.
4. Update `VertexIn` and any interstage struct in `src/shader.wgsl`.
5. If the new attribute is an integer that stays an integer across the stage, such as a texture
   layer, use an integer vertex format and a flat varying. Unpack packed per-corner fields to
   floats in the vertex stage instead.
6. Run `cargo test` for the CPU-side layout and mesh vectors, then `cargo run` for GPU validation.
7. If the layout is normative, update the spec or ADR before changing shipped data. The current
   shader is v0.0.2; the 36-byte and 40-byte layouts are planned contracts, not current code.

## Changing bindings

1. Add the resource declaration to WGSL with its group and binding.
2. Add the matching `BindGroupLayoutEntry`: binding, visibility, type, count, and
   `min_binding_size` for buffers.
3. Add the matching `BindGroupEntry` and the buffer, texture view, or sampler it references.
4. Add a new `BindGroupLayout` to `PipelineLayoutDescriptor` if the resource lives in a new group.
5. Set sampler filtering and address modes in the Rust sampler descriptor, not in WGSL. The shader
   sees a `sampler`, not a filtering mode.
6. Run `cargo run`; an unmatched binding or visibility fails pipeline validation.

The current bind group is group `0`, binding `0`, camera uniform, vertex visibility. The v0.0.4
texture array and sampler are planned; the spec fixes the texture array and per-vertex layer, not
the binding numbers. Choose numbers that match the Rust layout and document them in the change.

## Planned shader work

| Milestone | Change | Shader consequence |
|---|---|---|
| v0.0.3 | Chunk-local vertices and chunk origins | The vertex stage adds the chunk origin to the position. The mechanism is an implementation detail; a uniform or an instance attribute with `VertexStepMode::Instance` both work. Normals are unchanged. |
| v0.0.4 | 36-byte vertex with UV and layer, texture array | The vertex stage forwards UV and layer. The fragment stage samples `texture_2d_array<f32>` with nearest filtering and no mipmaps. The highlight is a separate line-list pipeline. |
| v0.0.5 | Greedy quads and LOD1 | UVs are in block units and the sampler repeats in U and V. LOD1 uses the same shader; the CPU scales positions and UVs, so the shader does not branch on LOD. |
| v0.0.6 | 40-byte light/AO vertex, day/night, fog, water | The vertex stage unpacks sky light, block light, and AO into separate float varyings; the fragment stage applies the spec's lighting and fog formulas and uses a separate transparent water pipeline with animated normals. |
| v0.0.7 | Eleven texture layers and Sand | The texture array grows; the layer index is data. The lighting and water shader logic do not change. |

### Textures (v0.0.4)

The textured vertex is 36 bytes: `position`, `normal`, `uv`, and `layer`. The layer is an integer
varying and must be flat. The fragment shader samples the array with the per-vertex layer. The
layer table, procedural tile generation, and upload path are in
[Texture and asset pipeline](textures.md).

```wgsl
struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
};

// Illustrative binding numbers. Match them to the Rust bind group layout.
@group(0) @binding(1) var block_texture: texture_2d_array<f32>;
@group(0) @binding(2) var block_sampler: sampler;

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let albedo = textureSample(
        block_texture,
        block_sampler,
        input.uv,
        i32(input.layer),
    );
    return vec4<f32>(albedo.rgb, 1.0);
}
```

The texture array and layer mapping are normative in the
[v0.0.4 spec](../specs/v0.0.4.md); the array replaces a flat atlas by
[ADR-0007](../decisions/0007-texture-array-for-block-textures.md). The binding numbers above are
illustrative because the spec does not fix them. The sampler uses nearest filtering, no mipmaps,
and repeat addressing in U and V from v0.0.5. Do not add atlas coordinate math: the array layer
isolates each tile. All four vertices of a quad must carry the same layer; flat interpolation takes
one value, so a mismatch is a mesher bug.

The highlight is a separate line-list pipeline. The spec leaves its colour and z-fighting offset
implementation-defined; keep that logic out of the opaque fragment path.

### Greedy quads and LOD (v0.0.5)

- UVs are in block units and can exceed `1.0`. The sampler repeats the tile in U and V; the shader
  must not clamp or wrap them.
- A greedy quad can cover many blocks. Do not assume UVs stay in `0..1`; a quad can span many
  tiles.
- LOD1 uses the same vertex and fragment shaders. The CPU scales positions and UVs; the shader does
  not select or branch on LOD.
- The v0.0.5 spec owns merge order and test vectors; shader authoring only needs the UV and layer
  contract. See the [v0.0.5 spec](../specs/v0.0.5.md).

### Light, AO, fog, and water (v0.0.6)

The v0.0.6 vertex is 40 bytes and adds one packed `light_ao` field:

| Bits | Field |
|---:|---|
| 0-3 | Sky light `0..=15` |
| 4-7 | Block light `0..=15` |
| 8-9 | AO level `0..=3` |
| 10-31 | Zero |

`light_ao` is per-corner data. Unpack it in the vertex stage into separate `f32` varyings so the
corner values interpolate across the quad. Do not pass the packed `u32` across the stage boundary
as a flat varying; flat interpolation would collapse the four corner values into one.

```wgsl
struct VertexIn {
    // ... position, normal, uv, layer ...
    @location(4) light_ao: u32,
};

struct VertexOut {
    // ... clip_position, normal, uv, flat layer ...
    @location(3) sky_light: f32,
    @location(4) block_light: f32,
    @location(5) ao_level: f32,
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var output: VertexOut;
    // ... clip_position, normal, uv, layer ...
    output.sky_light = f32(input.light_ao & 0xFu);
    output.block_light = f32((input.light_ao >> 4u) & 0xFu);
    output.ao_level = f32((input.light_ao >> 8u) & 0x3u);
    return output;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let sky = input.sky_light / 15.0;
    let block = input.block_light / 15.0;
    let ao = 0.55 + 0.15 * input.ao_level;
    // Apply the lighting and fog formulas from the spec.
    return vec4<f32>(sky, block, ao, 1.0);
}
```

The lighting formula, keyframes, fog range, and water animation are in the
[v0.0.6 spec](../specs/v0.0.6.md). [ADR-0011](../decisions/0011-voxel-light-and-baked-ao.md)
records why lighting is voxel light plus baked AO rather than shadow maps.

Shader-facing consequences:

- The day/night state needs uniform data for time, sun/moon direction and colours, ambient, and
  sky colour. The spec fixes the formulas and keyframes, not the uniform packing; define one
  struct, mirror it in Rust with explicit padding, and keep the sizes in sync.
- Fog needs the camera-to-fragment distance. Add the camera position or a suitable view-space
  value to the uniform; do not reconstruct world distance from `@builtin(position)`.
- The unlit face-shading path is a uniform-selected debug fallback, not a separate pipeline.
- Water uses a separate transparent pipeline: blending `SrcAlpha` / `OneMinusSrcAlpha`, alpha
  `0.65`, depth test `LessEqual`, depth writes off.
- Water normals animate from world position and `time_of_day`. Pass a world-position varying to
  the fragment stage; `@builtin(position)` is screen space.
- Water and opaque meshes share the vertex layout but not the pipeline state. Keep the fragment
  entry points separate.

### Sand and eleven layers (v0.0.7)

Sand uses block ID `8` and texture layer `10`; the array grows to eleven layers. The shader still
selects a layer from the vertex data, so no lighting or UV logic changes. See the
[v0.0.7 spec](../specs/v0.0.7.md).

## Validate

- `cargo run` is the authoritative check. It creates the shader module and pipeline inside a
  `wgpu::ErrorFilter::Validation` scope in `create_pipeline`.
- A WGSL parse error, an unmatched binding, a mismatched vertex layout, or a bad entry point
  becomes `RendererError::Validation`. The app logs `engine startup failed` and exits; it does not
  continue with a silently broken shader.
- `cargo test` and `cargo clippy` do not compile WGSL. CPU tests cover mesh and layout data, not
  shader compilation.
- `include_wgsl!` embeds the file at build time, but does not validate it at build time. A
  successful `cargo build` can still produce a shader that fails at startup.
- There is no hot reload and no WGSL formatter or linter in CI. Rebuild and restart after editing.
- For validation detail, run with `RUST_LOG=engine=debug,wgpu=warn`.
- Manual GPU acceptance: run the scene, resize the window, move the camera, and confirm there are
  no validation errors or missing faces. See [Testing](testing.md#manual-gpu-acceptance).
- For pipeline and shader inspection, use RenderDoc, Nsight, or PIX. They are external frame
  debuggers, not replacements for the startup validation path. See
  [Debugging](debugging.md#external-tools).

## Debug

WGSL has no portable print statement. To inspect a value, write it to the fragment colour
temporarily or use RenderDoc, then remove the debug output before committing.

| Symptom | Likely cause | Fix |
|---|---|---|
| Startup fails with a validation error | WGSL syntax error, unmatched binding, wrong entry point, or vertex layout mismatch | Read the `RendererError::Validation` text; check the shader line and the corresponding Rust declaration |
| Window opens but the scene is black | No vertices, wrong camera matrix, depth test rejecting fragments, or wrong clear color | Check the vertex count and camera uniform; disable depth writes temporarily only as a diagnostic |
| Entry point not found | Rust string and WGSL function name differ | Keep `vs_main` and `fs_main` in sync with `create_render_pipeline` |
| Binding not found or visibility error | Shader declares a group/binding not in the layout, or the stage is not visible | Add the layout entry or change `ShaderStages` |
| Garbage vertex data | Attribute format, offset, stride, or location mismatch | Compare `Vertex`, `VertexBufferLayout`, and `VertexIn` field by field |
| Integer varying validation error | Integer varying without flat interpolation | Add `@interpolate(flat)` in both the vertex output and fragment input |
| Faces vanish when culling is enabled | Winding or front-face mismatch | Keep counter-clockwise winding and verify the mesher's triangle order |
| Colors look washed out or too dark | Manual gamma applied on an sRGB target, or normals/light direction wrong | Write linear colors; let the sRGB target encode; check the normal and light direction |
| Depth flicker or z-fighting | Clip-space z outside `0..1`, near/far mismatch, or coplanar surfaces | Check the projection and depth state; do not clamp z in the shader |
| `textureSample` validation error | Called from a vertex stage or non-uniform control flow | Sample in the fragment stage, or use `textureSampleLevel` / `textureLoad` |

## Review checklist

- The WGSL entry points match `VertexState::entry_point` and `FragmentState::entry_point`.
- Every `@group`/`@binding` has a matching layout entry and visibility.
- Every `@location` has a matching vertex attribute with the right format and offset.
- The vertex stride and field offsets match `Vertex`; layout changes updated the mesher and tests.
- Clip space, depth, winding, and color-space rules above are respected.
- The shader has no unused bindings or dead entry points.
- `cargo run` succeeds on at least one backend with no validation errors.
- Resizing and camera movement do not expose holes, clipping, or depth artifacts.
- Planned changes cite the owning spec or ADR.

## References

- [ADR-0002: wgpu for graphics](../decisions/0002-wgpu-for-graphics.md)
- [ADR-0005: bytemuck for GPU data layout](../decisions/0005-bytemuck-for-gpu-layout.md)
- [ADR-0007: texture array for block textures](../decisions/0007-texture-array-for-block-textures.md)
- [ADR-0011: voxel lighting with baked AO](../decisions/0011-voxel-light-and-baked-ao.md)
- [v0.0.3 spec](../specs/v0.0.3.md)
- [v0.0.4 spec](../specs/v0.0.4.md)
- [v0.0.5 spec](../specs/v0.0.5.md)
- [v0.0.6 spec](../specs/v0.0.6.md)
- [v0.0.7 spec](../specs/v0.0.7.md)
- [Coordinates](coordinates.md)
- [Architecture](architecture.md)
- [Texture and asset pipeline](textures.md)
- [Testing](testing.md)
- [Debugging](debugging.md)
