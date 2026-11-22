# Why Mipmaps Are Generated In-Engine with Linear Minification

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-21  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Related:** ADR-0007 (texture array storage), ADR-0015 (texture references are presentation)  
**Scope:** Mipmap generation, mip count, and sampler filtering for the eleven-layer procedural block texture array. The storage model and layer table stay ADR-0007; the tile artwork, asset pipeline, compression, and UV conventions are out of scope.

---

## Summary

This document records how the block texture array is minified. The array gets a full mip chain generated in-engine at startup: each level is the 2×2 box average of the level below, down to a single texel. The sampler magnifies with `Nearest`, minifies with `Linear`, blends mip levels with `Linear`, disables anisotropy, and repeats in U and V.

The short version: distant blocks currently shimmer because a 16×16 tile is minified from one level with nearest sampling. A five-level chain generated from the same deterministic tile bytes removes most of that aliasing without adding an asset pipeline, without touching the mesher or UVs, and without weakening the layer isolation that motivated the array in ADR-0007. The cost is 3,740 extra bytes and about 18 µs of startup work.

---

## 1. Context

### 1.1 The project

ADR-0007 stores every block tile as one 16×16 RGBA8 layer of a `texture_2d_array` and defers mipmap and anisotropy policy to a later decision. The current implementation creates that array with `mip_level_count: 1` and a nearest/nearest sampler. Eleven procedural placeholder layers cover the block set; grass, wood, and the other per-face references are stable names resolved by the renderer (ADR-0015).

Greedy meshing merges coplanar faces into quads measured in block units, and the shader lets UVs exceed `1.0` so the sampler repeats the tile across the quad. At a distance, many texels project into one pixel. With one level and nearest minification, each pixel picks an arbitrary texel from the 16×16 tile, and small camera or sub-pixel changes make that choice jump. The visible result is shimmer on distant ground and water.

### 1.2 The constraints

- No external asset pipeline exists. The tiles are generated procedurally at startup, so there is no build step or GPU upload format that could own mip generation.
- wgpu/WebGPU has no built-in mipmap generation call. Generating levels means either a CPU pass or extra render/compute passes.
- Every layer keeps its own image; a filtered sample must never read another layer's texels.
- Greedy quads depend on repeat addressing in U and V. Any filtering policy must preserve that.
- Magnification must stay crisp: block art is deliberately pixel-sized, and blurring it up close is a regression.
- Tile size and layer count are small and fixed, so the decision should not add a texture-pipeline abstraction the project does not need yet.
- The milestone forbids new crates and changes to generation output. Texture work must not move world goldens.

---

## 2. Decision

**The block texture array uses a full mip chain generated in-engine at startup by 2×2 box filtering, with `mag_filter = Nearest`, `min_filter = Linear`, `mipmap_filter = Linear`, `anisotropy_clamp = 1`, and repeat addressing in U and V.**

For a 16×16 tile the chain has `floor(log2(16)) + 1 = 5` levels, sized 16×16, 8×8, 4×4, 2×2, and 1×1. Level 0 is the existing procedural tile bytes, unchanged. Every later texel is the rounded average of the four texels above it in the same layer; alpha is written as 255 because the tile set is fully opaque. Every level is uploaded to the same `texture_2d_array`, so the array, view, bind group, layer indices, and shader sampling call are unchanged.

`address_mode_w` stays `ClampToEdge`, as it was, because a 2D array does not use W for tiling. No shader or UV change is required: implicit LOD from the UV derivatives selects the chain, and repeat addressing wraps within the selected layer.

---

## 3. Why This Policy

### 3.1 In-engine generation follows the source of truth

The tiles are procedural bytes in `src/renderer/texture.rs`; there is no image file to prefilter. Generating the chain next to the tile generator keeps one deterministic source, works offline, needs no new crate, and is exercised by ordinary CPU unit tests. A build-time tool would have to duplicate or import the generator for a 15 KB texture.

### 3.2 A box filter matches the tile scale

Each mip texel represents exactly one 2×2 block of the level below, so a box average is the natural prefilter for power-of-two tiles. The chain is deterministic, uses only integer arithmetic, and is cheap enough that startup cost is not measurable against terrain generation. It also has an obvious test oracle: level 1 can be checked against the exact arithmetic mean for every texel.

### 3.3 Nearest magnification keeps block art crisp

