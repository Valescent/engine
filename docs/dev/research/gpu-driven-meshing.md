# GPU-driven meshing research

[GPU-Driven Voxel Meshing and Rendering Spec v1](<../../GPU-Driven Voxel Meshing and Rendering Spec v1.md>)
proposes a pipeline where the CPU only queues work: edits scatter into a voxel pool, compute shaders
mesh dirty chunks into a shared quad pool, compute culling compacts a draw list, and one multi-draw
indirect call renders vertex-pulled quads. This report checks that proposal against the engine as it
stands, separates the parts that pay for themselves today from the parts that do not, and records
what has to change before either can land.

It is research, not a commitment. The minimal-footprint variant and the compute mesher are
unscheduled research, not v0.0.9 scope: that milestone lands the CPU-built quad pool, and its scope
explicitly excludes the compute mesher, GPU-resident voxel and light pools, and readback-driven
allocation. The current per-chunk renderer is the fallback throughout. The companion document the
spec cites, "Voxel World Engine: Architecture v2", is not in this repository; this report is written
against the code and the recorded baseline.

No new measurements were taken for this report. Baseline numbers come from [Engine research](README.md)
and [Culling](../culling.md). Figures that are arithmetic on those numbers are marked as derived;
anything else is labelled as needing a measurement.

## Baseline it has to beat

Release build, Windows, RTX 4060, Vulkan, distance 24, vsync off, spawn view of seed 42:

| Metric | Value |
|---|---:|
| Loaded chunks | 5,289 |
| Visible chunks after frustum culling | 1,436 |
| Draw calls (opaque + water) | 1,934 |
| Visible vertices | 2,984,484 |
| CPU frame time | 2.2-2.6 ms |
| Working set | 626 MB |
| Greedy meshing, per LOD0 chunk | 0.8-1.4 ms |
| Greedy meshing, per LOD1 chunk | 0.23-0.47 ms |
| Mesh per chunk, after v0.0.8 packing | ~11 KiB |

Three consequences matter for the decision:

- **The frame has slack.** 2.2-2.6 ms of CPU against a 16.7 ms budget. Meshing is not on the frame
  path at all: it runs on the worker pool and the main thread only uploads finished meshes.
- **Draw submission was the measured cost in that baseline.** 1,934 draws, each with a chunk bind
  group and vertex and index buffer rebind, is described as the dominant CPU encoder cost in
  [Rendering](rendering.md).
- **The mesher is not a throughput cliff by itself.** 0.8-1.4 ms per LOD0 chunk is about 710-1,250
  chunks/s per worker, or ~5,700-10,000 chunks/s with eight workers busy (derived). A burst hits the
  eight-uploads-per-frame cap before it hits meshing, which is what the Part B discussion is really
  about.

## The spec is two projects

| Part | What it removes | What it adds | Risk |
|---|---|---|---|
| **A. Render path:** quad pool, vertex pulling, storage-buffer chunk info, compute culling, multi-draw indirect | Per-chunk vertex/index buffers, bind groups, origin uniforms, per-chunk draw setup | Persistent pools and a suballocator, indirect draw plumbing, a second render path | Low-medium |
| **B. GPU mesher:** compute binary greedy meshing | Worker-pool meshing and CPU quad construction | Quad-pool lifetime management, readback-driven trimming, GPU-side voxel and light data | High |

They can ship independently, and A is a prerequisite for B. Most of the spec's risk sits in B, while
most of the measured cost sits in what A removes.

### Options considered

| Option | Assessment | Outcome |
|---|---|---|
| Keep the current path | Holds 60 FPS at distance 24; draw count and per-chunk state grow linearly with distance | Baseline |
| CPU-only wins first: batch uploads, front-to-back order, water cutoff, horizon culling | Small, independent changes already listed in [Rendering](rendering.md) and [Culling](../culling.md) | Do first |
| Part A: quad pool, vertex pulling, storage-buffer chunk info, multi-draw indirect | Removes the measured bottleneck without compute or an allocator | Recommended |
| Part A plus compute culling | Adds GPU visibility and face-direction filtering, keeps the CPU mesher | Recommended next |
| Full spec, including the compute mesher | Also removes per-remesh uploads and worker load; adds pool lifetimes, GPU light data, and more VRAM | Deferred until burst latency is measured and matters |
| Minimal-footprint variant: streamed bricks and fixed quad pages | Same renderer shape without resident voxel or light pools, and without readback | Recommended for a far-view goal; unscheduled, since v0.0.9 excludes GPU pools and compute meshing |

