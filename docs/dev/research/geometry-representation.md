# Geometry representation research

The shared quad pool with vertex pulling and multi-draw indirect landed in v0.0.9 slice 4
([ADR-0017](../../decisions/0017-quad-pool-and-indirect-draws.md)): 12 bytes per quad instead of 60,
which took resident distance-32 geometry from 100.9 MB to 37.6 MiB (125.9 MB before shared index
buffers) and draws from 3,409 to 2. This report asks what comes after that, and whether any modern or
theoretical representation changes the plan. The one record experiment left open is the 8-byte form,
which needs flat per-quad light or a second light stream.

The short answer: after the quad pool, the remaining wins are **fewer quads, less overdraw, and a
cheaper far field**, not a more exotic geometry representation. wgpu 30 rules out most of the
hardware-driven options, and the voxel-specific research agrees that downsampling occupancy and
re-meshing beats mesh simplification.

## Baseline and the landed pool

| Metric | Pre-pool | 12-byte pool (landed) |
|---|---:|---:|
| Resident geometry, distance 32 | 100.9 MB | 37.6 MiB |
| Visible geometry, distance 32 | 30.1 MB | 7.52 MiB |
| Drawn quads, distance 32 | 657,000 | 657,000 |
| Draw calls | 3,409 | 2 |

The landed pool did not reduce drawn quads, only the bytes per quad and the submission cost. Anything
beyond it has to reduce what is drawn or how far it is drawn.

## wgpu 30 reality check

