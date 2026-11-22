# Why I Chose a Minimal Entity Runtime Instead of an ECS

**Document type:** Architecture Decision Record / Devlog Entry
**Status:** Accepted; scoped for v0.0.10
**Date:** 2026-11-22
**Project:** Valescent Engine
**Owner:** Project Owner
**Supersedes:** None.
**Scope:** The gameplay entity store, stable ids, spatial lookup, fixed-tick simulation, and save
records. It does not decide creature AI, animation, navigation, or a scripting layer.

---

## Summary

This document records why the engine starts with a small purpose-built entity store instead of an
ECS crate. The short version: v0.0.10 has one entity kind (a dropped item), the store needs stable
ids and deterministic save records more than it needs query machinery, and a small runtime can grow
activation tiers and creature data later without committing the whole codebase to an ECS model.

---

## 1. Context

### 1.1 The entity work in v0.0.10

Breaking a block spawns a dropped item. The item falls, rests on the ground, merges with nearby
stacks, is picked up, and despawns. It must survive save and reload.

Later releases add creatures, fish, particles with gameplay meaning, boats, and fishing bobbers.

### 1.2 The constraints

- The simulation runs on a fixed 60 Hz tick, separate from the render frame.
- Entities need stable ids across save and reload.
- Pickup queries happen every tick for every player; the store needs a spatial lookup that does not
  scan the whole world.
- Entity state must be written into the versioned player save without exposing internal indices.
- The 1,000-line gate and clippy pedantic apply.
- A full ECS would be a large dependency and a new programming model for a single entity kind.

---

## 2. Decision

**The engine uses a minimal entity store with stable `u64` ids, dense component vectors for the
entity kinds it has, a spatial hash for proximity queries, and explicit save records.**

- `EntityId(u64)` comes from a monotonic counter owned by the store; ids are never reused.
- v0.0.10 stores item entities in a `Vec<ItemEntity>` plus an id-to-index map.
- A spatial hash keyed by block cell maps to entity ids and is rebuilt when the dirty set changes.
- Physics, pickup, merging, and despawn run on the fixed tick.
- Save records use the item name, count, durability, position, velocity, and age. Internal indices
  are never saved.
- Creatures in 0.0.16 add a second component vector and a shared transform, not a generic ECS.

---

## 3. Why a minimal runtime

### 3.1 The data is small and known

One entity kind with five fields does not justify archetypes, queries, or system scheduling. A
`Vec` and a `HashMap` are the whole storage model.

### 3.2 Stable ids are the real requirement

Save compatibility needs ids that mean the same thing after reload. A monotonic counter plus saved
ids is simpler and more explicit than deriving ids from storage order.

### 3.3 Spatial lookup is bounded

Dropped items are local and few. A cell hash gives the pickup query a small candidate set and can
be rebuilt incrementally as entities move.

### 3.4 It leaves room for creatures

A second entity kind can add its own vector, transform component, and tick step. If the game later
needs many kinds and cross-cutting queries, the store can be replaced behind the same ids and save
records.

### 3.5 No new dependency or programming model

The rest of the engine is plain structs and explicit systems. An ECS would be a second way to
organise state, and the release does not need it.

---

## 4. Alternatives considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Minimal entity store** | Small, explicit, stable ids, easy save records | **Selected** |
| hecs | Mature ECS, but adds a dependency and archetype model for one entity kind | Not selected |
| bevy_ecs | Powerful, heavier, and brings a scheduler and query model the engine does not use | Not selected |
| Store items inside the inventory only | No world drops, no physics, no pickup | Not selected |
| Per-chunk entity lists | Natural spatial partitioning, but complicates cross-chunk movement and pickup | Not selected for 0.0.10 |

---

## 5. Counterarguments and disposition

| Counterargument | Disposition |
|---|---|
| A custom store will be rewritten when creatures arrive | Accepted with qualification. Creatures add a component vector and a tick step; a full ECS is only reconsidered if query complexity grows beyond that. |
| Spatial hashing is extra code | Accepted. Pickup runs every tick and should not scan every entity in the world. |
| Stable ids add a counter to the save | Accepted. The counter is part of the record so ids are not reused after reload. |
| An ECS would make future features easier | Rejected for now. "Future features" is not a measured need, and the 0.0.10 scope is one entity kind. |
| A `Vec` plus `HashMap` can drift out of sync | Mitigated by making the store the only mutator and testing removal, despawn, and reload. |

---

## 6. Consequences

### 6.1 Positive

- The entity code stays small enough to review and test exhaustively.
- Save records are explicit and versionable.
- Pickup has a bounded candidate set.

### 6.2 Negative

- Adding many entity kinds means adding parallel storage and tick code.
- Cross-kind queries are manual.
- The spatial hash needs maintenance as entities move and despawn.

### 6.3 Neutral

- Entity ids are opaque to gameplay code.
- The runtime is internal; there is no public entity API in 0.0.10.

---

## 7. Risks and mitigations

| Risk | Mitigation |
|---|---|
| The id map and vector diverge | Keep one insertion/removal path and test despawn, pickup, and reload. |
| The spatial hash goes stale | Rebuild it when the dirty set is non-empty; test movement across cell boundaries. |
| Entity count grows without bound | Cap the store, merge stacks, and despawn by age. |
| Save records grow with every entity | Cap entities and use name references, not repeated full definitions. |
| Creatures force a rewrite | Keep the id and record layer separate from storage so it can be replaced. |

---

## 8. Conclusion

A minimal entity runtime is the deliberate choice for v0.0.10 because the release has one entity
kind and needs stable ids, bounded pickup queries, and deterministic save records more than it
needs an ECS. I accept that creatures will add parallel storage; I will revisit a full ECS only if
the number of kinds and cross-cutting queries makes the explicit store a measured maintenance cost.

---

*This entry is part of the Valescent Engine decision log. Future entity work should reference
ADR-0020 when it depends on stable ids, the spatial hash, or the save record shape.*
