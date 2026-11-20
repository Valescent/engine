# Terrain pipeline

**Status:** Sketch; not scheduled.

**Kind:** Long-range design direction. This page is not a milestone scope, a spec, or an
[ADR](../decisions/index.md). It does not change the roadmap, and nothing here is committed
until a later ADR and spec say so. If this page disagrees with a spec, the spec wins.

## Why this page exists

The [roadmap](../roadmap.md) has shipped through [v0.0.7](../specs/v0.0.7.md), with v0.0.8 in
acceptance. v0.0.7 ended with a finite, deterministic, seeded voxel world: five noise fields,
heightmap terrain, caves, three biomes, trees, player-centred streaming, two LOD levels, and
modified-chunk saves. It is a complete first world, not a terrain platform.

This page describes the terrain platform that could grow after it. The goal is not more
content in the same shape. The goal is a pipeline whose stages can be added, measured,
replaced, and versioned without rewriting the world model each time.

> **The bet:** terrain is a pure, versioned function of `(seed, parameters, coordinate)`; player
> edits are a sparse authoritative overlay; every expensive stage is asynchronous, cancellable,
> budgeted, and measured.

## What v0.0.7 establishes

v0.0.7 has shipped; this table describes the contract it established.

| Area | v0.0.7 contract | What it leaves open |
|---|---|---|
| World identity | One decimal `u64` in `world/seed` | No generator or content version |
| Fields | Five normalized fields through `fastnoise-lite` 1.1.1 | Hardwired composition; no field graph |
| Shape | 2.5D heightmap plus 3D cave carve | Overhangs, arches, and true 3D density |
| Biomes | Plains, hills, desert; hard surface palettes | Blending, climate transitions, more than three |
| Features | Trees that fit inside one chunk | Cross-chunk structures and feature ordering |
| Streaming | Chebyshev ring, `R = 8..=16`, four chunks tall | Vertical streaming and seamless larger worlds |
| LOD | LOD0 and LOD1 with 4/8 hysteresis | LOD ladders, stitching, and distant terrain |
| Persistence | `VCHK` v3, modified chunks only | Generator version, edit overlays, journaling |
| Work | CPU generation, lighting, and meshing | GPU-assisted sampling or meshing |
| Diagnostics | Frame, chunk, vertex, and rebuild metrics | Capture/replay, golden hashes, cache metrics |

## Pressures

These are the places where extending the v0.0.7 shape starts to hurt.

- **The field composition is code.** Adding a ridge field, a river field, or a shelf field
  means editing generation rather than describing a new graph.
- **Heightmap-first limits geometry.** Caves can open the surface, but overhangs, arches, and
  floating rock are not expressible as a height plus a carve.
- **One LOD step is not a ladder.** A 2× step is enough for a 16-chunk ring, not for a horizon.
- **Features are chunk-local by construction.** Anything wider than 32 blocks is skipped.
- **A world has no generator version.** Changing noise or ordering silently changes existing
  terrain the next time an unmodified chunk is generated.
- **Edits are saved as whole chunks.** There is no delta, no journal, and no cheap undo.
- **The world is four chunks tall.** Vertical residency and tall terrain need a different
  streaming model.
- **Everything is CPU-side.** That is the right default, but there is no stated boundary for
  what a GPU may accelerate.
- **Determinism is asserted, not measured.** There are deterministic tests, but no golden
  hashes, capture files, or cross-process comparison.

## Design pillars

1. **Terrain is a pure function.** Generation reads a world description and a coordinate. It
   does not read the clock, global RNG, chunk load order, or mutable engine state.
2. **Saved data is the world.** A generated column is written once and thereafter only edited.
   There is no regeneration of a saved column, and the edit log exists for undo and audits, not
   for rebuilding terrain.
3. **One versioned description.** Seed, generator version, parameter set, and content pack are
   one identity. A world is pinned to that identity.
4. **Determinism has a defined boundary.** Same binary and platform is the baseline. Stronger
   guarantees are explicit work, not an accident.
5. **Every stage is a job.** Expensive work is asynchronous, cancellable, bounded, and
   replaceable. The main thread applies results and uploads; it never waits.
6. **Readiness is explicit.** A chunk is drawn only when baseline, overlay, light, and mesh are
   from the same revision and consistent with each other.
7. **Content is data.** Biomes, surface rules, features, and palettes should become data with
   stable identifiers, not branches in Rust.
8. **Measure or it did not happen.** Each stage has counters, timings, queue depth, cache
   behaviour, and a failure policy.
9. **Keep the fixed points.** Chunk-local meshes, right-handed Y-up coordinates, 32³ chunks,
   sorted deterministic output, and typed errors remain unless an ADR changes them.

## The pipeline at a glance

```text
world description (seed, generator version, parameters, content)
        |
        v
field graph ---> climate, terrain shape, caves, hydrology
        |                    |
        |                    v
        |            volume synthesis ---> materials, strata, water
        |                    |
        |                    v
        |            feature placement (discover, then apply)
        |                    |
        +--------------------+
                             |
                             v
                     generated section (immutable until edited)
                             |
        player edits ---> saved effective section <--- load/save
                             |
                             v
                     effective chunk data
                             |
                   +---------+---------+
                   |                   |
                   v                   v
                 light                mesh + LOD
                   |                   |
                   +---------+---------+
                             |
                             v
                     upload + render
```

