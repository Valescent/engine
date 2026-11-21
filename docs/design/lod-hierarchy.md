# Seed-native LOD hierarchy

**Status:** Design direction, accepted by [ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md). Not a milestone contract.

**Scope:** Long-range terrain representation, coarse generation, streaming, traversal, edit invalidation, and rendering. The current chunk, save, and lighting contracts remain authoritative until a milestone spec replaces the relevant parts.

This page describes the Voxy SeedGen-style hierarchy selected for long-range terrain. Voxy and Voxy SeedGen are all-rights-reserved; this is an independent interpretation of their behaviour, not a code source. The executable source remains authoritative for what runs. The current LOD0/LOD1/LOD2 path stays the fallback until the first slice is measured.

## 1. The problem with per-chunk LOD

The current LOD is derived per `32³` chunk. Every farther LOD reduces the cells inside that same chunk, but it does not reduce the number of chunks, meshes, or draws. Covering more distance therefore multiplies objects instead of growing the unit of work.

| Covered volume | L0 chunks | Current per-chunk LOD | Voxy-style section |
|---|---|---|---|
| `64³` blocks | 8 | 8 LOD1 meshes, `32³` cells total | 1 level-1 section, `32³` samples |
| `128³` blocks | 64 | 64 LOD2 meshes, `32³` cells total | 1 level-2 section, `32³` samples |
| `256³` blocks | 512 | 512 LOD2 meshes, 262,144 cells total | 1 level-3 section, `32³` samples |
| `512³` blocks | 4,096 | 4,096 LOD2 meshes, `2,097,152` cells | 1 level-4 section, `32³` samples |

The key property is the fixed sample budget. A section always stores `32³` samples. Its sample spacing doubles with the level, so its covered volume grows eightfold while its mesh, generation job, and draw count stay constant. The hierarchy does not make each cell cheaper; it makes the unit of work larger.

## 2. Level model

| Level | Sample spacing | Section extent | Samples | Vertical sections in a 128-block world |
|---|---|---|---|---|
| 0 | 1 block | `32³` blocks | `32³` | 4 |
| 1 | 2 blocks | `64³` blocks | `32³` | 2 |
| 2 | 4 blocks | `128³` blocks | `32³` | 1 |
| 3 | 8 blocks | `256³` blocks | `32³` | 1, half air |
| 4 | 16 blocks | `512³` blocks | `32³` | 1, mostly air |

Level 0 is the existing chunk. Level 1 is a first-class `64³` section generated at two-block spacing, not a `32³` chunk downsampled to `16³` cells. They cover the same `64³` block volume as eight L0 chunks, but they are different units.

### 2.1 Coordinates

For level `L`, let `S = 1 << L`.

```text
section_extent   = 32 * S
section_origin   = coord * section_extent
sample_origin    = section_origin + local * S
parent           = (L + 1, coord.div_euclid(IVec3::splat(2)))
child_index      = coord.rem_euclid(IVec3::splat(2))
```

Every component uses Euclidean division so negative coordinates map consistently. A parent has eight children. A child covers half of its parent's extent on each axis.

A section whose origin is above the world or whose top is below block 0 is empty. Empty sections are not stored. At levels 3 and 4, the single vertical section still contains 16 or 24 air sample layers above the terrain; a column hierarchy is an open question, not part of this direction.

## 3. Section data

A derived section needs:

| Field | Purpose |
|---|---|
| `key` | `(level, coord)`; the identity used by streaming, edits, and the node tree. |
| `samples` | `32³` block samples; 32 KiB with a one-byte `Block`. |
| `light` | Coarse sky and block light; 16 KiB per nibble plane, 32 KiB total. |
| `revision` | Discards stale worker results after an edit or re-request. |
| `state` | Empty, queued, generating, light-ready, mesh-ready, or failed. |

A sample is a filtered volume, not a block coordinate:

- **Solid** when any block in its `S × S × S` footprint is solid.
- **Water** when no block in the footprint is solid and at least one is water.
- **Air** otherwise.
- The stored block type is the representative surface type, not a consensus over every block.

