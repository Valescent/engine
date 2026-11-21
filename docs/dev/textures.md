# Texture and asset pipeline

The engine has no game texture assets yet. v0.0.4 introduced one `texture_2d_array` of
procedurally generated 16x16 RGBA8 block tiles; there is still no image asset pipeline. This page
covers the tile model, layer table, generation and upload path, sampler, and the rules for adding
tiles.

| Area | Status |
|---|---|
| Game texture files and game asset folder | None |
| `docs/assets/` documentation assets | Current, not game textures |
| Procedural block tiles | Current (v0.0.4) |
| Texture array, sampler, and UV/layer vertex | Current (v0.0.4) |
| Eleven layers now | Current |
| External asset pipeline and texture packs | Deferred; needs a separate decision |
| Mipmaps, anisotropy, and compression | Not used; deferred |
| Automated GPU texture tests | Not implemented; CPU mapping tests and manual GPU checks |

## Current state

- `src/shader.wgsl` samples the `texture_2d_array` with the per-vertex UV and layer.
- `src/v0/voxel.rs` defines `Vertex { cell, face, texel, texture_layer, reserved, light_ao }`, 12 bytes.
- `src/renderer.rs` has three bind groups: group 0 camera and frame uniform, group 1 per-chunk
  origin, and group 2 texture array plus sampler.
- The eleven tiles are generated procedurally in `src/renderer.rs`; there is no image decoder or
  texture asset crate.
- `docs/assets/` holds documentation-site files such as `custom.js` and `voxel-fallback.svg`. It is
  not a game texture directory.
- `world/` holds runtime chunk saves and is ignored by Git. `book/` is mdBook build output.
- There is no game `assets/` folder, texture pack, mipmap chain, anisotropic sampler, or runtime
  image loader.

## Pipeline at a glance

```text
spec layer table
    -> Block::layers / FaceLayers
    -> deterministic 16x16 RGBA8 tile bytes, one layer per tile
    -> one texture_2d_array, one mip level
    -> one bind group with the array view and sampler
    -> WGSL textureSample(array, sampler, uv, layer)
```

One array and one sampler serve every block face in every chunk draw. The layer index selects the
tile; there is no per-block binding and no flat atlas.

## Tile model

- Every tile is 16x16 RGBA8: 1024 bytes per layer.
- All layers share one size and one format.
- One mip level. Mipmaps are not generated.
- One layer per tile. A block face selects a layer, not a sub-rectangle of a shared image.
- Layer indices are explicit constants owned by the milestone specs. They are not derived from
  `Block` enum order or hash-map order.
- Layer indices are append-only. Later milestones add layers; they do not renumber existing ones.
- Air has no layers. Every solid block has a `FaceLayers` value. Water also has layers even though
  it is non-solid in v0.0.6.

### Layer table

| Layer | Tile | Milestone |
|---:|---|---|
| 0 | stone | v0.0.4 |
| 1 | dirt | v0.0.4 |
| 2 | grass top | v0.0.4 |
| 3 | grass side | v0.0.4 |
| 4 | grass bottom | v0.0.4 |
| 5 | wood top and bottom | v0.0.4 |
| 6 | wood side | v0.0.4 |
| 7 | leaves | v0.0.4 |
| 8 | lamp | v0.0.6 |
| 9 | water | v0.0.6 |
| 10 | sand | v0.0.7 |

The milestone specs own this table. This page reproduces it because the mapping is the part most
likely to drift during implementation.

### Block-face mapping

| Block | Top | Side | Bottom | Milestone |
|---|---:|---:|---:|---|
| Stone | 0 | 0 | 0 | v0.0.4 |
| Dirt | 1 | 1 | 1 | v0.0.4 |
| Grass | 2 | 3 | 4 | v0.0.4 |
| Wood | 5 | 6 | 5 | v0.0.4 |
| Leaves | 7 | 7 | 7 | v0.0.4 |
| Lamp | 8 | 8 | 8 | v0.0.6 |
| Water | 9 | 9 | 9 | v0.0.6 |
| Sand | 10 | 10 | 10 | v0.0.7 |
| Air | none | none | none | all |

The v0.0.4 `Block::layers` contract returns `Option<FaceLayers>` with `top`, `side`, and `bottom`
layers. Stone, Dirt, Leaves, Lamp, Water, and Sand repeat one layer on every face; Grass and Wood
need distinct top, side, or bottom values.

Block IDs are also stable on disk: Air `0`, Stone `1`, Dirt `2`, Grass `3`, Wood `4`, Leaves `5`,
Lamp `6`, Water `7`, and Sand `8`. Adding a block ID is a save-format change; follow
[ADR-0008](../decisions/0008-chunk-save-format.md) and the owning milestone spec.

