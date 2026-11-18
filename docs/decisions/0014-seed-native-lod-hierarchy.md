# Why I Chose Seed-Native Multi-Level LOD Sections

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-17  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None. Extends ADR-0010; the current per-chunk LOD remains the interim renderer and fallback.  
**Scope:** Long-range LOD representation, generation, streaming, traversal, edit invalidation, and rendering. L0 chunk storage, the VCHK save format, the lighting model, and the existing per-chunk LOD contract are not re-decided here.

---

## Summary

This document records why the engine adopts a Voxy SeedGen-style, seed-native LOD hierarchy for long-range terrain. It is both an official decision record and a devlog entry: the direction is accepted, and I am publishing the reasoning so later work can be traced back to it.

The short version: the engine gets five LOD levels, and every section stores the same `32³` samples at a spacing of `2^level` blocks. A level-1 section covers `64³` blocks, a level-4 section covers `512³` blocks, and each section can be generated directly from the seed at its own spacing. Coarse generation takes a surface-only fast path: it finds the air-to-solid transition from the two-dimensional height field and fills columns, instead of evaluating every block and every three-dimensional cave sample. A parent-first node tree keeps coarse geometry visible while finer sections generate, so refinement does not punch holes. Level 0 stays authoritative for collision, edits, lighting, and saves; coarse sections are derived rendering data. The current LOD0/LOD1/LOD2 path stays as the fallback until the first slice is measured.

The cost is a coarse/fine mismatch, missing small detail at coarse levels, a renderer project for the node tree, and a second generation path. I accept those costs because per-chunk LOD and per-chunk draws stop scaling at the render distances this engine is aiming for.

---

## 1. Context

### 1.1 The project

The current engine generates a 128-block-tall world from a seed:

- Two low-frequency two-dimensional noise fields drive `height(x, z)`.
- A temperature, humidity, and erosion classification selects one of three surface biomes.
- A three-dimensional cave field carves underground air.
- Water fills to sea level 32, and deterministic trees stand on plains and hills.
- Chunks are `32³` blocks, and `.vchk` files store edited L0 chunks as the authority.

Rendering is per chunk. LOD0 is the full block mesh. LOD1 and LOD2 are derived by downsampling the same chunk's `32³` block array to `16³` and `8³` cells. The renderer keeps one mesh per chunk and LOD and draws them separately. The v0.0.8 target is a `24`-chunk render distance at 60 FPS, with a possible `32`.

That model has a scaling limit. A render distance of 64 chunks is a `129 × 129` chunk square, or roughly 16,600 chunks. At that size, even one draw per visible chunk is too many, and every LOD mesh is still a separate derived object that needs its L0 neighbourhood. The mesh count and draw count grow with the chunk count, not with the covered volume.

The terrain-pipeline design already selects a Voxy SeedGen-style hierarchy as the long-range direction. Voxy and Voxy SeedGen are all-rights-reserved; this decision uses them as behavioural prior art only and does not copy their code. The architectural observation is independent: a hierarchy whose sections grow with LOD can keep a fixed per-section sample budget while covering eight times more world volume per level.

### 1.2 The constraints

My choice is constrained by the following:

- Generation must stay deterministic from the seed and independent of chunk order.
- The blocky greedy mesher, block palette, save format, and L0 block model stay the reference.
- L0 remains authoritative for collision, interaction, lighting, and edits.
- Coarse sections are rendering data and must be disposable.
- Worker generation and meshing stay on the existing pool; the main thread must not block on them.
- The first performance gate is a 64-chunk radius at 60 FPS before GPU traversal is justified.
- The world is 128 blocks tall for now; vertical streaming is deferred.
- The current per-chunk LOD must remain usable until the hierarchy is measured.
- Isosurface meshing remains out of scope.
- The system must degrade gracefully when a section is missing or invalid.

---

## 2. Decision

**Long-range LOD is a five-level, seed-native section hierarchy.**

