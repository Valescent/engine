# Culling

Culling decides which chunks reach the GPU. This page records what the renderer does today, the
nearby options, and the order they are worth building.

| Area | Status |
|---|---|
| Chunk frustum culling | Current (v0.0.3) |
| Backface culling | Current (v0.0.8) |
| Distance LOD (LOD0/1/2) | Current (v0.0.8) |
| Horizon (heightmap) culling | Proposed |
| GPU occlusion queries or a depth pyramid | Proposed |
| Multi-draw indirect batching | Proposed |

## Baseline

Release build, `render_distance_chunks = 24`, vsync off, the spawn view of seed 42:

| Metric | Value |
|---|---:|
| Loaded chunks | 5,289 |
| Visible chunks after frustum culling | 1,436 |
| Draw calls (opaque + water) | 1,934 |
| Visible vertices | 2,984,484 |
| CPU frame time | 1.8-2.1 ms |
| Working set | 617 MB |

The frustum removes about 73% of the loaded chunks in that view. The remaining frame cost is
mostly vertex shading and per-chunk draw setup; the 60 FPS budget at this distance is not the
bottleneck yet. Culling is what makes distances beyond `32` affordable.

## What runs today

### Frustum culling

`Renderer::draw_scene` tests each chunk's 32 m AABB against six planes before recording a draw.
The test runs once per loaded chunk per frame, plus once more for the water pass. It is cheap
(a few thousand AABB tests) and already skips most chunks outside the view.

### Distance LOD

The mesher downsamples far chunks: LOD1 from 8 chunk units, LOD2 from 16. That cuts vertex count
by roughly 8x and 64x, which is the strongest culling in the engine today. LODs only change the
geometry; they do not remove chunks.

### Backface culling

The terrain and water pipelines now discard back faces (`cull_mode: Some(Face::Back)`). Greedy
quads wind counter-clockwise around their outward normal for all six faces, which
`every_face_winds_counter_clockwise_around_its_normal` pins down. This does not reduce vertex
work; it removes rasterization and fragment shading for quads seen from behind.

The overlay pipeline deliberately keeps double-sided geometry.

### Depth testing

Opaque chunks write depth and test `Less`; water tests `LessEqual` without writing. Hidden
fragments are still shaded unless an earlier draw wrote the depth, so drawing front-to-back would
help overdraw. The current order is the chunk map order.

## Candidate techniques

| Technique | Removes | Cost | Notes |
|---|---|---|---|
| Backface culling | Back faces | None | Done. |
| Front-to-back sort | Overdraw | O(n log n) per frame | Sorts about 1,400 visible chunks; cheap, GPU-only win. |
| Water distance cutoff | Transparent draws beyond LOD1 | None | Water is full resolution; skip it far away. |
| Horizon (heightmap) culling | Chunks below the terrain silhouette | Per-frame horizon build from column tops | Catches hills and mountains; cannot see into caves or tunnels. |
| GPU occlusion queries | Any chunk hidden on screen, including caves | Query set plus one-frame-late readback | Handles what heightmaps cannot; adds synchronization. |
| Screen-space depth pyramid | Same as queries, no readback stall | Compute pass plus indirect draws | Most robust, most work. |
| Multi-draw indirect | Per-chunk draw setup | Megabuffer plus indirect args | Cuts 1,900 draws to a handful; the enabler for distance 48+. |

### Horizon culling sketch

1. `World` keeps a `column_top` height for every loaded column, updated on `set_block` (O(1) for
   placements; a short downward scan when the top block is removed) and rebuilt for a chunk when
   it is inserted or unloaded.
2. Per frame, bucket the loaded columns by azimuth bin from the camera and keep, per bin, the
   maximum elevation angle over distance (a small prefix-max or sorted list).
3. For each chunk, take its nearest horizontal distance and its highest top, and compare its
   elevation angle against the bin's horizon at that distance. Cull only when the chunk is
   strictly below the conservative horizon.

The heightmap is conservative by construction: it can only hide chunks that the terrain silhouette
actually covers. It does not help underground, and it needs care at the render-distance edge where
unloaded columns read as zero height.

### GPU occlusion sketch

Draw candidate chunks with an occlusion query set, then read back the previous frame's results and
skip chunks with zero pixels. One frame of latency is acceptable at 60 FPS; the complexity is in
the readback path and keeping the query set sized to the visible set. This is the only technique
here that handles caves and overhangs.

### Multi-draw sketch

Three megabuffers (one per LOD) with per-chunk vertex ranges, one bind group holding every chunk
origin (or an origin index in a storage buffer), and `multi_draw_indirect` with one indirect call
per batch. That removes the per-chunk bind group and vertex-buffer rebinds that dominate the CPU
side of 1,900 draws. It needs `Features::MULTI_DRAW_INDIRECT` and a wgpu fallback.

## Recommended order

1. **Horizon culling** — uses data the world already has, CPU-only, and directly cuts
   `visible_chunks` on hilly terrain. Measure the reduction before deciding on the next step.
2. **Water distance cutoff and front-to-back ordering** — small, independent, cheap.
3. **Multi-draw indirect** — the right fix once draw count, not pixels, is the limit.
4. **GPU occlusion or a depth pyramid** — only if caves and overhangs show up as a measured cost.

Each step must be measured against the same seed, camera, and render distance; use the policy in
[Benchmark results](benchmarks.md).
