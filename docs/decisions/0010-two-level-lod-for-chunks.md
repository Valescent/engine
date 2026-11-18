# Why I Chose Two LOD Levels with Conservative 2×2×2 Downsampling

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted; extended by v0.0.8, which adds LOD2 (4×4×4) without changing the conservative downsample rule.  
**Date:** 2026-11-15  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** Chunk LOD selection, LOD1 downsampling, transition boundaries, and cache policy for rendering. Collision, block edits, lighting data, saves, streaming, and the greedy mesher's output contract are covered by the milestone specs and are not re-decided here.

---

## Summary

This document records why the engine uses two chunk LODs: LOD0 at full block resolution, and LOD1 built from a conservative 2×2×2 downsample. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: two levels are enough to cut distant chunk geometry without a full LOD system, and a 2×2×2 cell that is solid when any source block is solid preserves silhouettes and avoids holes. LOD1 quads are greedy-meshed at half resolution, use the topmost solid block for texture, light, and AO, and always emit chunk-boundary faces so LOD transitions cannot crack. The cost is overdraw at chunk borders, some silhouette overfill, and a deliberately coarse two-level pop under hysteresis.

---

## 1. Context

### 1.1 The project

v0.0.5 adds distance rendering on top of the v0.0.4 block and texture model:

- Chunks are 32×32×32 and meshed independently.
- Greedy meshing reduces geometry per chunk, but distant chunks still cost vertices and draws.
- The renderer batches one draw per chunk and LOD.
- Full block data must remain for collision, editing, lighting, and saving; LOD is rendering-only.
- The v0.0.5 stress fixture is an 8×8 grid of 64 chunks, and the render-distance setting is 8–16 chunks.
- v0.0.6 adds per-vertex light/AO and transparent water; v0.0.7 adds streaming.
- Worker jobs and revisions already carry a target LOD.

The LOD strategy must therefore reduce distant geometry without changing world state, and it must not introduce visible cracks while chunks use different LODs.

### 1.2 The constraints

My choice is constrained by the following:

- Two LOD levels are in the v0.0.5 scope; more levels are deferred.
- LOD1 must be generated from full block data, not from a saved lower-resolution copy.
- LOD1 must use the same greedy face order and UV-in-block-units contract.
- LOD1 must not change collision, edits, lighting storage, or save data.
- Transition boundaries must not leave holes.
- LOD selection needs hysteresis to avoid flicker.
- LOD meshes are built asynchronously and uploaded within the existing budget.
- Water is transparent and sorted differently; it must not inherit opaque LOD assumptions blindly.
- Occlusion culling is out of scope.

---

## 2. Decision

**The renderer uses two LOD levels.**

- **LOD0** is the full-resolution chunk mesh.
- **LOD1** is built from a 2×2×2 downsample of the block grid.
  - A cell is solid when at least one of its eight source blocks is solid.
  - The cell's block type is the topmost solid source block; ties choose the lowest X, then the lowest Z.
  - LOD1 positions and UVs are scaled by two.
  - LOD1 uses the same greedy mesher and face order.
  - Every LOD1 chunk-boundary face is emitted regardless of the neighbour, so transitions cannot leave holes.
  - LOD1 uses the topmost source block as its representative for texture, light, and AO.
- **Selection:** Euclidean centre-to-centre distance between the player's chunk and the chunk, in
  chunk units.
  - LOD0 when distance is `<= 4.0`.
  - LOD1 when distance is `>= 8.0`.
  - Distances between 4 and 8 retain the current LOD.
  - Initial selection is LOD1 at `>= 8.0`, otherwise LOD0.
- **Cache:** a chunk may hold both LOD meshes. Edits invalidate both, increment the revision, and
  enqueue an urgent LOD0 rebuild plus background LOD1 work. The desired mesh becomes active when
  ready; while it is missing, the previous active mesh stays visible.
- **Water:** water meshes are full-resolution only in v0.0.6; LOD1 creates no water mesh.
- **Scope:** LOD only affects rendering. Collision, edits, light storage, and saves always use
  full-resolution data.
- **Exclusions:** no third LOD, no smooth geomorph, no occlusion culling, no water LOD.

The exact greedy, light/AO, and streaming contracts remain in the milestone specs. This ADR
records the LOD strategy, not a second copy of their byte or quad tables.

I accept this decision for the current scope. I will revisit it only if the assumptions in this
document stop holding.

---

## 3. Why I Chose This LOD Strategy

### 3.1 Two levels capture most of the win

The first LOD is where most distant geometry cost disappears: a 2×2×2 downsample reduces the
grid to one eighth of its cells before greedy merging. A third level would add more thresholds,
meshes, transitions, and cache pressure for a smaller marginal gain. Two levels match the
v0.0.5 scope and the stress fixture.