| Stage | Input | Output | Runs on | Cacheable |
|---|---|---|---|---|
| World description | World manifest and content | Versioned description | Main thread | Yes, once per world |
| Field graph | Description and position | Field samples | Workers, later GPU | Yes, per field tile |
| Volume synthesis | Fields and rules | Generated blocks | Workers | Yes, per generated chunk |
| Feature placement | Description and chunk | Feature index and writes | Workers | Yes, deterministic |
| Residency | Description and player | Jobs and readiness state | Main thread | No |
| Lighting | Effective blocks | Light field | Workers | Derived |
| Meshing | Effective blocks, light | Mesh per LOD | Workers, later GPU | Derived |
| Upload and render | Mesh buffers | Frames | Main thread | No |
| Edits and persistence | Input and saved sections | Effective sections, edit log | Main thread and workers | Authoritative |

### 1. World description

A world is more than a seed. The description should carry:

- the `u64` seed;
- a generator version that pins field composition, evaluation order, and feature rules;
- a parameter set or parameter hash;
- a content pack hash for block definitions, biomes, surface rules, and feature types;
- a format version for the description itself.

The description is written to the world directory before the first generated chunk is saved. A
world without a description is either migrated from the v0.0.7 `world/seed` layout or rejected
with a typed error. Guessing is not allowed.

This is the stage that makes old worlds portable. A generator change that alters output is a
new generator version, never an in-place edit to an existing one.

### 2. Field graph

The field graph replaces a fixed list of five sampled fields with a declared graph: sources,
transforms, combines, and consumers. The useful distinction is not "more noise"; it is that
each field has a name, frequency, seed stream, evaluation cost, and consumer.

Candidate field families:

- **Continentalness and erosion** for large landform shape.
- **Ridged and billowed fields** for mountain chains and cliffs.
- **Temperature, humidity, and seasonality** for climate and biome blending.
- **River and basin fields** for hydrology.
- **Cave and overhang fields** for 3D density.
- **Surface and strata fields** for material variation.

Two properties matter more than the field list:

- **Evaluation is tiled.** A chunk should be able to request a field tile with a stated
  resolution and receive the same values no matter what else has been generated.
- **Composition is versioned.** Reordering a combine changes output and therefore bumps the
  generator version.

Field sampling is the most natural first GPU candidate, because it is wide, regular, and has no
cross-chunk dependencies. The CPU implementation remains the reference and the fallback.

### 3. Volume synthesis

Heightmap-first terrain cannot express overhangs, arches, or floating volumes. A volume-first
stage treats the chunk as a 3D field: solid or air from density, then material from rules.

The stage should be able to produce both shapes:

- **Heightmap mode** for performance and for compatibility with the v0.0.7 world style.
- **Density mode** for overhangs and true 3D landforms.

Both modes must feed the same downstream chunk baseline. The renderer, lighting, collision, and
save layers should not know which mode produced the blocks.

Material synthesis belongs here too: stone strata, dirt depth, sand, snow, and surface rules by
biome and slope. A raw density field is geometry; a world needs material rules that are as
deterministic as the geometry.

### 4. Feature placement

Features need two phases:

1. **Discover.** A deterministic pass over a feature region produces candidates with stable
   identifiers. It depends only on the world description and the region, not on which chunks
   are loaded.
2. **Apply.** A chunk pass consumes candidates that intersect the chunk and writes their
   blocks. It can run when the chunk is sampled, regardless of load order.

This is the structure that lets a tree, a ruin, a bridge, or a cave entrance cross a chunk
boundary without being skipped or duplicated. The current v0.0.7 rule — skip anything that does
not fit in one chunk — is a deliberate simplification, and this stage is where it ends.

Feature identifiers should be stable enough to deduplicate and to make a feature index
cacheable. A candidate that intersects an unloaded chunk is recorded, not lost.

### 5. Residency and scheduling

Residency is the traffic controller. It decides what is requested, at which resolution, in
which order, and when a result is still wanted.

A useful state machine for one chunk:

```text
Missing -> Queued -> Sampled -> Featured -> Lit -> Meshed -> Uploaded -> Ready
                          \         \        \        \
                           +---------+--------+--------+--> Dirty
```

- `Dirty` returns the chunk to the earliest invalidated stage, not always to generation.
- A result is accepted only when its revision, LOD, and description identity still match.
- Cancellation is explicit. Dropping a result is not the same as cancelling work.

Priority classes should be small and meaningful:

| Class | Examples | Rule |
|---|---|---|
| Urgent | Player edit, current column | Never dropped; applied first |
| Visible | Chunks in the frustum and nearby | Bounded queue; forward bias |
| Prefetch | Ahead of movement, inside hysteresis | Dropped before Visible |
| Background | Distant LOD, cache fill, regeneration | Dropped first; no frame deadline |

The v0.0.7 player guard can remain as the safety boundary until residency is fast enough and
seamless enough to remove it. Removing the guard is an ADR, not a side effect.

### 6. Lighting

Lighting stays derived. The far-out version adds three requirements:

- **Cross-border consistency.** A light result must be valid for the chunk plus its border,
  and a neighbour's edit must dirty the same light set regardless of load order.
