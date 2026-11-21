# Why I Chose a Texture Array over a Flat Atlas for Block Textures

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted; mipmap and sampling policy extended by ADR-0016  
**Date:** 2026-11-15  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** Block texture storage and sampling for the chunk and greedy meshers: 16×16 tiles, per-face layers, layer indices in vertices, and UV tiling in block units. The asset pipeline, texture compression, mipmap policy, water animation, and save formats are out of scope and are decided elsewhere.

---

## Summary

This document records why block textures live in one `texture_2d_array` with one layer per tile instead of a flat 2D atlas. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: v0.0.5 greedy meshing merges coplanar faces into quads measured in block units, and those quads must repeat a tile cleanly. A texture array gives every tile its own layer, so UVs can simply exceed `1.0` and wrap inside that layer; a flat atlas would need atlas-coordinate math, padding, or splitting quads at tile boundaries, and filtering would bleed between neighbouring tiles. The tradeoff is that every layer must share one size and format and the array has a fixed layer budget, which is acceptable for 16×16 block tiles.

---

## 1. Context

### 1.1 The project

Block rendering has specific texture requirements:

- **Regular tiles.** Every block tile is 16×16 RGBA8 in the current milestone. Grass uses different top, side, and bottom tiles; wood uses different top and side tiles; later Lamp, Water, and Sand add more tiles.
- **Per-face mapping.** A block face selects a texture layer, not a sub-rectangle of a shared image. The v0.0.4 texture table and its v0.0.6/v0.0.7 extensions own the actual layer indices.
- **Greedy quads.** v0.0.5 merges adjacent coplanar faces of the same block type into larger quads. A quad can span many blocks, and its UVs are measured in block units, so the tile must repeat across the quad.
- **One draw per chunk and LOD.** The renderer batches a whole chunk mesh into one draw. It cannot switch a material or texture between individual block faces inside that draw.
- **No asset pipeline yet.** Placeholder tiles are generated procedurally. There is no external texture-pack format to preserve.
- **No bleeding.** Adjacent blocks can use different tiles; a filtered sample from one tile must never touch another tile's pixels.

These requirements favour a texture storage model where each tile is an isolated image and the mesh only needs a layer index plus ordinary tiling UVs.

### 1.2 The constraints

My choice is constrained by the following:

- Every chunk draw must share one block-texture binding; per-face material switches are not available.
- Greedy quads must repeat a tile across their full width and height.
- Tile filtering must not sample neighbouring tiles.
- Per-face layers must be selectable independently, including grass top/side/bottom.
- The model must work with wgpu/WebGPU and the existing vertex layout.
- The layer budget must cover the planned block set and leave room for more.
- The model must not force an external asset pipeline or a particular compression scheme.

---

## 2. Decision

**Block textures use one `texture_2d_array`, with one 16×16 RGBA8 layer per tile. Vertices carry a per-face layer index, UVs are in block units, and the sampler repeats in U and V.**

This covers the opaque and transparent block meshes, the per-face layer mapping, and the layer indices used by the v0.0.4 face mesher and the v0.0.5 greedy output. The layer table itself is owned by the milestone specs: eight layers in v0.0.4, ten in v0.0.6, and eleven in v0.0.7. A flat atlas and one-texture-per-block bindings are not selected.

The asset pipeline, mipmaps, anisotropy, compression, and texture-pack format remain separate decisions. This decision does not change the save format, and it does not change the water animation model.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose a Texture Array

### 3.1 Greedy quads tile without atlas math

The v0.0.5 mesher emits one quad for a run of equal faces. UVs run from `(0, 0)` to `(w, h)` in block units, and the sampler repeats the tile. With a texture array, the layer index selects the tile and the repeat happens inside that layer. With a flat atlas, the same quad would need per-fragment modulo arithmetic to keep sampling one tile, or the mesher would have to split at atlas boundaries and give up much of the merge.

### 3.2 Layer isolation prevents bleeding

Texture arrays keep each layer in a separate image. Nearest filtering has no cross-tile contamination, and a future mipmap or anisotropic setting still samples within one layer. A flat atlas needs padding, half-texel insets, or clamped UVs to stop neighbouring tiles from bleeding into each other; those fixes complicate every generated tile and every UV.

### 3.3 UVs stay simple

A vertex stores the ordinary face UV and a layer index. There are no atlas offsets, scales, rotations, or per-block sub-rectangles in the mesher or shader. The same face-corner UV order works for every block and every greedy width. That keeps the mesher deterministic and the v0.0.5 test vectors straightforward.

### 3.4 Per-face layers are explicit

Grass needs different top, side, and bottom tiles. Wood needs different top and side tiles. A layer index per face expresses that directly. In an atlas, the same mapping becomes four atlas rectangles plus atlas UV conversion, and every new block risks a coordinate mistake.

### 3.5 One binding serves every chunk

The renderer can bind one texture array and one sampler for all chunk draws. A separate texture per block would require one bind group per block type or a bindless material array; both break the current one-draw-per-chunk batching story.

### 3.6 New block types extend the layer table

Adding Sand, Lamp, or Water adds a layer instead of repacking an atlas or moving existing tiles. The layer budget is far below the WebGPU baseline of 256 layers, so the planned eleven layers leave ample room before another storage decision is needed.

### 3.7 wgpu supports the model directly

`texture_2d_array` is a core WebGPU texture type. wgpu accepts the array and sampler binding without extensions, and the WGSL sample operation takes a layer index per quad. ADR-0002 already selected wgpu; this uses the feature the renderer was chosen to expose.

### 3.8 Why not a flat atlas or one texture per block?