Water wins no cell that also contains solid. The conservative any-solid rule overfills a shoreline by at most one sample; it never erases a solid surface. The coarse water mesh is built from the water samples in a separate pass.

Coarse sections are derived data. They are not written to `world/chunks/`, do not enter `VCHK`, and can be deleted without changing the world. Level 0 remains the saved authority.

## 4. Coarse generation

### 4.1 Surface-only fast path

The current generator is a height field plus a three-dimensional cave field, water fill, and trees. Coarse generation uses the height field and the sea-level rule, and skips the cave and tree passes at levels 2–4.

For each of the `32 × 32` sample columns:

```text
s = 1 << level
origin = coord * 32 * s

for each sample column (sx, sz):
    x0 = origin.x + sx * s
    z0 = origin.z + sz * s
    top_height = max height over the sample footprint

    for each sample layer sy:
        y0 = origin.y + sy * s
        if y0 <= top_height:
            solid sample
        else if y0 <= SEA_LEVEL:
            water sample
        else:
            air sample
```

The expensive part is the noise evaluation, not the write. A column write is `32` sample stores. The fast path avoids evaluating the cave field and the per-block material rule for every block in the covered volume. That is the SeedGen-style saving: find the transition, then fill a blocky column.

The representative column is the footprint column with the maximum height; ties choose the lowest X, then the lowest Z. The surface sample uses that column's biome and the `surface_block` depth rules. Samples below the surface use stone or the appropriate subsurface block. This keeps grass on top of grass and dirt under it without a second material system.

### 4.2 Conservative footprint sampling

The height field varies inside one coarse sample. Taking the maximum height over the footprint makes the sample solid when any corner is solid. It can overfill, but it cannot leave a hole under a higher corner.

| Level | Footprint columns | First-slice probe |
|---|---|---|
| 1 | `2 × 2` | All four block columns. |
| 2 | `4 × 4` | All sixteen block columns, or a `3 × 3` probe if measurement allows. |
| 3 | `8 × 8` | `3 × 3` probe: corners, edge midpoints, centre. |
| 4 | `16 × 16` | `3 × 3` probe: corners, edge midpoints, centre. |

The first slice compares the probe result with an exhaustive maximum and records the mismatch rate and where it occurs. A mismatch that produces a visible hole is a bug; a mismatch that adds a block at a silhouette is accepted overfill. If the sparse probe misses a cliff, raise the probe count for that level.

`top_height` maps to a sample layer with Euclidean division:

```text
top_sample = (top_height - origin.y).div_euclid(s)
```

Clamp the write range to `0..32`; a section above the surface is all air, and a section entirely below it is all solid. Water uses `(SEA_LEVEL - origin.y).div_euclid(s)` and only fills samples above the solid top.

### 4.3 Detail policy

| Level | Surface | Caves | Trees | Water | Lighting |
|---|---|---|---|---|---|
| 0 | Full block rules | Full 3D cave field | Full tree pass | Full water | Existing BFS and AO |
| 1 | Height surface | Optional near-surface sample; off by default in the first slice | None | Coarse water | Per-column sky; BFS later |
| 2–4 | Height surface | None | None | Coarse water | Per-column sky; BFS later |

Caves are invisible at hierarchy distance except where they meet the surface. A filled cave entrance at level 1 is the first visual risk to check. If it is distracting, L1 samples the cave field only in the top few sample layers; it does not evaluate the full column. Trees are small enough to drop above L0; a coarse tree pass would need its own placement and ownership rules.

The detail policy is a level rule, not a per-section heuristic. Every section at a level uses the same rule so the output is deterministic and comparable.

### 4.4 Cost shape