## Source of truth and code layout

| Fact | Owner |
|---|---|
| Layer table, block IDs, and face mapping | [v0.0.4](../specs/v0.0.4.md), [v0.0.6](../specs/v0.0.6.md), and [v0.0.7](../specs/v0.0.7.md) specs |
| Texture-array storage model and sampler intent | [ADR-0007](../decisions/0007-texture-array-for-block-textures.md) |
| Save-format block IDs | [ADR-0008](../decisions/0008-chunk-save-format.md) |
| Shader sampling and WGSL conventions | [Shader authoring](shaders.md) |
| Runtime chunk saves | [v0.0.4 spec](../specs/v0.0.4.md) and [ADR-0008](../decisions/0008-chunk-save-format.md) |

Current code layout:

- `src/block.rs`: `Block`, `FaceLayers`, `Block::layers`, and the stable layer mapping.
- `src/v0/voxel.rs`: the texture vertex and the highlight mesh; `src/v0/mesh.rs` owns greedy meshing.
- `src/renderer.rs`: texture array creation, upload, sampler, bind group, and draw path.
- `src/shader.wgsl`: `texture_2d_array<f32>` and `sampler` sampling.

The v0.0.4 scope lists `src/block.rs`, `src/v0/voxel.rs`, and `src/renderer.rs`. It does not
prescribe a separate texture module. Add one only if procedural generation outgrows those files;
do not create a placeholder module.

## Procedural placeholder tiles

The v0.0.4 placeholder tiles are generated in code, not loaded from files. Exact pixels and
colours are not normative.

- Generation must be deterministic: the same layer and inputs produce the same bytes on every
  run.
- Do not use an unseeded RNG, wall-clock time, file I/O, or network input.
- Generate exactly 16x16 RGBA8, alpha `255` for opaque tiles. The water tile can stay opaque;
  water transparency comes from the pipeline blend state, not the tile alpha.
- Generate layer-major data: layer `0` bytes, then layer `1`, up to `TEXTURE_LAYERS - 1`.
- Keep the generator small and testable. A fixed seed derived from the layer index is enough; the
  visual pattern is not a contract.

```rust
const TEXTURE_SIZE: u32 = 16;
const TEXTURE_LAYERS: u32 = 8; // 10 in v0.0.6, 11 in v0.0.7
const BYTES_PER_TILE: usize = (TEXTURE_SIZE * TEXTURE_SIZE * 4) as usize;

// generate_tile(layer) returns BYTES_PER_TILE deterministic RGBA8 bytes.
let mut pixels = Vec::with_capacity(BYTES_PER_TILE * TEXTURE_LAYERS as usize);
for layer in 0..TEXTURE_LAYERS {
    pixels.extend_from_slice(&generate_tile(layer));
}
```

`TEXTURE_LAYERS` is `8` in v0.0.4, `10` in v0.0.6, and `11` in v0.0.7. Do not generate an empty
array or an array shorter than the table; an out-of-range layer index is a bug, not a fallback.

### Colour space and format

The specs say RGBA8 but do not choose between `wgpu::TextureFormat::Rgba8Unorm` and
`wgpu::TextureFormat::Rgba8UnormSrgb`. Decide once and keep it consistent:

- `Rgba8UnormSrgb` treats the stored bytes as sRGB-encoded and returns linear values to the
  shader. Use it when the generator writes display-referred sRGB bytes.
- `Rgba8Unorm` treats the stored bytes as linear. Use it when the generator writes linear bytes.
- Do not generate sRGB bytes into a `Rgba8Unorm` texture, or linear bytes into an sRGB texture,
  and expect the same result. The surface is usually `Bgra8UnormSrgb`, so the shader should work in
  linear colour and let the target encode.

Record the chosen format in the implementation or an ADR if it becomes a contract. This is an
implementation choice, not something to vary per tile.

## Upload and binding

The v0.0.4 target is one `texture_2d_array`, one view, one sampler, and one bind group entry set for
all chunk draws. The snippet below was checked against `wgpu` 30; it is the target shape, not
current code. It uses `Rgba8UnormSrgb`; substitute `Rgba8Unorm` if the generator writes linear
bytes.

