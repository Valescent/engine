# Why I Chose Voxel Light and Baked AO over Shadow Maps

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-15  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** The v0.0.6 lighting model: 4-bit sky/block light, flood-fill propagation, per-vertex baked ambient occlusion, the shader combination of voxel light and directional sun/moon light, and the decision not to add shadow maps, screen-space AO, or real-time global illumination in this milestone. Day/night keyframes, fog, water shading, and the exact vertex byte layout are owned by the v0.0.6 spec.

---

## Summary

This document records why the engine uses voxel light propagation plus baked ambient occlusion instead of shadow maps. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: a voxel world needs readable caves, overhangs, and lamp falloff, and those are local-light problems that sky/block light BFS solves directly. AO adds contact shading per vertex at almost no runtime cost. A shadow map would add another pass, depth resources, cascades, bias decisions, and transparency edge cases while still needing the voxel light for caves. The tradeoff is that the sun and moon cast no sharp shadows, light is 4-bit and uncoloured, and AO is not a replacement for directional shadows.

---

## 1. Context

### 1.1 The project

The world is a voxel grid with specific lighting requirements:

- **Caves and overhangs.** A heightmap and 3D caves create enclosed spaces. A sun shadow alone cannot make a cave dark; voxel light must propagate into it.
- **Lamps.** Block light sources must fade with distance through the world and across chunk borders.
- **Day/night.** The directional sun and moon, ambient colour, sky colour, and fog change over a ten-minute cycle.
- **Chunk streaming.** Light is derived from blocks, recomputed after generation or load, and updated after edits. It is never saved.
- **Greedy meshing.** Per-vertex light and AO must survive merging, which means quads split when their corner values differ.
- **Transparent water.** Water is a separate pass with depth writes off; its lighting must not rely on an opaque shadow-map path.
- **Readability over realism.** The milestone goal is a readable, atmospheric voxel world, not a physically based renderer.

These requirements point toward local block light plus cheap contact shading, not a single directional shadow pass.

### 1.2 The constraints

My choice is constrained by the following:

- Lighting must be deterministic and derivable from block data.
- Light must not be saved; it is recomputed after load and edits.
- Lighting must run on the v0.0.5 worker pipeline and integrate with revisions.
- Caves, overhangs, lamps, and water must be lit without a GPU compute requirement.
- The vertex budget must stay bounded; the v0.0.6 vertex carries one packed light/AO field.
- Greedy meshing must not interpolate across light or AO discontinuities.
- The day/night cycle must update directional light and ambient/sky colour.
- Shadow maps, screen-space AO, and real-time GI are not in the v0.0.6 scope.
- Coloured light is out of scope.

---

## 2. Decision

**The engine uses voxel light propagation plus baked per-vertex ambient occlusion. It does not use shadow maps, screen-space AO, or real-time global illumination in v0.0.6.**

The lighting model:

- Each chunk stores 32³ `sky_light` and `block_light` byte arrays, values `0..=15`.
- Sky light is `15` in open columns and propagates through non-solid cells with one level of attenuation per step.
- Block light starts at `14` from Lamp cells and propagates through non-solid cells with one level of attenuation per step.
- Solid blocks stop propagation; Water does not attenuate.
- Light is derived, never saved, and recomputed after generation or load.
- Edits recompute a 3×3×3 chunk neighbourhood plus affected vertical columns; light and meshes are invalidated through the v0.0.5 revision pipeline.
- Light is sampled per corner from the non-solid cells around the face corner.
- AO is computed per vertex from the two edge-adjacent blocks and the diagonal block around the corner, mapped to four levels.
- Greedy quads split when their corner light or AO values differ.
- The vertex carries packed sky/block light and AO; the shader combines texture, AO, voxel light, ambient, directional sun/moon, and fog.
- The directional sun/moon is not shadowed.

This decision does not add a shadow-map pass, a cascaded shadow atlas, SSAO, a light probe volume, or a GPU ray-tracing path. It also does not add coloured light; the light fields are scalar.

The exact BFS, AO formula, shader keyframes, vertex packing, and water path remain in the
[v0.0.6 spec](../specs/v0.0.6.md).

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose Voxel Light and Baked AO

### 3.1 Local light explains caves and lamps

Sky light and block light are functions of the block grid. An open column is bright; a covered cave is dark until light BFS reaches it; a Lamp fades with distance. A shadow map only darkens direct sun; it cannot illuminate a cave or explain a lamp. Voxel light is the mechanism the world actually needs.