| Level | Sample spacing | Section extent | Samples per section | Vertical sections in a 128-block world |
|---|---|---|---|---|
| 0 | 1 block | `32³` blocks | `32³` | 4 |
| 1 | 2 blocks | `64³` blocks | `32³` | 2 |
| 2 | 4 blocks | `128³` blocks | `32³` | 1 |
| 3 | 8 blocks | `256³` blocks | `32³` | 1, half air |
| 4 | 16 blocks | `512³` blocks | `32³` | 1, mostly air |

The decision includes:

- **Section identity.** A section is `(level, coord)`, with world origin `coord × 32 × 2^level`. Level 0 coordinates are the existing `ChunkCoord`. A parent at level `L + 1` is `coord.div_euclid(2)` and has eight children.
- **Direct seed generation.** Every level can be generated from the seed at its own sample spacing. A coarse section does not wait for eight full-resolution children and does not downsample them.
- **Surface-only fast path.** For each of the `32 × 32` sample columns, coarse generation evaluates the height field over the sample footprint, finds the top solid sample, and fills the column from that transition. It does not evaluate the cave field or per-block density for every cell in the column.
- **Conservative sampling.** A coarse sample is solid when any block in its footprint is solid. The footprint probe takes the maximum height; the first slice measures the probe against an exhaustive maximum and records mismatches. Overfill is preferred to a hole.
- **Detail policy.** L0 carries caves, trees, and full water. L1 carries the height surface and water, and may sample caves near the surface if the first slice shows a visible problem. L2–L4 skip caves and trees. Water is represented at every level as a separate coarse water surface.
- **Coarse lighting.** Coarse sections compute light from their own occupancy, not by mipping L0 light. The first slice uses per-column sky light and zero block light; later slices run the existing BFS on the coarse sample grid.
- **Node hierarchy.** The renderer walks a tree. A node is drawn as a leaf or replaced by its eight children using projected size or screen-space error. A coarse parent stays visible until all eight children are ready.
- **L0 authority.** Collision, interaction, edits, light storage, and saves use L0. Coarse sections are derived and can be regenerated.
- **Edit invalidation.** An L0 edit invalidates overlapping coarse sections immediately to level 2 and lazily beyond. A rebuilt coarse section uses the seed plus the modified-chunk set, not a live scan of every child.
- **Staged renderer.** Selection and traversal start on the CPU. GPU traversal and indirect draws come only after the CPU hierarchy is correct and the performance gate is measured.
- **Fallback.** The existing LOD0/LOD1/LOD2 path remains the default until the first slice passes its comparison and the current LOD is retired by a later decision.

The exact APIs, byte layouts, and milestone contracts belong to the spec that schedules the work. This ADR records the representation and the direction.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose This LOD Hierarchy

### 3.1 A fixed sample budget scales with covered volume

Every section has `32³ = 32,768` samples. Level 1 covers eight times the volume of level 0, level 2 covers eight times level 1, and so on. A level-4 section covers the same volume as 4,096 L0 chunks, but it still has one `32³` mesh and one draw. That is the property that makes a 64-chunk radius tractable: draw count tracks the number of visible hierarchy nodes, not the number of chunks.

### 3.2 Direct generation removes the eight-child dependency

The current LOD1 and LOD2 need the L0 chunk's block array. A coarse hierarchy that waited for eight children would inherit the full cost of generating them; at level 4 that is 4,096 chunks before one mesh exists. Seed-native generation breaks that dependency. A coarse section is a function of the seed, its level, and its coordinate, so it can be generated in one job and discarded without touching L0.

### 3.3 The surface fast path fits this generator

The height field is two-dimensional. A coarse column does not need the three-dimensional cave field, the per-block material rule, or the tree pass to find its top solid sample. It needs the maximum height over the sample footprint and the sea-level rule. That is the SeedGen insight adapted to this engine: sample the transition, not every cell. It is also why this design is practical here before a general density-field engine would be.

### 3.4 The hierarchy hides refinement instead of popping through a hole

A missing child must not become a hole. The rule is parent-first: keep drawing the coarse parent until all eight children are ready, then swap. The hierarchy refines toward the player; it does not tear down geometry first. This is the same insurance as LOD1's always-emitted boundary faces, moved up a level.

### 3.5 One blocky mesher still serves every level

