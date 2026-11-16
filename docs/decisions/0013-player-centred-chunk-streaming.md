# Why I Chose Finite Player-Centred Chunk Streaming with Hysteresis

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-15  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** The v0.0.7 streaming architecture: render-distance configuration, chunk-column load/unload rings, queue priority, readiness gating, player movement guard, unload saving, and fog/camera-far integration. Generation algorithms, save format, LOD, lighting, and the worker pool are separate decisions.

---

## Summary

This document records why the engine streams a finite set of chunk columns around the player instead of loading a whole world or tracking visible chunks only. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: the player is the centre of a square Chebyshev load ring, chunks stay resident through a two-chunk unload margin, and movement into a column that is not fully generated, lit, meshed, and uploaded is blocked. That gives bounded memory, no visible holes, and a place for background generation without a hard loading screen. The tradeoff is an invisible wall at the frontier, a square rather than circular ring, fixed vertical coverage, and per-frame commit/upload limits.

---

## 1. Context

### 1.1 The project

v0.0.7 generates chunks on workers and streams them around the player:

- The world is four chunks tall and arbitrarily wide in X/Z.
- Chunks are generated from a seed; modified chunks are saved and unmodified chunks regenerate.
- Generation, lighting, and meshing run on the v0.0.5 worker pool.
- The main thread applies world state and GPU uploads.
- LOD selection and frustum culling run on the loaded chunks.
- The render distance is configurable from 8 to 16 chunks.
- The engine has no server, no infinite-world guarantee, and no streaming save format of its own.
- The player must never fall through or walk into an unready column.

These requirements call for a bounded, player-centred residency policy rather than a whole-world load or a purely visible-set renderer.

### 1.2 The constraints

My choice is constrained by the following:

- Memory and worker queues must be bounded.
- The player must not see a missing column at the frontier.
- Generation may be slower than movement; the policy needs a defined response.
- Modified chunks must be saved before they are dropped.
- Unmodified chunks must be regenerable from the seed.
- Lighting must be ready before a chunk is visible.
- LOD and frustum culling must receive only valid chunks.
- The main thread must budget commits and uploads per frame.
- The world is four chunks tall; vertical streaming is not required.
- Multiplayer, servers, and infinite world guarantees are out of scope.

---

## 2. Decision

**The engine uses a finite, player-centred, Chebyshev-square streaming policy with a two-chunk unload margin and a render-ready movement guard.**

Render distance:

- `R = world.render_distance_chunks`, default `12`, valid `8..=16`.
- A chunk column is a vertical stack of the four world chunks sharing `(x, z)`.

Residency:

| Chebyshev distance from the player's column | Action |
|---|---|
| `<= R` | Load and keep resident. |
| `R < d <= R + 2` | Keep resident if already loaded; do not newly load. |
| `> R + 2` | Unload. |

Generation and queue order:

- Generate columns nearest first.
- Within the load set, order by:
  1. Columns in the player's forward half-plane.
  2. Squared Euclidean distance from the player's chunk.
  3. `(x, z)` ascending.
- Commit at most two columns to the world per frame.
- Upload at most two chunk meshes per frame, matching v0.0.5.

Readiness:

- A column is ready only when all four vertical chunks are generated, lit, meshed, and uploaded.
- A column that is not ready is not drawn.
- The previous LOD mesh or a generated but not yet uploaded column must not create a visible hole; the guard below covers the player's path.

Movement guard:

- Before applying an axis movement, test the resulting player AABB against the residency/readiness state.
- If the AABB would overlap a column that is not ready, reject that axis movement and zero its velocity component.
- The current player column must remain ready.
- The spawn column is made ready before movement is enabled.

Unloading:

- Save a modified chunk before dropping it.
- If an unload save fails, log the error and keep the chunk in memory until the next save.
- Release CPU light data, GPU meshes, and water meshes.
- Cancel or discard pending generation, light, and mesh work for unloaded chunks through revisions.

Fog and camera:

- Camera far is `(R + 2) * 32` metres.
- Fog starts at `0.7 * R * 32` and ends at `R * 32`.
- Fog colour follows the v0.0.6 sky colour.
- This supersedes the v0.0.6 fixed `100 m` fog range.

The finite render distance is deliberate: v0.0.7 provides a bounded streamed world, not an
infinite server-backed world. The exact generation, save, lighting, LOD, and worker contracts
remain in their specs and ADRs.