### 3.2 The worker pipeline already fits

Light propagation is CPU data over immutable chunk snapshots. It runs on the v0.0.5 worker pool alongside generation and meshing, with revisions and bounded queues. It does not need a second GPU pass, a render-graph node, or new GPU resources.

### 3.3 Light stays derived

Light is recomputed from blocks after load and edits. It never becomes stale on disk, and a regenerated chunk receives correct light from its current neighbours. That matches the v0.0.7 modified-only persistence model.

### 3.4 AO is cheap contact shading

AO comes from three solid neighbours around each face corner and is baked into the vertex. It darkens creases and under overhangs without a screen-space noise/blur pass. It is deterministic, chunk-local, and already part of the greedy output.

### 3.5 Per-corner light and AO preserve gradients

The greedy key includes all four corner light/AO tuples, so a quad only merges when the values match. This avoids smearing a light or AO gradient across a large quad. The cost is extra quads at discontinuities, but the result is stable and testable.

### 3.6 The shader stays small

The final lighting is a texture sample multiplied by AO and a sum of ambient, directional sun/moon, and warm block light, followed by fog. There is no shadow-map lookup, cascade selection, bias tuning, or shadow-atlas management in the shader.

### 3.7 Streamed and regenerated chunks fit

Light is derived from blocks, so a chunk can be generated, lit, meshed, unloaded, and regenerated without a light sidecar. Modified chunks save blocks only; unmodified chunks regenerate and relight. A shadow map would need its own cache invalidation and resolution policy on top.

### 3.8 Water keeps one lighting path

Water is transparent and uses the same voxel light/AO fields and the same shader combination, with alpha blending in a separate pass. It does not need a shadow-map transparency variant or a separate shadow receiver path.

### 3.9 Why not shadow maps, SSAO, or real-time GI?

- **Shadow maps** add a depth pass, cascades or a large atlas, bias and peter-panning decisions, resolution tradeoffs, transparency rules, and a moving-sun update path. They still need voxel light for caves and lamps, so they would be an addition, not a replacement.
- **Screen-space AO** is view-dependent, needs noise/blur, and can miss contact outside the screen. Baked voxel AO is cheaper and deterministic for this grid.
- **Real-time GI** is far outside the milestone's scope and cost envelope.
- **No AO** would leave corners and overhangs flat; AO is inexpensive and directly improves readability.
- **Coloured light** would multiply the light channels and vertex budget; the scope deliberately keeps light scalar.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Voxel sky/block light + baked AO** | Explains caves, lamps, overhangs, and contact shading; deterministic CPU work; no extra GPU pass; fits streaming and persistence. | **Selected** |
| **Shadow maps (sun/moon)** | Sharp cast shadows; extra depth pass/cascades/atlas, bias and resolution work, transparency edge cases; still needs voxel light for caves. | Not selected for v0.0.6 |
| **Screen-space AO (SSAO)** | Contact shading without baked vertex data; view-dependent, noisy, blur-dependent, and not a light-propagation model. | Not selected |
| **Real-time global illumination** | Physically richer; expensive and outside the milestone's scope. | Not selected |
| **Baked lightmaps only** | Could capture static light; poor for edits, lamps, and day/night changes. | Not selected |
| **No AO** | Cheaper; flat corners and overhangs. | Not selected |
| **Coloured light** | Richer lamps and colour mixing; more channels, memory, vertex data, and tuning. | Deferred |
| **Shadow maps plus voxel light** | Best of both eventually; two systems and two cost centers now. | Future possibility, not v0.0.6 |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **The sun and moon cast no sharp shadows.** | Accepted. Directional light is unshadowed; voxel light and AO carry the readability. A shadow map is a later quality decision, not a prerequisite for the milestone. |
| **AO is not a shadow.** | Accepted. AO darkens contact and crevices; it does not project a directional silhouette. The decision deliberately trades cast shadows for cheap, deterministic local shading. |
| **4-bit light can band.** | Accepted. The shader interpolates per-corner values and AO softens the result. Coloured or higher-precision light would increase memory and vertex cost; revisit only if banding is a measured problem. |
| **Light BFS costs CPU after every edit.** | Accepted. The dirty region is bounded, work runs on the v0.0.5 pipeline, and revisions discard stale results. If edits become the bottleneck, the region or job partitioning can be tuned. |
| **Greedy quads split at light/AO changes.** | Accepted. Correct gradients are worth extra quads; the stress scene measures the tradeoff. |
| **Light is scalar, not coloured.** | Accepted. Coloured light is out of scope and would multiply the light fields and shader work. |
| **Light can leak through Water.** | Accepted. Water does not attenuate in v0.0.6; this is an explicit scope choice. Underwater falloff or coloured absorption would be a later decision. |
| **Caves may receive too much or too little light at borders.** | Accepted. Sky exposure includes vertical columns, and edits dirty a 3×3×3 neighbourhood plus affected columns. Border cases are covered by light tests. |
| **The vertex gains a packed light/AO field.** | Accepted. One `u32` fits sky, block light, and AO and avoids separate per-channel attributes. |
| **No GI means no colour bleed or bounce.** | Accepted. The milestone target is readable atmosphere, not physically based global illumination. |
| **Shadow maps could be added later.** | Accepted. This ADR does not forbid them; it records that they are not the v0.0.6 solution and that voxel light remains the base regardless. |