- **Vertical awareness.** Sky light through a tall world needs either loaded chunks above or a
  deterministic sky-occlusion summary. Loading the whole column above the player does not scale.
- **Light-aware LOD.** Distant LOD meshes need a defined rule for light and AO. The current
  representative-block rule is a starting point, not a universal answer.

Lighting is also a natural candidate for incremental repair instead of full recompute. The
pipeline should allow a chunk to be repaired from its previous light field and a bounded dirty
set, with a full recompute as the reference path.

### 7. Meshing and LOD

Meshing stays deterministic and chunk-local. The LOD ladder grows, for example 1, 2, 4, and 8
blocks per cell, with hysteresis between levels. The difficult part is not another downsample;
it is the seams.

Options to evaluate:

- **Conservative boundary faces**, as [v0.0.5](../specs/v0.0.5.md) does, to guarantee no holes
  at the cost of overdraw.
- **Skirts** around chunks, cheap but visible from above and at silhouettes.
- **Stitching** with a transition mesh, more complex but cleaner.
- **A different far representation**, such as a heightfield or impostor beyond the mesh ring.

The choice belongs in an ADR with screenshots, triangle counts, and a measured frame cost. A
seam strategy that only works for opaque blocks is incomplete; water and transparency need a
rule too.

LOD changes invalidate only the mesh stage. That distinction is what makes a larger render
distance affordable.

### 8. Upload and render

The renderer should receive immutable mesh results and a small, explicit upload queue. It
should not know how a chunk was generated.

The far-out render path may add:

- indirect draws or batching instead of one call per chunk;
- GPU culling and LOD selection;
- occlusion culling after the frustum pass;
- a far-terrain pass outside the chunk mesh ring;
- camera-relative rendering and an origin rebase before `f32` precision becomes visible.

The main thread still owns world mutation and uploads. A GPU pass may propose a result, but the
CPU remains the authority for collision, edits, and persistence.

### 9. Edits and persistence

> **Superseded by the [terrain interview](terrain-interview.md).** The confirmed model saves
> *effective* (post-edit) sections as the single source of truth; a saved column is never
> regenerated, and the edit log is an audit and undo trail, not a re-application layer. There
> is no baseline cache. Disposable caches are limited to derived data that is a pure function of
> `(description, tile)`.

The save model should move from "whole modified chunk" to "effective section plus edit log".

- **Effective section** is the authoritative record of what exists: generated, then edited.
  It is saved for every column the player generates and is never rebuilt from the generator.
- **Edit log** records what changed, with the old value, for undo, conflict reporting, and
  migration audits. It never overrides a saved section.
- **Derived caches** (field tiles, hydrology tiles, feature candidates) are pure functions of
  `(description, coord)` and live outside the world directory. Deleting them changes nothing.

`VCHK` keeps loading its existing versions. A later format adds the world description identity,
a per-section baseline hash, and the edit-log records. The exact layout is a spec decision, not
this page.

When the generator version changes, an existing world has three honest options:

1. **Pin and keep.** The world stays on its old generator version and the engine keeps that
   evaluator available.
2. **Migrate.** A one-way migration transforms the stored sections, reports every edit it
   could not preserve, and never regenerates a saved section.
3. **Fork.** The world is copied to a new identity, leaving the original untouched.

Silently regenerating a world under new rules is not an option. It destroys player work and
makes bug reports irreproducible.

## Data model

```text
WorldDescription
  seed, generator version, parameters, content hash
        |
        +--> SavedSection      effective blocks, authoritative, never regenerated
        |         ^
        |         |
        |    EditLog           old/new per block, audit and undo only
        |
        +--> Derived            field tiles, hydrology, candidates (disposable)
                    |
                    v
              EffectiveSection  read by collision, light, mesh
                    |
          +---------+---------+
          |                   |
       LightField           Mesh (per LOD)
```

| Layer | Authority | Persisted | Invalidated by |
|---|---|---|---|
| World description | World manifest | Yes | Explicit migration or fork |
| Saved section | Effective blocks | Yes | Player edits |
| Edit log | Audit and undo | Yes | Compaction, retention |
| Derived tiles | Disposable | Optional cache | Description change |
| Light | Derived | No | Effective section change |
| Mesh | Derived | No | Effective section, light, or LOD change |

The effective chunk is the only thing collision, interaction, lighting, and meshing should read.
That keeps the baseline and overlay from leaking into every consumer.

## Determinism and versioning

### Keyed randomness

Every random choice comes from a named stream keyed by world identity plus a stable address:
stage, chunk coordinate, feature identifier, or field name. There is no global RNG and no
dependence on call order.

### Float boundary

The current noise stack evaluates in `f32`. Rust and the platform do not promise bit-identical
floating-point results across compilers, targets, or optimisation settings. The baseline
promise is therefore:

- the same binary, platform, and world description produce the same world;
- golden tests compare block hashes on the reference platform;
- cross-platform bit equality is an explicit goal only if a later ADR accepts the cost.

If cross-platform equality becomes necessary, the options are fixed-point or integer field
sampling, quantised intermediates, or a recorded baseline cache. The choice must not be
discovered after worlds exist.

### Generator version

`GeneratorVersion` is a stable identifier for the complete evaluation pipeline: field graph,
synthesis rules, feature rules, and ordering. It is stored in the world description and
included in every generation cache key.

