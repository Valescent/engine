# GPU-Driven Voxel Meshing and Rendering: Spec v1

Status: Draft · Nov 19, 2026 · Companion to "Voxel World Engine: Architecture v2"

## 1. Purpose and goals

Define a GPU-driven pipeline that turns voxel data into visible geometry with minimal CPU involvement, so the engine can show millions of voxels at the minimum spec (4 cores / 8 threads, 16 GB RAM, 8 GB VRAM, 60 fps at 1080p).

**Goals**

- CPU does orchestration only: it queues work and submits a fixed number of passes per frame. It never builds vertices, sorts faces, or loops over chunks to draw them.
- Geometry is stored as one packed integer pair per quad (vertex pulling), with no vertex buffers and no index buffers.
- Visibility is decided on the GPU (frustum, face direction, occlusion), so draw cost scales with what is visible, not with what is loaded.
- Edits and simulation events (avalanches, wildfires, spells) remesh within a per-frame budget without frame spikes.
- Works on desktop via wgpu now, and does not block a later console port.

**Non-goals (v1)**

- Web / WebGL2 target (storage buffers and compute are required).
- Ray-marched global illumination. Near-field sun shadows stay as already decided in the architecture doc.
- Meshing the entire 4 cm resolution world. Fine detail is only meshed where blocks are subdivided.

**Honest expectation on resources.** This design cuts CPU work sharply. It does not make RAM or VRAM free: the quad pool and voxel brick pool live in VRAM (the depth pyramid joins them in v2), and CPU RAM still holds the authoritative world data, entity state, and mod runtime. Actual numbers must be measured against the budgets in section 10.

## 2. Terms

| Term | Meaning |
| --- | --- |
| Block | 1 m cube, the basic edit unit |
| Sub-voxel | 4 cm cube. A subdivided block holds 25 × 25 × 25 of them |
| Chunk | 16 × 16 × 16 blocks (16 m) |
| Quad | One merged rectangular face, the unit of geometry |
| SSBO | Storage buffer, bound as `var<storage>` in WGSL |
| Vertex pulling | The vertex shader reads quad data from an SSBO using `vertex_index` |
| Hi-Z | Hierarchical depth pyramid used for occlusion tests |

## 3. Pipeline overview

1. Edit upload: scatter edits into the voxel pool, mark chunks dirty.
2. Mesh: compute passes turn dirty chunks into packed quads.
3. Allocate: the host reserves a range in the shared quad pool and the GPU writes the quads into it (§6.5).
4. Cull: compute passes build a compacted indirect draw list.
5. Draw: multi-draw indirect with vertex pulling.

Everything between steps 1 and 5 runs on the GPU except the host-side range allocation and readback trim described in §6.5. The CPU records the same fixed set of passes each frame. The depth pyramid arrives with Hi-Z occlusion culling in v2 (§7).

## 4. Data structures

### 4.1 Voxel brick pool (SSBO)

- One large storage buffer of fixed-size bricks, plus a page table (chunk to brick indirection). Chunks load and free without reallocating the buffer.
- Palette compression: each brick stores a small palette and 1 to 8 bit indices.
- Sparse subdivision: a block is either a single material or a pointer to a 25³ sub-brick. Meshing descends only where the subdivided flag is set.
- Padded borders: the mesher works on a padded copy with one voxel of neighbor data on each side. A chunk is 18 blocks wide when padded and a sub-brick is 27 wide, so each column fits in one `u32`. This also removes cross-chunk reads during face culling.

### 4.2 Quad record (8 bytes, `vec2<u32>`)

| Field | Bits |
| --- | --- |
| Position in chunk (x, y, z) | 5 each (block level), plus sub-voxel offset where subdivided |
| Width, height (greedy extents) | 5 each, extended for sub-brick quads |
| Face direction | 3 |
| Material ID | 12 |
| Ambient occlusion | 2 per corner (8 total) |
| Scale flag (block vs sub-voxel) | 1 |

Twelve bits is a hard cap, not a range. The block-level fields total 37 bits, and a subdivided quad
adds a 5-bit sub-voxel offset per axis (15 bits), leaving exactly 12 for material in `vec2<u32>`. A
16-bit material would need 68 bits and cannot fit; more than 4,096 materials need a larger record or
an indirection.