## Part A: the render path

### Pros

- **Removes the measured bottleneck.** 1,934 draws become a handful of `multi_draw_indirect` calls.
  The per-chunk bind group (`src/renderer.rs:666`), origin uniform, and vertex/index rebinds
  disappear, which is the cost the rendering report names first.
- **Deletes per-chunk GPU state.** Each resident chunk owns an origin uniform buffer, a bind group,
  and up to four vertex/index buffer pairs, so distance 24 holds about 5,300 of each for chunks that
  are often out of view.
- **Cuts geometry memory.** Today a quad costs 48 bytes of vertex data plus a shared index buffer: four
  12-byte vertices, with six indices in one static pattern. A vertex-pulled quad costs 8-12 bytes. The
  settled distance-32 view retains 100.9 MB of vertex buffers and draws 30.1 MB; the same quads are
  17-25 MB as packed quads.
- **Removes the upload step from streaming.** A chunk is drawable once its quad range is filled and
  its draw record is visible, so "mesh finished" stops meaning "allocate buffers and copy".
- **Stops draw cost growing with distance.** Draw count tracks the compacted visible set, not the
  chunk count, which is the stated prerequisite for distance 48+ in [Culling](../culling.md).
- **Keeps every contract that is expensive to change.** Voxel storage, `VCHK`, collision, picking,
  streaming, light, and LOD stay CPU-side and untouched.
- **Aligns with the documented next step.** [Rendering](rendering.md) already recommends an indexed
  chunk-info buffer and a megabuffer with multi-draw; this is that plan with the quad format added.

### Cons

- **Device limits have to move for the compute path.** `src/renderer.rs:331` starts from
  `Limits::downlevel_webgl2_defaults()`, which sets `max_storage_buffers_per_shader_stage`,
  `max_storage_buffer_binding_size`, and `max_compute_workgroup_storage_size` to zero. When the
  adapter supports it, the quad-pool path already raises the first two to 2 bindings and the pool
  size; what stays unavailable is compute, because `max_compute_workgroup_storage_size` is still
  zero. The culling pass and the GPU mesher both need an explicit larger request. That is a
  deliberate portability decision being reversed: the WebGL2 target goes away, which the spec
  already excludes.
- **The 256 MiB buffer cap remains.** `using_resolution()` raises texture dimensions only, and the
  quad-pool path raises `max_buffer_size` only if the pool exceeds the default, so current pools stay
  under the 256 MiB cap. Larger pools need slabs under that cap or an explicit larger request plus a
  capability check.
- **Two render paths, permanently.** Adapters without `MULTI_DRAW_INDIRECT_COUNT` or
  `INDIRECT_FIRST_INSTANCE` need the current per-chunk path, kept correct and tested.
- **Debuggability drops.** There is no per-chunk buffer to inspect in a capture; a bad draw means a
  GPU capture and a pool offset, not a buffer label.
- **CI cannot see any of it.** `.github/workflows/ci.yml` runs fmt, clippy, test, doc, and typos on a
  runner with no GPU. Shader and pool correctness is manual plus whatever CPU-side unit tests cover
  the packing.
- **Shader iteration stays slow.** Shaders are embedded with `include_wgsl!` (`src/renderer.rs:990`),
  and [Shader authoring](../shaders.md) records that there is no hot reload. Every quad-layout change
  is a rebuild and restart.

## Part B: the GPU mesher

### Pros