---

## 6. Consequences

### 6.1 Positive

- Caves and overhangs become dark from sky-light propagation.
- Lamps provide local warm light with distance falloff.
- AO adds contact shading to corners and crevices.
- Lighting is deterministic and derived from block data.
- Light is never saved, so it cannot go stale on disk.
- Light and AO work with chunk generation, streaming, and regeneration.
- The shader avoids a shadow-map pass, cascades, bias, and shadow atlas.
- Water shares the same light/AO fields.
- Day/night can update ambient, sun/moon, and fog without rebuilding shadow resources.

### 6.2 Negative

- No sharp sun/moon or lamp shadows.
- 4-bit scalar light can band and cannot mix colours.
- Light BFS adds CPU work after edits and generation.
- Greedy quads split at light/AO discontinuities.
- Light arrays cost 64 KiB per chunk before packing.
- Water does not attenuate light.
- AO is a contact approximation, not a directional shadow.
- A future shadow map or GI path would be an additional system, not a small shader toggle.

### 6.3 Neutral

- Shadow maps, SSAO, coloured light, and GI remain separate future decisions.
- The v0.0.6 spec owns the exact BFS, AO, keyframe, and vertex contracts.
- The v0.0.7 streaming pipeline regenerates and relights unmodified chunks.
- Light remains a runtime-only representation.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Light or AO leaks across chunk borders. | Include border light, test cross-chunk propagation and corner sampling, and remesh every light-dirty chunk. |
| Edits leave stale light or meshes. | Dirty the 3×3×3 neighbourhood plus affected columns; use revisions and reject stale worker results. |
| 4-bit banding is visible. | Interpolate per-corner values and use AO; test the keyframes; reconsider precision only if measured. |
| Greedy splitting loses performance. | Measure vertex counts and frame time in the stress fixture; accept extra quads where correctness requires them. |
| Light arrays use too much memory. | 64 KiB per chunk before packing; profile before adding a packed representation. |
| Water lighting or transparency looks wrong. | Keep water in the separate transparent pass with depth writes off; use the same light/AO fields; test. |
| Edits cause light/mesh hits. | Keep light and mesh jobs on the worker pool, bound the dirty region, and keep uploads at two meshes per frame. |
| A future shadow map is added without reconciling the model. | Treat it as a new ADR; keep voxel light as the base and measure the combined cost. |
| AO looks blocky at distance. | LOD1 uses the representative block's AO; accept the approximation at distance and revisit if it is visible. |
| Day/night changes look flat without cast shadows. | Use ambient/sky keyframes, sun/moon direction, AO, fog, and water animation; accept the stylised look. |

---

## 8. Conclusion

Voxel light with baked AO is my deliberate, readability-first choice for v0.0.6. I am selecting it because sky/block BFS explains caves, overhangs, and lamps directly, because AO adds cheap contact shading per vertex, because light stays derived and stream-friendly, and because the shader avoids a shadow-map pass and its bias, cascade, resolution, and transparency problems. I acknowledge that the sun and moon cast no sharp shadows, that light is scalar and 4-bit, that BFS costs CPU after edits, and that AO is only a contact approximation. I manage those through bounded dirty regions, worker revisions, per-corner greedy splitting, stress-scene metrics, and the option of a later shadow-map or GI decision that builds on this base. I therefore accept this decision as the engine's lighting model, with the exact v0.0.6 contracts left to the milestone spec and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on voxel light, baked AO, shadow maps, or global illumination.*