The 8-byte record carries AO but no per-corner sky or block light, unlike the accepted 12-byte
renderer record, which spends 40 bits on four corners of sky/block/AO. To keep the current light
gradients, pair every quad with a light stream carrying the missing 4-bit sky and 4-bit block
values: four corners × 8 bits is 32 bits, a 4-byte record or 8 bytes when padded to a `vec2<u32>`;
AO stays in the quad record. If a single combined stream is preferred, mirror the 12-byte contract's
10-bit sky/block/AO corners (40 bits, 5 bytes) and leave the quad's AO field unused. The vertex
shader reads the quad and its light record with the same index. If no stream is added, this section
must explicitly revise the contract to flat per-quad light, a visible quality change to accept
rather than a silent omission.

Exact bit layout is finalized in an implementation task. Pack and unpack code is written once in Rust, unit-tested on the CPU, and mirrored in WGSL.

### 4.3 Quad pool (SSBO)

- One big shared buffer, sub-allocated per chunk range.
- Each chunk has 6 sub-ranges, one per face direction, so culling can skip back-facing groups.
- If `max_storage_buffer_binding_size` is exceeded, split into several pool buffers and bucket chunks by pool.

### 4.4 Chunk info buffer (SSBO)

- Per chunk: wrapped chunk coordinates (i32), pool IDs, range offsets and counts per direction, LOD level, bounds, and flags.
- Indexed in the vertex shader via `first_instance`.

### 4.5 World precision

The world is a torus of about 2^21 m per axis, which exceeds f32 precision. Rules:

- Chunk coordinates are `i32` and wrap on X and Z.
- The renderer computes a camera-relative chunk offset on the CPU or in a tiny compute pass, so vertex math is done in small f32 values near the camera.
- Neighbor lookups at the seam use wrapped coordinates.

## 5. Stage: edit upload

- Edits arrive from the authoritative world (single player, or server state in co-op) into a staging ring buffer.
- A scatter compute pass patches the voxel pool.
- Each edit pushes affected chunk IDs, including neighbors if a border voxel changed, into the dirty list.
- Simulation events that change many blocks (avalanche, fire) push in bulk. The remesh budget (section 6.6) protects frame time.

## 6. Stage: binary greedy meshing (compute)

### 6.1 Column bitmasks

For each (x, z) column, build one `u32` where bit y means solid. Shared slab data is loaded into `var<workgroup>` memory first.

### 6.2 Face detection by shifting

- Up faces: `col & ~(col >> 1)`
- Down faces: `col & ~(col << 1)`
- The same pattern runs for the other two axes using neighboring columns.
- Interior faces vanish automatically, so buried geometry is never emitted.

### 6.3 Run extraction

Use bit intrinsics (`countTrailingZeros`) to find a run start and length, then grow in the second axis while the next row contains the same run and material. Clear merged bits as you go.

### 6.4 Material and AO handling

- Merge only faces with the same material ID.
- Per-corner AO is part of the merge key. Faces merge only when AO is equal. If quad counts are too high, a later option is to relax this and accept slight shading error.

### 6.5 Output and allocation

- Per-workgroup counters in workgroup memory, then one global `atomicAdd` per workgroup to reserve output space. Subgroup ballots can replace this where wgpu subgroup support is available.
- Primary strategy: a CPU-side suballocator (TLSF or offset-allocator style) hands each remesh a conservative range. A readback one to two frames later gives the true count and the range is trimmed.
- Alternative: count pass, prefix sum, write pass (deterministic, but meshes twice).
- Double-buffered remesh: write the new mesh to a fresh range, swap the chunk's draw record, and free the old range only after in-flight frames complete. This prevents popping and use-after-free.

### 6.6 Budget

- A per-frame cap on remeshed chunks (target to be tuned) with priority by distance to the camera and by player-caused edits first.
- Chunks waiting in the queue keep drawing their previous mesh.

### 6.7 Sub-voxel meshing

Subdivided blocks mesh through the same algorithm on 27-wide padded sub-bricks, with quads flagged as sub-voxel scale. Faces on the boundary between a subdivided block and a solid neighbor must be emitted at the correct resolution.

## 7. Stage: culling (compute)

All tests feed one compacted draw list.

1. **Frustum culling:** chunk AABB against six planes. Required in v1.
2. **Face-direction culling:** a +X range is skipped when the camera is on the −X side of the chunk, and so on. Removes about half of the quads for free. Required in v1.
3. **Empty and buried chunk skip:** chunks with zero quads produce no draw record.
4. **Distance and LOD culling:** distant chunks use coarser merged meshes (2x, 4x), with skirts to hide seams.
5. **Hi-Z occlusion culling:** test chunk bounds against the depth pyramid using the two-phase method: draw last frame's visible set, rebuild the pyramid, test the remainder, draw newly visible chunks. v2.
6. **Cave and visibility-graph culling:** precomputed per-chunk bits for which chunk faces can see each other, flood-filled from the camera. Strong underground. v2.
7. **Cluster culling (optional):** split large chunks into clusters of about 64 quads with bounds, for finer culling. Later.
8. **Per-cascade culling:** shadow passes run their own culling.