`mag_filter = Nearest` preserves the deliberate pixel look when a tile is enlarged on screen. Linear magnification would soften every block face for no aliasing benefit, because magnification does not shimmer the way minification does.

### 3.4 Linear minification and mip blending remove the shimmer

At minification, `min_filter = Linear` filters within the chosen level and `mipmap_filter = Linear` blends the two nearest levels. Between them, they keep the sample close to the tile's average instead of picking one arbitrary texel, which is what removes the distant crawl. The cost is up to eight texel fetches per fragment instead of one; for 16×16 tiles with a small chain, that is a normal trilinear sample and not a new performance class.

### 3.5 No anisotropy for now

Anisotropic filtering would help most at grazing angles, such as ground seen toward the horizon. It also multiplies fetches further, and the deferred question explicitly includes whether the array should use it. With 16×16 placeholder tiles and a five-level chain, trilinear sampling already removes the dominant shimmer. Setting `anisotropy_clamp = 1` states the choice in code and keeps the sampler valid for the filtering bind group. Anisotropy can be raised later without changing the chain, the upload path, or the UVs.

### 3.6 Layer isolation still holds

Mip generation runs once per layer, reading only that layer's four source texels and writing only that layer's destination texels. The texture is still a `texture_2d_array`; sampling takes one integer layer index, and WebGPU filtering is defined inside that layer's image and its mip chain. There is no cross-layer interpolation, no shared atlas gutter, and no coordinate that could address a neighbouring tile. Repeat addressing wraps at the layer edge, which is the same behaviour the greedy UV contract already relies on: a quad that spans several blocks repeats the tile instead of sampling its neighbour. The layer-isolation test builds solid, distinct colours per layer and asserts every level of every layer keeps its own colour, so any accidental cross-layer read fails immediately.

### 3.7 Mip seams do not need UV changes

Because each layer has its own complete chain, the top and bottom levels wrap inside the same image, and U/V repeat covers the swap between levels. There is no atlas border to bleed and no half-texel inset to add. The existing block-unit UVs and the shader's `textureSample(array, sampler, uv, layer)` call are correct as they stand, so this decision changes no shader or mesher code.

---

## 4. Costs

### 4.1 Memory

Each layer stores 256 + 64 + 16 + 4 + 1 = 341 texels instead of 256. At 4 bytes per RGBA8 texel that is 1,364 bytes per layer, and 15,004 bytes for all eleven layers, versus 11,264 bytes for level 0 alone. The chain adds **3,740 bytes (+33.2%)** to the block texture. This is the logical texture size; drivers may round allocations up, but the order of magnitude is a few kilobytes.

### 4.2 Generation time

A release-mode measurement of 10,000 complete chain builds averaged **18.2 µs per chain** on the development machine. Generation happens once, during renderer startup, before the first frame. There is no per-frame CPU cost and no runtime allocation after startup.

### 4.3 Sampling cost

Trilinear minification fetches up to eight texels instead of one. The array is tiny and already resident, block shading dominates the fragment cost, and the change affects minified fragments only. No frame-time regression was measured for this decision; the visual check is the demo smoke run recorded with the implementation.

---

## 5. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Five-level CPU box chain, nearest mag, linear min/mip, no anisotropy** | Deterministic, no asset pipeline, no UV change, fixes the shimmer, keeps block art crisp, small fixed cost. | **Selected** |
| **Keep one level with nearest filtering** | Simplest; distant minification keeps aliasing and shimmer, the problem this decision exists to solve. | Not selected |
| **One level with linear minification** | Softens distant texels but still samples a single 16×16 level, so aliasing remains at strong minification. | Not selected |
| **GPU-generated mips (render or compute passes)** | Avoids a CPU pass; wgpu exposes no mip-generation helper, so this means extra pipelines, a per-layer draw/dispatch, and new validation surface for 15 KB of data. | Not selected |
| **Build-time mip generation** | Moves work off startup; requires an asset pipeline the engine does not have and duplicates the procedural generator. | Not selected for now |
| **Linear magnification** | Smooths close-up tiles; blurs the intended pixel art and does not address minification aliasing. | Not selected |
| **Anisotropic filtering** | Better at grazing angles; more fetches, and ADR-0007 deferred the policy. Trilinear already removes most visible shimmer at this tile size. | Deferred |
| **Mip chain averaged in linear light** | More physically correct for an sRGB format; adds decode/encode and changes replacement art less predictably. The placeholder tiles make the difference minor. | Deferred |
| **Clamp addressing** | Hides some edge cases; breaks greedy tiling across quads, which ADR-0007 requires. | Not selected |

