# Why I Chose a Quad Pool with Indirect Draws

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted; implemented in v0.0.9 slice 4 (2 draws, 37.6 MiB, 1.9 ms)  
**Date:** 2026-11-19  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None. Extends ADR-0010 (per-chunk LOD), ADR-0014 (long-range LOD), and the megabuffer recommendation in the rendering research.  
**Scope:** The chunk render path: geometry representation, pool, chunk table, and submission. It does not decide the meshing algorithm, GPU meshing, culling passes, shadows, or sub-voxels.

---

## Summary

This document records why the engine replaces per-chunk vertex and index buffers with one shared
pool of 12-byte quad records, vertex pulling, and indirect draws. The short version: measured resident
geometry is 125.9 MB and 3,409 draws cost 4.3 ms of CPU at distance 32; a quad pool projects to about
37.6 MiB and at most a handful of draws, while preserving per-corner light and keeping the current
renderer as a fallback.

---

## 1. Context

### 1.1 The measured baseline

Settled windowed run, seed 42 spawn, distance 32, after the v0.0.9 storage and CPU work:

| Metric | Value |
|---|---:|
| Loaded chunks | 9,285 |
| Visible chunks | 2,435 |
| Draw calls | 3,409 |
| Indices drawn | 3.94 M |
| Resident geometry | 100.9 MB (125.9 before shared index buffers) |
| Visible geometry | 30.1 MB |
| Frame CPU | 4.30-4.35 ms |

Each quad costs 60 bytes: four 12-byte vertices plus six 16-bit indices. Every resident chunk owns an
origin uniform, a bind group, and up to four vertex/index buffer pairs. Every draw rebinds all of it.

### 1.2 The constraints

- Per-corner sky light, block light, and AO must look the same as today; light is part of the greedy
  merge key and interpolates across merged quads.
- Native desktop only: Vulkan, DX12, Metal. No WebGL2.
- The fallback path must keep working when a feature is missing.
- The 256 MiB default buffer limit and 128 MiB storage binding limit apply unless raised.
- The CPU mesher stays the reference and the oracle.
- Water keeps its own pipeline and blend state.
- Headless runs and the default test suite must not need a GPU.

---

## 2. Decision

**Chunk geometry is stored as 12-byte quad records in a shared pool, pulled by the vertex shader,
and submitted with indirect draws.**

### 2.1 Quad record

Three `u32` words per quad:

| Word | Fields |
|---|---|
| W0 | `x:5 y:5 z:5 width-1:5 height-1:5 face:3 flags:4` |
| W1 | `layer:8 ao0:2 ao1:2 ao2:2 ao3:2 c0 sky:4 block:4 c1 sky:4 block:4` |
| W2 | `c2 sky:4 block:4 c3 sky:4 block:4` + 16 spare |

- Positions and extents are chunk-local cells, extents stored minus one so a full 32-wide quad fits.
- `face` is the six-direction index. `flags` carry water, sub-voxel, and two spare bits.
- `layer` is the face texture layer, resolved by the mesher as today. A material table is deferred
  until mods need more than eight bits.
- Corners pack exactly like `pack_light_ao`: 4-bit sky, 4-bit block, 2-bit AO, in the mesher's corner
  order.
- Six vertex invocations per quad, corner ids `[0, 1, 2, 0, 2, 3]`, no index buffer.

The mesher emits the record alongside the vertex mesh as of v0.0.9; the renderer still draws the
vertices until the pool lands. `src/quad.rs` owns the pack/unpack and the CPU mirror of the corner
rule that the shader will use.

The 8-byte variant in the GPU-driven spec is deferred: it requires flat per-quad light or a second
light buffer, and it saves 8 MB against a 40 MB target that 12 bytes already meets.

### 2.2 Pool

- One flat array of 12-byte records, divided into pages of 64 quads (768 bytes).
- A chunk reserves a contiguous run of pages sized to its quad count; a surface chunk of about 2,000
  quads takes eight pages. The run is first-fit from a coalescing free list keyed by first page, so a
  chunk address is just `first_quad + vertex_index / 6`.
- Freed runs wait two frames in a deferred list, then coalesce into the free list. A remesh allocates
  the new run before freeing the old one, so a transition cannot read freed data.
- Water uses the same pool with a separate range in the chunk record.
- Capacity is a fixed budget of 3.2M quads (38.4 MB). A chunk that does not fit falls back to the
  per-chunk path; growth by reallocation is a later decision.