Every level is still a `32³` voxel grid with a block type per sample. The existing greedy mesher can mesh it with a sample scale of `2^level`. The face order, UV-in-block-units contract, and vertex layout stay the same. There is no second isosurface pipeline and no per-level mesher.

### 3.6 Coarse sections are derived, so mistakes are recoverable

Coarse sections are not saved. If a coarse rule is wrong, the section can be regenerated. If an edit lands, the affected section can be rebuilt from the seed plus the modified set. L0 remains the authority, so the hierarchy cannot corrupt collision, interaction, or saved world state.

### 3.7 Why not keep the per-chunk LOD, mip L0 light, or build a chunk octree?

- **Keep the current per-chunk LOD.** It is simple and already works, but its mesh and draw count grow with chunk count. LOD2 delays the limit; it does not remove it.
- **Mip L0 meshes or light into coarse meshes.** Every coarse mesh would still need a neighbourhood of full-resolution data first. It also makes coarse lighting a copy of fine lighting rather than a coarse computation.
- **Octree of `32³` chunks.** A chunk octree groups the existing chunks, but each node still stores or derives eight children at full resolution. It does not give a coarse section its own seed-native representation.
- **A separate far mesh or heightfield ring.** It can cover the horizon cheaply, but it is a second representation with its own seam, edit, and lighting rules. The hierarchy is one representation that refines.
- **Isosurface terrain.** It would change the block, texture, collision, and save model. This engine is blocky; the hierarchy is blocky too.
- **GPU-only generation.** It would make the CPU reference path and headless tests disappear. The design keeps CPU generation authoritative and treats GPU work as an acceleration.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Five-level seed-native sections with a surface fast path** | Fixed `32³` sample budget, direct seed generation, one mesher, parent-first transitions. | **Selected** |
| **Keep LOD0/LOD1/LOD2 per chunk** | Simple and already implemented; mesh and draw count grow with chunk count. | Retained as fallback |
| **Downsample eight L0 children into a coarse section** | Exact consistency; inherits full L0 generation cost and memory. | Not selected for long range |
| **Mip L0 light into coarse light** | Reuses fine lighting; coarse mesh still needs full-resolution children. | Not selected |
| **Chunk octree without coarse sampling** | Groups existing chunks; no seed-native coarse representation. | Not selected |
| **Separate far mesh or horizon ring** | Cheap horizon; adds a second representation and seam rules. | Deferred |
| **Isosurface or Transvoxel LOD** | Smooth surfaces; changes the block representation. | Out of scope |
| **GPU-only traversal and generation** | High ceiling; removes the CPU reference and headless tests. | Deferred until the CPU hierarchy is measured |
| **Precomputed static far terrain** | Fast to draw; cannot follow edits or a new seed without regeneration. | Not selected |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **Direct coarse sampling can disagree with L0 at silhouettes.** | Accepted. Conservative any-solid sampling overfills rather than erases; the first slice measures pop and mismatch. A geomorph or a finer L1 probe is a later decision. |
| **A level-1 section does not reduce sample count against eight LOD1 chunks.** | Accepted. Both cover `64³` blocks with `32³` cells. The level-1 win is one generation job, one mesh, one draw, and no L0 dependency; the larger win arrives at levels 2–4. |
| **Caves and trees disappear at coarse levels.** | Accepted. They are small at hierarchy distances. L1 may sample caves near the surface if the comparison shows a distracting filled entrance; L2+ stay surface-only. |
| **Coarse lighting is approximate.** | Accepted. Per-column sky light is the first slice; the BFS on coarse occupancy comes later. Coarse light is not mipped from L0. |
| **The node tree is a renderer project.** | Accepted. Selection and traversal start on the CPU behind a flag; the current LOD remains the fallback until the gate passes. |
| **Coarse sections consume memory and streaming slots.** | Accepted. A section is 32 KiB of samples plus light; the hierarchy has far fewer sections than chunks. Unload and parent retention are explicit. |
| **Edit invalidation can cascade.** | Accepted with qualification. Invalidate immediately to level 2 and lazily beyond; rebuild from the seed plus the modified set. Measure the rebuild cost. |
| **Levels 3 and 4 contain large air volumes in a 128-block world.** | Accepted. One vertical section covers the world at each of those levels; a column hierarchy is an open question if vertical streaming arrives. |
| **More levels mean more transitions.** | Accepted. The node selection uses projected error and hysteresis, not fixed distance bands. The parent-first rule prevents holes at every transition. |
| **It duplicates the current LOD path during migration.** | Accepted. The fallback is deliberate; retiring the old path is a later decision after measurements. |
| **Determinism must survive a second generation path.** | Accepted. Coarse generation is a pure function of seed, level, and coordinate. Tests pin fixed inputs and outputs. |