I accept this decision for the current scope. I will revisit it only if the assumptions in this
document stop holding.

---

## 3. Why I Chose This Streaming Architecture

### 3.1 The player is the natural residency centre

Generation, lighting, and meshing all serve what the player can approach. Centring residency on
the player's column keeps the work set aligned with movement, and the forward-half-plane priority
gives the chunks the player is about to enter a head start.

### 3.2 Meanwhile a finite ring bounds memory and queues

A whole-world load grows without bound and makes startup proportional to the world. A finite
Chebyshev ring bounds resident columns, worker queue depth, and GPU buffers by `R`, independent
of how far the player has travelled.

### 3.3 The two-chunk margin absorbs movement

Unloading exactly at `R` would churn chunks when the player crosses a boundary. The `R + 2`
margin keeps columns resident through small back-and-forth movement and gives the generator
headroom. Fog ends at `R`, so the unload margin sits behind the visible edge.

### 3.4 The guard prevents holes and falls

Blocking movement into an unready column is conservative, but it makes the frontier explicit:
the player cannot walk into unloaded air, fall through a missing floor, or see a missing column
underfoot. The alternative is to allow movement and generate synchronously, which produces
hitches or a visible hole.

### 3.5 A column is the readiness unit

The world is only four chunks tall, so a column is the smallest unit that guarantees a complete
vertical space. Waiting for all four chunks avoids a player falling through a generated floor
into an ungenerated chunk below. Vertical streaming is unnecessary for this world height.

### 3.6 Commits and uploads have frame budgets

Two columns committed per frame and two mesh uploads per frame bound main-thread work. The
queue runs on workers, while the main thread only applies completed results and uploads. If the
budget falls behind the player, the guard blocks rather than dropping data.

### 3.7 Fog hides the finite edge

Fog ends at the render distance and the unload margin extends beyond it. The player sees terrain
fade into the sky before the unload boundary, so the finite world edge is not a hard visual cut.

### 3.8 Modified chunks are saved before they disappear

Streaming drops chunks continuously. Saving modified chunks on unload, and keeping them in
memory if the save fails, prevents silent edit loss. Unmodified chunks need no file and can be
regenerated from the seed.

### 3.9 Why not a whole-world load, a visible-only set, or a circular radius?

- **Whole-world load** has unbounded memory and startup, and no need for streaming.
- **Visible-only residency** creates holes when the camera turns or the player moves; it also
  makes collision and lighting depend on the view.
- **A circular/Euclidean radius** includes fewer corner chunks but makes the ring and fog
  mismatch; the square Chebyshev ring is simple and predictable.
- **Generate-on-demand without a guard** can hitch or drop the player; the guard is the explicit
  failure response.
- **A loading screen** would interrupt play; the guard keeps the world continuous and blocks
  only at the frontier.
- **Infinite/server-backed streaming** is a different project scope.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Finite Chebyshev ring, R load, R+2 unload, guard** | Bounded memory, no holes, no full-world startup, worker-friendly. | **Selected** |
| **Load the whole world at startup** | Simple residency; unbounded startup and memory. | Not selected |
| **Fixed radius with no hysteresis** | Less state; churn and visible transitions at the boundary. | Not selected |
| **Unload immediately at R** | Lower memory; frequent reloads and hitches. | Not selected |
| **Visible/frustum-only residency** | Minimal memory; holes when turning, and collision depends on view. | Not selected |
| **Circular/Euclidean ring** | Fewer corner chunks; fog/ring mismatch and less predictable counts. | Not selected |
| **Generate synchronously on demand** | No background queue; main-thread hitches and blocking waits. | Not selected |
| **Loading screen at the frontier** | Clear readiness; interrupts movement. | Not selected |
| **Infinite/server-backed streaming** | Seamless larger world; out of scope. | Not selected |
| **Vertical streaming** | Lower memory in a tall world; the world is four chunks tall. | Not selected |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the
record stays honest.