### 3.2 Any-solid downsampling preserves silhouettes

A cell being solid when any source is solid is conservative: it can overfill an edge, but it does
not erase geometry that exists at full resolution. That is the right tradeoff when the
alternative is a visible hole. The overfill is hidden or negligible at LOD distance.

### 3.3 Topmost-type selection keeps surfaces recognizable

Using the topmost solid source block for the cell's type, texture, light, and AO keeps grass on
top of grass and dirt in the ground. Ties by lowest X then Z make the choice deterministic. A
average or majority type would need more rules and could show a different surface material at
distance.

### 3.4 The same greedy mesher keeps one contract

LOD1 reuses the v0.0.5 face order, merge order, UV-in-block-units rule, and vertex layout. Only
the cell size (2), the boundary exception, and the block representative differ. There is one
mesher to test and one deterministic output contract, not a second renderer path.

### 3.5 Boundary overdraw prevents cracks

LOD0 and LOD1 neighbours can have different silhouettes. Culling a LOD1 boundary face against a
LOD1 cell could remove a face that the neighbouring LOD0 mesh does not cover, producing a hole.
Always emitting LOD1 boundary faces overdraws hidden surfaces, but it cannot crack. At LOD
distance, the extra boundary quads are a cheap insurance policy.

### 3.6 Hysteresis avoids flicker at the threshold

The 4/8 band means a chunk at the boundary does not switch LOD every frame. It becomes LOD1
beyond 8 and returns to LOD0 below 4. The eight-chunk stress fixture cannot exercise the scope's
initial 8/16 thresholds, so 4/8 was chosen as the first tuned pair; it is explicitly a tuning
value, not a permanent law.

### 3.7 The cache hides asynchronous rebuilds

Both LOD meshes may be cached. When the desired mesh is missing, the previous active mesh stays
visible instead of leaving a hole. The worker result replaces only its LOD buffer, and revisions
discard stale work. The two-mesh upload budget keeps the main thread bounded.

### 3.8 Full data stays authoritative

LOD1 is a rendering representation only. Collision, block edits, lighting storage, and saves use
the full-resolution chunk. That means LOD can be regenerated or discarded without touching world
state, and it cannot corrupt persistence or interaction.

### 3.9 Water is deliberately excluded from LOD1

Water is transparent, animated, and drawn in a separate pass with depth writes off. A downsampled
water mesh would need its own transition, sorting, and light rules. v0.0.6 keeps water at full
resolution, accepting the extra cost in exchange for a correct transparent surface. A water LOD
is a later decision.

### 3.10 Why not one LOD, more LODs, majority sampling, or skirts?

- **One LOD** avoids transitions but gives up the entire distance win.
- **Three or more LODs** add thresholds, meshes, cache memory, and transition combinations for a
  marginal reduction beyond LOD1.
- **Majority sampling** can drop a solid edge and create holes.
- **Topmost-only solidity** could miss a lower overhang entirely.
- **Mesh simplification/decimation** is harder to keep deterministic and crack-free for a voxel
  grid than fixed 2×2×2 downsampling.
- **Skirts or stitching** can hide cracks but add geometry and border logic; conservative
  boundary overdraw is simpler and robust.
- **Water LOD1** adds transparency-specific transition work before the opaque LOD path is
  proven.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Two LODs, any-solid 2×2×2, always-emit boundaries** | Cheap, deterministic, preserves silhouettes, no cracks, reuses one mesher. | **Selected** |
| **One LOD** | No transition cost; no distance reduction. | Not selected |
| **Three or more LODs** | Finer distance steps; more thresholds, meshes, cache, and transitions. | Not selected for v0.0.5 |
| **2×2×2 majority downsampling** | Fewer overfilled cells; can drop thin edges and leave holes. | Not selected |
| **Topmost-only solidity** | Simple surface rule; can miss lower overhangs. | Not selected |
| **Mesh decimation / simplification** | Can reduce arbitrary geometry; difficult to keep deterministic and crack-free for voxels. | Not selected |
| **Boundary culling against LOD1** | Fewer boundary quads; can crack against LOD0 neighbours. | Not selected |
| **Skirt geometry / stitching** | Hides cracks with extra geometry; more border logic. | Not selected |
| **Water LOD1** | Reduces transparent geometry; adds sorting and transition complexity. | Not selected in v0.0.6 |
| **Occlusion culling** | Additional culling win; separate from LOD selection. | Deferred |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the
record stays honest.