- No readback: the CPU mesher knows the quad count, so it writes the draw arguments directly.

### 2.3 Chunk table

A storage buffer of 32-byte records: `origin: [f32; 4]` (chunk minimum corner plus LOD scale),
`opaque_first`, `opaque_quads`, `water_first`, and `water_quads`. The vertex shader indexes it by the
draw's `first_instance` (or by `@builtin(instance_index)` when `first_instance` is unavailable). The
range fields index the flat quad array; a chunk's range is contiguous and page-aligned. The camera
subtracts its own position on the CPU before the origin enters the table, which keeps the shader in
small f32 values.

### 2.4 Submission

- The CPU frustum-culls visible chunks and writes one non-indexed indirect record per chunk:
  `vertex_count = 6 * quads`, `instance_count = 1`, `first_vertex = 0`, `first_instance = slot`.
- One `multi_draw_indirect` for opaque and one for water. Indirect arguments are 16 bytes each;
  offsets need 4-byte alignment.
- `INDIRECT_FIRST_INSTANCE` is requested. Without it, the fallback draws each chunk from the pool
  with the same vertex shader and a per-draw slot uniform, which still removes the per-chunk vertex
  and index buffers.
- `MULTI_DRAW_INDIRECT_COUNT` is not required for this slice. It is Vulkan/DX12 only (Metal's entry
  points are stubs) and is deferred to GPU culling.
- `multi_draw_indirect` is gated by `DownlevelFlags::INDIRECT_EXECUTION`, present on the desktop
  backends.

### 2.5 Limits and fallback

The device requests storage buffers, `INDIRECT_FIRST_INSTANCE`, and the adapter's buffer limits. This
drops the WebGL2-level request that has constrained the renderer until now. The current per-chunk
path stays behind a capability check and keeps passing its tests; `ENGINE_LEGACY_RENDER=1` forces it
for A/B runs.

This decision does not include the compute mesher, a resident voxel or light pool, GPU culling, or
sub-voxels. Those remain separate decisions.

---

## 3. Why this shape

### 3.1 It is the only measured way to reach the geometry target

125.9 MB was 2.1 M quads at 60 bytes. Shared index buffers already removed 25 MB, leaving 100.9 MB at
48 bytes per quad. Twelve bytes per quad puts the same quads in 25.2 MB, and 8 bytes in 16.8 MB. No
vertex compression short of removing the vertex buffer reaches 40 MB.

### 3.2 It removes the submission cost, not just bytes

3,409 draws at 4.3 ms is roughly 1.3 microseconds of frame time per draw, most of it encoder work.
One indirect call per pass replaces 3,409 bind-group and buffer rebinds. Even if the pool were free,
the draw count would still need this change.

### 3.3 It keeps the lighting contract

Per-corner light and AO fit in 12 bytes. The alternative, 8 bytes, forces flat per-quad light or a
second light stream. The engine ships smooth lighting; the decision protects it, and 37.6 MiB is inside
the budget.

### 3.4 It unifies water and removes index formats

Water becomes a range in the same pool, not a second set of buffers. With no index buffers there is
no 16/32-bit split and no per-chunk index allocation; the one shared u16 pattern remains only for the
fallback path until it is retired.

### 3.5 It is the substrate for later work

Chunk LOD, Hi-Z culling, a compute mesher, and the far-field ring all write or read the same pool and
chunk table. Choosing a quad pool now means those later changes replace the producer, not the renderer.

### 3.6 Why not a vertex megabuffer first

A megabuffer of 12-byte vertices would keep 48 bytes per quad of vertices plus index bytes and land
near 100 MB with index sharing, still over target. It would carry over the chunk table and batching
plumbing but be replaced as soon as quads landed. The plumbing is small enough to build once, in the
final format.

### 3.7 Why not GPU meshing or compute culling now

Meshing is off the frame path and the frame is draw-bound. A compute mesher adds voxel and light VRAM
and pool lifetimes to fix a problem the engine does not have; the analysis is in the GPU-driven report.
Compute culling needs the pool and chunk table first.

---