- **Removes the per-remesh upload, which is what a burst actually hits.** `MAX_UPLOADS_PER_FRAME` is
  8 (`src/lib.rs:88`), so a 200-chunk burst needs about 25 frames to become visible, roughly 0.42 s
  at 60 FPS. The meshing is 160-280 ms of total worker CPU (derived from 0.8-1.4 ms per LOD0 chunk),
  so the upload cap, not meshing, sets that latency. Compute writes the quad range in place, so the
  cap stops applying.
- **Removes the worker-pool ceiling.** The pool has a backlog cap ([Benchmarks](../benchmarks.md)) and
  also carries generation and light jobs. An avalanche, wildfire, or fast flight competes with those
  for CPU workers and their shared scratch; compute meshing does not.
- **Frees the workers for latency-sensitive jobs.** Generation (~2.5 ms per column) and light BFS stay
  on the CPU; meshing leaves the queue entirely.
- **Makes 4 cm sub-voxels conceivable.** A subdivided 32³ chunk holds 512 M sub-voxel samples
  (32³ blocks × 25³ sub-voxels). That is not a CPU worker job, and it is the one content direction
  the spec enables that the current path cannot reach.
- **Keeps a free oracle.** The CPU mesher stays in tree as the reference and the fallback, which the
  spec requires and the test suite already relies on.

### Cons

- **No win in the steady state.** With no dirty chunks, a GPU mesher does nothing for a 2.2-2.6 ms
  frame. Its benefit is burst latency, and this engine has no bursts yet.
- **Edit latency probably gets worse.** The conservative-range allocation plus readback trim in spec
  section 6.5 is one to two frames behind. Breaking and placing a block is the interaction where that
  is felt, and the current path puts a finished mesh on screen as soon as the upload budget allows.
- **Most of that upload win is available without compute.** Raising `MAX_UPLOADS_PER_FRAME`, or writing
  many finished meshes per frame, is a small change that removes the same cap. Measure it before a
  compute mesher is justified.
- **Edits are still gated by CPU light.** `src/light.rs:151` recomputes the field and
  `src/lib.rs:1307` waits for it before the mesh job is enqueued. Unless light moves to compute or is
  uploaded per chunk, a placed block gains nothing from a faster mesher.
- **VRAM probably goes up, not down.** The quad pool is smaller than the mesh buffers it replaces, but
  the GPU now also needs the voxel data and light it currently keeps only in RAM (see below).
- **The 8-byte quad does not survive this engine's lighting.** The current vertex carries 4-bit sky,
  4-bit block, and 2-bit AO per vertex (`src/voxel.rs`). Four corners of that is 40 bits, so the quad
  needs 12 bytes unless lighting becomes flat per quad.
- **Chunks are 32³, not the spec's 16³.** Padded columns are 34 voxels, not 18, so the column bitmask
  no longer fits `u32`; greedy extents can also be 32 wide, which a 5-bit field cannot hold. Both are
  contained changes, but they are changes.
- **Worst failure mode.** Quad-pool lifetime bugs surface as flicker, stale geometry, or reads of a
  freed range. They are invisible to the Rust type system, absent from CI, and hard to reproduce.
- **No async compute.** wgpu has no separate compute queue, so meshing competes with drawing for the
  same time and still needs the per-frame remesh budget the spec describes.
- **Maintains two meshers forever.** The CPU path stays for the oracle, the fallback, and headless
  runs, so every block, lighting, or LOD change lands twice.

## Conflicts with the current code

