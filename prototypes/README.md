# Prototypes

Standalone experiments for ideas that are not ready to enter the engine. Each folder is its own Cargo
package with no dependency on the `engine` crate, so a failed idea can be deleted or rewritten without
touching the renderer, the world, or the tests. They are not workspace members and are not built by
the main crate, CI, or the release process.

Both prototypes come from [Geometry representation research](../docs/dev/research/geometry-representation.md).

| Prototype | Question it answers | Run |
|---|---|---|
| [`chunk-lod-hi-z/`](chunk-lod-hi-z/) | Can a Hi-Z compute pass cull chunk AABBs and write an indirect draw list without CPU involvement? | `cargo run --release` |
| [`far-heightfield-ring/`](far-heightfield-ring/) | How much geometry does a clipmap ring need, how large are its seams, and do skirts hide them? | `cargo run --release` |

Each folder has its own README with the expected output and limitations. The Hi-Z prototype is
compute-only and verifies itself against synthetic depth scenes; it does not render terrain. The
ring prototype is CPU-only, prints a geometry and seam report, and writes a top-down `preview.ppm`.

## Results so far

**Far heightfield ring.** Five levels cover 1,024 m with 34,816 triangles (18,305 vertices), 1.66% of
a full-resolution 1 m grid over the same extent. The largest T-junction gap between a coarse edge and
the finer edges beneath it is 6.1 m and is covered by two-sided 8 m skirts (8 m up and 8 m down). The
preview shows the ring layout.

**Chunk LOD + Hi-Z.** A compute pass over 4,096 chunk AABBs culls 1,459 of the 2,815 frustum-visible
chunks (51.8%) against a synthetic 256x256 depth pyramid, including all 1,406 chunks the CPU ground
truth marks occluded. An empty depth pyramid reproduces the CPU frustum set exactly (2,815 of 2,815).
The pass writes `DrawIndexedIndirectArgs` with `first_instance = chunk_id`, so the chunk table lookup
is already in the shape ADR-0017 needs. It needs Vulkan and `INDIRECT_FIRST_INSTANCE`.

## Ground rules

- **No engine imports.** A prototype may copy a small idiom from the engine, but must build on its own.
- **Headless by default.** Verification must run without a window so it can run unattended, but
  headless is not the same as CPU-only: the Hi-Z prototype needs a Vulkan GPU adapter, rejects CPU
  adapters, and requires `INDIRECT_FIRST_INSTANCE`. A window is an optional extra, never the only way
  to see the result.
- **Deterministic.** Fixed seeds, no wall-clock, no network.
- **Delete freely.** A prototype is evidence for a decision, not a compatibility promise.

When a prototype produces a result worth keeping, record it in
[Benchmark results](../docs/dev/benchmarks.md) or the research report, then either promote the code
into the engine with tests or delete the folder.