```rust
let texture = device.create_texture(&wgpu::TextureDescriptor {
    label: Some("block texture array"),
    size: wgpu::Extent3d {
        width: TEXTURE_SIZE,
        height: TEXTURE_SIZE,
        depth_or_array_layers: TEXTURE_LAYERS,
    },
    mip_level_count: 1,
    sample_count: 1,
    dimension: wgpu::TextureDimension::D2,
    format: wgpu::TextureFormat::Rgba8UnormSrgb,
    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    view_formats: &[],
});

queue.write_texture(
    wgpu::TexelCopyTextureInfo {
        texture: &texture,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
        aspect: wgpu::TextureAspect::All,
    },
    &pixels,
    wgpu::TexelCopyBufferLayout {
        offset: 0,
        bytes_per_row: Some(TEXTURE_SIZE * 4),
        rows_per_image: Some(TEXTURE_SIZE),
    },
    wgpu::Extent3d {
        width: TEXTURE_SIZE,
        height: TEXTURE_SIZE,
        depth_or_array_layers: TEXTURE_LAYERS,
    },
);

let texture_view = texture.create_view(&wgpu::TextureViewDescriptor {
    dimension: Some(wgpu::TextureViewDimension::D2Array),
    ..Default::default()
});

let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
    label: Some("block sampler"),
    address_mode_u: wgpu::AddressMode::Repeat,
    address_mode_v: wgpu::AddressMode::Repeat,
    address_mode_w: wgpu::AddressMode::ClampToEdge,
    mag_filter: wgpu::FilterMode::Nearest,
    min_filter: wgpu::FilterMode::Nearest,
    mipmap_filter: wgpu::MipmapFilterMode::Nearest,
    lod_min_clamp: 0.0,
    lod_max_clamp: 0.0,
    compare: None,
    anisotropy_clamp: 1,
    border_color: None,
});
```

- `bytes_per_row` is 64 for a 16-pixel RGBA8 row. `rows_per_image` is 16. `pixels` is layer-major.
- `Queue::write_texture` accepts tightly packed rows and stages the alignment internally. If you
  replace it with a manual buffer plus `copy_buffer_to_texture`, the buffer row pitch must be a
  multiple of `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT` (256), so pad each row.
- The sampler uses nearest filtering, no mipmaps, and repeat addressing in U and V. `address_mode_w`
  is unused by a 2D array; `ClampToEdge` keeps the intent explicit. `lod_max_clamp: 0.0` and
  `anisotropy_clamp: 1` prevent mip and anisotropic sampling.
- Create the texture and sampler inside the renderer's validation error path, or push a
  `wgpu::ErrorFilter::Validation` scope around them. A wrong size, format, usage, or bind group
  must fail startup with a typed error, not a silently black texture.
- Check `TEXTURE_LAYERS <= device.limits().max_texture_array_layers` before creating the texture.
  The eleven current layers are far below the baseline 256, but the check keeps the failure clear
  if the set grows.

### Bind group

| Binding | Visibility | Type | Resource |
|---:|---|---|---|
| group 0, binding 0 | Vertex and fragment | Uniform buffer, `min_binding_size` 160 | Camera and frame settings |
| group 2, binding 0 | Fragment | Texture, `D2Array`, `Float { filterable: true }` | Block texture array |
| group 2, binding 1 | Fragment | Sampler, `Filtering` | Block sampler |

Group 1 holds the per-chunk origin uniform. The group and binding numbers are fixed by the Rust
layout; keep them in sync with the WGSL declarations. One array and one sampler serve every chunk
draw; do not create a bind group per block type.

## Vertex and shader contract

