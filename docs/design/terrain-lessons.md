# Terrain reference notes: MishMash's micro voxel engine

**Status:** Research notes, not scheduled.

**Kind:** Analysis of an external project, written for the v0.0.10 terrain work. It is not a milestone
scope, a spec, or an [ADR](../decisions/index.md), and it commits nothing. Where it disagrees with
[ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md) or the
[terrain pipeline](terrain-pipeline.md), those documents win.

## Why this page exists

v0.0.10 turns the terrain into the 1 m block plus 4 cm detail model. The closest public reference to
that shape is a solo project published as the "Micro Voxel Engine" devlog series by MishMash
(`@MishMash95`). It runs a 24 km world at 45-50 FPS on an M1 Pro with 10-20 cm voxels, which sits
between our 1 m blocks and our planned 4 cm detail, and its author has described the terrain pipeline
in unusual detail.

There is no source, no binary, and no write-up beyond Reddit posts and videos. Everything here is what
he states, paraphrased and attributed, plus our mapping. Treat it as field notes from a peer, not as a
spec.

**Quoted numbers are his**, and they moved as the project evolved. Where two dates disagree, both are
recorded.

## Baseline

| | |
|---|---|
| Language and API | C++ with Metal (metal-cpp); libraries limited to imgui, openAL, zstd |
| Hardware | M1 Pro, unified memory |
| Voxel scale | 0.2 m (11 Jun), 0.1 m (23 Jun), 0.2 m referenced again (7 Aug) |
| Chunk | `64³` |
| World | 16x16x1 km (Jun) -> 16x16x2 km (Jul) -> 24x24 km (Aug), fixed global heightmap |
| Frame cost | 10-20 million vertices; about two thirds of frame time is vertex work |
| Memory | 3.2-4 GB unified across builds; GPU voxel data was 2.9 GB in June and 2.5 GB in August |

The voxel scale is the punchline for our roadmap: his "micro" is 10-20 cm, and it is still far finer
than a 1 m block. Our 4 cm detail is finer than his entire world, which means his terrain techniques
are not about voxel size and should transfer.

## His pipeline

### 1. Base shape

A fixed global heightmap from "a fairly basic standard fractal noise function with a few layers,
similar to old school perlin", plus contextual local detail layers. The global map is precomputed and
sets the continental shape; local variance and caves are generated per chunk. The world is finite on
purpose, so generation rules can be well defined and features can be intentional.

He plans "infinite sub-islands" connected by soft-infinite ocean: crossing a boundary moves to a new
sector with its own local map generation.

### 2. Detail passes

On top of the height field:

- **Stamping** passes that write shaped voxel volumes into the terrain.
- **Carving** passes for canyons and rivers.
- **Erosion rounds** applied to the heightmap to find downhill flow, which is what carves rivers.
  Most mountains are untouched by that pass.
- **Caves**: stacked noise, deterministic feature generators (stalagmites that fill upward and
  downward), and an erosion pass for crevices.
- **Four biomes** with surface rules and feature probabilities.

### 3. Macro chunks and the sieve

This is the part worth stealing.

Every chunk generation function takes a **resolution divisor**, which he calls a *sieve*, and runs
natively at 1/N resolution. The same applies to feature generators and stamps. Nothing is downsampled
after the fact, so a coarse chunk is a real coarse generation of the same function rather than an
average of a fine one.

Consequences he reports:

- A 1/64-resolution chunk costs about the same as a full-resolution chunk and covers enormous
  distance. Bands could be pushed further at roughly constant cost.
- Coarse chunks record and resolve **their own local edits** and are saved and cached independently,
  so a far chunk never needs the full-resolution copy in memory to show a player's change.
- Bands generate high detail to low detail. An **adaptive fog** shortens the effective draw distance
  when a fast-moving player outruns the bands, then restores it.

### 4. LOD transitions and distant meshing

- **Transient transitions**, not continuous dithered blending. Both LODs are drawn only briefly at the
  boundary, then one remains. He measures **~30% cheaper** than a continuous dithered band and calls it
  "nearly as smooth". He explicitly rejected continuous dither because it doubles vertex pressure.