A change that can alter any block output bumps the version. A change that only alters
performance, diagnostics, or memory layout does not.

### Golden hashes

For a fixed world description and a list of chunk coordinates, store the expected effective
block hash and, separately, the expected mesh hash. These tests catch accidental terrain
changes across refactors. They are not a substitute for behavioural tests; they are the alarm
that says a change was not supposed to affect output.

### Migration policy

A world is pinned to its description. Upgrading the engine never changes an existing world
unless the player explicitly chooses a migration or fork. Migration reports what it could not
preserve. The report is part of the feature, not an error path hidden behind a log line.

## Scale and LOD

The current ring is finite and horizontal. A larger world needs four separate decisions:

- **Horizontal reach.** How far can the player see and travel before the ring is too small?
- **Vertical reach.** Is the world still four chunks tall, or does residency follow the player
  vertically?
- **LOD reach.** How many LOD steps, and at what distances do they switch?
- **Precision reach.** When does `f32` world space need camera-relative rendering or an origin
  rebase?

These are coupled. A taller world changes sky light and vertical streaming; a longer view
changes LOD and upload budgets; a larger travel range changes precision and save identity.

The chosen direction is a Voxy-style hierarchy: full-detail sections near the player and
coarser first-class sections further out. A separate far representation is only needed if the
hierarchy cannot reach the target distance. The boundaries are measured, not guessed.

## Scheduling, budgets, and backpressure

Every stage declares four things:

- **Maximum work per frame**, in items or milliseconds.
- **Maximum in-flight jobs**, so a fast camera cannot grow the queue without bound.
- **Cancellation rule**, so stale work stops rather than finishing into a discard.
- **Failure behaviour**, so one bad chunk does not stall the pipeline.

The main thread applies results at a bounded rate. If work falls behind, the pipeline degrades
in a defined order: distant LOD, then prefetch, then background cache fill. Visible chunks and
player edits are last to be dropped. The player guard or fog is the visible consequence, not a
frame hitch.

Derived tiles can be cached on disk, outside the world directory:

```text
cache/{description_hash}/{tile_x}_{tile_z}.tiles
```

A cache entry is valid only when its description hash matches. A corrupt or stale entry is
deleted and regenerated. These caches are optional and never hold player changes; saved
sections are the only authority for what exists in the world.

## Observability and failure

The pipeline should be diagnosable without a debugger:

- **Metrics:** stage latency, queue depth, in-flight jobs, cache hit rate, resample count,
  baseline hash mismatches, upload bytes, visible chunks per LOD, and frame time.
- **Trace:** one chunk's journey from request to ready, with the stage that delayed it.
- **Capture/replay:** seed, description, camera path, and edit log in one file; replay produces
  the same visible result.
- **Debug views:** chunk bounds, LOD level, light field, field values at a probe point, and the
  baseline/overlay split for a selected chunk.
- **Failure policy:** typed errors at stage boundaries; no partial world load; worker panics
  contained and reported; GPU device loss recoverable or fatal with a clear error.

A pipeline that cannot explain why a chunk is missing, dark, or low-detail is not finished.

## Fixed points

Unless an ADR says otherwise, the following stay:

- right-handed, Y-up coordinates, one block per metre, as recorded in
  [Coordinates](../dev/coordinates.md);
- `32³` chunks and `ChunkCoord` ordering;
- chunk-local mesh vertices and per-chunk origins;
- deterministic sorted iteration for output;
- typed errors instead of panics on disk and input data;
- no global mutable state and no worker access to GPU or `World`;
- derived light and meshes are never persisted as authority;
- unmodified terrain remains regenerable from the world description.

## Evolution horizons

These are research horizons, not roadmap versions. Each one should be split into real
milestones only after its ADR and spec exist.

| Horizon | Outcome | Exit criteria |
|---|---|---|
| H0 — Close the loop | Golden hashes, capture/replay, stage timings, and a clean pipeline boundary after v0.0.7 | A fixed seed reproduces the same world across runs; a captured session replays |
| H1 — Versioned description | World manifest, generator version, field graph, and content hash | Old worlds stay stable; changing a parameter is an explicit new version |
| H2 — Volume-first synthesis | Density terrain with overhangs and material rules, alongside heightmap mode | Heightmap worlds remain reproducible; density fixtures have expected hashes |
| H3 — Cross-chunk features | Two-phase feature discovery and application | Feature placement is independent of chunk load order and streaming |
| H4 — LOD hierarchy | More LOD levels, hierarchical selection, and a measured render distance | No holes at transitions; distant terrain stays inside the frame budget |
| H5 — Hydrology and erosion | Rivers, basins, water table, erosion, and sediment fields | Deterministic hydrology; no chunk-order dependence |
| H6 — Edit overlay and journal | Delta persistence, crash-safe journal, and explicit generator migration | An edit survives restart and migration without silent loss |
| H7 — GPU-assisted stages | GPU field sampling, culling, or meshing behind measured feature flags | A measured win over the CPU reference on the same scene |
| H8 — Wider world and tooling | Floating origin, far terrain, and live field/content authoring | Large travel ranges stay precise; tuning does not require a rebuild per change |

## Practical build order

