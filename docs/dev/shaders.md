# Shader authoring

Valescent Engine renders with `wgpu` and writes its shaders in WGSL. This page covers the current
shader, the interface it shares with the Rust pipeline, authoring conventions, validation, and the
shader changes planned for later milestones.

| Area | Status |
|---|---|
| Shader source and embedding | Current |
| Camera uniform and vertex/normal interface | Current |
| Textured block shader | Current (v0.0.4) |
| Greedy-quad and LOD shader inputs | Current (v0.0.5) |
| Light, AO, fog, and water | Current (v0.0.6) |
| Packed 12-byte vertices, indexed draws, and back-face culling | Current (v0.0.8) |
| Eleven texture layers and Sand | Current (v0.0.7) |
| Shader hot reload | Not implemented |
| Automated WGSL tests | Not implemented; startup validation and manual GPU checks |

## Current shader

The chunk shader is `src/shader.wgsl`; `src/highlight.wgsl` and `src/overlay.wgsl` are separate
modules for the outline and the debug overlay. `create_pipeline` embeds each with
`wgpu::include_wgsl!` and creates the opaque, water, highlight, and overlay pipelines. `include_wgsl!`
embeds the source at build time, but WGSL is parsed and validated when the shader module is
created at startup. Editing a file requires a rebuild and restart; there is no hot reload.

Keep shader source beside the renderer and embed it with `include_wgsl!`. A second independent
pipeline gets its own `src/<name>.wgsl`; small variants can share a module with distinct entry
points. There is no runtime shader loader or asset path.

| Stage | Interface |
|---|---|
| Vertex input | `@location(0) cell_face: vec4<u32>` (chunk-local cell XYZ plus face index), `@location(1) texel: vec4<u32>` (UV X, UV Y, texture layer, reserved), `@location(2) light_ao: u32` |
| Uniforms | `@group(0) @binding(0)` camera, 160 bytes; `@group(1) @binding(0)` chunk `origin: vec3<f32>` |
| Textures | `@group(2) @binding(0)` block `texture_2d_array<f32>`; `@group(2) @binding(1)` nearest `sampler` |
| Vertex output | `@builtin(position) clip_position`, `@location(0) normal`, `@location(1) uv`, `@location(2) flat layer`, `@location(3) light`, `@location(4) fog_distance`, `@location(5) world_position` |
| Fragment output | `@location(0) vec4<f32>` |
| Pipeline state | Triangle list, back-face culling, `Depth32Float`, `Less`, depth write enabled, one color target |

The vertex stage unpacks the cell and face, computes
`world_position = chunk.origin + cell`, then
`clip_position = view_projection * vec4(world_position, 1.0)`, and forwards the normal, UV, flat
layer, unpacked light/AO, fog distance, and world position. The fragment stage samples the array
and applies ambient, sun, moon, and lamp terms plus distance fog. `fs_water` adds animated normals
and alpha `0.65` for the separate transparent pipeline.

The interface, in brief:

```wgsl
struct Camera {
    view_projection: mat4x4<f32>,
    sky_colour: vec4<f32>,
    ambient_colour: vec4<f32>,
    sun_colour: vec4<f32>,
    sun_direction: vec4<f32>,
    // x: unlit flag, y: far plane, z: time of day, w: fog end distance.
    settings: vec4<f32>,
    camera_position: vec4<f32>,
};

struct Chunk {
    origin: vec3<f32>,
    padding: f32,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(1) @binding(0)
var<uniform> chunk: Chunk;

@group(2) @binding(0)
var block_texture: texture_2d_array<f32>;

@group(2) @binding(1)
var block_sampler: sampler;

struct VertexIn {
    @location(0) cell_face: vec4<u32>,
    @location(1) texel: vec4<u32>,
    @location(2) light_ao: u32,
};

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
    @location(3) light: vec3<f32>,
    @location(4) fog_distance: f32,
    @location(5) world_position: vec3<f32>,
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

- The current layout has three groups:
  - group `0`, binding `0`: the 160-byte camera and frame uniform, visible to the vertex and
    fragment stages.
  - group `1`, binding `0`: the 16-byte chunk origin uniform, visible to the vertex stage.
  - group `2`, bindings `0` and `1`: the block texture array and sampler, visible to the fragment
    stage.
- The shader declares `camera`, `chunk`, `block_texture`, and `block_sampler` with the matching
  groups and bindings. Changing either number without the other fails pipeline creation.
- The camera uniform is visible to both stages: the vertex stage reads the view-projection,
  camera position, far plane, and time of day; the fragment stage reads the colours, sun
  direction, unlit flag, and fog end.
- The camera buffer is 160 bytes, matching the seven `vec4`/matrix fields in `Camera`. If the
  struct grows, update the buffer size, `min_binding_size`, and the Rust write together.
- The chunk origin buffer holds `vec3<f32>` plus one padding float, which matches the uniform
  address-space alignment. The padding is not read.
- The texture is a ten-layer `Rgba8UnormSrgb` array with one mip level. The sampler is nearest
  on every filter and repeats in U and V, clamping in W.
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

- `Vertex` is `#[repr(C)]`, derives `Pod` and `Zeroable`, and is 12 bytes: `cell` at offset `0`
  (three `u8` cell coordinates), `face` at `3` (one of six face indices), `texel` at `4`
  (two `u8` UVs), `texture_layer` at `6`, `reserved` at `7`, and `light_ao` at `8`.