- **Blocky meshing at LOD+1, marching cubes from LOD+2.** Blocks "look quite bad at distance
  otherwise". He rejected dual contouring as too expensive for the gain.
- Occlusion culling is his largest optimisation but causes popping he is still tuning.

### 5. Props

Two tiers, because "keeping identical prop coverage for trees and items without instantiating millions
of entities" was the hardest part:

- **Large props** (trees, structures) use the same decoration pass as detail chunks and match
  generation 1:1. IDs are deterministic from position and type, so destroying one in a detail chunk
  cascades a removal through every macro level.
- **Small props** (grass, foliage) are scattered by an approximate distribution that matches the biome,
  at coarse chunk resolution with cluster spawn. He accepts the mismatch with the simulated area:
  "a good trade off to avoid millions of tracked grass items".
- Batches are gathered from both the live entity set and the macro prop lists into instanced draws.
  Grass becomes cards much closer to the camera. The surprising result: **prop rendering got faster**
  when he increased view distance, because instancing and cards removed more geometry than the extra
  distance added.
- Trees are fully procedural (trunk height, branches, leaf clusters), with about five cached variants
  and vertex-animated sway.

### 6. Persistence and streaming

- Only manipulated chunks are written to disk.
- An LRU cache holds unloaded chunks in memory: 256 MB default, about **5,000 compressed `64³`
  chunks**, tunable to 2-3 GB.
- Edits cascade down the LOD levels and each level saves independently, so modifications resolve
  without reloading detail chunks. Coarse levels "take only a small fraction of the total, and
  compress really well".
- Props are destructible up close but non-persistent beyond a time: the raw prop asset stays resident
  and the edits do not.

## Mapping onto our pipeline

| His technique | Our position | Recommendation |
|---|---|---|
| Sieve: generate natively at 1/N | ADR-0014 already chooses spacing-native sections; our LOD1/LOD2 still **downsample** a `32³` chunk | Adopt the sieve for the far levels. This is the single highest-value item. |
| Macro-chunk edit overlays saved per level | `VCHK` saves modified chunks only, at one level | Design per-level overlays into the save format before v0.0.10 ships, not after |
| Transient LOD transitions | We already have hysteretic LOD selection at 4/8/16 chunks | Keep. Confirm the boundary is transient, not a dithered double draw |
| Marching cubes at far LOD | We emit blocky greedy quads at every LOD; ADR-0014 makes isosurface work out of scope | Reopen only with a new ADR. If far LOD noise is the complaint, this is the proven fix |
| Fixed global heightmap + local detail | Our generation is per-chunk noise with no global map | A precomputed global map is what makes "intentional features" possible; consider for v0.0.11 |
| Deterministic prop IDs + macro prop lists | No prop system yet | Requirement for trees and structures: IDs must be a pure function of position and type |
| Grass as an approximation, not entities | No grass | Same approach when foliage lands |
| Adaptive fog from loaded bands | Fog is a function of render distance only | Cheap addition to streaming: gate the fade on band readiness |
| LRU cache of compressed chunks | Unload frees; no cache | Optional, measure before adding |
| Occlusion culling | Out of scope in v0.0.8 | His experience says expect popping; budget tuning time, not just the cull |

## What we would need to build

1. **Sieve in the generator.** Thread a level parameter through every generation stage so a level-`L`
   section samples the same world at spacing `2^L` without downsampling. Determinism tests at each
   level, plus an anti-alias test that a coarse section has the same ocean line and biome boundaries
   as the fine one.
2. **Per-level edit overlays.** Save format change, cascade on edit, load and merge per level. This is
   the item with the longest tail: it touches save, streaming, meshing, and the edit path.
3. **A far mesher.** If blocky far terrain still reads as noise, a surface-nets pass at LOD2+ with
   seam handling against blocky LOD0/LOD1. New ADR, new mesher, new tests.
4. **Prop IDs.** A registry with deterministic IDs, macro-list membership, and cascade removal.
5. **Band scheduler.** Generate bands high to low, track readiness per band, drive adaptive fog.

## What not to copy

