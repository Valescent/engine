# Rendering research

The renderer is deliberately simple: greedy meshes, one vertex and index buffer per LOD per chunk,
a texture array, and one draw per visible chunk. That holds 60 FPS at distance 24. This report
covers what changes if distance or content grows.

## Baseline (distance 24)

| Metric | Value |
|---|---:|
| Visible chunks | 1,436 |
| Draw calls | 1,934 (opaque + water) |
| Visible vertices | 2,984,484 |
| Vertex size | 12 bytes packed, four vertices and six 16/32-bit indices per quad |
| CPU frame | 1.8-2.1 ms before packing; 2.2-2.6 ms at the settled distance-24 run after packing, while streaming |
| Per-chunk GPU objects | Origin uniform buffer + bind group + one vertex and index buffer per built LOD + water |

The memory and bandwidth figures below were measured before the v0.0.8 packing change; the
options that landed are marked so the report still reads as a record.

## Cost model

- **Vertex bandwidth.** 12 bytes packed, four vertices and six indices per quad. A 1,000-quad chunk
  costs 48 KB of vertex data plus 12 KB of 16-bit indices. At distance 24 the resident vertex
  buffers are the second largest memory consumer after chunks.
- **State changes.** Each draw rebinds the chunk bind group and a vertex buffer. At 1,934 draws that
  is the dominant CPU encoder cost, and it grows linearly with distance.
- **Overdraw.** Opaque chunks draw in chunk-map order with depth test and write. Front-to-back order
  would cut shaded fragments; back faces are now culled.
- **Texture sampling.** One array lookup per fragment with a nearest sampler; no mipmaps or
  anisotropy, so distant surfaces shimmer and sample poorly.

## Options

### 1. Indexed draws — implemented in v0.0.8

Emit four vertices and six indices per quad. Saves 28% of vertex bytes with 16-bit indices and lets
the post-transform cache reuse shaded vertices. Water and opaque meshes both benefit. The mesher
keeps a per-chunk index buffer and picks 16- or 32-bit indices per chunk.

### 2. Packed vertices — implemented in v0.0.8

Positions are chunk-local, so `u8` triples are plenty; the normal is one of six values, and the
texture layer fits in the same word as the light/AO. The live vertex is 12 bytes, down from 40.

### 3. One origin buffer, not one per chunk

The chunk origin is a four-float uniform buffer plus a bind group per chunk. A single storage buffer
of origins indexed by draw (or a vertex attribute carrying the chunk index) removes thousands of
bind groups and buffers. wgpu exposes storage buffers and, on some backends, binding arrays; the
portable version is an index in a storage buffer read by the vertex shader.

### 4. Megabuffer and multi-draw indirect

Append every chunk's vertices into one buffer per LOD and draw ranges from an indirect buffer. On
wgpu 30, `multi_draw_indexed_indirect` with `MULTI_DRAW_INDIRECT_COUNT` collapses thousands of draws
into one call; backend support varies and Metal may emulate it, so keep a per-chunk fallback. This is
the natural follow-on to the origin buffer and the culling work.

### 5. Front-to-back ordering

Sort visible chunks by squared distance to the camera before drawing. With depth testing already on,
near geometry writes depth first and hides far fragments. Cost is one sort of ~1,400 entries per
frame; benefit is GPU-only and grows with overdraw.

### 6. Texture quality

Add mipmaps and anisotropic filtering to the array; mip generation is a one-time cost per layer.
If assets ever grow past procedural 16x16, BC7 (desktop) or ASTC (mobile) cuts texture memory. For
the current procedural set the win is quality, not size.

### 7. Depth prepass or Hi-Z

A depth-only pass over large occluders, then a depth test for everything else, reduces overdraw but
adds a pass. With culling in place it is only worth it if overdraw is measured. A Hi-Z pyramid is the
same data the GPU-driven culling path needs.

### 8. Shadows

Not a culling or bandwidth topic, but the obvious next visual feature. Cascaded shadow maps over the
same chunk meshes would add a depth pass per cascade and a second sampling path. Research separately
before committing; it changes the frame shape more than any item above.

## Recommendation

1. Packed, indexed vertices. Done in v0.0.8.
2. Replace per-chunk origin buffers with an indexed storage buffer.
3. Front-to-back ordering; a few lines for a GPU win.
4. Megabuffer plus multi-draw once draw count, not vertices, is the limit.
5. Mipmaps and anisotropy for quality.

Items 2 and 4 are the first two steps of [GPU-driven meshing](gpu-driven-meshing.md), which also
covers vertex pulling, a shared quad pool, and a compute mesher.

Validate with `draw_calls`, `visible_vertices`, `GPU` time under Tracy, and the distance-24 scene.
Keep the v0.0.4 reference mesher for the existing correctness tests.

## References

- [Post-transform cache](https://www.opengl.org/wiki/Post_Transform_Cache) — indexed triangle lists.
- [wgpu `RenderPass::multi_draw_indexed_indirect`](https://docs.rs/wgpu/latest/wgpu/struct.RenderPass.html) — batching entry point.
- [Greedy meshing and vertex pooling](https://nickmcd.me/2021/04/04/high-performance-voxel-engine/) — per-orientation pools.
- [Aokana GPU-driven voxel rendering](https://arxiv.org/abs/2505.02017) — visibility buffers and Hi-Z for voxel worlds.