| Level | L0 chunks covered | L0 block columns | L0 cell evaluations (up to) | Coarse height probes | Coarse sample writes |
|---|---|---|---|---|---|
| 1 | 8 | 4,096 | 262,144 | 4,096 | 32,768 |
| 2 | 64 | 16,384 | 2,097,152 | 9,216–16,384 | 32,768 |
| 3 | 512 | 65,536 | 16,777,216 | 9,216 | 32,768 |
| 4 | 4,096 | 262,144 | 134,217,728 | 9,216 | 32,768 |

The L0 comparison is the cost of generating the same volume at full resolution, including the 3D cave field. It is an upper bound because caves only evaluate below the surface, but the trend is the point: the coarse section's work stays flat while the covered volume grows. A level-4 section can be generated when its 4,096 L0 children have not been generated at all.

The first slice measures real timings rather than relying on this table. Record generation time per section, probes per section, and the ratio to the eight-LOD1 baseline.

### 4.5 Determinism

Coarse generation is a pure function of the seed, level, and coordinate:

- No global RNG and no chunk-order dependence.
- Fixed iteration order: `sz`, then `sx`, then `sy`.
- The same `height`, `biome`, `sea_level`, and `surface_block` helpers as L0.
- Euclidean division for negative coordinates.
- No floating-point accumulation that depends on worker count.

A generator change that alters coarse output is a generator version change even though coarse sections are not saved. The seed and description stay pinned; the derived cache is disposable, but its inputs must be recorded.

### 4.6 Coarse lighting

Coarse light is computed from coarse occupancy, not mipped from L0. The first slice uses:

- Sky light `15` in every sample above the solid/water top of its column.
- Sky light `0` below that top.
- Block light `0`; the generator places no lamps.

That is enough to light the surface and keep the coarse terrain from being fullbright. Later slices run the existing BFS on the `32³` sample grid with a one-sample border from neighbouring sections. Ambient occlusion is computed on the coarse grid by the mesher, exactly as it is for L0. Coarse light is not a copy of L0 light and is not expected to match it sample for sample.

## 5. Streaming and residency

### 5.1 Ownership

- `World` owns L0 chunks as it does now.
- `World` or a sibling cache owns derived sections keyed by `(level, coord)`.
- Level 0 remains authoritative; a coarse section can be rebuilt or discarded at any time.
- The cache is outside the world directory. Deleting it changes nothing.

### 5.2 Request order

The hierarchy is requested parent-first:

1. Determine the target node set from the camera and the projected-error rule.
2. Request ancestors before descendants.
3. Keep a coarse parent visible until all eight children are ready.
4. Cancel a request whose section is no longer on screen or whose revision is stale.

Requests use the same worker pool and bounded queue as chunk generation. A coarse job is larger in covered volume but smaller in per-block work; the queue treats it as one job with a level and coordinate. A failed coarse job leaves the parent visible and records the failure; it does not stall the hierarchy.

### 5.3 Unload

A section unloads when it is outside the hierarchy target and not an ancestor of a target. Prefer dropping the farthest leaves first. Retain a parent while any child is loading or visible. L0 unload stays independent; a coarse section does not keep a full L0 neighbourhood resident.

### 5.4 Edits and invalidation

An L0 edit is authoritative and must reach the coarse representation:

- Mark the edited L0 chunk dirty as today.
- Find the coarse sections whose volume contains the edited block.
- Invalidate levels 1 and 2 immediately; queue levels 3 and 4 lazily when they are visible.
- Rebuild from the seed plus the modified-chunk set. Coarse generation does not scan live L0 children.
- Increment the section revision so stale worker results are discarded.

For the first slice, an edit can invalidate more sections than strictly necessary; the coarse rebuild is cheap. A per-section edit index is a later optimization if the modified set grows large. Hidden underground edits still bump the revision because the coarse surface may not change, but the rebuild is bounded.

The invalidation rule is deliberately asymmetric: near levels rebuild immediately because a visible pop there is obvious; far levels can wait until the section is selected again. The lazy budget is an open question.

### 5.5 Persistence

