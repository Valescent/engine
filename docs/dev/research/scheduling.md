# Scheduling and streaming research

The worker pool has been the engine's main scaling tool since v0.0.5. With generation, light, and
mesh jobs now sharing it, the scheduling policy matters as much as the algorithms.

## Current state

- **Pool:** micropool, one thread per core (11 on the reference machine), a results channel, and an
  admission cap of four jobs per worker (44 in flight). Background jobs are dropped when full;
  urgent jobs are always admitted.
- **Priorities:** urgent is used for edits and the initial mesh pass; everything else is background.
- **Frame budget:** at most two columns committed per frame and at most two meshes uploaded per
  frame. Light and generation results are drained fully.
- **Job size:** generation is one column (four chunks, ~2.5 ms), light is one chunk with a 3x3
  footprint (864 KiB of snapshots plus a 2.65 MB working grid), mesh is one chunk and LOD.
- **Main-thread work:** building light and mesh snapshots, committing generation results, uploading
  meshes, and the level work set.

Measured during a distance-24 load: 125-150 generated columns per second, with a 1.8-2.1 ms frame
once the ring is full.

## Observations

1. A light job allocates ~3.5 MB of scratch. With 44 jobs in flight that is up to ~150 MB of
   transient memory and a lot of allocator traffic. The BFS itself is fast; the setup dominates.
2. Every column round-trips through the main thread: generation completes, the frame inserts
   chunks, the next frame builds light snapshots, then meshes. Three hops per column.
3. Mesh uploads are capped at two per frame, but light and generation results are not paced. A
   burst can make one frame do all of the commit work.
4. Job priority is binary. Near chunks and far chunks compete equally, so the first ring can fill
   slower than it needs to while the camera is moving.
5. Saves are synchronous on the main thread (see [Memory](memory.md)).

## Options

### 1. Worker-local scratch

Give each worker a reusable light grid and a reusable snapshot buffer, cleared per job. This removes
almost all light-job allocation without changing results. Cheap to implement, low risk, and directly
measurable as allocator time and working set.

### 2. Chain generation and light on the worker

Have a generation job compute light for its four chunks before returning, or enqueue the light jobs
from the worker. That removes one main-thread hop and keeps the pool busy without waiting for the
next frame. The main thread still owns inserting chunks into `World`.

### 3. Column jobs

Treat generation, light, and first mesh as one job per column, or as a small pipeline inside the
pool. Fewer round trips, better locality, and a natural unit for prefetch prioritisation. The cost
is a coarser admission unit: a column job is several times longer than a chunk job, so the tail
latency grows.

### 4. Distance and screen priority

Sort background requests by distance (and optionally whether the column is in the forward
half-plane, which streaming already does). Mesh light and mesh jobs for the near ring first. A small
priority queue per class, or simply requesting the near columns before the far ones, is enough.

### 5. Adaptive upload budget

Scale the mesh upload cap with frame time: allow more uploads when the frame is under budget and
fewer when it is not. This smooths the load phase without a fixed two-per-frame ceiling. Keep a hard
cap for worst-case frames.

### 6. Velocity prefetch

Extend the streaming order with the player's horizontal velocity so columns ahead of movement enter
the queue first. The forward half-plane rule is a coarse version of this; weighting by velocity is
more accurate during strafing and turning.

### 7. Async I/O

Move `load_saved_chunk` on the generation path and `save_chunk` on unload to an I/O thread with a
queue. Chunk loads are small; the win is avoiding filesystem stalls on synced or slow disks. Pair
with atomic temp-file writes.

### 8. Pool configuration

Expose the worker count (environment variable or config) and keep the micropool/rayon A/B path.
The ADR already records micropool as the default; a configurable count helps low-core machines and
profiling.

## Recommendation

1. Worker-local light scratch. Lowest effort, removes the largest hidden allocation.
2. Chain light into the generation job. Big latency win for the first ring.
3. Distance priority for light and mesh requests.
4. Adaptive upload budget with a hard cap.
5. Async I/O for loads and saves.
6. Column jobs only if the chained pipeline is measured to be too chatty.

## Measurement

- Add queue metrics: in-flight count, per-class depth, completed jobs per second, and worker
  utilization. `metrics.rs` already has plots for queued jobs; extend them.
- Time the load phase from first generation to last ready column at distances 12 and 24.
- Watch `frame_ms` variance during load, not just the mean.

## References

- [ADR-0009: micropool for chunk workers](../../decisions/0009-micropool-for-chunk-workers.md) — the pool decision and benchmark methodology.
- [Benchmark results](../benchmarks.md) — the load and frame numbers this report uses.
- [Culling](../culling.md) — hierarchical traversal gives a natural priority order.