- **The voxel scale.** 0.1-0.2 m over 24 km costs him about 2.5-2.9 GB of voxel data. Our 4 cm detail
  must be surface-local, as the [lighting page](lighting-pipeline.md) argues.
- **Mesh-only rendering without normals.** He drops normals for style and memory and gets soft shading
  from a wide PCF kernel. Our shader is normal-driven; adopting this would change the whole art
  direction, not one system.
- **Greedy meshing for dynamic chunks.** He uses it for static props only, because re-running it on
  every edit is too slow. We already run greedy on chunks; keep an eye on edit latency under detail.
- **No persistence for prop edits.** Fine for his ambience demo, not for a game with a build system.

## Open questions

- Does the sieve violate our "terrain is a pure function of `(seed, parameters, coordinate)`" pillar?
  It should not, but it does mean the function takes a level, and every feature must be level-aware.
- Do per-level overlays need their own generator version, or can one version cover the whole ladder?
- Is a far isosurface mesher worth the seam work, or does a better far material palette hide the
  blockiness more cheaply?
- How does a global heightmap interact with our seed and edit contracts? It is derived state, so it
  must be rebuildable, or saved and versioned.
- Does adaptive fog read as a bug when the world shrinks during flight, or is it invisible at 45 FPS?

## Sources

**Identity.** `@MishMash95` on YouTube and `u/MGMishMash` on Reddit describe the same project under the
same titles and stack (C++, Metal, M1 Pro, "Micro Voxel Engine"). No public cross-link exists, so the
identity is inferred, but consistently so.

**Posts.**