- The buffer layout uses `array_stride = size_of::<Vertex>()` and three attributes:
  `Uint8x4` at `0` (cell XYZ plus face), `Uint8x4` at `4` (UV X, UV Y, layer, reserved), and
  `Uint32` at `8` (packed light/AO). Their `shader_location` values are `0`–`2`.
- Every `@location` in `VertexIn` needs exactly one matching `VertexAttribute`, and the WGSL type
  must match the format. `Uint8x4` is a `vec4<u32>`; `Uint32` is a `u32`.
- If you add a field, use `std::mem::offset_of!` for the attribute offset instead of guessing.
  Keep the stride as `size_of::<Vertex>()`; do not hard-code a second size.
- The mesher and the vertex layout are coupled. A layout change updates the mesher, its tests,
  the Rust attribute list, and the WGSL struct in the same change.
- Positions and UVs are whole-block `u8` values in `0..=255`. A greedy quad spans at most a
  chunk, so the packed form covers every vertex the mesher emits.

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
- The current opaque pipeline uses `Depth32Float`, `Less`, depth write enabled, and
  `cull_mode: Some(Face::Back)`. Water uses `LessEqual`, no depth writes, and the same back-face
  culling. The overlay and highlight keep double-sided geometry.

## Coordinates, depth, and color

- The world is right-handed and Y-up. The camera's default forward is `-Z`.
- The projection is right-handed with a `0..1` depth range
  (`glam::camera::rh::proj::directx`). WGSL clip space is `x, y` in `-1..1` and `z` in `0..1`.
  Do not add an OpenGL-style `z` remap.
- Do not flip Y in the shader. The projection and viewport already produce the expected
  orientation.
- Normals are axis-aligned and derived in the shader from the packed face index; there is no
  vertex normal attribute. There is no model transform, so the shader does not need a normal
  matrix. From v0.0.3, chunk meshes are chunk-local and the renderer adds the chunk origin; a
  translation changes positions, not normals. Normalize a normal after interpolation in the
  fragment stage; interpolation shortens non-unit vectors.
- Winding is counter-clockwise from outside the voxel. The terrain and water pipelines cull back
  faces (`cull_mode: Some(Face::Back)`), so keep counter-clockwise as the front face and verify
  the mesher's vertex and index order.
- Shader colors are linear RGB. The surface format from `Surface::get_default_config` is usually
  `Bgra8UnormSrgb`; the target encodes linear values for display. Do not apply gamma manually.
  If a non-sRGB target is introduced, decide where encoding happens instead of adding `pow`
  calls to arbitrary shaders.
- Opaque fragments return alpha `1.0`. Water uses the separate `fs_water` entry point and the
  blended pipeline; do not use `discard` to fake transparency.
- Positions are `f32` metres. The far plane is `(render_distance + 2) * 32 m`; at the default
  distance `12` it is `448 m`. Moving large world offsets into the shader needs a coordinate
  change, not just a wider type.

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

