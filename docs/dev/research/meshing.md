# Meshing research

The mesher is off the frame path: workers build chunks and the main thread only uploads finished
results. That makes meshing a throughput problem, not a latency one. The measured baseline below
shows where worker time goes, which of the usual voxel-meshing tricks apply to this engine, and why
binary greedy meshing is worth doing as its own slice rather than as part of the far-view work.

This is research, not a commitment. Nothing here is scheduled; the v0.0.9 milestone defers mesher
rewrites and the current greedy mesher remains the oracle for any replacement.

## Baseline

Release build, Ryzen 5 7600 (6C/12T), Windows 11, `rustc 1.99.1`, 32³ chunks with four-bit packed
blocks. Each row meshes one chunk of a 3x3 column patch with its light field; see
[Benchmark results](../benchmarks.md) for the command and the raw output.

| Fixture | LOD0 | LOD1 | LOD2 | LOD0 quads | Water pass |
|---|---:|---:|---:|---:|---:|
| Seed 42 spawn surface | 0.97 ms | 0.28 ms | 0.08 ms | 2,005 | 0.40 ms |
| Seed 42 spawn ground | 1.53 ms | 0.48 ms | 0.13 ms | 2,273 | 0.43 ms |
| Flat slab | 1.00 ms | 0.24 ms | 0.07 ms | 2 | 0.37 ms / 0.0 dry |
| All air | 0.42 ms | 0.09 ms | 0.04 ms | 0 | 0.38 ms / 0.0 dry |

The second water figure is post-change: a chunk with no water now returns an empty mesh without
scanning, which removes 0.37 ms from every land-locked chunk. The spawn surface and ground fixtures
carry water, so their water pass is unchanged.

Snapshot and border preparation costs roughly 0.03 ms per chunk now that snapshots are 16 KiB; the
pre-packing build measured 0.056 ms. A light job costs about 8.6 ms per chunk on the stress scene,
which is larger than any single mesh build. Numbers vary by 15-25% between runs; the ranking is
stable.

Three consequences:

- **The scan dominates.** Six faces x 32 slices x 32 x 32 cells is 196,608 cell visits per opaque
  build. An all-air chunk still costs 0.42 ms, so more than a third of a surface build is spent
  proving that nothing is there.
- **Water is full resolution wherever it exists.** The water pass does not follow the chunk's LOD,
  and a wet chunk still scans the full grid. Dry chunks now return before scanning and runtime only
  queues a water mesh for chunks that contain water, so land-locked chunks pay nothing.
- **Output size barely affects time.** The flat slab emits two quads for the same cost as the
  surface chunk's 2,005, so the cost is per cell, not per quad.

## Where the time goes

1. `greedy` allocates one `Vec<Option<MaskKey>>` per mesh build (1,024 entries, roughly 24-28 KiB)
   and `fill(None)`s it per slice, so the scratch is reused across all 192 face/slice passes instead
   of being reallocated.
2. Every cell calls `block_at`; `exposed` runs only when the block matches the pass, and it may
   consult the border map for edge cells.
3. Every exposed cell computes four corner keys: sky and block light maxima plus AO. This is the
   part that scales with surface area rather than volume.
4. A chunk that contains water scans the full grid at LOD0; a dry chunk returns before scanning.
5. The light job allocates `blocks` and `sky` working grids (2 × 96³ `u8`, 1.77 MB) and a
   `block_light` grid only when a lamp is present (0.88 MB more, 2.65 MB total), builds a
   9,216-entry `Vec<i32>` column ceiling (36 KiB), and makes at least three full 884,736-cell passes.

## Options

### 1. Reusable scratch and a water early-out — landed

Hoist the greedy mask out of the face loop and `fill(None)` per slice; skip the water pass when the
chunk has no water. Both are structural, not algorithmic, and keep output identical. Landed with the
v0.0.9 storage work: dry chunks build water in 0.0 ms instead of 0.37-0.38 ms, and the mask change
takes 5-10% off opaque builds (surface 0.97 → 0.91 ms, all-air 0.42 → 0.38 ms).

### 2. Skip slices and columns that cannot emit

A chunk can store its minimum and maximum non-air Y and a per-column solid flag or top/bottom. The
face loop can then skip slices outside the range and skip columns that are entirely air or entirely
buried. Cost: a few bytes per chunk, maintained on `set_block`. Expected: 10-30% of the scan on
terrain, more underground. Low risk; output is unchanged.

### 3. Binary greedy meshing with 64-bit columns

The standard technique: build occupancy bitmasks per axis, then find faces with shifts
(`col & !(col >> 1)` and `col & !(col << 1)`), iterate set bits with `trailing_zeros`, and merge runs
with `trailing_ones` instead of per-cell comparisons.

The usual objection is that 32³ chunks plus a one-cell border need 34 bits and core WGSL has no
64-bit integers. That constraint applies only to a **GPU** mesher. This mesher is CPU code, so u64
columns are free: 34 padded cells per column, one mask per column, two axes derived by transposing.

The catch is the merge key. The current mesher merges only when all four packed sky/block/AO corner
words match (`MaskKey` in `src/mesh.rs`). Bitmask extraction is fast, but the merge still has to
compare those keys, and lighting varies smoothly, so strict comparison limits quad size. Two
variants:

- **Keep the light key.** Identical output, smaller win: the per-cell corner gather remains.
- **Relax light out of the key** (the GPU-driven spec's §6.4 style: merge on material + AO, sample
  light at the quad corners after merging). Larger quads and a much simpler merge, at the cost of
  interpolating light across a merged face instead of stepping between faces. This is a look
  decision, not a correctness one, and it is cheap to measure on the CPU first.

Expected: 2-3x on opaque worker time with the relaxed key, less with the strict key. Risk is medium
because it replaces the algorithm; the existing mesher stays as the test oracle either way.

**Measured attempt (strict key).** A bitmask version was implemented and benchmarked: occupancy and
passability columns for the three axes, padded by one cell, with face extraction as a shift and a
mask. The output matched the oracle exactly (248 tests), but it was **slower** on this build:

| Fixture | Current | Bitmask | Change |
|---|---:|---:|---:|
| Spawn surface LOD0 | 1.03 ms | 1.02 ms | flat |
| Spawn ground LOD0 | 1.51 ms | 1.53 ms | flat |
| Flat slab LOD0 | 0.98 ms | 1.08 ms | +10% |
| All air LOD0 | 0.37 ms | 0.48 ms | +30% |
| Water, surface | 0.44 ms | 0.64 ms | +45% |
| Water, ground | 0.46 ms | 0.68 ms | +48% |

The build evaluates passability for every padded cell, while the current scan short-circuits the
exposure test when a cell does not match. The per-cell corner/light key and the merge dominate, so
the bitmask face scan does not pay for itself with the strict key. The change was reverted; the
existing mesher remains the baseline.

The conclusion is the one the section already predicted: bitmask extraction only pays when the merge
key is cheap. The next experiment is the **relaxed key** (merge on material and AO, sample light at
the merged quad corners), with an oracle diff and a look check, before retrying bitmasks.

### 4. One shared index buffer

Every quad uses the same four-vertex, six-index pattern, so a single static index buffer repeated to
the maximum chunk size replaces per-chunk index buffers. That removes the 12 index bytes of the
60 bytes per quad: 20% of resident geometry, with no shader or visual change. It needs a unified
index format (u32) for chunks above 65,535 vertices, which the quad pool does anyway. This is a
renderer change and belongs with the quad-format work, not with the mesher.

### 5. Flat border scratch

`BorderBlocks` is a `HashMap<IVec3, Block>` consulted for every out-of-chunk cell. A padded 34³
scratch array (39 KiB, stack or worker-local) removes hashing and bounds checks. It costs a copy per
job, like the current map, but reads become arithmetic. Worth doing only if profiling shows border
lookups matter.

### 6. Opt-in native code generation

`-C target-cpu=native` lets LLVM use AVX2 for the scans and merges. It makes binaries
machine-specific, so it must be a local profile or an environment flag, never the shipped default
or CI. Measure before and after; the mesher is branch-heavy, so the win may be small.

## What not to do

- **Switch to 16³ chunks.** The spec assumed it; this engine is 32³, and changing it touches `VCHK`,
  streaming, collision, light columns, and LOD thresholds. u64 columns remove the reason to care.
- **Store 34³ padded chunks.** It removes border lookups but adds 20% to block memory, against the
  direction of the storage work. Use a padded scratch copy per job instead.
- **Cull before meshing.** A persistent-mesh engine must have geometry ready when the camera turns;
  frustum culling is a render-time decision. Skipping mesh jobs for off-screen chunks would pop
  geometry in on every turn.
- **Move meshing to the GPU for speed.** The quad pool already removed the draw bottleneck: distance
  32 settles at 2 draws and 1.84-1.87 ms. What remains is worker time (0.8-1.4 ms per LOD0 chunk)
  and burst latency, both off the frame path. A GPU mesher would spend VRAM on voxels and light to
  move an off-thread cost, so it is not justified until a measured burst misses its latency target.
  See [GPU-driven meshing](gpu-driven-meshing.md).

## Recommendation

1. **Done in v0.0.9:** reusable scratch and the dry-water early-out.
2. Add slice/column skipping next; it is small, safe, and helps every chunk.
3. Decide the merge key before retrying bitmasks: the strict-key version measured slower, so the
   next step is the relaxed key described in option 3, with an oracle diff and a look check.
4. If the relaxed key is accepted, retry binary greedy with u64 columns and compare worker time and
   quad counts on the same fixtures.
5. Do the shared index buffer with the quad pool, not separately.

Validate with `bench_mesh_build`, the headless settle run, and a quad-by-quad comparison for the
relaxed-key experiment. Worker time is the metric; frame time only moves if a burst is waiting on
the pool.

## References

- [Binary greedy meshing](https://github.com/cgerikj/binary-greedy-meshing) - column bitmasks, shift
  face extraction, and trailing-bit merges.
- [Greedy meshing and vertex pooling](https://nickmcd.me/2021/04/04/high-performance-voxel-engine/) -
  per-orientation pools.
- [Vertex formats part 2](https://www.yosoygames.com.ar/wp/2018/03/vertex-formats-part-2-fetch-vs-pull/)
  - fetch versus pull tradeoffs.
- `docs/GPU-Driven Voxel Meshing and Rendering Spec v1.md` - the GPU-side counterpart, including the
  §6.4 AO-only merge key.
