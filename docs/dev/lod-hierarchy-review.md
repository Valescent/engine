# LOD hierarchy design review

**Status:** Initial design review of [Seed-native LOD hierarchy](../design/lod-hierarchy.md) and
[ADR-0014](../decisions/0014-seed-native-lod-hierarchy.md), written before implementation. This page
is historical: the hierarchy landed in v0.0.9 slices 6-7. See [Post-slice status](#post-slice-status)
for the current state.

**Baseline at review time:** the v0.0.9 storage/renderer increment is committed at `5c1c89e`. At
distance 32 (9,285 chunks, 1,024 m) the world uses 334.6 MB and the frame is 0.48 ms with 2 draw
calls and 39.31 MiB of geometry reservation.

## Verdict

**Post-slice:** the hierarchy is implemented for levels 1-4 out to 8 km. The settled 2 km view is
0.466 ms frame / 0.235 ms GPU with 3.58 M vertices and 2 draws; the settled 8 km view is
0.55-0.56 ms / 0.327 ms GPU with 1,259 visible nodes and 2 draws, after the section horizon removes
646 of 1,330 in-frustum sections. All five blockers below were resolved during implementation, and L0
at detail 16 is 85.9 MB resident. Manual acceptance remains open.

**Initial verdict (historical):** the direction is sound and the first slice is worth building. The
fixed `32³` sample budget, direct seed generation, and parent-first refinement are the right shape
for long-range terrain. At review time the design was not implementation-ready: five decisions
blocked the first slice, and the memory budget did not reconcile with the 2 km/400 MB target.

## What is strong

- **The unit is right.** A section's cost is independent of its covered volume; that is the only
  way draw count stops tracking chunk count.
- **No L0 dependency.** Direct seed generation avoids generating 4,096 L0 chunks before one level-4
  mesh exists.
- **One mesher and one block palette.** Reusing the greedy contract avoids a second material and
  lighting system.
- **Parent-first refinement.** Keeping the parent until all eight children are ready prevents the
  most common hole-at-transition failure.
- **Disposable derived data.** No `VCHK` change, and a bad coarse rule is recoverable.
- **The fallback is explicit.** The per-chunk LOD remains until the new path is measured.
- **The risks are written down honestly**, including silhouette disagreement, missing caves/trees,
  approximate light, and the temporary duplication of two LOD systems.

## Blocking decisions before slice 1 (historical)

All five were resolved in the landed implementation; the table records the review as written.

| # | Decision | Why it blocks | Recommendation |
|---|---|---|---|
| 1 | **L0 / hierarchy boundary** | The design never says where per-chunk geometry stops and sections start, or how the two coexist without gaps or double walls. The current L0 mesher only knows L0 neighbours. | Define a detail radius in chunks (start at 16). Draw L0 inside it and sections outside. For the seam, let L1 overlap one section under the L0 ring and have L0 treat the overlay as absent; L1 emits its boundary faces. Add a seam test before visual comparison. |
| 2 | **Edit data for coarse rebuilds** | "Rebuild from the seed plus the modified-chunk set" is not enough: the `modified` set holds coordinates, not blocks, and an edited chunk may be unloaded with its data only in `VCHK`. | Add a bounded edit overlay (chunk coordinate to packed blocks) for modified chunks that are not resident, or read the saved chunk during a coarse rebuild. The overlay keeps the worker job independent of the live world and the disk. |
| 3 | **Coarse light seam** | Per-column sky light (`15` above, `0` below) meets the L0 BFS/AO at the detail radius. The seam will be a hard dark line at every cliff and cave mouth, which can dominate the first-slice comparison. | Run the existing BFS on the coarse occupancy for level 1 in the first slice, or blend the coarse border from the adjacent L0 light. Do not claim visual parity on per-column light alone. |
| 4 | **Sample-space meshing** | The current LOD1/LOD2 mesher downsamples L0 and samples a representative full-resolution anchor with ±1 block corners. A section is a genuine sample grid; reusing the anchor path would give the wrong AO/light neighbourhood and wrong boundary culling. | Add a sample-space mesher input: `block_at(cell)` in section cells, `scale` for positions/UVs, and neighbours in sample units. Keep the greedy contract and the water pass separate. |
| 5 | **Resident-section budget** | 334.6 MB of L0 at 1,024 m is already 84% of the 2 km/400 MB target. Coarse sections add 48-64 KiB each; the design has no section-count or memory table per view. | Add a section-count budget per level for the 2 km and 8 km cameras, and shrink the L0 detail radius when the hierarchy is on. Target: L0 at 16 chunks (about 90 MB) plus coarse rings, measured against 400 MB. |

## Slice 1 plan, refined

The design's [first slice](../design/lod-hierarchy.md#7-first-slice) is good. Sequence it as:

1. **Coordinate math.** `SectionKey { level, coord }`, extent, origin, sample origin, parent,
   child index, Euclidean division. Unit tests including negative coordinates.
2. **Edit overlay.** Add the bounded modified-chunk overlay before the generator, because every
   later test depends on it. Test: edit a block, unload the chunk, rebuild the section, observe
   the edit.
3. **Coarse generator (level 1).** Surface fast path, conservative max-height probe, water rule,
   per-column light. No caves or trees. Fixed-input tests and an exhaustive-probe comparison test.
4. **Sample-space mesher.** A `32³` grid at scale 2 with the greedy contract. Compare its output
   against the eight current LOD1 chunk meshes for one `64³` region.
5. **One section on screen.** Put it behind `ENGINE_HIERARCHY=1`, draw it at a fixed world position,
   and capture generation time, mesh time, vertices, draw count, bytes, and the L0/L1 seam.
6. **Parent/child handoff.** One parent and eight children, with a forced delay on one child; the
   parent must stay visible until the last child is ready.
7. **Metrics.** Section counts and bytes per level, generation probes, mesh time, visible nodes,
   draw calls, and seam screenshots in the per-second log. Reuse the existing metrics path.

## Tests the design is missing

- Coarse height vs exhaustive footprint maximum over a fixed region: mismatch count and maximum
  error, not just "the fast path matches".
- Water shoreline: coarse water coverage vs L0 water at a fixed coastline, including overfill.
- L0/L1 seam: no gap, no double wall, and no visible light step at a cliff and a cave mouth.
- Edit overlay: an edit made while the L0 chunk is unloaded reaches a rebuilt section.
- Parent readiness with empty children: an empty child counts as ready; the parent swaps only when
  the non-empty children are ready.
- Section unload: an ancestor is retained while any child is loading or visible.
- Memory: resident section bytes per level for a fixed camera, asserted against the budget table.

## Memory budget sketch

The target camera needs a table, not a slogan. For orientation:

| View | L0 detail | L0 bytes | Coarse sections | Coarse bytes | Total |
|---|---|---:|---:|---:|---:|
| 2 km | 16 chunks (512 m) | ~89 MB | ~1,200 at 48 KiB | ~58 MB | ~147 MB + light/mesh headroom |
| 2 km | 32 chunks (1,024 m) | 334.6 MB | ~1,200 | ~58 MB | ~393 MB, no headroom |
| 8 km | 16 chunks | ~89 MB | ~2,000 at 48 KiB | ~96 MB | ~185 MB + headroom |

The numbers are placeholders until section selection is measured. The point is that L0 at 32
chunks leaves almost no room for the 2 km target; the hierarchy must reduce the detail radius, not
add sections on top of it.

## Minor documentation issues

- `lod-hierarchy.md` §6.2 says "a node is either drawn or replaced by eight children"; state that
  the parent stays drawn until all children are ready and then swaps atomically, so no frame draws
  both.
- The design says "World or a sibling cache" owns sections. Pick the sibling cache for slice 1; it
  keeps `World` and `VCHK` untouched and makes disposal obvious.
- The `32³` section light is described as 16 KiB per nibble plane, 32 KiB total; add whether the
  coarse block-light plane is omitted (as L0 does) when no lamp is in range. Coarse generation
  places no lamps, so it should be omitted.
- The first-slice comparison against eight LOD1 chunks is fair only if the current LOD1 meshes are
  drawn from the same `64³` region and the same camera; record the exact camera and a screenshot
  pair.

## Recommendation

Build slice 1 with the five blocking decisions resolved first, and make the L0 detail radius a
config value from the start (`hierarchy.detail_chunks`). Keep the hierarchy behind
`ENGINE_HIERARCHY=1` until the seam and memory tables pass. The four-level selection and the 2 km
gate should not start until one level-1 section is generated, meshed, drawn, and compared against
the current LOD1 meshes without a seam.

## Post-slice status

Landed on the `lod-hierarchy` branch:

- `src/section.rs`: `SectionKey` level/extent/origin/parent/child math, level-1 generation with the
  conservative footprint maximum, the `SectionEdits` modified-chunk overlay, per-column coarse sky
  light, and a sample-space mesh entry point.
- `mesh::build_section_mesh` and `ChunkSnapshot::from_chunk`: the greedy mesher runs directly on a
  `32³` sample grid with a scale, with no LOD downsample.
- `light::LightField::coarse`: `15` above the column top, `0` below, no block plane.
- Tests: coordinate round trips (including negative coordinates), level-1 vs exhaustive seed rule,
  sea-level water, edit raise/clear, coarse light, section mesh shape, and edit-overlap isolation.
- `bench_section`: one level-1 section generates in 629 µs, meshes in 1,053 µs, and uses 32 KiB of
  samples + 19.2 KiB of light (19,652 B) for 2,193 quads; the eight LOD1 chunks covering the same
  volume mesh in 2,360 µs for 3,613 quads and use 281.5 KiB of samples + light.
- The quad pool keys slots by `NodeKey::{Chunk, Section}`. `ENGINE_HIERARCHY=1` stops L0 at
  `ENGINE_DETAIL_CHUNKS` (default 16), selects level-1 sections from 512-768 m and level-2 sections
  from 768-2,048 m, generates one per frame, and uploads opaque and coarse water geometry. The spawn
  view settles at 3,871 nodes (767 visible), 3.98 M vertices, 2 draws, 0.264 ms GPU, and 31.49 MiB
  of allocated geometry. The camera still looks into the near ridge, so the seam needs a view over
  open water/horizon.

Still blocking the visual gate:

- Manual acceptance and the release decision. The 2 km and 8 km gates are measured, far-level
  projected refinement/parent retention are implemented, and the capture controls are in place.

The earlier blockers are resolved: the hierarchy renderer/streaming path, the L0 detail radius and
water seam, coarse BFS light, the 2 km selection/memory table, edit invalidation, and far-level
projected refinement/parent retention. The 8 km gate holds at the benchmark's steady state
(0.55-0.56 ms frame / 0.327 ms GPU, 2 draws, 74.62 MiB allocated geometry, and 94.24 MiB
reservation); L0 detail 16 is 85.9 MB resident.