The pipeline diagram describes runtime shape, and the horizons above describe outcomes. This
section describes dependency order: what to build next if the terrain track is treated as a
greenfield sequence of vertical slices. It is not the v0.0.3–v0.0.7 milestone order. Phases 0–1
overlap shipped and scoped milestones; phases 2–9 are post-v0.0.7 work. If they run earlier, they
are spikes feeding ADRs, not a replacement for the milestone sequence. The mapping table shows
where each phase lands in the existing plan, and the corrections below are the places where the
supplied order needs adjustment before it becomes a schedule.

A phase is done only when the world is runnable, the new stage has a debug view, the output is
deterministic under replay or a golden hash, and the phase has a profile capture. That gate is
the part worth keeping from the vertical-slice advice.

| Phase | Slice | Proves | Repo mapping |
|---|---|---|---|
| 0 — Foundations | Block registry, keyed seed streams, chunk arrays and neighbours, height/density/biome debug views | The data model and observability | Project skeleton and camera are shipped; block/chunk work maps to v0.0.3/v0.0.4; debug views are H0 |
| 1 — Base terrain | 2D fBm heightmap, height-to-voxel fill, naive chunk mesh, streaming and worker generation | A flyable deterministic world | v0.0.3 meshing plus the v0.0.7 generation/streaming base |
| 2a — Macro heightfield | 2D fBm plus erosion-ready height output | A stable macro surface for hydrology and climate | H2/H5 |
| 2b — 3D detail | 3D fBm, density caves and overhangs | Volume terrain at full detail | H2 |
| 2c — LOD hierarchy | Voxy-style multi-level sections, seed-native coarse generation, hierarchical rendering | Extreme render distance without full-res generation | H4; renderer ADR required |
| 3 — Erosion and hydrology | 2.5D thermal and hydraulic erosion, priority-flood filling, D8 flow accumulation and river carving | Valleys, lakes, rivers, and believable slopes | H5 |
| 3b — Volume-aware repair | Local 3D erosion and hydrology inside interaction regions emitted by 2b | Caves, overhangs, and water stay coherent | H2/H5 |
| 4 — Climate and biomes | Temperature and humidity, altitude/latitude adjustment, orographic rain shadow, biome table and blending | Logical biome borders | H5 plus the v0.0.7 biome rules |
| 5 — Geology and materials | Strata, folds and faults, rock resistance, ore veins | Cliffs and caves that read as geology, not paint | H2 |
| 6 — Advanced landforms and caves | Cave types, implicit landform primitives, rule-based landform tree | Landmarks that noise alone cannot produce | H2/H3 |
| 7 — Ecology and scatter | Poisson or best-candidate scatter, soil and moisture, L-system flora | Forests with edges and density gradients | H3 |
| 8 — Dynamic simulation | Fluids, material reactions, fire spread | A world that changes without the player | A separate project; currently a non-goal |
| 9 — Scale and polish | SVDAG or another far representation, async cancellation, invariants, profiling | Reach and stability | H0/H7/H8, with some items moved earlier |

### Order corrections

- **Determinism belongs in Phase 0, not Phase 9.** Golden hashes, invariants, and replay are
  phase gates. A build order that waits until item 35 to test determinism contradicts its own
  rule about not starting a phase until the previous one is deterministic.
- **Cancellation belongs in Phase 1.** Epochs, revisions, and stale-result rejection are part
  of streaming, not polish. Adding them after streaming means rewriting the streaming path.
- **Geology must feed erosion, not follow it.** Rock resistance is an input to erosion. Build
  the erosion solver with a resistance map from day one; the initial map can be uniform. Then
  strata can fill that map without reordering the stages.
- **Use 2.5D as the macro solver and volume-aware only as a local repair.** Thermal, hydraulic,
  priority-flood, and D8 are 2.5D heightfield algorithms. Run them on the macro heightfield,
  then generate 3D detail, then repair the interaction regions in 3D. Do not turn every slope
  and river into a 3D flow problem. That is why Phase 2 is split into 2a/2b and Phase 3 has a
  bounded 3b repair.
- **Blocky is chosen; isosurface meshing is out of scope.** Surface Nets, Marching Cubes, and
  Transvoxel solve a different representation. They are not part of this track unless a later
  ADR reopens the world model. The LOD direction is Voxy-style hierarchy, not Transvoxel
  stitching.
- **Do not treat LOD seam work as final before the macro terrain stabilizes.** Every erosion,
  climate, or landform change re-tests the seams. This is reasonable for a spike and expensive
  as production order.
- **Keep dynamic simulation out of this pipeline.** Phase 8 is a separate simulation project
  and contradicts the current non-goal. It should not block terrain completion.
- **Crate names are candidates, not decisions.** The accepted choices remain
  `fastnoise-lite` ([ADR-0012](../decisions/0012-fastnoise-lite-and-opensimplex2.md)) and
  micropool ([ADR-0009](../decisions/0009-micropool-for-chunk-workers.md)). Replacing either
  needs an ADR and a benchmark.