---

## 6. Counterarguments and Disposition

| Counterargument | Disposition |
|---|---|
| **Trilinear sampling costs more than a single fetch.** | Accepted. It is up to eight fetches on minified fragments only, over a tiny resident array, and it buys the anti-aliasing this decision is for. |
| **Averaging stored sRGB bytes is not linear-light correct.** | Accepted as a deliberate approximation for placeholder art. The filter stays deterministic and matches the bytes the tile generator produced; a linear-light chain can replace it without changing the policy or the tests' shape. |
| **Distant tiles lose detail and look softer.** | Accepted and intended. That is what prefiltering does; level 0 is untouched and magnification still nearest, so close-up art stays pixel-crisp. |
| **A 1×1 top level averages each tile to one colour.** | Accepted. A tile that covers less than a pixel can only contribute its average; otherwise it aliases. |
| **Every tile is assumed opaque.** | Accepted. Alpha is forced to 255, which is correct for the current eleven layers. Cutout or translucent tile art needs a new alpha policy before it is added. |
| **Anisotropy would look better on ground at the horizon.** | Accepted. Trilinear is the low-risk first step; anisotropy is a sampler change that can be decided separately once there is real art and a scene to measure. |
| **The chain duplicates data that a future asset pack will also provide.** | Accepted. When an asset pipeline arrives it must supply matching levels or the renderer keeps generating them; either way the sampler policy stands. |
| **More levels mean more upload calls at startup.** | Accepted. Five calls per array instead of one, each with a few kilobytes, once at startup. |

---

## 7. Consequences

### 7.1 Positive

- Distant terrain and water stop crawling between arbitrary texels.
- The tile source remains the single deterministic definition of the art.
- Layer isolation is unchanged; no atlas padding or UV inset is needed.
- Magnification stays nearest, so block art stays crisp up close.
- No mesher, shader, vertex, save-format, or generation change.
- The policy is covered by CPU tests that do not need a GPU.

### 7.2 Negative

- The texture grows from 11,264 to 15,004 bytes.
- Startup does about 18 µs of extra CPU work.
- Minified fragments fetch up to eight texels.
- Filtering averages encoded sRGB bytes rather than linear light.
- The alpha-255 rule is only valid while every tile is opaque.

### 7.3 Neutral

- The array, layer table, bind group layout, and sampling call are unchanged.
- Mip selection uses the shader's existing UV derivatives; greedy and scaled LOD quads need no special case.
- A future asset pipeline or anisotropy decision can build on this chain instead of replacing it.

---

## 8. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A mip level has the wrong size, count, or byte length. | Derive the count from the tile size and assert every level's dimensions and length in unit tests. |
| A generator change silently alters level 0. | Compare level 0 to the unchanged generator and pin an FNV-1a digest of its bytes in a test. |
| Downsampling is wrong or non-deterministic. | Test level 1 against the exact 2×2 rounded average for several layers, and compare two complete builds byte-for-byte. |
| A mip filter reads a neighbouring layer. | Generate each layer independently and test solid per-layer colours through every level. |
| The sampler silently stays nearest/min nearest. | Set all three filter modes explicitly and keep the sampler descriptor in the reviewed texture module. |
| Gamma-incorrect filtering shows as a brightness shift. | Record the approximation here; revisit with linear-light averaging if real art exposes it. |
| Anisotropy is enabled by a default change. | Set `anisotropy_clamp = 1` explicitly. |
| Non-opaque art arrives without an alpha policy. | Force alpha 255 in the downsample and treat transparent tiles as a new decision. |

---

## 9. Conclusion

The block array gets its full five-level mip chain from the same deterministic bytes that already define the tiles, and the sampler magnifies nearest, minifies linear, and blends levels linearly with no anisotropy. This removes the distant minification shimmer that a single 16×16 level cannot avoid, while preserving crisp close-up art, repeat-based greedy tiling, and the per-layer isolation that justified the array in ADR-0007. The costs are 3,740 bytes, about 18 µs of one-off startup work, and up to eight fetches on minified fragments; the policy and its approximations are recorded above. I accept this as the block-texture mipmap and sampling policy for the current milestone.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on block-texture filtering, mip levels, or the sampler state. ADR-0007 continues to own the array storage model; its mipmap deferral is resolved here rather than rewritten.*