| Spec assumption | Engine today | Consequence |
|---|---|---|
| Chunk is 16³, padded column is 18 and fits `u32` | `CHUNK_SIZE` is 32 (`src/world.rs:21`); padded column is 34 | Split columns into two 16-high slabs, or take the optional `SHADER_INT64` feature, which the spec rules out |
| Quad extents fit 5 bits | A greedy quad can span 32 cells | Store extent minus one, or widen to 6 bits |
| Quad is 8 bytes with per-corner AO | Per-vertex sky, block, and AO (`src/voxel.rs`) | 12-byte quad, or flat per-quad light |
| CPU does orchestration only | Light BFS is CPU work and gates the mesh job (`src/lib.rs:1307`) | Upload the light field per chunk, or port the BFS to compute |
| Compute and storage buffers are core | The quad-pool path raises storage-buffer limits to 2 bindings and the pool size when supported; `max_compute_workgroup_storage_size` stays zero (`src/renderer.rs:331`) | Request the compute limits and drop the WebGL2 target, or there is no compute at all |
| Pools split only past `max_storage_buffer_binding_size` | The render path raises the binding limit to the pool size and keeps `max_buffer_size` at 256 MiB or the pool size, whichever is larger | Slab larger pools or check the adapter limit before enabling the path |
| `first_instance` indexes chunk info | There are no indirect draws; each chunk has a bind group and an origin uniform | `INDIRECT_FIRST_INSTANCE` becomes required; without it the fallback is the current path |
| World is a 2²¹ m torus with wrapping `i32` chunk coordinates | Coordinates are plain `i32` with `checked_mul` (`src/world.rs:571`); nothing wraps | Wrapping, camera-relative offsets, and seam lookups are net-new and touch `VCHK`, streaming, and light |
| Near-field sun shadows are already decided | The engine has no shadow maps | No conflict today; the mesher and culling must account for a shadow pass when they land |
| Sub-voxels are 4 cm, 25³ per block | Blocks are 1 m and there is no subdivision model | Sub-voxels are new world data and new save semantics, not just mesher work |
| Transparency is bucketed and sorted by chunk | Water is a separate full-resolution mesh drawn unsorted | The pool needs a transparent range or a second pool, plus a sort |

## Quad bit budget

Two layouts fit the engine's needs. The 8-byte form only works if lighting becomes flat per quad.

| Field | 8-byte, flat light | 12-byte, per-vertex light |
|---|---:|---:|
| Position in chunk, 3 × 5 bits | 15 | 15 |
| Sub-voxel offset, 3 × 5 bits (subdivided quads only) | 15 | 15 |
| Extent minus one, 2 × 5 bits | 10 | 10 |
| Face direction | 3 | 3 |
| Material ID | 12 | 12 |
| Scale flag (block or sub-voxel) | 1 | 1 |
| AO, 2 bits per corner | 8 | - |
| Sky, block, AO, 10 bits per corner | - | 40 |
| **Total** | **64 of 64** | **96 of 96** |

Both fit exactly once the sub-voxel offset is included, which is why material is capped at 12 bits; a
16-bit material would need 68 and 100 bits. The choice is really a lighting decision, and the spec's
open questions do not list it.

## Memory at distance 24

Measured from the windowed run in [Benchmarks](../benchmarks.md); the voxel and light pools do not
exist today.

| Store | Size | Basis |
|---|---:|---|
| Vertex buffers, visible | 22.5 MB | 497k quads drawn, 48 B per vertex quad plus the shared index buffer |
| Vertex buffers, resident | 83.3 MB | every retained LOD plus water |
| Vertex buffers, resident (distance 32) | 100.9 MB | 2.10M quads retained, measured |
| Vertex buffers, visible (distance 32) | 30.1 MB | measured |
| Quad pool, 8 B, visible | 4.0 MB | 497k quads |
| Quad pool, 8 B, resident | 11.8 MB | 1.48M quads at distance 24 |
| Quad pool, 12 B, resident | 17.8 MB | same count |
| Voxel pool, 1 byte per block | ~169 MB | 5,289 chunks × 32 KiB |
| Voxel pool, 4-bit palette | ~85 MB | crude estimate; palette compression is data-dependent |
| Light pool, two nibble planes, block plane sparse | ~100 MB | 5,289 chunks × 19 KiB typical |

So the render half is a clear memory win, and the mesher half is a memory cost unless the palette
compression is good. The spec's section 10 leaves this split open; it should carry a number before
the voxel pool is built.

## Pre-pool measured renderer baseline and wgpu facts

Settled windowed run before the quad pool, seed 42 spawn, vsync off, from
[Benchmarks](../benchmarks.md):