| Supplied choice | Current decision | What to do |
|---|---|---|
| `noise-rs` or `quick-noise` | `fastnoise-lite` 1.1.1 with OpenSimplex2 | Keep the accepted noise interface; benchmark alternatives behind it; replace only by ADR |
| `rayon` generation | micropool worker pool | Keep micropool; rayon remains a benchmark-only development dependency |
| Surface Nets, Marching Cubes, Transvoxel | Blocky greedy mesher; isosurface out of scope | Do not adopt; they solve a different representation. Reopen only with an ADR. |
| `voxelis` SVDAG | Optional SVDAG experiment, not a dependency | Evaluate as a representation and engine, not a drop-in compressor |
| `symbios-ground` | Engine-owned generation pipeline | Evaluate as a reference or spike; do not let it own the world model without an ADR |
| `mcubes` | No marching-cubes dependency | Not needed while blocky is chosen; revisit only if an ADR reopens isosurface terrain |

### The fastest path to a convincing world

The supplied shortcut is directionally right: macro shape, water, climate, and strata read as a
real landscape much sooner than another density octave. The dependency-correct version is:

```text
0 -> 1 -> 2a -> 3 (uniform resistance hook) -> 4 -> 5 (fills the hook) -> 2b -> 3b (hybrid repair) -> 2c
```

The runtime order is already geology, then erosion; only the implementation of the geology
provider is deferred. Build the erosion solver with a resistance input in Phase 3, even if the
first provider returns a uniform value. Phase 5 replaces that provider with strata. Because the
final output changes, this is a generator version bump and an explicit world migration or fork,
not a silent regeneration. If material-aware erosion belongs in the first world, pull basic
strata and resistance forward to Phase 3 and leave folds, faults, and ores in Phase 5.

Phase 2b adds the caves and overhangs once the macro world is stable enough to preserve. Phase
3b repairs the local places where water or erosion meets that 3D detail. Phase 2c adds the LOD
hierarchy after the full-detail terrain is stable; it is a renderer project, not a meshing
tweak.

## Chosen direction

These are the current selections for the terrain track. They are direction, not implementation
contracts; the matching ADR and spec still own the details.

| Area | Choice | Consequence |
|---|---|---|
| Mesh representation | Blocky greedy | The existing block, texture, collision, light, and save model stays the reference. Isosurface extraction is out of scope unless a later ADR reopens it. |
| Versioning and persistence | Saved effective sections + audit log | A generated column is saved and never regenerated; the edit log provides undo and migration reporting; upgrade is pin, migrate, or fork, and migration transforms stored data rather than regenerating it. `VCHK` keeps loading v1–v3. |
| LOD | Voxy SeedGen-style multi-level, seed-native hierarchy | LOD sections become first-class terrain generated at their own resolution and rendered through a node hierarchy. This replaces the per-chunk derived LOD as the long-range direction. |
| Erosion and hydrology | Hydrology in v1; erosion deferred to v2. Hybrid 2.5D macro default with bounded volume-aware repair remains the v2 direction. | Rivers, lakes, and carving ship in v1 without an erosion solver; `heightfield_post` is a reserved no-op and nothing downstream reads it. |

### Interview decisions

These answers pin the first terrain track. They narrow the design; they do not replace the ADRs
and specs that will implement it.

#### World and macro terrain

| Decision | Choice |
|---|---|
| World height | 384 blocks (`0..=383`), sea level 128, flat bedrock at `y=0`, `MAX_TERRAIN_Y = 352` by construction. 12 sections of `32³` per column, all resident. |
| First world scale | Roughly a 2,000-block radius. |
| Macro grid | 4 blocks per cell, global lattice at `4k+0.5`, bilinear to blocks. |
| Height | `height = SEA + base(continentalness) + relief(roughness) * shape(peaks) * coast_mask`, `f32` throughout, quantised once by `top_solid_y = floor(h)` in a derived 130×130 `HeightTile`. |
| Hydrology | 512×512-block tiles with a 64-block border, D8 on the macro grid, priority-flood with `EPS = 1/64`, per-basin lake levels from the outlet. |
| Climate | Fixed `+x` wind march over the macro height; rain shadow and maritime coast terms feed humidity. |
| Erosion | **Not in v1.** `heightfield_post` is a reserved no-op; rivers and carving never read erosion output. |
| First vertical slice | Macro heightfield, rivers, lakes, rain shadow, biomes, strata, caves, and the 3D detail band. |
| First visual acceptance | Rivers reach the sea, lakes sit in basins, 3+ biomes show rain shadow, strata show on cliffs, overhangs appear on steep rough ground, caves open at masked mouths. |
| Next slice | Erosion solver behind `heightfield_post`, volume-aware repair, further LOD levels. |

#### Hybrid repair

| Decision | Choice |
|---|---|
| Scope | Water/cave intersections only for the first pass. |
| Region | A `32³` block box plus an 8-block margin. |
| Fallback | Mask the water, keep the cave, and record the region. |

#### LOD hierarchy

| Decision | Choice |
|---|---|
| Ladder | Three levels in v1: `1`, `2`, `4` blocks per cell, `32³` cells each. LOD1 derives from LOD0 by most-frequent-solid downsample; nothing is generated directly at a coarse level. |
| Lighting | No coarse light field and no BFS. Per-face sky approximation at mesh time, block light 0, AO 0, with a shader curve easing the LOD0/LOD1 step. |
| Water | Coarse water from the same downsample rule; water/air faces only. |
| Consistency | Most-frequent-solid wins (ties: lowest id), not conservative any-solid. Seams are covered by `SKIRT_DEPTH = 2` cell skirts. |
| Edit invalidation | LOD1/LOD2 rebuild lazily as class 4 work; LOD is never authority. |
| Performance gate | `VIEW_RADIUS = 16` at 60 FPS on the reference machine, LOD2 at the horizon, no frame over 20 ms. |
| Far terrain | LOD2 at radius 64 covers the ~2,000-block world; no separate far pass. |