- `VCHK` and `world/seed` are unchanged.
- Coarse sections are never saved as world state.
- A derived cache may live outside the world directory; it is disposable, keyed by world description and generator version, and deleting it changes nothing.
- Loading a world regenerates coarse sections from the seed and reapplies edits through the modified set.

## 6. Rendering

### 6.1 Meshing

Every level is a `32³` voxel grid. Reuse the greedy mesher with:

- Sample scale `S = 2^level`.
- Section world origin for position placement.
- The same face order, UV-in-block-units contract, vertex layout, AO, and light packing.
- A separate water mesh from water samples.

Boundary rules:

- Within one level, cull faces against the neighbouring section's samples as usual.
- If a boundary neighbour is missing or coarser, emit the boundary face. This is the same insurance as LOD1's always-emitted boundary faces; it can overdraw, but it cannot crack.
- Keep adjacent visible nodes within one level where selection allows it. The parent-retained transition covers the moment before children are ready.

### 6.2 Node selection

A node is either drawn or replaced by eight children. Selection uses projected size or screen-space error, not a fixed distance band:

```text
projected_error = section_extent / distance * pixels_per_radian
refine when projected_error > refine_threshold
collapse when projected_error < collapse_threshold
```

The refine threshold is larger than the collapse threshold, giving hysteresis. A node can refine only when all eight children are mesh-ready; otherwise the parent stays. Frustum culling rejects whole subtrees. Occlusion culling is a later addition, not part of the first slice.

### 6.3 CPU first, GPU later

The first implementation keeps traversal on the CPU and produces a draw list of leaf sections. It must pass the performance gate before GPU traversal is justified:

- 64-chunk radius at 60 FPS.
- No main-thread frame above 33 ms once the first ring is ready.
- Visible node count and draw count recorded.

GPU traversal, indirect draws, and occlusion move the same node data to the GPU after the CPU hierarchy is correct. They do not change the generation or invalidation rules.

### 6.4 Water, fog, and transitions

- Coarse water is a separate transparent mesh; it is not merged into the opaque mesh.
- Water uses the same sea-level rule at every level and renders after opaque geometry with depth writes off, as today.
- Fog starts and ends at the hierarchy's current render limit. A coarse section beyond the fog end is not requested.
- The current per-chunk LOD remains the fallback renderer until the hierarchy passes its comparison.

## 7. First slice

The first slice is deliberately one level and one comparison. It proves the unit and the fast path before the renderer becomes a project.

1. Add level, coordinate, origin, parent, and child math with unit tests, including negative coordinates.
2. Add `generate_section(level = 1)` with the surface fast path, conservative footprint probe, water, and per-column sky light. No caves or trees.
3. Add a `32³` coarse mesh path at scale 2 using the existing greedy mesher.
4. Put the path behind a feature flag or environment switch. Draw one level-1 section covering a `64³` block region and compare it with the eight current LOD1 chunk meshes covering the same region.
5. Compare generation time, vertex count, draw count, memory, and visual result. Record pops at cave entrances and shorelines.
6. Add levels 2–4 and a CPU node tree with parent-first selection and hysteresis.
7. Add immediate edit invalidation for levels 1–2 and lazy invalidation above.
8. Measure the 64-chunk gate. Move traversal to the GPU only if the CPU hierarchy passes and draw submission is the remaining cost.

Acceptance for the slice:

- One level-1 section covers `64³` blocks and is generated without L0 children.
- The parent stays visible while children generate; no hole appears.
- The current LOD fallback still passes its checks.
- Generation time, vertex count, draw count, and resident memory are recorded next to the baseline.
- Cave-mouth and shoreline pops are captured and judged, not ignored.

## 8. Tests and metrics

### 8.1 Unit tests

