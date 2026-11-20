# Engine research

These reports look beyond the current milestone at changes that would make the engine faster, smaller,
or easier to verify. Each one states the measured baseline, the options, their cost and risk, a
recommendation, and how to validate it. They are research, not commitments: nothing here is
scheduled until it lands in a scope document.

## Baseline

Release build, Windows, RTX 4060, Vulkan, 1280x720, `ENGINE_NO_VSYNC=1`:

| Scene | Chunks | Working set | CPU frame | Draws | Vertices |
|---|---:|---:|---:|---:|---:|
| `render_distance_chunks = 12` | 1,434 | 295 MB | 0.8 ms | - | - |
| `render_distance_chunks = 24` | 5,289 | 626 MB | 2.2-2.6 ms | 1,934 | 2.98 M |

Per stored chunk: 16 KiB of four-bit packed blocks, about 19 KiB of packed light (the block-light
plane is omitted when no lamp is in range), and roughly 11 KiB of mesh after the v0.0.8 vertex
packing, plus GPU bookkeeping. Generation costs about 2.5 ms per column
(four chunks). Greedy meshing measured 0.8-1.5 ms per LOD0 chunk, 0.23-0.47 ms per LOD1, and
0.05-0.13 ms per LOD2 at v0.0.9, plus a 0.36-0.45 ms water pass per chunk; the earlier 0.48/0.10 ms
figures came from the v0.0.5 stress scene and are not comparable. See
[Benchmark results](../benchmarks.md).

## Reports

| Report | Headline opportunity | Expected effect | Effort | Risk |
|---|---|---|---|---|
| [Memory](memory.md) | Nibble block IDs and compressed saves (indexed, packed vertices landed in v0.0.8) | 30-50% of resident memory | Medium | Low |
| [Rendering](rendering.md) | One origin buffer and batched indirect draws (indexed, packed vertices landed in v0.0.8) | Fewer bytes per quad, fewer state changes | Medium | Low-medium |
| [GPU-driven meshing](gpu-driven-meshing.md) | Quad pool and vertex pulling without resident voxel pools; compute meshing deferred | Fewer draws and bytes per quad; distance via the LOD hierarchy | Medium-high | Medium |
| [Meshing](meshing.md) | Binary greedy with 64-bit columns, scratch reuse, dry-water skip | 2-3x worker time; frame time unchanged | Medium | Medium |
| [Geometry representation](geometry-representation.md) | Chunk LOD + Hi-Z, far heightfield ring, 6-8 B records | Half the quad-pool memory; fewer distant quads | Medium-high | Medium |
| [Scheduling](scheduling.md) | Reusable light scratch and worker-side chaining | Less allocation and fewer round trips | Medium | Low |
| [World generation](worldgen.md) | Two-phase features and cached 2D fields | Structures and cheaper generation | Medium-high | Medium |
| [Simulation and tooling](simulation-and-tooling.md) | Fixed timestep, headless runner, perf gates | Determinism and regressions caught early | Low-medium | Low |

## Method

- Prefer changes with a measurable metric already in the engine: `frame_ms`, `rebuild_ms`,
  `upload_ms`, `generation_ms`, working set, draw calls, and vertex counts.
- Measure on the same seed, camera, and render distance. Windows throttles covered windows, so keep
  the window foregrounded and compare CPU frame times rather than raw FPS.
- Any change that touches `VCHK`, light, or the mesher needs a headless test before a live run.
- Record results in [Benchmark results](../benchmarks.md) when a change lands.