- The vertex is 12 bytes: packed cell and face, packed UV and layer, and the light/AO word. Light/AO is described in [Shader authoring](shaders.md#light-ao-fog-and-water-v006).
- Greedy quad UVs are in block units, so a merged face can span several tiles; the repeat sampler tiles the texture within the array layer.
- The layer is a `u32` unpacked from the second vertex word and a flat interstage value. All four vertices of a face or
  greedy quad carry the same layer.
- WGSL declares `texture_2d_array<f32>` and `sampler` and samples with
  `textureSample(array, sampler, uv, i32(layer))`.
- Do not add atlas coordinate math. The array layer isolates the tile; the sampler repeats within
  it.

See [Shader authoring](shaders.md#textures-v004) for the shader-side interface and the flat
interpolation rule.

## Adding a block or tile

1. Update the owning spec with the block ID, layer index, face mapping, and any save-format
   version change. Block IDs are on disk; follow [ADR-0008](../decisions/0008-chunk-save-format.md).
2. Append the new layer after the existing layers. Never renumber, reorder, or reuse a layer index.
3. Add the `Block` variant with its stable ID. Update `is_solid`, `layers`, and any hotbar or
   selection mapping the block participates in.
4. Add deterministic tile generation for the new layer.
5. Increase `TEXTURE_LAYERS` and the array depth. Check the device limit.
6. Teach the mesher to assign the layer to every face vertex. Update the face mapping and mesh
   tests.
7. Add CPU tests for the block/layer mapping and vertex layout. Run manual GPU acceptance on all
   six faces.
8. Update the spec, glossary, and this page if the workflow changes.

## Changing a tile

- Placeholder pixels and colours are not normative. Regenerate the tile without changing its layer
  index.
- If a tile's appearance becomes a contract, record it in the owning spec or a new asset decision.
  Do not leave appearance rules only in generator code.
- Do not renumber existing layers. A changed layer index silently changes every block that uses it.
- Texture layer indices are not stored in save files; saves store block IDs. Changing a tile does
  not require a save-format bump unless a block ID changes.
- Re-run the visual checks: all faces of the affected block, nearest filtering, no bleeding, and
  no missing layer.

## External assets and texture packs

An external asset pipeline, texture-pack format, mipmaps, anisotropy, and compression are deferred
by [ADR-0007](../decisions/0007-texture-array-for-block-textures.md). There is no runtime image
loader and no game `assets/` folder today.

If an external pipeline is introduced, it needs its own ADR and must:

- produce one `texture_2d_array` with one layer per tile;
- preserve the layer table, layer order, and block-face mapping;
- keep every layer 16x16 RGBA8 for the current block set unless a new decision changes the tile
  size or format;
- validate dimensions, format, layer count, and colour space before upload;
- decide mipmap and filtering policy explicitly, including whether nearest/no-mipmaps remains;
- keep block IDs and save data independent of layer order;
- fail with a typed error on a malformed pack, rather than sampling a wrong or missing layer.

A future `assets/` directory would be a new top-level folder. Do not create it before the asset
decision; the documentation site's `docs/assets/` is not the game asset location.

## Validate

CPU tests should cover:

- `TEXTURE_LAYERS` matches the spec and the generated array depth.
- Every solid block returns `Some(FaceLayers)`; Air returns `None`; Water returns `Some`.
- The top, side, and bottom mapping for every block, including Grass and Wood.
- The 12-byte packed vertex layout: `cell`/`face`, `texel`/`layer`/`reserved`, and `light_ao`
  words, with their offsets.
- Deterministic tile generation: the same layer produces the same bytes; every tile is exactly
  16x16 RGBA8.
- Layer indices are contiguous from `0` to `TEXTURE_LAYERS - 1`.

Manual GPU acceptance should cover:

- All six faces of each block use the expected tile.
- Grass has distinct top, side, and bottom tiles; Wood has distinct top and side tiles.
- Nearest filtering shows crisp texels; no mip blur.
- No sampling bleeds between layers.
- Greedy quads repeat the tile across their full width and height (v0.0.5).
- Water uses the correct layer and the separate transparent pipeline (v0.0.6).
- No `wgpu` validation errors, missing layers, or black faces.

There are no automated GPU tests; see [Testing](testing.md#manual-gpu-acceptance). Run with
`RUST_LOG=engine=debug,wgpu=warn` when diagnosing texture or bind-group validation. Use RenderDoc,
Nsight, or PIX to inspect the array view and sampler state.

## Review checklist

- The layer table, specs, `Block::layers`, and `TEXTURE_LAYERS` agree.
- Layer indices are explicit, contiguous, append-only, and not derived from enum order.
- The texture is one `D2Array` with the expected size, format, layer count, and one mip.
- The sampler is nearest, repeat in U/V, no mipmaps, and no anisotropy.
- The bind group layout and WGSL group/binding declarations match.
- The vertex cell, face, texel, layer, and light/AO fields match the 12-byte layout and the mesher.
- No flat atlas, per-block texture binding, or runtime image loader was added.
- Tile generation is deterministic and does not pin non-normative pixels.
- CPU mapping tests and manual GPU acceptance pass.
- No game texture files or `assets/` folder were added without a decision.

## References

- [ADR-0007: texture array for block textures](../decisions/0007-texture-array-for-block-textures.md)
- [ADR-0008: chunk save format](../decisions/0008-chunk-save-format.md)
- [v0.0.4 spec](../specs/v0.0.4.md)
- [v0.0.5 spec](../specs/v0.0.5.md)
- [v0.0.6 spec](../specs/v0.0.6.md)
- [v0.0.7 spec](../specs/v0.0.7.md)
- [Shader authoring](shaders.md)
- [Architecture](architecture.md)
- [Testing](testing.md)
