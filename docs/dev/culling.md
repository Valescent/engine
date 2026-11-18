# Culling

Culling decides which chunks reach the GPU. This page records what the renderer does today, the
nearby options, and the order they are worth building.

| Area | Status |
|---|---|
| Chunk frustum culling | Current (v0.0.3) |
| Backface culling | Current (v0.0.8) |
| Distance LOD (LOD0/1/2) | Current (v0.0.8) |
| Hierarchical frustum culling | Proposed |
| Horizon (heightmap) culling | Proposed |
| Software (raster-based) occlusion | Proposed |
| Voxel visibility graph (cave flood fill) | Proposed |
| GPU occlusion queries or a depth pyramid | Proposed |
| Multi-draw indirect batching | Proposed |
| GPU-driven Hi-Z | Proposed, distance 64+ |

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
side of 1,900 draws. wgpu 30 exposes `multi_draw_indexed_indirect` and the
`MULTI_DRAW_INDIRECT_COUNT` feature; support varies by backend and Metal may emulate the call, so
verify on the target backend and keep a single-draw fallback.

## Other techniques

The list above is not exhaustive. These are the remaining families worth knowing, in rough order
of fit for this engine.

### Hierarchical (cluster) frustum culling

Group chunk AABBs into a quadtree or octree over chunk columns and test cluster boxes top-down,
recursing only into visible clusters. At distance 24 the 5,289 chunk tests cost little; at
distance 48 they do not. Clusters also give the traversal order that software occlusion and
occlusion queries want.

- **Data:** a tree rebuilt incrementally as columns load and unload, or a fixed grid of 8x8 chunk
  blocks with a max-height bound.
- **Saves:** most of the per-frame AABB tests; no draw reduction on its own.
- **Risk:** none to image correctness.
- **Effort:** small.

### Software (raster-based) occlusion culling

Rasterize a coarse set of occluders into a small CPU depth buffer (for example 320x180), then test
each candidate chunk AABB against it. Conservative coverage, pixel bias, and a short occluder list
(nearest chunks first, largest solid chunks only) keep it fast enough to run every frame. This is
the technique used by several voxel engines, including JuanDiegoMontoya's raster-based renderer, and
is described by Narkowicz and in Intel's masked software occlusion paper.

- **Saves:** chunks hidden by any geometry, including caves and overhangs, not just silhouettes.
- **Cost:** roughly 0.3-1.5 ms CPU for a small buffer; scales with occluder count, not world size.
- **Risk:** under-conservative rasterization causes holes; bias and test order must be tuned.
- **Effort:** medium. It composes with the horizon pass: horizon culls cheaply, software occlusion
  catches what is left.

### Voxel visibility graph (cave flood fill)

For each chunk, precompute a six-bit mask of which neighbouring chunks are reachable through
transparent cells. Each frame, flood fill from the camera's chunk through faces that point toward
the camera (`N · V < 0`), and draw only reached chunks. This is the "advanced cave culling"
algorithm used by Minecraft-style engines; it follows tunnels exactly and needs no rasterization.

- **Saves:** everything unreachable through empty space, including deep caves and sealed pockets.
- **Cost:** per-chunk masks recomputed after edits; one BFS over the reachable chunks per frame.
- **Risk:** correctness and edit maintenance; on open terrain the fill reaches almost everything in
  the frustum, so the win is underground.
- **Effort:** medium-high.

### Portal culling and precomputed PVS

Rooms and portals, or a potentially-visible-set per region, need a static, authored topology. A
procedural, editable world invalidates both as the player digs. Deferred; revisit only if the engine
grows hand-authored interiors.

### Screen-size (sub-pixel) culling

Project the chunk AABB and skip chunks whose screen extent is below one or two pixels. At distance
48 the far ring contributes vertices but almost no pixels. Fog hides the pop.

- **Saves:** vertex and draw work at the horizon.
- **Cost:** one projection per candidate, cheap.
- **Risk:** popping at the cull threshold; tune with fog distance.

### Temporal visibility

Reuse the previous frame's visible set, expanded by one chunk, and test only the new candidates.
Cheap and stable for a walking camera; pairs naturally with occlusion queries. Not worth it while
frames are around 1 ms.

### Sub-chunk sections

Split each 32^3 chunk into 16^3 sections and cull them individually. More granularity, especially
for caves, but it multiplies draw calls; only sensible after batching or GPU-driven drawing.

### GPU-driven Hi-Z

The endgame: a depth pyramid from the previous frame, a compute pass that tests chunk AABBs and
writes indirect draw args, and one or two multi-draw calls. The Aokana voxel framework describes
this shape for open worlds. It removes nearly all per-chunk CPU work and pairs with LOD and
visibility buffers.

- **Needs:** storage buffers, compute, indirect draws, and a depth-pyramid resolve.
- **Effort:** high; the right investment for distance 64+.

### Impostors and skyline rendering

Not culling, but the far-distance alternative: stop meshing the outer ring and render a heightfield
silhouette or billboard impostors. Worth evaluating if distance beyond 48 becomes a goal.

## References

- [Aokana: A GPU-Driven Voxel Rendering Framework for Open World Games](https://arxiv.org/abs/2505.02017) — Hi-Z occlusion and visibility buffers for voxel worlds.
- [Krzysztof Narkowicz: Software occlusion culling](https://knarkowicz.wordpress.com/2010/09/27/software-occlusion-culling/) — raster-based CPU occlusion.
- [Intel: Masked Software Occlusion Culling](https://www.intel.com/content/dam/develop/external/us/en/documents/masked-software-occlusion-culling-779241.pdf) — conservative masked rasterization.
- [JuanDiegoMontoya/Voxel_Engine](https://github.com/JuanDiegoMontoya/Voxel_Engine) — raster-based occlusion culling and one-call chunk drawing.
- [Voxel.Wiki references: flood-fill and potentially visible sets](https://voxel.wiki/wiki/references/) — cave culling and visibility graph links.

## Recommended order

1. **Cluster frustum culling** — small, safe, and keeps the per-frame test count flat as distance
   grows.
2. **Horizon culling** — uses data the world already has, CPU-only, and directly cuts
   `visible_chunks` on hilly terrain.
3. **Water distance cutoff and front-to-back ordering** — small, independent, cheap.
4. **Software occlusion culling** — the cave- and overhang-aware CPU step once horizon is in place;
   the visibility graph is the alternative if underground play becomes central.
5. **Multi-draw indirect** — the right fix once draw count, not pixels, is the limit.
6. **GPU-driven Hi-Z** — for distance 64+, once the CPU path is exhausted.

Each step must be measured against the same seed, camera, and render distance; use the policy in
[Benchmark results](benchmarks.md).