#### Identity and streaming

| Decision | Choice |
|---|---|
| Content identity | One content pack pinned at world creation. |
| Edit migration | Coordinate first, stable feature ID when available; report conflicts. |
| Seed migration | Auto-create a v0 world description on load, pinned to the current generator and default content hash. |
| Removed blocks | Preserve the edit as an unknown/placeholder block and report it. |
| Baseline cache | None. Saved sections are the authority; only derived tiles may be cached, outside the world directory, and deleting them changes nothing. |
| Player guard | Remove after 64 chunks at 60 FPS with no holes; keep a debug guard. |
| Capture | World description, camera path, edit log, and stage timings. |

### 2.5D macro versus volume-aware

"2.5D" describes the erosion and hydrology input, not the final world. Both approaches still
produce a 3D block world. The chosen model is hybrid: 2.5D is the default, and a bounded
volume-aware pass repairs the places where a heightfield is not enough.

```text
2.5D macro:   height(x, z) plus a material column
              erosion and flow move material along one surface per column
              caves and overhangs are carved afterward

Volume-aware: solid(x, y, z)
              erosion and flow read and write the whole 3D volume
              water can pass under overhangs and through caves
```

The difference is what the erosion solver can see:

- A river valley, a lake basin, a mountain slope, and a waterfall are 2.5D problems.
- A river that disappears into a cave, erosion under an overhang, or a lake that drains through
  a cave are volume-aware problems.
- A heightfield cannot represent two surfaces in one column, such as a bridge over a valley or
  an overhang above a cave. An erosion pass on a heightfield cannot reason about them.

The hybrid rule:

| Situation | Solver |
|---|---|
| Open slopes, valleys, river channels, and surface lake basins | 2.5D |
| A waterfall over a cliff | 2.5D, then a local 3D check at the lip and plunge pool |
| A cave entrance, sinkhole, or water entering a cave | Volume-aware |
| Erosion or water under an overhang, arch, or bridge | Volume-aware |
| A lake or river intersecting 3D detail | Volume-aware repair |
| Anything outside a generated interaction mask | 2.5D |

The pipeline order is:

1. Build the macro heightfield.
2. Run thermal/hydraulic erosion, priority-flood, and D8 flow on it.
3. Generate caves, overhangs, arches, and other 3D detail.
4. Let the detail pass emit interaction regions: cave mouths, undercuts, water intersections,
   overhang bases, and waterfall lips.
5. Run the volume-aware repair only inside those regions, with a margin and deterministic
   region order.
6. If the repair cannot resolve a region, mask the water or cave and record it; never let it
   stall generation.

The volume-aware pass is local, not global. It reads and writes a 3D block or density window,
never the whole world. It can remove or deposit material, reroute or block water, and adjust a
cave mouth, but it must be idempotent and bounded.

Full volume-aware erosion would make every slope, river, and lake a 3D flow problem. That is a
much larger solver, much harder to keep deterministic, and mostly invisible: the difference
appears only where water or erosion meets overhangs and caves. The hybrid keeps the cheap 2.5D
solver for the common case and pays for 3D only where the result is visible.

### Voxy SeedGen-style LOD

[ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md) records the decision, and
[Seed-native LOD hierarchy](lod-hierarchy.md) expands the representation, generation, streaming,
and rendering. This section remains the terrain-pipeline summary.

Voxy and Voxy SeedGen are all-rights-reserved. Use them as behavioural prior art, not as a code
source. The architecture below is an independent interpretation.

Voxy's LOD is not a rendering-only downsample. It is a multi-resolution world:

- Five levels. Every section stores `32³` samples.
- Level 0 stores one block per sample and covers `32³` blocks.
- Level 1 stores two blocks per sample and covers `64³` blocks.
- Level 2 covers `128³` blocks, level 3 covers `256³`, and level 4 covers `512³`.
- Each section can be generated directly from the seed at its own sample spacing. It does not
  need eight full-resolution child sections first.
- SeedGen samples the air-to-solid transition instead of evaluating every cell in a column.
  That is what makes a very large radius affordable.
- The renderer walks a node hierarchy. A node is either drawn as a leaf or replaced by its
  eight children, chosen by screen-space size, distance, and occlusion.
- Coarse geometry stays visible while finer children are generated, so the hierarchy refines
  toward the player instead of popping through a hole.
- It is still blocky section meshing. It does not use Transvoxel.

For this engine, that means:

- `World` gains a first-class section at each LOD level, not just a `GpuMesh` per chunk.
- The current LOD1 is one `32³` chunk at two-block cells; a Voxy-style level 1 section is
  `64³` blocks at two-block samples. They are not the same unit.
- Generation takes a resolution and needs a surface-only fast path for coarse levels.
- Streaming requests sections by level around the player; the hierarchy is 3D.
- The renderer needs a node tree, GPU traversal, and indirect draws. That is a renderer
  project, not a meshing tweak.
- An edit at level 0 invalidates its parent sections up the hierarchy.
- Level 0 remains authoritative for collision, interaction, and overlay application. Coarse
  levels are rendering data and can be regenerated.