## 4. Alternatives considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **12-byte quad pool + indirect** | Meets the memory target, removes draw setup, keeps lighting | **Selected** |
| Shared index buffer only | 20% memory, no draw reduction, already landed at 100.9 MB | Landed as a step |
| 12-byte vertex megabuffer | 60 bytes per quad, misses 40 MB, index format split | Not selected |
| 8-byte flat-light quad | 17 MB but changes shading | Deferred |
| Dynamic uniform offsets for origins | 64 KiB binding, 256-byte alignment, 256 chunks per bind group | Rejected |
| Compute mesher with resident voxel pool | Adds 170-255 MB VRAM, fixes a non-bottleneck | Deferred |
| GPU compute culling now | Needs the pool first; `MULTI_DRAW_INDIRECT_COUNT` absent on Metal | Deferred |
| Per-chunk draws from the pool | Keeps vertex pulling but not the batching win | Fallback |

---

## 5. Counterarguments and disposition

| Counterargument | Disposition |
|---|---|
| **Vertex pulling re-reads each quad six times.** | Accepted. Each invocation reads 12 bytes; caches make it bandwidth-neutral against 48 bytes of vertices, and it removes the index fetch. The prototype and the slice A/B both measure it. |
| **Pool lifetime bugs are the worst failure mode.** | Accepted with qualification. Fixed pages, a free list, and a two-frame age remove fragmentation and most lifetime bugs; the fallback path remains. |
| **12 bytes costs 8 MB over the 8-byte form.** | Accepted. The target is 40 MB; preserving smooth lighting is worth 8 MB. |
| **`first_instance` is not universal.** | Accepted. The fallback keeps per-chunk draws from the same pool; only the batching is lost. |
| **Page rounding wastes space.** | Accepted. Pages are 64 quads, so a chunk wastes at most 63 quads; the slice measures the total against the 40 MB gate. |
| **Indirect draws add a CPU args upload.** | Accepted. About 50 KB per frame at 2,500 visible chunks; one write. |
| **The 256 MiB buffer cap could bite later.** | Accepted with qualification. 38.4 MB now; slabs or a raised limit when the pool grows. |
| **The fallback doubles the render paths.** | Accepted. It is required for Metal feature gaps and for regression comparison. |

---

## 6. Consequences

### 6.1 Positive

- Resident geometry measured 37.6 MiB (38.4 MB reserved), visible 7.52 MiB.
- Draw calls collapse from 3,409 to 2 in the measured run.
- Water and opaque share one pool; no per-chunk index buffers or index format selection.
- The chunk table and pages are the substrate for LOD, Hi-Z, and a compute mesher.
- Per-corner light and AO survive unchanged.

### 6.2 Negative

- The renderer gains a pool, a chunk table, and indirect argument building.
- The vertex shader gains bit unpacking and six invocations per quad.
- Metal loses count draws; the fallback path must stay correct.
- The existing per-chunk path remains until the fallback is retired.

### 6.3 Neutral

- The CPU mesher becomes a quad producer; its greedy algorithm is unchanged.
- `VCHK`, lighting, streaming, and collision are untouched.
- Water is still unsorted.

---

## 7. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Quad bit layout is wrong or lossy | Rust pack/unpack, WGSL mirror, and CPU tests; compare against the existing mesher |
| Pool lifetime bug corrupts geometry | Fixed pages, two-frame age, page swap; oracle tests and a debug validator |
| Vertex pulling is slower on a target GPU | A/B the slice against the shared-index path at distance 32; keep the fallback |
| Indirect draws unsupported or emulated | `INDIRECT_EXECUTION` check; per-chunk fallback; Metal count draws stay off |
| Page rounding exceeds the budget | 64-quad pages bound waste to 63 quads per chunk; the slice measured 37.6 MiB |
| Light interpolation changes | Keep the corner packing exact; compare rendered frames and vertex values |
| Buffer or binding limits | 38.4 MB pool under 256 MiB; chunk table under 128 MiB; slab later |

---

## 8. Conclusion

The 12-byte quad pool with vertex pulling and indirect draws is the deliberate, measurement-gated
choice for chunk geometry. It is selected because it is the only measured path to the 40 MB and
ten-draw targets, it preserves per-corner lighting, it removes both the vertex and index buffers, and
it is the substrate for the LOD and culling work that follows. I acknowledge the pool-complexity,
vertex-pull bandwidth, and Metal feature gaps, and manage them with fixed pages, an A/B against the
current path, and a permanent fallback. The exact bit layout, page size, and table record are fixed here; the
implementation is accepted when v0.0.9 slice 4 passes its gate.

---

*This entry is part of the Valescent Engine decision log. Future work that changes chunk geometry,
submission, or the fallback should reference this document and record a superseding decision.*