| Distance | Chunks | Visible | Draws | Indices drawn | Geometry | Visible geometry | Frame CPU |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 24 | 5,289 | 1,436 | 1,934 | 2.98 M | 89.3 MB | 28.5 MB | 2.24-2.39 ms |
| 32 | 9,285 | 2,435 | 3,409 | 3.94 M | 125.9 MB | 37.6 MB | 9.06 ms |

Two readings changed the plan:

- **The resident/visible gap is frustum culling, not duplicate LODs.** A static camera meshes each
  chunk once at its desired LOD, so 126 MB resident against 37.6 MB visible is simply the chunks
  behind the camera. Stale LOD slots are still retained across transitions and grow without bound
  when the player moves; pruning them is a cheap fix, not a 60% saving on the static baseline.
- **Draws grew 76% from distance 24 to 32 but CPU time grew 3.8x.** Some of the 9 ms is not the draw
  loop. Profile the record step against submit/upload before assuming a megabuffer captures all of
  it.

Verified against the pinned `wgpu-types` and `wgpu-hal` 30.0.2 source:

| Assumption | Result |
|---|---|
| `Features::MULTI_DRAW_INDIRECT` exists | No; the base call is gated by `DownlevelFlags::INDIRECT_EXECUTION` |
| `MULTI_DRAW_INDIRECT_COUNT` on desktop | Vulkan and DX12 only; the Metal backend's count entry points are `//TODO` stubs |
| `INDIRECT_FIRST_INSTANCE` needed for chunk ids | Needed for non-zero `first_instance` in indirect args; direct `draw_indexed` accepts it without the feature |
| Indirect offset alignment | 4 bytes, not 256; `DrawIndexedIndirectArgs` is 20 bytes, tightly packed |
| Uniform dynamic offsets for chunk origins | 64 KiB binding and 256-byte alignment cap this at 256 chunks per bind group |
| `IMMEDIATES` for per-chunk data | Exists, but `max_immediate_size` defaults to 0 and must be requested |

Cheap wins found independently of the spec, and their status:

| Win | Status |
|---|---|
| Cache resident geometry bytes instead of summing every frame | Landed |
| One traversal and one frustum test for opaque and water | Landed |
| Prune non-active LOD slots after the replacement is resident | Landed |
| Skip water meshing for chunks with no water | Landed |
| Reuse the greedy mask allocation | Landed |
| Lazy block-light grid and a flat ceiling array in the light job | Landed |
| Budgeted, deterministic level work and selection on upload | Landed |
| One shared static index buffer | Landed |
| Quad pool with 12-byte records (ADR-0015) | Landed: 2 draws, 37.6 MiB, 1.9 ms at distance 32 |

Measured effect of the landed items: the settled distance-32 frame fell from 9.06 ms to 4.30-4.35 ms
on the reference machine, resident geometry fell from 125.9 MB to 100.9 MB, and visible geometry from
37.6 MB to 30.1 MB; draw count stayed at 3,409. The quad pool then took it to 2 draws, 37.6 MiB
resident, 7.52 MiB visible, and 1.84-1.87 ms. Distance 24 stayed at 2.3-2.4 ms, where draw
submission still dominates. Headless settle time fell from 15.1 s to 11.7 s at distance 24 and from
29.1 s to 21.6 s at 32. The stale-LOD pruning matters when moving, not at rest: a static camera
builds each chunk once.

