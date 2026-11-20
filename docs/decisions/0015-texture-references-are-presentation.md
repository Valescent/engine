# Why Texture References Stay Out of the Content Hash

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-20  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** Per-face texture references on content-pack block records and their relationship to `record_hash`, `pack_hash`, the pinned world description, and saved sections. The texture array itself is ADR-0007; the atlas artwork, UV policy, and save formats are out of scope.

---

## Summary

This document records the decision that a block's per-face texture references live on its content-pack record, and that they are **presentation-only**: they are excluded from `record_hash` and therefore from `pack_hash`. A re-texture changes what the block looks like and nothing else — not the pinned description, not a section's baseline hash, not whether an existing world loads.

The short version: the content hash exists to catch changes that make saved blocks mean something different. A texture name does not change what a block *is*. Hashing it would make every re-texture invalidate every world and every golden for no correctness benefit, so the reference travels with the record but not with the hash.

---

## 1. Context

### 1.1 The project

The v1 terrain stack keeps block identity in a content pack (`A5`). Each `BlockRecord` carries the properties generation, lighting, meshing, and collision read: opacity, sky attenuation, emission, render shape, render layer, collision, replaceable, liquid, hardness, and drops. The pack is name-sorted, ids are derived from names, and the whole pack folds into one `pack_hash`.

Textures are a later addition. The renderer's eleven procedural layers are placeholder art selected by name (`block.rs`, `demo_texture_layer`), and the legacy path already maps grass to three distinct faces and wood to two. The v1 mesher stored a single layer per block, so grass drew its side tile on top.

### 1.2 The constraints

- A world is pinned to a description that embeds the content hash (`CONTENT = "<name>", <record_version>, <hash>`). A different hash is rejected rather than loaded with guessed blocks.
- A saved section stores a `baseline_hash` over the generated ids it was edited from. Different content, different baseline.
- Per-face references are required: grass has a top, side, and bottom tile; wood has a top and side tile; cross-quad plants use their side tile.
- There is no external asset pipeline yet. Texture names resolve against procedural placeholder layers, and an unknown name must not be fatal.
- A block's texture may change without changing its physics, lighting, or generation behaviour.

---

## 2. Decision

**Per-face texture references (`textures.top`, `textures.side`, `textures.bottom`) live on `BlockRecord` as stable names, and they are excluded from `record_hash`.**

The mesher resolves each reference to a texture-array layer and writes that layer into the packed quad for the face it belongs to: `+Y` uses the top reference, `-Y` uses the bottom reference, and `±X`/`±Z` plus cross quads use the side reference.

Because `record_hash` excludes them, `pack_hash` is unchanged by a re-texture, and `check_compatible` accepts a pack that differs only in textures. No format bump is required and no saved world is invalidated.

The names follow the same stable-name grammar as block names (`namespace:name`, lowercase), so they can be validated and later resolved by an asset pipeline.

---

## 3. Why Not Hash Textures

### 3.1 The hash protects meaning, not appearance

The content hash answers one question: "do the ids in this world mean what they meant when it was saved?" Opacity, collision, emission, and render shape all change that answer. A texture does not. Hashing appearance would make the guard fire on changes that cannot corrupt a world.

### 3.2 Re-texturing must not orphan worlds

Section baselines and description pins are compared by hash. If a texture name entered the hash, fixing a placeholder tile would reject every existing world and force a migration for a purely visual change. That is a high cost for no safety gain.

### 3.3 The mesh already carries the result

The quad stores the resolved layer index, not the texture name. Two worlds with different texture references but identical blocks generate identical geometry and differ only in the layer stored in each quad — exactly the separation this decision describes.

### 3.4 Textures need a source of truth anyway

Keeping the reference on the record means a block's appearance is defined in one place, next to its physics. A side table keyed by block name would work but splits the record and invites drift; numeric layer indices in the pack would couple content to the renderer's array layout.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Per-face names on the record, excluded from the hash** | One source of truth, per-face mapping, re-texture without world churn, unknown names fall back. | **Selected** |
| **Per-face names inside `record_hash`** | Strongest identity; every placeholder edit invalidates worlds, baselines, and goldens, and needs a record-version bump. | Not selected |
| **A texture map beside the pack, keyed by block name** | No record change; splits appearance from the block and duplicates the name set. | Not selected |
| **Numeric layer indices in the record** | Compact; couples content to the renderer's array order and breaks when layers are repacked. | Not selected |
| **Atlas rectangles in the record** | No resolution step; bakes a specific atlas layout into world-independent content. | Not selected |

---

## 5. Counterarguments and Disposition

| Counterargument | Disposition |
|---|---|
| **A texture change can still break a screenshot test or a visual baseline.** | Accepted. Visual expectations are tracked separately from world identity; a re-texture is expected to change them. |
| **Two packs can now claim compatibility while looking different.** | Accepted and intended. Compatibility means "the blocks behave the same"; appearance is allowed to differ. |
| **Names need validation to stay resolvable.** | Accepted. All three references are validated with the block-name grammar when the pack loads. |
| **Unknown texture names must not be fatal.** | Accepted. The renderer falls back to the stone layer, matching the existing placeholder policy. |
| **Skirts inherit the boundary face's layer, not the side layer.** | Accepted for now. Skirts are crack fillers on coarse LOD meshes; carrying the block id would add a field to the packed quad. Recorded as a known gap. |
| **A record can now be identical in hash but different in bytes.** | Accepted. `record_hash` is a semantic hash by design; the pack is compiled in, so there is no byte-level file to compare. |

---

## 6. Consequences

### 6.1 Positive

- Re-texturing a block never invalidates a world, a description pin, or a golden.
- Each block defines its appearance next to its physics.
- Grass, wood, and any future block get true per-face textures.
- Unknown texture names degrade to a visible fallback instead of failing a load.

### 6.2 Negative

- Two packs with the same hash can render differently.
- Texture references are not covered by the compatibility check.
- Skirt quads on coarse meshes use the boundary face's tile.

### 6.3 Neutral

- The renderer still owns the layer table; content owns the names.
- The save format is unchanged.
- A future asset pipeline can resolve names to layers without touching world identity.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A texture name is malformed or empty. | Reject at pack load with `DescriptionInvalid { key: "CONTENT" }`. |
| Two blocks reference the same name by accident. | Allowed; a shared texture is a valid authoring choice. |
| A re-texture silently changes what players see. | The changelog records appearance changes; visual baselines are separate from world identity. |
| Layer indices drift when the array changes. | Names resolve through one function with a tested mapping; indices never enter content. |
| A future author expects textures to be hashed. | This ADR and the `record_hash` docs state the exclusion explicitly. |

---

## 8. Conclusion

Texture references belong on the block record because appearance is a property of the block, and they stay out of the content hash because appearance is not identity. The record keeps one source of truth, the mesher resolves each face to its own layer, and a re-texture remains a presentation change that cannot reject a world. The cost is that compatibility no longer implies identical pixels; that is acceptable for a compiled-in content pack and is recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on the content hash or on per-face texture identity.*