| Date | Post | Subreddit |
|---|---|---|
| 2026-06-11 | [New large scale Voxel engine in C++ and Metal](https://reddit.com/r/VoxelGameDev/comments/1u2jx1h/new_large_scale_voxel_engine_in_c_and_metal/) | r/VoxelGameDev |
| 2026-06-11 | [Same post, cross-posted](https://reddit.com/r/gameenginedevs/comments/1u2jxbb/new_large_scale_voxel_engine_in_c_and_metal/) | r/gameenginedevs |
| 2026-06-21 | [Voxel Engine #2 - Basic Mechanics, Placement, Lighting Engine](https://reddit.com/r/VoxelGameDev/comments/1ubvokc/voxel_engine_2_basic_mechanics_placement_lighting/) | r/VoxelGameDev |
| 2026-06-23 | [I built a Micro-Voxel engine](https://reddit.com/r/GraphicsProgramming/comments/1udkyzj/i_built_a_microvoxel_engine/) | r/GraphicsProgramming |
| 2026-06-29 | [Adding Caves (Devlog 3)](https://reddit.com/r/proceduralgeneration/comments/1uizbap/adding_caves_to_my_micro_voxel_engine_devlog_3/) | r/proceduralgeneration |
| 2026-07-07 | [Simulating Procedural Ponds (Devlog 4)](https://reddit.com/r/VoxelGameDev/comments/1upohtk/simulating_procedural_ponds_in_my_micro_voxel/) | r/VoxelGameDev |
| 2026-07-14 | [Raytracing Volumetric Clouds using Voxels (Devlog 5)](https://reddit.com/r/VoxelGameDev/comments/1uwcw65/raytracing_volumetric_clouds_using_voxels_devlog_5/) | r/VoxelGameDev |
| 2026-07-21 | [Adding Heat simulation and Survival](https://reddit.com/r/VoxelGameDev/comments/1v2w4r2/adding_heat_simulation_and_survival_to_my_micro/) | r/VoxelGameDev |
| 2026-08-04 | [Simulating Ocean Tides](https://reddit.com/r/VoxelGameDev/comments/1vfbiuf/simulating_ocean_tides_in_my_micro_voxel_game/) | r/VoxelGameDev |
| 2026-08-18 | [Improving Rendering Distance](https://reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/) | r/VoxelGameDev |

**Claims mapped to comments.**

| Claim in this page | Source |
|---|---|
| 0.2 m voxels, 16x16x1 km, 4-5 GB, far background mesh | [11 Jun](https://reddit.com/r/VoxelGameDev/comments/1u2jx1h/new_large_scale_voxel_engine_in_c_and_metal/) |
| 0.1 m voxels, 16x16 km cap, 3.2 GB, 40-100 FPS | [23 Jun](https://reddit.com/r/GraphicsProgramming/comments/1udkyzj/i_built_a_microvoxel_engine/) |
| World at 16x16x2 km | [24 Jul](https://reddit.com/r/VoxelGameDev/comments/1v2w4r2/adding_heat_simulation_and_survival_to_my_micro/ozfcuqg/) |
| World at 24x24 km, soft-infinite ocean, sector plan | [19 Aug](https://reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/p4jug2r/) |
| Sieve, native 1/N generation, edit overlays, transient transitions, adaptive fog | [18 Aug post](https://reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/) |
| Prop tiers, deterministic IDs, cascade removal, instanced batches, grass cards | [26 Aug](https://reddit.com/r/proceduralgeneration/comments/1vtpcz9/improving_terrain_and_render_distance_in_my_micro/p63405g/) |
| Per-level edits saved independently, heightfield plus stamps | [26 Aug](https://reddit.com/r/proceduralgeneration/comments/1vtpcz9/improving_terrain_and_render_distance_in_my_micro/p63torf/) |
| Transient versus continuous dithered LOD, 30% cheaper, vertex-bound | [18 Aug](https://reddit.com/r/VoxelGameDev/comments/1vrqluv/improving_rendering_distance_in_my_micro_voxel/p4i855d/) |
| Marching cubes from LOD+2, blocky at LOD+1, DDA slower in practice | [30 Jun](https://reddit.com/r/proceduralgeneration/comments/1uizbap/adding_caves_to_my_micro_voxel_engine_devlog_3/ounbh2i/) |
| Meshing over DDA, no vertex normals, vertex animation | [5 Aug](https://reddit.com/r/proceduralgeneration/comments/1vg6nb0/simulating_procedural_oceans_in_my_micro_voxel/p1umkec/) |
| LRU cache, 256 MB, ~5,000 compressed `64³` chunks | [3 Oct](https://reddit.com/r/GraphicsProgramming/comments/1udkyzj/i_built_a_microvoxel_engine/pdnll9t/) |
| 10-20 M vertices, water on 1 m cells (5x5x5 voxels) | [7 Jul](https://reddit.com/r/VoxelGameDev/comments/1upohtk/simulating_procedural_ponds_in_my_micro_voxel/ow40ta4/) |

**Videos.**

| Video | Series |
|---|---|
| [#1 - World generation](https://www.youtube.com/watch?v=V33oPxAFwf8) | Micro Voxel Engine |
| [#2 - Basic Mechanics, Global illumination, Creatures](https://www.youtube.com/watch?v=7k6PwmvjBW4) | Micro Voxel Engine |
| [Devlog 3 - Caves](https://www.youtube.com/watch?v=qFcWtRgE25s) | Micro Voxel Engine |
| [Devlog 4 - Procedural Ponds](https://www.youtube.com/watch?v=ZxfV7su168U) | Micro Voxel Engine |
| [Devlog 5 - Volumetric Clouds](https://www.youtube.com/watch?v=-vqWzDaWUKk) | Micro Voxel Engine |
| [Heat simulation and Survival](https://www.youtube.com/watch?v=yZ6TXirilUU) | Micro Voxel Engine |
| [Ocean Tides](https://www.youtube.com/watch?v=rCPDkSX5f1o) | Micro Voxel Engine |
| [3,000m+ mountains](https://www.youtube.com/watch?v=xGkWWfO87no) | Micro Voxel Engine |
| [Fishing in Micro Voxel Rivers](https://www.youtube.com/watch?v=kyd6aBP1-4Q) | Micro Voxel Engine |
| [Cozy Halloween Ambience](https://www.youtube.com/watch?v=gUB3xZ2QU_k) | Micro Voxel Engine |

The videos are music and on-screen text; they carry no narration or captions, which is why the
technical detail in this page comes from the posts and comments.

**Access note.** Reddit blocks direct fetches from this machine. The data above was retrieved through
the PullPush archive API (`api.pullpush.io/reddit/search/comment?author=MGMishMash` and the matching
submission endpoint), which returns post and comment bodies rather than rendered pages.