| Feature | Status | Consequence |
|---|---|---|
| Mesh shaders | Experimental `EXPERIMENTAL_MESH_SHADER`; Naga compiles WGSL only for Vulkan, DX12/Metal need passthrough; no queries | Not a portable solo-engine path |
| Work graphs | Not supported ([wgpu #9720](https://github.com/gfx-rs/wgpu/issues/9720)) | No GPU-side pipeline sequencing |
| Geometry shaders | Not supported | Cluster expansion must be CPU or compute-written |
| Sparse resources / residency | Not supported | SVDAG streaming is manual atlas management |
| `MULTI_DRAW_INDIRECT_COUNT` | DX12 and Vulkan only; Metal entry points are stubs | Metal needs fixed counts plus zero-instance draws |
| `INDIRECT_FIRST_INSTANCE` | Apple3+/Mac1+, Vulkan, DX12 | Per-draw record indexing is portable to desktop |
| Default limits | 128 MiB storage binding, 256 MiB buffer | A 125.9 MiB pool fits, but leaves little headroom |

The portable GPU-driven shape stays: a compute cull writes indirect records, and one or two
`multi_draw_indexed_indirect` calls draw them.

## Ranked survey

| # | Technique | Removes | Cost and main risk | 2-4 weeks? |
|---|---|---|---|---|
| 1 | Chunk LOD (voxel stride 2/4, re-mesh) + Hi-Z cull + indirect count | 50-75% of quads and vertex work at range | 2-3 weeks; seams and popping | Yes |
| 2 | Far heightfield or horizon ring | 60-80% of distant chunks; near-constant triangle count | 1-2 weeks; overhangs, boundary seam | Yes |
| 3 | 6-8 byte attribute-free quad record with deferred material lookup | Half of the 25 MB plan; less vertex bandwidth | 1-2 weeks; material fetch in the fragment shader | Yes |
| 4 | Chunk dedup and material palette | 2-5x if terrain repeats | 1-2 weeks; low reuse on noisy worlds | Yes, low priority |
| 5 | Hardware visibility buffer (R32_UINT ids) + compute resolve | G-buffer bandwidth and overdraw shading | 2-3 weeks; resolve cost, no MSAA | Maybe |
| 6 | Compute raymarched bricks or SVDAG (Aokana-style) | The whole vertex and index pool | 6-12 weeks; divergence, fill rate, streaming | No |
| 7 | Full SVDAG or SVO | Meshing and up to 9x memory | 8-16 weeks; editing, collision, streaming | No |
| 8 | Nanite-style cluster DAG for voxels | Cluster culling, not much LOD quality | High; simplification fits disconnected voxels poorly | No |
| 9 | Mesh shaders | Vertex and index buffers | Experimental and backend-specific | No |
| 10 | Work graphs | CPU pipeline sequencing | Absent in wgpu | No |

### 1. Chunk LOD, Hi-Z, and GPU-driven submission

Nanite's durable ideas are cluster culling, a persistent LOD structure, and indirect submission, not
mesh simplification ([Nanite, SIGGRAPH 2021](https://www.advances.realtimerendering.com/s2021/Karis_Nanite_SIGGRAPH_Advances_2021_final.pdf)).
For voxels, the correct LOD is occupancy downsampling and re-meshing, which this engine already does
at the chunk level; the next step is finer selection (screen-space error) plus a compute pass that
culls chunk AABBs against a depth pyramid and writes the indirect list. Aokana reports that
disconnected voxel clusters merge poorly, which argues against meshlet-style simplification
([Aokana](https://arxiv.org/abs/2505.02017)).

The [Hi-Z prototype](../../../prototypes/chunk-lod-hi-z/) validates the culling half: a compute pass
over 4,096 chunk AABBs culls 51.8% of the frustum-visible set against a synthetic depth pyramid, with
an exact match to the CPU frustum set when the pyramid is empty. It writes the indirect records in
the shape [ADR-0017](../../decisions/0017-quad-pool-and-indirect-draws.md) needs.

### 2. Far heightfield or horizon ring

Beyond the voxel window, a heightfield ring can cover the horizon with a near-constant triangle
budget ([geometry clipmaps](https://hhoppe.com/proj/geomclipmap/)). It cannot show caves or
overhangs, and it needs a blend at the voxel boundary, but it removes most distant chunks for a small
amount of code. This is complementary to the seed-native hierarchy in
[ADR-0014](../../decisions/0014-seed-native-lod-hierarchy.md): the hierarchy keeps the blocky look and
edits; the ring is the cheaper fallback if hierarchy seams eat the schedule.

The [far heightfield ring prototype](../../../prototypes/far-heightfield-ring/) measures the trade:
five levels cover 1,024 m with 33,792 triangles, 1.61% of a full-resolution grid over the same extent.
Its largest T-junction gap is 6.1 m, hidden by 8 m skirts. That is a strong result for the far field
and a weak one for anything that needs to look close.

### 3-4. Record compression and dedup

The landed 12-byte record can shrink to 6-8 bytes with a 5-bit origin and extent per axis, a 2-bit
axis, a 1-bit sign, 12-bit material, and 3-bit AO; chunk identity belongs in the draw record, not the
quad. That only works if light becomes flat per quad or moves to a second stream, which is the
experiment left open. Octahedral normal encoding is for arbitrary normals and does not apply here.
Meshopt-style codecs target static triangle meshes and are irrelevant to runtime greedy quads; they
would only help offline-baked far LODs. Chunk or brick deduplication helps only if terrain repeats,
which noisy generated worlds mostly do not.

### 5-10. What is not worth it yet

- **Visibility buffers** are credible and wgpu-feasible, but they shift cost to a resolve pass and are
  best combined with the quad pool and deferred material fetch.
- **Raymarched bricks or SVDAGs** replace meshing, editing, collision, and streaming at once. Recent
  work improves compression and traversal, but compact pointer encodings cost 15-25% traversal time,
  and wgpu has no sparse residency. The shippable sliver is chunk dedup, not a DAG.
- **Nanite for voxels** fails on the thing Nanite is for: merging disconnected voxel geometry into
  coherent clusters produces poor LOD.
- **Per-voxel soft rasterization** reconstructs up to twelve triangles per visible voxel and explodes
  near the camera ([VirtualVoxel](https://doi.org/10.1145/3588028.3603664)).
- **Mesh shaders and work graphs** are not portable in wgpu 30.
- Euclideon's "Unlimited Detail" has no peer-reviewed description; treat it as marketing.

## Recommendation

1. **The quad pool, shared index buffer, and indirect draws are done.** The open record item is the
   6-8 byte attribute-free experiment: it saves 8-16 MB against the landed 38.4 MB pool but needs
   flat per-quad light or a second light stream, so measure the visual and bandwidth trade before
   accepting it.
2. **Prototype chunk LOD + Hi-Z + indirect count.** Biggest remaining GPU-time win, and the
   foundation for any later virtual geometry. Seams are the risk.
3. **Prototype the far heightfield ring** in parallel or as the fallback: 1-2 weeks, large visible
   win, independent of the LOD work.
4. Skip SVDAG, Nanite-style clusters, mesh shaders, and work graphs until the engine has a concrete
   need they uniquely solve.

The v0.0.9 hierarchy already targets the far field with blocky sections. Before adding a second
representation, measure how far the hierarchy reaches at 60 FPS; the ring only earns its place if the
hierarchy cannot cover the horizon cheaply.

Standalone experiments live in [`prototypes/`](../../../prototypes/README.md). Both are now verified:
the ring covers 1,024 m at 1.61% of full-resolution triangles, and the Hi-Z pass culls 51.8% of the
frustum-visible chunks with an exact empty-pyramid match. They are not workspace members and do not
affect the engine build.

## References

- [Aokana: GPU-driven voxel rendering](https://arxiv.org/abs/2505.02017) - Hi-Z, visibility buffers,
  DAG streaming, and negative results for meshlet-style voxel LOD.
- [Nanite, SIGGRAPH 2021](https://www.advances.realtimerendering.com/s2021/Karis_Nanite_SIGGRAPH_Advances_2021_final.pdf)
  - cluster culling and indirect submission.
- [Sparse voxel DAGs](https://doi.org/10.1145/2461912.2461913), [HashDAG](https://doi.org/10.1111/cgf.13916).
- [VirtualVoxel](https://doi.org/10.1145/3588028.3603664) - per-voxel triangle explosion.
- [Geometry clipmaps](https://hhoppe.com/proj/geomclipmap/).
- [meshoptimizer](https://meshoptimizer.org/) and the 2026 billion-triangle retrospective.
- [wgpu mesh shading spec](https://github.com/gfx-rs/wgpu/blob/v30/docs/api-specs/mesh_shading.md),
  [work graphs issue](https://github.com/gfx-rs/wgpu/issues/9720).