- Level extent, origin, sample origin, parent, and child index, including negative coordinates.
- A coarse sample is solid when any block in its footprint is solid.
- A coarse sample is water only when no solid block is present.
- Surface material follows the representative column and depth rules.
- Sea-level fill stops at the coarse water top.
- The fast path matches an exhaustive footprint maximum on fixed height fixtures.
- An out-of-world section is empty.
- Coarse light is `15` above the surface and `0` below in the first slice.
- Meshing at scale `S` scales positions and UVs and keeps greedy face order.
- A missing or coarser boundary neighbour emits the boundary face.
- Parent-first selection keeps the parent until all eight children are ready.
- Hysteresis does not flicker at the refine/collapse boundary.
- An edit invalidates the overlapping level-1 and level-2 sections and leaves a stale revision unusable.

### 8.2 Metrics

Record per level:

- Generation time per section and probes per section.
- Mesh time and vertex count per section.
- Visible node count, leaf count, and draw count.
- Resident section count and bytes.
- Pops or holes observed at level transitions.
- Main-thread frame time and queue depth.

The existing per-second debug line and Tracy plots should gain section metrics rather than a second logging system.

## 9. Risks and open questions

| Risk or question | Current position |
|---|---|
| How many footprint probes are enough? | Start with the table in 4.2; compare with an exhaustive maximum and raise the count where a mismatch creates a hole. |
| Do cave entrances at level 1 need a near-surface cave sample? | Test the first slice; add it only if the filled entrance is worse than the extra cost. |
| Are trees missed at level 1? | Accepted for now. A coarse tree pass would need its own placement and edit rules. |
| Does coarse water crack against L0 water? | Test shorelines and the L0/L1 boundary; emit boundary faces and keep the water mesh separate. |
| Is per-column sky light enough? | It is the first slice. Move to the BFS on coarse occupancy before claiming visual parity. |
| How expensive is edit invalidation? | Immediate to level 2, lazy above; measure rebuild time and coalesce dirty sections per frame. |
| Is a full 3D octree necessary at 128 blocks tall? | The current direction uses one vertical section at levels 2–4. Revisit only if vertical streaming or memory demands it. |
| When is a separate far pass needed? | The five-level hierarchy is the far pass for now. Reopen if the 64-chunk gate fails or the world grows beyond the hierarchy's coverage. |
| When does GPU traversal land? | After the CPU hierarchy passes the gate and draw submission is the measured bottleneck. |

## 10. Relationship to the current code

| Area | Current code | Change when scheduled |
|---|---|---|
| Generation | `src/v0/generation.rs` height, biome, cave, water, trees | Add a coarse surface-only path that shares the height and sea-level helpers. |
| World data | `src/v0/world.rs` dense L0 chunks, modified set, `VCHK` | Add derived section storage and edit invalidation; keep L0 authority and the save format. |
| Meshing | `src/v0/mesh.rs` greedy mesher, LOD1/LOD2 downsample | Add a `32³` sample-grid input with a scale and section origin; keep the greedy contract. |
| Jobs | `src/v0/mesher.rs` generation, light, mesh jobs | Add section jobs with level, coordinate, and revision. |
| Streaming | `src/v0/streaming.rs` load rings and readiness | Add parent-first section targets and unload rules; L0 streaming stays. |
| Renderer | `src/renderer.rs` per-chunk LOD buffers | Add per-section buffers, node traversal, and a draw list; GPU indirect later. |
| Config | `src/config.rs` render distance | Add a hierarchy switch and thresholds during measurement. |
| Tests | `src/lib.rs` benchmark-shaped tests | Add section coordinate, generation, meshing, and selection tests. |

## Non-goals

- Isosurface terrain, Surface Nets, Marching Cubes, or Transvoxel.
- SVDAG or raymarching representation work.
- A save-format change for derived sections.
- GPU-only generation or traversal as the first implementation.
- A separate far pass before the hierarchy is measured.
- Vertical streaming for a world taller than 128 blocks.
- General-purpose LOD for entities, water simulation, or gameplay systems.

## Related documents

- [Terrain pipeline](terrain-pipeline.md) — the wider terrain direction and the interview decisions.
- [ADR-0010](../decisions/0010-two-level-lod-for-chunks.md) — the original two-level LOD and its transition rules.
- [ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md) — the decision this page expands.