1. Update `Vertex` in `src/v0/voxel.rs`. Keep `#[repr(C)]`, `Pod`, and `Zeroable`.
2. Update the mesher and its tests. The v0.0.3, v0.0.4, and v0.0.6 specs pin the field order and
   byte sizes for their milestones; v0.0.8 changes the live layout without changing those
   historical contracts.
3. Update `VertexBufferLayout` in `src/renderer.rs`: stride, attribute formats, offsets, and
   `shader_location` values.
4. Update `VertexIn` and any interstage struct in `src/shader.wgsl`.
5. If the new attribute is an integer that stays an integer across the stage, such as a texture
   layer, use an integer vertex format and a flat varying. Unpack packed per-corner fields to
   floats in the vertex stage instead.
6. Run `cargo test` for the CPU-side layout and mesh vectors, then `cargo run` for GPU validation.
7. If the layout is normative, update the spec or ADR before changing shipped data. The current
   shader uses the 12-byte packed v0.0.8 vertex; the 40-byte v0.0.6 layout is documented in that
   milestone's spec as history.

## Changing bindings

1. Add the resource declaration to WGSL with its group and binding.
2. Add the matching `BindGroupLayoutEntry`: binding, visibility, type, count, and
   `min_binding_size` for buffers.
3. Add the matching `BindGroupEntry` and the buffer, texture view, or sampler it references.
4. Add a new `BindGroupLayout` to `PipelineLayoutDescriptor` if the resource lives in a new group.
5. Set sampler filtering and address modes in the Rust sampler descriptor, not in WGSL. The shader
   sees a `sampler`, not a filtering mode.
6. Run `cargo run`; an unmatched binding or visibility fails pipeline validation.

The current bind groups are group `0`, binding `0` (160-byte camera uniform, vertex and fragment
visibility), group `1`, binding `0` (chunk origin uniform, vertex visibility), and group `2`,
bindings `0` and `1` (texture array and sampler, fragment visibility). The v0.0.7 layer table is
data, not a binding; adding a layer does not change the interface.

## Shader work by milestone

| Milestone | Change | Shader consequence |
|---|---|---|
| v0.0.3 | Chunk-local vertices and chunk origins | Implemented as a 16-byte origin uniform in group `1`. The vertex stage adds the chunk origin to the position. Normals are unchanged. |
| v0.0.4 | 36-byte vertex with UV and layer, texture array | Implemented. The vertex stage forwards UV and layer; the fragment stage samples `texture_2d_array<f32>` with nearest filtering and no mipmaps. The highlight is a separate line-list pipeline. |
| v0.0.5 | Greedy quads and LOD1 | Implemented. UVs are in block units and the sampler repeats in U and V. LOD1 uses the same shader; the CPU scales positions and UVs, so the shader does not branch on LOD. |
| v0.0.6 | 40-byte light/AO vertex, day/night, fog, water | Implemented: the 40-byte vertex carried packed light/AO, the uniform carries sky/ambient/sun colours, sun direction, far plane, and camera position, the fragment stage applies the lighting and fog formulas, and `fs_water` adds animated normals with alpha `0.65` in a blended, non-writing pass. |
| v0.0.7 | Eleven texture layers and Sand | Implemented. The texture array grew to eleven layers; the layer index is data. |
| v0.0.8 | Packed 12-byte vertices, indices, back-face culling, LOD2 | Implemented. Cells, face, UVs, and layer pack into two `u8x4` words, `light_ao` stays a `u32`, quads emit four vertices and six indices, and the opaque and water pipelines cull back faces. The shader unpacks the face index to a normal and the cell to a position. |

### Textures (v0.0.4)

The vertex is 12 bytes: packed cell and face, packed UV and layer, and the light/AO word. The layer is an integer
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
@group(2) @binding(0) var block_texture: texture_2d_array<f32>;
@group(2) @binding(1) var block_sampler: sampler;

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
- LOD1 and LOD2 use the same vertex and fragment shaders. The CPU scales positions and UVs; the shader does
  not select or branch on LOD.
- Each quad emits four vertices and six indices, so the shader sees one vertex per corner rather
  than one per triangle. Face normals still come from the face index, and the post-transform
  cache can reuse the four shaded vertices.
- The v0.0.5 spec owns merge order and test vectors; shader authoring only needs the UV and layer
  contract. See the [v0.0.5 spec](../specs/v0.0.5.md).

### Light, AO, fog, and water (v0.0.6)