Stream compaction: survivors append into the draw list with an atomic or prefix sum, and the final count is written to a buffer for indirect-count draws.

## 8. Stage: rendering

- **Vertex pulling:** 6 vertices per quad, `quad = quads[vi / 6]`. No vertex or index buffers.
- **Draw:** `multi_draw_indirect_count` where available, with a looped `draw_indirect` fallback.
- **Chunk ID:** `first_instance` indexes the chunk info buffer, so the wgpu feature `INDIRECT_FIRST_INSTANCE` is required.
- **Texturing:** pixel-art PBR textures in a texture array. Greedy quads tile with `fract(uv)` in the fragment shader, with mip bias or explicit gradients to avoid seam artifacts.
- **Depth:** reverse-Z with early-Z, drawing chunks roughly front to back.
- **Transparency:** a separate pass for water and other transparents, bucketed and sorted by chunk.
- **Foliage and characters:** unchanged from the architecture doc. Voxel trunks and rocks use this pipeline, while foliage stays instanced.

## 9. wgpu requirements and limits

| Item | Requirement |
| --- | --- |
| Compute and storage buffers | Core |
| `INDIRECT_FIRST_INSTANCE` | Required |
| Multi-draw indirect (count) | Optional, with fallback |
| Subgroup operations | Optional, with fallback |
| `max_storage_buffer_binding_size` | Raise from the default, or split pools |
| `max_storage_buffers_per_shader_stage` | Budget bindings across passes |
| Async compute queue | Not available. Meshing shares GPU time with rendering |
| 64-bit integers | Not in core WGSL. Use `u32` columns or `vec2<u32>` |

## 10. Budgets (targets to measure, not guarantees)

- VRAM total for this system within a fraction of the 8 GB minimum, leaving room for textures, shadows, and entities. Exact split is set after the first profiling pass.
- Remesh cost per chunk and per frame cap.
- Quads per frame after culling.
- Draw cost on the minimum-spec GPU at 1080p.
- CPU cost: orchestration only. Target is a small, fixed per-frame cost independent of visible chunk count.

Each budget needs a number once a baseline exists. Telemetry (already on by default with opt-out) should record frame timings for these passes.

## 11. Multiplayer, mods, and saves

- Meshes are client-side and derived data. They are never sent over the network or saved, so determinism of the mesher does not matter.
- Server or host sends voxel edits only. Clients remesh locally.
- Mods that add blocks or materials register material IDs and textures through the WASM API. The mesher treats materials as opaque IDs, with flags for transparency and emission.

## 12. Implementation plan

1. Quad pack/unpack in Rust plus WGSL, with CPU unit tests.
2. CPU reference mesher (binary greedy) for correctness comparison, used as the oracle for GPU output.
3. Vertex-pulling render path from a CPU-built quad pool, with frustum and face-direction culling in compute.
4. GPU binary greedy mesher with the suballocator and double-buffered remesh.
5. Edit scatter, dirty queue, and per-frame budget.
6. Sub-voxel (4 cm) meshing.
7. LOD with skirts.
8. Hi-Z two-phase occlusion and the visibility graph.
9. Profiling pass, set budgets, tune.

The CPU reference mesher is kept permanently as a test oracle and a fallback for adapters that lack required features.

## 13. Risks

- **Allocator complexity:** fragmentation and lifetime bugs in the quad pool. Mitigation: start with fixed-size pages.
- **Sub-voxel boundaries:** faces between subdivided and solid blocks are easy to get wrong. Mitigation: oracle tests.
- **Hi-Z correctness:** two-phase culling causes flicker if the pyramid is stale. Mitigation: ship after frustum and face-direction culling are stable.
- **Feature gaps:** multi-draw indirect count is not available everywhere. Mitigation: keep the looped fallback tested.
- **Shared GPU time:** no async compute queue, so meshing competes with rendering. Mitigation: the per-frame remesh budget.
- **Console port:** limits and features differ per backend. Mitigation: keep the CPU mesher and fallback paths alive.

## 14. Open questions

- Final quad bit layout, including how sub-voxel quads encode position and size.
- LOD ratios and the distance at which blocks collapse into coarser meshes.
- Whether to keep AO as a merge key or relax it.
- Quad pool size and number of pools.
- Whether foliage instancing shares the culling pass.