**Decision update:** go straight to the quad pool plus indirect draws rather than a 12-byte-vertex
megabuffer first. The 40 MB target needs the 8-12 byte quad record, and a quad pool removes the
per-chunk index-format split; a vertex megabuffer would only carry over the chunk-info and batching
plumbing. The two stages in [Staging](#staging) can therefore be merged.

Beyond that pool, the next wins are fewer drawn quads rather than a different representation: chunk
LOD with Hi-Z culling, a far heightfield ring, and 6-8 byte records. See
[Geometry representation](geometry-representation.md).

## Minimal-footprint variant

The spec's resident voxel and light pools are the wrong budget profile for a far-view goal: they add
185-269 MB of VRAM before a single extra metre is visible. A variant keeps the same renderer shape
without resident pools.

**Stream bricks, do not pool them.** A small ring of slots holds padded `34³` block and light data
for the chunks currently being meshed. At one byte per block and a nibble per light cell that is about
59 KiB per slot, or 78 KiB near a lamp; 16-64 slots is 1-5 MB. The CPU already builds a padded
`ChunkSnapshot` per job, so this is the same copy plus the light bytes. Startup streams 5,289 chunks
once, roughly 310 MB of transfer, and then the ring is reused.

**Page the quads.** Average chunk geometry is ~188 quads (11 KiB at 60 B per quad, derived), so two
page classes, 256 and 1024 quads, cover most chunks. Surface and ground chunks do not: they emit
about 2,005 and 2,273 quads, so a chunk reserves a chain sized from its previous count plus one page
of headroom, and those classes take three or four 1,024-quad pages (3,072-4,096 quads). A remesh
takes a free chain, the chunk record swaps to it, and the old chain returns to the free list after
the frames in flight that could read it. If a dispatch still overruns its chain, the chunk keeps its
previous mesh and falls back to the per-chunk path for that frame, so an unusually dense chunk costs
a frame rather than a corrupted draw. No suballocator, no fragmentation beyond the class rounding,
and no readback: a fixed chain lets the dispatch write its own indirect count, so the trim step in
spec section 6.5 disappears along with the 1-2 frame latency it causes.

| | Resident pools (spec) | Minimal variant |
|---|---:|---:|
| VRAM for voxels and light | 185-269 MB | 1-5 MB ring |
| VRAM for geometry | 12-30 MB | 12-25 MB with one active LOD per chunk |
| Readback | required for trimming | none; fixed pages and GPU-written counts |
| Upload per remesh | quads only | padded brick (~59 KiB) |
| Edit latency | 1-2 frames | one page swap |

The costs are real: a padded brick upload per remesh, a dispatch per batch, three or four pages for
surface chunks, and the same fallback path. It is also the only variant that keeps an
integrated GPU from duplicating the world in shared memory.

For distance, the variant is paired with the seed-native hierarchy in
[ADR-0014](../../decisions/0014-seed-native-lod-hierarchy.md): coarse sections keep a `32³` sample
budget while covering 8x more volume per level, so section count and draw count grow with the
logarithm of distance instead of its area. That combination is the eventual far-view direction, not
this milestone: [v0.0.9 Far View](../v0.0.9-scope.md) lands the CPU-built quad pool and the hierarchy
without GPU voxel/light pools or a compute mesher.

## Staging

Each step has a gate and a stop point. Steps 1-3 are useful even if the compute mesher is never built.

0. **Measure first.** Add a `mesh_build` bench beside `benches/pool_sim.rs` for LOD0/1/2 on fixed
   snapshots, and record the peak dirty-chunk count in the stress scene. The 0.8-1.4 ms LOD0 range is
   the current fixture measurement; the burst size is not recorded at all. Then raise
   `MAX_UPLOADS_PER_FRAME` and re-run the burst, because that is the cheapest experiment and it sets
   the bar the compute mesher must beat.
1. **Quad pack and unpack** in Rust, mirrored in WGSL, unit-tested, with the CPU greedy mesher emitting
   quads that pack and unpack losslessly. Gate: quad counts, covered area, and material histograms
   match `mesh.rs` on fixed chunks.
2. **Storage-buffer chunk info and a megabuffer** of CPU-built vertices, one indirect call, current
   per-chunk path retained. Gate: identical pixels, `draw_calls` under 20, no `frame_ms` regression.
   This is the smallest change that removes the measured bottleneck.
3. **Vertex pulling** from a CPU-built quad pool. Gate: GPU time within roughly 10% of step 2 and lower
   resident VRAM.
4. **Compute culling:** frustum, face direction, empty skip, one compaction pass. Gate: `frame_ms` equal
   or better, culling under 0.2 ms.
5. **Compute mesher** with fixed-size pages, double-buffered ranges, and readback trimming, after the
   light question in the next section is answered. Gate: a 200-chunk burst settles in fewer frames than
   the raised-upload CPU baseline from step 0, edit-to-visible latency within one frame of today, and an
   oracle diff against the CPU mesher is clean.
6. **Only then** LOD skirts, Hi-Z two-phase occlusion, and sub-voxel meshing.

Stop points: after step 3 if draws and memory were the only problems; after step 4 if dirty-chunk
throughput is acceptable in play.

## Validation

- Same method as the rest of the research set: seed 42, distance 24, vsync off, foreground window, CPU
  frame times rather than FPS.
- Metrics already in the engine: `frame_ms`, `gpu_ms`, `draw_calls`, `visible_vertices`, `rebuild_ms`,
  `upload_ms`, working set, and the once-per-second debug line. Add a `mesh_burst_settle_frames` plot
  for step 5.
- Packing and culling math get CPU unit tests. Oracle comparisons run on a machine with a GPU behind an
  environment variable, because [Conventions](../conventions.md) requires the default suite to run
  without a GPU or a window.
- Shader compilation is validated at startup today; there is no automated WGSL test harness, so any
  layout change needs a manual capture until one exists.
- Record real results in [Benchmarks](../benchmarks.md) when a step lands.

## Decision points

These need answers from the owner, not from more research:

1. **Is the WebGL2-level limit request still a goal?** If yes, every compute pass is out, including
   culling and the GPU mesher, and the render path has to stay on vertex buffers and `draw_indirect`
   only.
2. **Does lighting stay per vertex?** If yes, the quad is 12 bytes and the pool grows 50%. If no, flat
   per-quad lighting is a visible quality change to accept in the same decision.
3. **Where does light live after an edit?** Upload the CPU field as a storage buffer (simple, ~100 MB
   at distance 24, still CPU-BFS-bound), or port the BFS to compute (large, removes the gate).
4. **Chunk size stays 32³?** Recommended, with split columns. Changing it would touch `VCHK`,
   streaming, collision, and light for no measured gain.
5. **Is the 2²¹ m wrapping world real?** If it is a later milestone, keep the mesher and shader
   camera-relative now so the pools do not need rewriting.
6. **What content needs bulk remeshing, and what does raising the upload cap buy?** Without an answer,
   step 5 has no acceptance test that is not synthetic.

## Recommendation

Build the render half, defer the mesher half.

The measured problems are draw count and per-chunk GPU state, and step 2 fixes both without compute,
without an allocator, and without a quad format decision. Steps 3 and 4 then deliver the spec's
visibility goals at low risk. Bursts are limited today by the upload cap, which is a small change to
measure first; if that is not enough, and bulk simulation or sub-voxels arrive, the compute mesher
starts to earn its risk. Until then the CPU light gate means an edit would not even become visible
faster.

If the mesher is built anyway, answer decision points 1-3 first. Building it on top of a solved quad
format, a batched draw path, and a GPU-resident light field is a bounded project. Building it first
means solving all of those while also debugging pool lifetimes.

For a far-view goal the order is different again: take the
[minimal-footprint variant](#minimal-footprint-variant) and the seed-native hierarchy, and skip the
resident pools entirely. That combination is unscheduled research; v0.0.9's far view is the CPU
hierarchy plus the quad pool, without GPU pools or a compute mesher.

## References

- [GPU-Driven Voxel Meshing and Rendering Spec v1](<../../GPU-Driven Voxel Meshing and Rendering Spec v1.md>)
  - the proposal under review.
- [Rendering](rendering.md) - megabuffer, chunk-info buffer, and multi-draw recommendation.
- [Culling](../culling.md) - the current culling stack and the GPU-driven endgame.
- [Memory](memory.md) - per-chunk memory and the v0.0.8 packing results.
- [wgpu `multi_draw_indexed_indirect`](https://docs.rs/wgpu/latest/wgpu/struct.RenderPass.html).
- [Binary greedy meshing](https://github.com/cgerikj/binary-greedy-meshing) - the compute algorithm the
  spec adapts.
- [Aokana: GPU-driven voxel rendering](https://arxiv.org/abs/2505.02017) - Hi-Z and visibility for voxel
  worlds.