---

## 6. Consequences

### 6.1 Positive

- Draw and mesh count scale with hierarchy nodes instead of chunk count.
- Coarse sections generate directly from the seed and do not require L0 children.
- The surface fast path skips three-dimensional cave and tree work at coarse levels.
- One greedy mesher and one block palette cover every level.
- Parent-first refinement cannot punch holes while children generate.
- Coarse sections are disposable; L0 remains authoritative.
- The design can be introduced behind a flag without removing the current LOD.
- The first slice is measurable against eight existing LOD1 chunk meshes.

### 6.2 Negative

- Coarse silhouettes can differ from L0 and pop at transitions.
- Caves and trees are absent above the detail cutoff.
- Coarse lighting and water need their own rules.
- The renderer gains a node tree, selection logic, and eventually indirect draws.
- Streaming gains per-level residency and parent-retention rules.
- Edits need upward invalidation and a modified-set-based rebuild path.
- The engine temporarily carries two LOD systems.

### 6.3 Neutral

- The VCHK save format and world seed contract are unchanged.
- Coarse sections are derived cache data, not world state.
- The current per-chunk LOD remains valid until the new path is measured.
- GPU traversal, occlusion culling, and vertical streaming remain future decisions.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Coarse overfill or underfill produces visible holes or walls. | Use conservative any-solid sampling; compare the footprint probe with an exhaustive maximum; test cave mouths and cliffs. |
| Parent/child transitions pop. | Keep the parent until all eight children are ready; use projected-error hysteresis; record pops in the first-slice capture. |
| Coarse sections become expensive to generate at level 4. | Keep the surface fast path free of 3D cave and tree work; cap the probe count; measure per-level generation time. |
| The node tree is never finished. | Stage it: CPU selection first, GPU traversal only after the 64-chunk gate. Keep the current LOD fallback. |
| Edit invalidation cascades too far. | Immediate to level 2, lazy beyond; rebuild from the modified set; coalesce dirty sections per frame. |
| Coarse water cracks against L0 water. | Render coarse water as a separate mesh with the same sea-level rule; test shorelines and the L0/L1 boundary. |
| Coarse light looks wrong next to L0 light. | Start with per-column sky light; run the existing BFS on the coarse grid before relying on visual parity. |
| The second generation path drifts from L0 rules. | Pin the shared height, biome, sea-level, and material helpers; add fixed-output tests for both paths. |
| Memory grows with both LOD systems. | Unload coarse sections outside the hierarchy; release the fallback path's meshes when the flag is off; measure resident memory. |
| A future generator change breaks coarse output. | Version the generator and the coarse rule together; coarse sections are disposable, but their inputs must be pinned. |

---

## 8. Conclusion

The five-level, seed-native LOD hierarchy is my deliberate, measurement-gated choice for long-range terrain. I am selecting it because a fixed `32³` sample budget keeps every section's mesh cost bounded while coverage grows eightfold per level, because direct seed generation removes the eight-child dependency, because this heightmap generator can find the surface without evaluating every cell, and because a parent-first node tree refines without holes. I acknowledge the coarse/fine mismatch, missing caves and trees, approximate coarse lighting, the renderer work, and the temporary duplication of the current LOD path. I manage those through conservative sampling, the parent-retained transition, a CPU-first renderer, immediate-to-level-2 edit invalidation, and the existing per-chunk LOD as the fallback. I therefore accept this decision as the engine's long-range LOD direction, with the exact contracts left to the milestone spec and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on long-range LOD representation, generation, traversal, or invalidation.*