- **Flat atlas.** A single image is compact and familiar, but greedy tiling across blocks is awkward: either the shader reconstructs tile-local UVs or the mesher splits quads at tile boundaries. Padding and filtering bleed are additional costs. It is a reasonable fallback for irregular sprites, not for a small set of regular, repeating tiles.
- **One texture per block.** Tiling is simple, but each draw would need a different material binding. That either multiplies draw calls or requires bindless textures. Neither is justified for eleven 16×16 tiles.
- **A 3D texture.** It could store layers, but `texture_2d_array` expresses the intent directly and avoids treating the layer axis as a volume.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Texture array** | One binding, isolated layers, natural repeat, per-face layers, trivial UVs, greedy-friendly, room for eleven layers. | **Selected** |
| **Flat 2D atlas** | One image and broad tooling support; atlas UV math, padding/inset work, filtering bleed, and greedy tiling/splitting. | Not selected |
| **One texture per block type** | Simple tiling and no atlas; breaks one-draw batching, needs per-block bind groups or bindless. | Not selected |
| **Bindless texture array** | Flexible material count; more complex binding model and limits, unnecessary for eleven regular tiles. | Not selected for this scope |
| **3D texture** | Layer storage works; semantically a volume, while the data is a set of 2D tiles. | Not selected |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **The layer count is fixed.** A texture array cannot grow indefinitely. | Accepted. The planned set is eleven layers and the WebGPU baseline is 256. If the block set outgrows that, the fallback is a second array or a separate material decision. |
| **All layers must share size and format.** Block tiles are all 16×16 RGBA8 today. | Accepted. This is exactly why the array fits. Heterogeneous or higher-resolution textures need a separate decision. |
| **The layer index adds four bytes per vertex.** | Accepted. It is one `u32` on a vertex that already carries position, normal, UV, and light/AO data, and it removes per-face texture bindings. |
| **A flat atlas handles irregular sprites and different sizes.** | Accepted. Block tiles are regular and fixed-size; the atlas advantage does not apply. |
| **Mipmaps and anisotropy still need care.** | Accepted. Arrays isolate layers, but mipmaps can still blur within a layer. The current milestone uses nearest filtering and no mipmaps; a future filtering decision can revisit mipmaps without changing the storage model. |
| **Texture packs must pack layers in the right order.** | Accepted. There is no external asset pipeline yet. When one arrives, it must produce a layer array and a matching layer table; procedural placeholders avoid that cost now. |
| **Layer indices can drift when blocks are reordered.** | Accepted. Layer indices are explicit constants owned by the block specs, and tests cover the mapping. They are not derived from enum order. |
| **The sampler must repeat.** A clamp sampler would stretch one tile across a greedy quad. | Accepted. The renderer must use repeat addressing in U and V; the greedy UV contract depends on it. |
| **It does not solve water animation or transparency ordering.** | Accepted. Those are separate v0.0.6 decisions. The array only supplies the water tile. |

---

## 6. Consequences

### 6.1 Positive

- Greedy quads repeat tiles without atlas-coordinate reconstruction.
- Texture layers cannot bleed into each other.
- Per-face layers are a direct index instead of an atlas rectangle.
- One texture binding serves every chunk draw.
- Adding a block type is a layer-table change, not an atlas repack.
- Mesher UVs remain in block units and the test vectors stay simple.
- The model works with wgpu without extensions.

### 6.2 Negative

- Every layer shares one size and format.
- The array has a fixed layer budget.
- Vertices carry a layer index.
- A future texture-pack pipeline must pack layers and preserve indices.
- Mipmap and anisotropy policy still needs its own decision.
- A flat atlas would be more flexible for irregular sprite sheets, which this engine does not use.

### 6.3 Neutral

- The layer table is owned by the milestone specs, not by this ADR.
- Procedural placeholder tiles can be generated directly into array layers.
- A future texture pack can convert atlas tiles into layers without changing the mesher contract.
- The save format and water/lighting decisions are unaffected.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| The block set exceeds the layer budget. | Assert the array size against the device limit; the planned eleven layers are far below 256; split into a second array only if a later block set requires it. |
| A wrong layer index samples the wrong tile. | Keep explicit layer constants and a tested block-face mapping; never derive layer indices from enum order. |
| A clamp sampler stretches greedy quads. | Require repeat addressing in U and V; cover a wide greedy quad in the mesh tests. |
| A texture layer has the wrong size or format. | Enforce 16×16 RGBA8 at upload and return a typed error on mismatch. |
| Filtering or mipmaps bleed within a layer. | The current milestone uses nearest and no mipmaps; decide mipmap policy separately, with layers isolated from cross-tile bleeding. |
| A texture pack reorders layers. | Record layer indices in the specs and test the pack against them. |
| The texture array is accidentally replaced by an atlas. | This ADR and the v0.0.4/v0.0.5 specs state the array contract; a change requires a new ADR. |

---

## 8. Conclusion

A texture array is my deliberate, greedy-aware choice for block textures. I am selecting it because it lets the v0.0.5 mesher emit large quads with ordinary block-unit UVs and repeat sampling, because isolated layers eliminate atlas bleeding and padding work, and because one texture binding preserves one draw per chunk. I acknowledge that every layer must share one size and format, that the layer count is bounded, and that the layer index travels in the vertex. I manage those through an explicit layer table, device-limit checks, repeat addressing, 16×16 texture validation, and a separate decision for any future mipmap or asset-pipeline work. I therefore accept this decision as the engine's block-texture storage model, with the layer table left to the milestone specs and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on block texture storage or the greedy UV tiling contract.*