| Counterargument | Disposition |
|---|---|
| **Any-solid downsampling overfills silhouettes.** | Accepted. LOD1 may look slightly thicker or blockier at edges. It is preferable to a hole, and the overfill is at distance. |
| **Always emitting LOD1 boundary faces adds overdraw.** | Accepted. The extra faces are hidden inside solid neighbours in the common case and prevent cracks in the mixed case. Measure if boundary overdraw becomes visible in the frame budget. |
| **Two levels can pop.** | Accepted. Hysteresis and the previous-active fallback reduce flicker, but there is no smooth geomorph. A third level or blending would be a later decision. |
| **The 4/8 thresholds are tuned to one fixture.** | Accepted. The scope's initial 8/16 could never select LOD1 in an 8×8 grid. 4/8 is a starting point; retune if the stress fixture or render-distance range changes. |
| **LOD1 light and AO are approximate.** | Accepted. They use the topmost representative block, which preserves the surface material but not every full-resolution corner. This is a deliberate distance simplification. |
| **Caching both meshes uses more GPU memory.** | Accepted. Two optional meshes per chunk are bounded by chunk count and are the cost of switching LODs without rebuilding on every threshold crossing. |
| **Water stays full-resolution.** | Accepted. Transparent water has separate sorting and pass rules; downsample it only after the opaque LOD path and water rendering are both stable. |
| **There is no occlusion culling.** | Accepted. Frustum culling and LOD are the v0.0.5 scope; occlusion is a separate decision. |
| **Fixed 2×2×2 sampling is crude.** | Accepted. It is deterministic, cheap, and easy to test. A better simplification metric is a later decision if visual quality requires it. |
| **Boundary overdraw can show double surfaces at LOD transitions.** | Accepted. The alternative is a hole. The overdraw is visually preferable at distance and can be revisited with stitching later. |

---

## 6. Consequences

### 6.1 Positive

- Distant chunks use roughly one eighth of the cells before greedy merging.
- Silhouettes are preserved rather than eroded.
- LOD transitions cannot crack at chunk boundaries.
- One greedy mesher and one vertex contract cover both levels.
- Full-resolution data stays authoritative for collision, edits, light, and saves.
- Cached meshes and hysteresis avoid rebuilds and flicker on small camera movements.
- Async LOD rebuilds fit the worker pool and revision model.
- Water transparency keeps its own correct path.

### 6.2 Negative

- LOD1 overfills tight silhouettes.
- LOD1 boundary overdraw adds hidden geometry.
- Two discrete levels can pop rather than morph.
- Both LOD meshes may occupy GPU memory.
- The 4/8 thresholds are fixture-specific and need retuning if the world size changes.
- LOD1 lighting and AO are representative rather than exact.
- Water does not receive the LOD reduction yet.

### 6.3 Neutral

- LOD is rendering-only; the save format is unchanged.
- More LOD levels, water LOD, occlusion culling, and stitching remain future decisions.
- The stress fixture and metrics are the tuning surface, not a CI gate.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| LOD transitions show holes. | Always emit LOD1 boundary faces; test LOD0/LOD1 neighbours explicitly. |
| Any-solid overfill becomes distracting. | Accept at distance; compare in the stress fixture; revisit the sampling rule if silhouettes become a visual problem. |
| LOD pops at the threshold. | Use 4/8 hysteresis; keep the previous active mesh until the desired mesh is ready; test both directions. |
| Thresholds do not suit a larger world. | Retune against the stress fixture or a larger fixture; record a new decision if the range changes materially. |
| LOD1 light/AO mismatch is visible. | Use the topmost representative; test surface transitions; accept distance approximation. |
| Both LOD caches grow too large. | Keep two optional meshes per chunk; release both on unload; measure GPU memory. |
| Boundary overdraw hurts the frame budget. | Measure with metrics; if it matters, evaluate stitching or a different boundary rule in a later ADR. |
| Water is expensive at distance. | Keep full-resolution water for correctness in v0.0.6; revisit water LOD after transparency is stable. |
| A future feature needs more levels. | Add a new ADR; do not silently extend this decision. |

---

## 8. Conclusion

Two LODs with conservative 2×2×2 downsampling are my deliberate, distance-aware choice for chunk
rendering. I am selecting them because the first downsample captures most of the geometry win,
because any-solid sampling preserves silhouettes while topmost selection preserves surface
identity, because always-emitted boundaries make mixed LOD transitions crack-free, and because
both meshes reuse the v0.0.5 greedy contract. I acknowledge the overfill, boundary overdraw,
two-level pop, approximate light/AO, cache memory, and the absence of water LOD or occlusion
culling. I manage those through hysteresis, the previous-active fallback, explicit transition
tests, stress-scene metrics, and a documented path to more levels or stitching if measurements
justify them. I therefore accept this decision as the engine's LOD strategy, with the exact mesh
contracts left to the milestone specs and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on chunk LOD selection, downsampling, or transition boundaries.*