A practical start:

1. Keep the blocky greedy mesher.
2. Make level 1 a first-class section generated at two-block spacing over `64³` blocks, and
   compare it with the eight current LOD1 chunk meshes covering the same region.
3. Add levels 2–4 and a CPU-side node tree with distance selection.
4. Move traversal and occlusion to the GPU once the CPU hierarchy is correct.
5. Keep the current per-chunk LOD as the fallback until the hierarchy is measured.

The open questions are coarse/fine consistency (direct coarse sampling versus conservative
downsample), lighting and water at coarse levels, upward edit invalidation, and whether a
`128`-block-tall world needs a full 3D octree or a column hierarchy is cheaper.

## Non-goals

- Fluid simulation, weather, seasons, and erosion at simulation scale.
- Multiplayer, server-authoritative terrain, and networking.
- Scripting or a full modding API.
- GPU generation as the only path.
- Replacing the voxel data model with raymarching or SVDAGs before either is measured.
- Isosurface terrain (Surface Nets, Marching Cubes, Transvoxel) unless a later ADR reopens the
  mesh representation.
- Full-volume erosion and hydrology as a global solver; the volume-aware pass is local and
  interaction-driven.
- Infinite, pre-generated worlds with no residency policy.
- An editor that ships before the pipeline is observable.

## ADRs still to write

The high-level choices are recorded in Chosen direction above. The LOD hierarchy decision is
[ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md); the rows below remain open.

| ADR | Question | Trigger |
|---|---|---|
| Terrain graph | Declarative graph or fixed code stages? | H1 |
| Determinism boundary | Same binary or cross-platform bit equality? | H1 |
| Generator migration | How are edit conflicts reported and resolved? | H1 |
| Volume synthesis | Heightmap plus carve, density, or both? | H2 |
| Feature model | Two-phase index or chunk-local only? | H3 |
| Lazy LOD updates | What latency budget do lazy L3–L4 updates have? | H4 |
| Far representation | When does the hierarchy need a separate far pass? | H4/H8 |
| Save v4 | Overlay records, snapshots, and journal layout | H6 |
| Cache keys | How are macro and LOD caches keyed and shared across worlds? | H6 |
| GPU role | Acceleration with CPU authority, or GPU authority with readback? | H7 |
| Precision | Camera-relative rendering, origin rebase, or both? | H8 |

## Risks

| Risk | Mitigation |
|---|---|
| A generator change silently rewrites existing worlds. | Pin the description; version every output-changing change; require explicit migration or fork. |
| The field graph becomes a slow interpreter. | Compile the graph to a per-world evaluator; measure against fixed code. |
| Cross-platform determinism is promised too late. | Define the boundary before worlds ship; keep block hashes and capture files. |
| LOD hierarchy becomes a rendering project of its own. | Keep the per-chunk LOD as the fallback; build the Voxy-style hierarchy behind a feature flag and measure node count, traversal time, and draw cost. |
| The hybrid pass grows into a second full erosion pipeline. | Keep it bounded to interaction regions; measure how many chunks trigger it; fall back to masks. |
| Volume-aware repairs create seams between regions. | Use overlapping windows and deterministic region order; test boundary fixtures. |
| Direct coarse sampling pops visibly. | Measure at 64 chunks; add a screen-space error gate or geomorph only if the pop is worse than the fog transition. |
| The overlay and cache disagree about authority. | Cache is always disposable; overlay is always authoritative; validate both against the description. |
| A taller world breaks sky light and residency. | Treat vertical streaming as an H4 decision, not a constant change. |
| GPU acceleration forks the pipeline. | Keep one CPU reference path and make GPU paths feature-gated and measured. |
| Diagnostics arrive after the bugs. | H0 lands metrics, hashes, and replay before the pipeline grows. |

## Open questions

### Terrain and hydrology

- How are tiled macro caches keyed, versioned, and invalidated when the world description
  changes?
- Where do tiled hydrology and climate caches live, and how do they stream with the player?
- When does the world outgrow 128 blocks, and what does vertical streaming require?

### Hybrid repair

- How often can the hybrid pass trigger before it becomes a second erosion pipeline?
- How are repair seams tested across overlapping regions?
- Which interaction types come after water/cave intersections?

### LOD hierarchy

- At 64 chunks, is the five-level hierarchy enough, or is a separate far pass needed?
- What latency budget do lazy L3–L4 updates have?
- How much popping is acceptable in practice?

### World identity and streaming

- Is the 2 GB cache budget global across worlds or per world?
- What is the next vertical slice after 3D detail + hybrid repair?

## Relationship to the roadmap

The [roadmap](../roadmap.md) ends at v0.0.7, and the
[v0.0.7 scope](../dev/v0.0.7-scope.md) says further work becomes its own project. This page is
the terrain part of that project: a direction for what the pipeline could become, not a claim
that it is next.

v0.0.3 through v0.0.7 still decide the chunk data model, worker pool, LOD behaviour, lighting,
save format, and streaming policy. This page should absorb those decisions, not compete with
them. Each horizon starts with an ADR when its assumptions become real, then a spec when the
implementation is scheduled.

The executable source remains authoritative for what runs. This page is authoritative for
nothing until it becomes one of those documents.