The light/AO word is one packed `u32`; in the current 12-byte vertex it is `@location(2)`:

| Bits | Field |
|---:|---|
| 0-3 | Sky light `0..=15` |
| 4-7 | Block light `0..=15` |
| 8-9 | AO level `0..=3` |
| 10-31 | Zero |

`light_ao` is per-corner data. Unpack it in the vertex stage into an interpolated varying so the
corner values blend across the quad. Do not pass the packed `u32` across the stage boundary as a
flat varying; flat interpolation would collapse the four corner values into one.

```wgsl
struct VertexIn {
    // ... packed cell/face and texel/layer words ...
    @location(2) light_ao: u32,
};

struct VertexOut {
    // ... clip_position, normal, uv, flat layer ...
    @location(3) light: vec3<f32>,   // sky, block, AO
};

@vertex
fn vs_main(input: VertexIn) -> VertexOut {
    var output: VertexOut;
    // ... clip_position, normal, uv, layer ...
    output.light = vec3<f32>(
        f32(input.light_ao & 0xFu) / 15.0,
        f32((input.light_ao >> 4u) & 0xFu) / 15.0,
        f32((input.light_ao >> 8u) & 0x3u),
    );
    return output;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let ao = 0.55 + 0.15 * input.light.z;
    // Sky and block light feed the lighting formula from the spec.
    return vec4<f32>(input.light.x, input.light.y, ao, 1.0);
}
```

The fragment stage applies the lighting formula below; the v0.0.4 face-shading path is kept behind
the unlit flag. The lighting formula, keyframes, and fog range are in the
[v0.0.6 spec](../specs/v0.0.6.md).
[ADR-0011](../decisions/0011-voxel-light-and-baked-ao.md) records why lighting is voxel light
plus baked AO rather than shadow maps.

### Frame uniform (v0.0.6)

`Camera` grows to 160 bytes (`min_binding_size = 160`) and is visible to both stages:

| Field | Size | Use |
|---|---:|---|
| `view_projection` | 64 | `mat4x4<f32>` |
| `sky_colour` | 16 | Fog and clear colour |
| `ambient_colour` | 16 | Ambient term |
| `sun_colour` | 16 | Sun term |
| `sun_direction` | 16 | Sun direction; moon is its negation |
| `settings` | 16 | `x` unlit flag, `y` far plane, `z` time of day, `w` fog end |
| `camera_position` | 16 | Fog distance in the vertex stage |

The shader applies:

```text
sky = light.x, block = light.y, ao = 0.55 + 0.15 * light.z
ambient = ambient_colour * sky
sun = sun_colour * max(dot(n, sun_dir), 0) * max(sun_dir.y, 0) * sky
moon = moon_colour * max(dot(n, -sun_dir), 0) * max(-sun_dir.y, 0) * sky
lamp = lamp_colour * block
lit = albedo * ao * (ambient + sun + moon + lamp)
fog = clamp((distance - 0.7 * fog_end) / (0.3 * fog_end), 0, 1)
colour = clamp(mix(lit, sky_colour, fog), 0, 1)
```

`fog_end` is `render_distance_chunks * 32` metres and is written into `settings.w`; the far plane
stays two chunks beyond it. Bind group 0 must include `FRAGMENT` visibility; the fragment stage
reads the uniform.

Shader-facing consequences:

- Fog needs the camera-to-fragment distance. The vertex stage computes it from the world position
  and `camera_position`; do not reconstruct world distance from `@builtin(position)`.
- The unlit face-shading path is a uniform-selected debug fallback, not a separate pipeline.
- Water uses a separate transparent pipeline: blending `SrcAlpha` / `OneMinusSrcAlpha`, alpha
  `0.65`, depth test `LessEqual`, depth writes off.
- Water normals animate from world position and `time_of_day`. Pass a world-position varying to
  the fragment stage; `@builtin(position)` is screen space.
- Water and opaque meshes share the vertex layout but not the pipeline state. Keep the fragment
  entry points separate.

### Sand and eleven layers (v0.0.7)

Sand uses block ID `8` and texture layer `10`; the array is eleven layers. The shader still
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
| Faces vanish or holes appear at chunk borders | Winding, index order, or front-face mismatch | Keep counter-clockwise winding and verify the mesher's triangle order |
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
