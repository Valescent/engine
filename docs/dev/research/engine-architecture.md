# Engine architecture notes

**Status:** Reference. This page records the engine-facing parts of the internal voxel engine
architecture design and maps them to the current code. It is not a promise to restructure the
engine.

**Source:** Internal voxel engine architecture and optimization notes. The source proposes a
crate-per-domain layout (`vox-block`, `vox-storage`, `vox-generation`, `vox-meshing`, `vox-render`,
`vox-world`, `vox-input`, `vox-player`, `vox-game`, `vox-audio`, `vox-bench`). The project keeps
its current workspace layout and adopts the principles, not the crate names.

## 1. The source's build order

The source's one-sentence order is:

> Block storage -> naive mesh -> naive render -> cull faces -> 1D greedy -> 2D greedy -> pack
> vertices -> instance quads -> one draw call -> per-face meshes -> remove normal -> add second
> length -> optimize block tables when the CPU is the bottleneck.

The three ordering rules are:

1. Correct before fast. Every optimization is a testable diff from a known-good baseline.
2. Triangle count first, then vertex size, then draw-call count.
3. Bit-packing last, once the fields are known.

## 2. What the current engine already has

| Source concept | Current implementation |
|---|---|
| Stable block ids | `terrain-core::content` derives ids from sorted stable names; the legacy `Block` enum has stable on-disk ids |
| 32x32x32 chunks | `terrain-core` sections are 32x32x32; the legacy world stores 32^3 chunks |
| Dense storage | Legacy chunks use 4-bit packed ids; `terrain-core` sections use paletted, bit-packed storage |
| Face culling | `src/v0/mesh.rs` and `terrain-core::mesher2` |
| Greedy meshing | The v0 greedy mesher and the v1 `mesher2` |
| Neighbour padding | `crates/terrain-core/src/padded.rs` |
| DDA raycasts | `src/raycast.rs` |
| Streaming ring | `src/demo/stream.rs` and `src/demo/mod.rs` (v1), `src/v0/streaming.rs` (legacy) |
| LOD sections | ADR-0014 and the level 1-4 hierarchy |
| Pooled geometry and indirect draws | ADR-0017, `src/quad_renderer.rs`, `src/quad.wgsl` |
| Chunk revisions and stale-result rejection | `src/demo/edit.rs`, `src/demo/persist.rs`, `src/demo/mod.rs`, `src/v0/mesher.rs` |
| Frame-origin-relative rendering | `src/renderer/origin.rs` |
| UI and audio | egui and kira (ADR-0018, ADR-0019) |
| Headless generation, light, and mesh tests | `cargo test`, `terrain-gen` |

The engine is well past the source's naive phases. The current baseline is a tested fallback
renderer, a quad pool, seeded LOD sections, and headless terrain gates.

## 3. Gaps worth revisiting

These are source optimizations the engine has not adopted. They are candidates, not commitments:

| Candidate | Why it may matter | Current state |
|---|---|---|
| Binary greedy meshing | Bitmask/TZCNT runs can make the mesher 5-20x faster at the same triangle count | Scalar greedy is the oracle |
| Per-face-direction meshes | Enables mesh-level backface culling and frees vertex bits | One mesh per section with face data |
| Single draw / SSBO draw id | One world draw with per-draw section data | Two indirect draws plus a shared chunk table |
| 32-bit or 4-byte quad payload | Fewer bytes per quad; memory-bandwidth win | 12-byte quad records (ADR-0017) |
| Palette/RLE for the legacy path | Lower resident memory for the fallback | Legacy uses 4-bit packing; v1 is paletted |
| SIMD block-property gathers | Mesher property queries | Scalar tables |
| Region files / batched chunk store | Fewer files, faster bulk save | Per-chunk section files and a journal |
| 6 vs 26 neighbour padding | AO needs the 26-neighbourhood; culling needs 6 faces | Padded grid exists; the halo size is a future choice |
| Hot/cold block tables | Keeps the mesher's hot table in cache | Content records are in-memory structs |

The 32-bit-per-quad layout in the source is not automatically better than the engine's current
12-byte record. It trades texture count, run length, and shader reconstruction for memory. Measure
before changing it.

## 4. What the engine keeps

- The current module boundaries and the `terrain-core` / `engine` split.
- `terrain-core` stays headless; no `wgpu`, `winit`, or profiler dependency.
- The scalar greedy mesher stays the oracle until a binary path matches its output and wins on
  frame time, mesher time, and memory.
- The per-chunk fallback path stays until the quad pool is the default and proven.
- The 1,000-line file gate and the deterministic headless gates stay.

## 5. What this means for crafting

The architecture notes reinforce the crafting plan:

- Recipes, item definitions, and tags are loaded and validated once, then resolved by dense ids.
- No string lookup or hash lookup in the per-craft or per-tick path.
- Block properties stay in the content pack; recipe grammar stays in the gameplay layer.
- The recipe book is a view over the recipe table and the current inventory; it does not scan the
  world.
- Quality/rarity instances, when they arrive, use a side table, so stackable materials remain
  small.
- Station state and queues, when they arrive, belong in the fixed-tick gameplay state and the
  player/world save, not in the renderer.

## 6. Relationship to current docs

- [Architecture](../architecture.md) is the current module and frame map.
- [Terrain pipeline](../../design/terrain-pipeline.md) and
  [modular structure](../../design/modular-structure.md) are the current terrain contracts.
- [Quad pool and indirect draws](../../decisions/0017-quad-pool-and-indirect-draws.md) is the current
  geometry path.
- [Terrain research](README.md) holds the measured meshing and streaming notes.
- [Crafting system](../../design/crafting-system.md) is the adapted crafting design.
