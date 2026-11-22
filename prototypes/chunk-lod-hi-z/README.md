# chunk-lod-hi-z

A standalone, headless prototype of the chunk-LOD + Hi-Z occlusion culling +
indirect-submission idea from
[`docs/dev/research/geometry-representation.md`](../../docs/dev/research/geometry-representation.md).

It is an experiment, not production code. There is no window, no rendering, and
no mesh LOD selection yet. The default binary builds a synthetic voxel world on
the CPU, runs a WGSL compute cull on the GPU, reads the results back, and exits
non-zero if the visibility assertions fail.

## What it proves

- **Synthetic chunk table.** A 64x64 grid of 32 m chunks (4096 chunks) with an
  AABB per chunk. Heights come from a deterministic wave/ridge function so the
  data looks like terrain without assets.
- **Frustum + Hi-Z compute cull.** `src/cull.wgsl` tests each AABB against six
  camera planes, projects its eight corners to a screen-space rectangle, and
  compares the chunk's nearest view depth against a min-depth pyramid over that
  rectangle. Chunks that pass append one `DrawIndexedIndirectArgs` record
  (`index_count = 6`, `instance_count = 1`, `first_index = 0`,
  `base_vertex = 0`, `first_instance = chunk_id`) and the same id to a parallel
  visible-id buffer, using an atomic counter.
- **CPU-built Hi-Z.** A 256x256 f32 depth image is generated in two variants:
  - empty/far depth (all reverse-depth values 0), which must cull nothing;
  - a screen-filling occluding wall at 1200 m view depth, which must hide
    roughly half of the frustum-visible chunks.

  The full 9-level (256x256 down to 1x1) min pyramid is flattened into one
  storage buffer. Depth uses the reverse-Z convention `1 - view_depth / far`,
  so the pyramid stores the *farthest* occluder per 2x2 block and the test
  `chunk_reverse_depth < occluder_reverse_depth - bias` is conservative.
- **Headless verification.** The pass writes a counter, indirect records, and
  chunk ids. The prototype copies them to a `MAP_READ` staging buffer, maps it,
  and asserts:
  - the empty scene's visible set exactly equals the CPU frustum set;
  - no duplicate ids are emitted;
  - every indirect record has the expected five fields;
  - the wall scene emits only chunks that survived the CPU frustum test;
  - every chunk comfortably in front of the synthetic wall is still emitted;
  - every chunk well behind the synthetic wall is absent;
  - the wall culls a meaningful number of chunks (roughly half in the default
    setup).

## Run

Requires a working Vulkan adapter. The prototype requests Vulkan explicitly,
rejects CPU adapters, and requires `INDIRECT_FIRST_INSTANCE`. It fails loudly
with a non-zero exit if no usable GPU is present.

```sh
cd prototypes/chunk-lod-hi-z
cargo check
cargo clippy -- -D warnings
cargo run --release
```

Expected output on the development machine (RTX 4060, Vulkan):

```text
adapter: NVIDIA GeForce RTX 4060 (Vulkan)
HIZ chunks=4096 visible=1356 occluded=1459 max_level=8
HIZ empty chunks=4096 visible=2815 occluded=0 max_level=8
report: scene=wall frustum=2815 frustum_culled=1281 ground_truth_occluded=1406 hi_z_occluded=1459 occlusion_ratio=51.8% draws_checked=1356
report: scene=empty cpu_frustum=2815 gpu_visible=2815 exact_match=true
verify: OK
```

The adapter name is machine-specific. The counts are deterministic for the
fixed camera, grid, wall depth, and bias in `src/main.rs`; changing those
constants changes the expected counts. `visible` is the number of chunks that
would be submitted in the wall scene, `occluded` is the Hi-Z cull count, and
`frustum_culled` covers chunks rejected before the Hi-Z test.

Passing any argument (including `--render`) is an error by design: this
prototype is compute-only.

## Layout

- `src/main.rs` - synthetic world, CPU Hi-Z builder, wgpu setup, readback, and
  assertions.
- `src/cull.wgsl` - one compute entry point: frustum test, screen-rect
  projection, pyramid sampling, atomic append.
- `Cargo.toml` - `wgpu = "30"`, `pollster`, `bytemuck`, `glam`; `publish = false`.
- `rustfmt.toml` - local formatting settings so the crate stays standalone.

## Limitations

- **Synthetic depth, not a rendered frame.** The Hi-Z input is a generated
  image, not the depth buffer of a real previous pass. There is no
  occlusion feedback loop, temporal stability check, or depth regeneration.
- **No rendering.** The prototype never creates a surface, swapchain, or render
  pipeline, and never calls `multi_draw_indexed_indirect`. It only produces the
  records a renderer would consume.
- **No LOD meshing or selection.** Chunks have no meshes and no vertex/index
  data. "LOD" is represented only by the chunk-level culling/setup; screen-space
  error selection and stride-2/4 re-meshing are not implemented.
- **No streaming, editing, seams, or popping.** The world is fixed and fully
  resident. The 64x64 grid contains 4096 AABBs, far below a production budget.
- **Conservative screen-rect Hi-Z.** The test samples a small 2x2 footprint at
  a mip chosen to cover the projected AABB; it is intentionally conservative
  (may keep chunks that a per-pixel test would reject) and has no two-pass
  refinement.
- **One fixed camera.** There is no flycam or animation, so the numbers are a
  single snapshot rather than a frame-time profile.