| Counterargument | Disposition |
|---|---|
| **The guard feels like an invisible wall.** | Accepted. It is preferable to falling through ungenerated terrain or seeing a hole. Forward priority and the R+2 margin reduce how often it is hit. |
| **A square ring includes corner chunks that a circle would omit.** | Accepted. The square is simple, predictable, and matches the horizontal chunk grid; fog hides the finite edge. |
| **All four vertical chunks per column are coarse.** | Accepted. The world is four chunks tall; vertical streaming would add state for little benefit. |
| **Two columns per frame may lag behind a fast player.** | Accepted. The guard blocks at the frontier; measurements can tune the budget if movement is too constrained. |
| **Two mesh uploads per frame can make columns visually lag.** | Accepted. The column is not visible until ready and the player cannot enter it; LOD caching keeps previous meshes visible when possible. |
| **Unload saves can block on file I/O.** | Accepted. Each chunk is one small file, and failures keep the chunk in memory. Moving saves off-thread is a later decision. |
| **The load set is finite.** | Accepted. v0.0.7 is a finite streamed world; an infinite/seamless world is later work. |
| **The guard checks rendering readiness, not only collision.** | Accepted. It is conservative but prevents visible holes. The cost is earlier blocking than a collision-only guard. |
| **R+2 memory is larger than the visible set.** | Accepted. The margin buys hysteresis and generation headroom; memory remains bounded by R. |
| **Fog may hide too much nearby terrain at low R.** | Accepted. Fog start is 70% of the visible radius; R is configurable from 8 to 16 and can be tuned. |
| **A player can still outrun generation in a pathological case.** | Accepted. The guard is the backstop; it blocks rather than exposing a hole. |

---

## 6. Consequences

### 6.1 Positive

- Resident columns, worker queues, and GPU buffers are bounded by render distance.
- Startup does not load the whole world.
- The two-chunk margin reduces reload churn.
- Forward-half-plane priority makes the chunks ahead of movement ready first.
- The movement guard prevents falling through or entering unready columns.
- Columns are visible only after generation, lighting, meshing, and upload.
- Fog hides the finite edge before the unload boundary.
- Modified chunks are saved before they are dropped.
- Unmodified chunks regenerate from the seed.
- The main thread has explicit per-frame commit and upload budgets.

### 6.2 Negative

- The frontier can feel like an invisible wall.
- The ring is square rather than circular.
- Vertical coverage is fixed at four chunks.
- Per-frame commit and upload caps can lag behind fast movement.
- Unload saves are synchronous file work.
- The world is finite; there is no seamless/infinite streaming.
- A column can remain in memory after a failed unload save.
- The readiness guard is stricter than a collision-only check.

### 6.3 Neutral

- Render distance is configuration, not a new streaming algorithm.
- The save format and generation algorithm are separate decisions.
- LOD and frustum culling consume the loaded/ready set.
- An infinite server-backed world and async save I/O remain future work.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A player falls through an unready column. | Guard the player AABB against not-ready columns; keep the current column ready; test spawn and both movement axes. |
| A visible hole appears at the frontier. | Mark a column visible only after all four chunks are ready; fog ends at R; unload at R+2; test transitions. |
| Generation falls behind the player. | Prioritize forward columns, nearest first; cap commits per frame; block at the frontier instead of exposing holes. |
| Unload save fails. | Log the error, keep the chunk in memory, retry on the next save; never silently drop modified data. |
| Stale jobs mutate unloaded chunks. | Cancel/discard through revisions; reject results for unloaded or not-current columns. |
| Memory grows beyond the ring. | Bound residency by R+2; release CPU light/GPU/water on unload; measure loaded chunk count. |
| Mesh uploads lag commits. | Keep the column not-ready until meshes upload; preserve previous LOD meshes; tune the two-mesh cap. |
| File I/O hitches the frame. | Keep per-chunk saves small; measure unload cost; move saves to workers in a later decision if needed. |
| The square ring and fog edge look wrong. | Fog ends at R and the unload margin extends beyond it; test from corners and along axes. |
| The player is blocked too early. | Tune R, forward priority, and the commit budget; the guard is the safety boundary, not a target. |

---

## 8. Conclusion

Finite player-centred chunk streaming with hysteresis is my deliberate, bounded-world choice for
v0.0.7. I am selecting it because residency follows the player, because a Chebyshev ring with an
R+2 margin keeps memory and worker queues bounded while hiding reload churn, because forward
priority and per-frame budgets keep generation aimed at movement, and because a readiness guard
prevents holes and falls at the frontier. I acknowledge the invisible-wall frontier, the square
ring, fixed vertical coverage, per-frame caps, and synchronous unload saves. I manage those
through fog at R, the unload margin, revisions, ready-only visibility, save-before-drop, and
metrics. I therefore accept this decision as the engine's streaming architecture, with the
finite world and async-save limitations recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on chunk residency, render distance, streaming priority, or the movement guard.*
