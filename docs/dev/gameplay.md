# Gameplay core

**Status:** `0.0.10.1` items and save, `0.0.10.2` UI, `0.0.10.3` crafting Phase 1, and the
`0.0.10.4.1` survival state plus `VPLY` version 2 are implemented. Damage, death/respawn, the
survival HUD, audio, and the full acceptance pass remain.

**Scope:** The headless gameplay state and the v1 demo wiring: items, inventory, tools, breaking,
drops, item entities, survival state, the fixed tick, and the player save. It does not cover the UI
layer, recipes, damage sources, audio, creatures, or the legacy chunk world's interaction path.

This page is the detailed reference. The release plan is
[0.0.10-0.0.19](roadmap-0.0.10-0.0.19.md), the milestone scope is [v0.0.10](v0.0.10-scope.md), the
file layout is in [Architecture](architecture.md#modules), and the wire format is in
[Save format](save-format.md#player-save-vply).

## Where it runs

| Path | Interaction | Items |
|---|---|---|
| v1 terrain demo (`TERRAIN_DEMO=1`) | Hold-to-break, drops, pickup, inventory-backed placement, player save | Full gameplay core |
| Legacy chunk world (default) | v0.0.4 instant break and place | Five hard-coded block slots, no inventory or player save |

The demo is the 0.0.10 acceptance target. The legacy path stays as a fallback until the demo
becomes the default in 0.0.11. Headless runs and stress scenes do not tick gameplay and never load
or write a player save.

## Module map

| Path | Responsibility |
|---|---|
| `src/game/mod.rs` | The internal exports for the gameplay core. It names no `winit` or `wgpu` type. |
| `src/game/item.rs` | Item names, derived ids, definitions, kinds, and stacks. |
| `src/game/inventory.rs` | Nine hotbar slots, twenty-seven main slots, one offhand, and stack operations. |
| `src/game/tools.rs` | Tool classes, tiers, break rules, speed multipliers, and the drop gate. |
| `src/game/drops.rs` | The content-pack drop table resolved through the item registry. |
| `src/game/entities.rs` | Dropped-item entities, spawn merging, physics, pickup, spatial hash, despawn, and save records. |
| `src/game/survival.rs` | Health, stamina, death flag, spawn point, and the fixed-tick stamina drain/regeneration. |
| `src/game/save.rs` | The `VPLY` envelope, CRC32, validation, atomic write, and quarantine. |
| `src/game/state.rs` | `GameState`: inventory, survival, entities, break progress, fixed tick, and save/load. |
| `src/app/game.rs` | The only world-aware glue: target names, hardness, break completion, drops, placement, and save paths. |
| `src/renderer/items.rs` | The placeholder dropped-item cube pass. |
| `src/highlight.wgsl` | The block outline, tinted by break progress. |

## Frame and tick

`GameState` runs a fixed 60 Hz step (`1 / 60 s`) behind an accumulator. The accumulator is capped
at `0.25 s`, so a long hitch cannot run hundreds of steps at once.

- **Fixed tick:** dropped-item physics, pickup, despawn, and break progress.
- **Frame:** player movement, camera, target resolution, placement, and save state.

Player movement stays frame-driven because the camera has no render interpolation yet. That is the
one deliberate clock split; it is removed when interpolation lands.

The per-frame order in the demo is:

1. Resolve the targeted cell with the DDA raycast against `DemoRuntime::is_solid`, within `6 m`.
2. Read the target's stable block name and content-pack hardness.
3. Read the selected tool and compute the break duration.
4. Feed the target and duration to `GameState::set_break_target`.
5. Run `GameState::tick`, which advances entity physics and break progress.
6. Take a completed break, remove the block, compute drops, spawn entities, and damage the tool.
7. Pick up nearby entities.
8. Upload the item cubes and the highlight progress.

Releasing the button, losing focus, or pressing `Escape` cancels the break. Changing the target
resets progress to zero.

## Items

### Identity

An item is identified by a stable `namespace:name` string, such as `core:stone` or
`core:wooden_pickaxe`. Numeric ids are derived by sorting the definitions by name at load, then
assigning `0, 1, 2, ...`. This follows the content-pack rule in
[ADR-0015](../decisions/0015-texture-references-are-presentation.md): numeric ids are an in-memory
index, not an on-disk identity.

- Save files store names, never numeric ids.
- Adding an item changes later numeric ids but never invalidates a save.
- Two registries built from the same definition set assign the same ids on every run.
- `ItemId` is a `u16` newtype. The built-in table has 27 entries, well below the limit.

### Built-in table

The registry is built from the content pack plus the built-in tools and materials:

| Group | Names | Max stack | Kind |
|---|---|---|---|
| Block items | `core:bedrock`, `core:boulder`, `core:clay`, `core:dirt`, `core:grass`, `core:gravel`, `core:ice`, `core:lamp`, `core:leaves`, `core:ore_copper`, `core:ore_iron`, `core:planks`, `core:plant`, `core:sand`, `core:snow`, `core:stone`, `core:torch`, `core:wood`, `core:workbench` | 64 | Block |
| Tools | `core:wooden_pickaxe`, `core:wooden_axe`, `core:wooden_shovel`, `core:wooden_sword`, `core:stone_pickaxe`, `core:stone_axe`, `core:stone_shovel`, `core:stone_sword`, `core:iron_pickaxe` | 1 | Tool |
| Materials | `core:stick` | 64 | Material |
| Food | `core:apple` | 64 | Food, 4 nutrition |

`core:air` and `core:water` are not items. Water is a liquid and has no placement item yet. Every
non-air, non-liquid content-pack record becomes a block item automatically, so a future record
does not need a second item table entry.

`core:planks`, `core:torch`, and `core:workbench` are in the content pack as of `0.0.10.3`.
`core:furnace` remains referenced by the break rules but is reserved for Phase 2 and is not in the
pack, so it cannot be obtained or placed yet.

### Stacks and durability

`ItemStack` carries an item id, a count, and a durability. Durability is the remaining points for a
tool and zero for everything else. A tool stack always has count one. Breaking a block applies one
durability point to the selected item; a tool at zero is removed from its slot.

## Inventory

The inventory has 37 slots:

| Range | Slots | Purpose |
|---|---|---|
| `0..9` | 9 | Hotbar |
| `9..36` | 27 | Main inventory |
| `36` | 1 | Offhand |

The selected hotbar index is `0..8`. The inventory screen uses a held cursor stack for
pick/place/swap; split and move remain state APIs used by the screen.

### Operations

| Operation | Behaviour |
|---|---|
| `add(stack)` | Merge into existing stacks with room in slot order, then fill empty slots in slot order. Returns the part that did not fit. |
| `remove(slot, count)` | Remove up to `count` from one slot and return the removed stack. |
| `take_one(slot)` | `remove(slot, 1)`. |
| `split(slot)` | Move half (rounded down) into the first empty slot. Returns the moved stack, or `None` when the stack is smaller than two or no slot is free. |
| `move_stack(from, to)` | Move to an empty slot, merge same-item stacks up to the limit, or swap different stacks. |
| `quick_move(index, range)` | Merge into the destination range, then fill empty slots. Returns `true` when the source slot is empty. |
| `damage(slot, amount)` | Subtract durability. Returns `true` when the item breaks and the slot is cleared. |
| `count(item)` | Total count across every slot, as `u32`. |
| `is_full()` | True only when all 37 slots are occupied. |

A partial add is not an error: the caller receives the leftover and may leave it on the ground.
`EntityStore::pickup` uses that rule, so a full inventory does not destroy a drop.

## Crafting

**Status:** Phase 1 is implemented in `0.0.10.3`; the full design is in
[the crafting system](../design/crafting-system.md).

Crafting uses one grammar for every recipe:

```text
station · tier · pattern · output · cost · unlock · discipline
```

Phase 1 uses `Station::Inventory` (2x2), `Station::Workbench` (3x3), `Tier::T1`,
`CraftCost::Instant`, and `Unlock::Start`. The fields are present from day one so Phase 2 can add
station tiers, costs, and discovery without reshaping every recipe.

- **Patterns.** Shaped recipes trim the occupied grid and match at any offset; a 2x2 grid cannot
  make a 3x3 recipe. Tagged recipes match a multiset of item tags. Hybrid patterns are deferred.
- **Tags.** Phase 1 has `wood`, `planks`, `stone`, and `fuel`.
- **Surfaces.** The inventory gains a 2x2 grid and output slot. A placed `core:workbench` opens a
  3x3 screen. Closing either screen returns the grid and cursor to the inventory; the grid is
  transient and is not in the `VPLY` save.
- **Recipe book.** Every Phase 1 recipe is known at start. The book shows ingredients, station,
  tier, discipline, and a craftable/missing state; it has search and one-click craft. It does not
  save glimpsed or unknown recipes yet.
- **Salvage.** A static table returns a small amount of material from tools, workbench, and torches.
  It is not reverse-engineering and does not recover affixes or quality.
- **New blocks.** `core:planks`, `core:workbench`, and `core:torch` join the content pack. The
  torch emits light and needs a solid support. `core:furnace` and smelting are Phase 2.

Quality, rarity, sub-rarity, affixes, catalysts, discipline XP, station upgrades, experimentation,
ruins/boss recipes, reverse-engineering, queues, portable stations, and automation are **not** in
Phase 1. They need the item-instance side table, a player-save version, and their own releases;
see [the crafting design](../design/crafting-system.md).

## Survival

`game::survival::Survival` carries health (`0..=20`), stamina (`0..=100`), a death flag, and the
respawn point. It is part of `GameState` and advances stamina on the fixed 60 Hz step: sprinting
drains 20 points per second, and idle regeneration restores 15 per second.

Health, stamina, and the spawn point are stored in `VPLY` version 2. A version 1 file loads with
full health and stamina and uses the saved player position as the spawn point; the next save writes
version 2.

Damage sources, the death/respawn flow, and the survival HUD land in the rest of `0.0.10.4`. Until
then `GameState::tick` advances stamina with sprinting off, and the state is persisted but not
shown.

## Tools, hardness, and breaks

### Tool table

| Item | Class | Tier | Durability |
|---|---|---:|---:|
| `core:wooden_pickaxe` | Pickaxe | 1 | 60 |
| `core:wooden_axe` | Axe | 1 | 60 |
| `core:wooden_shovel` | Shovel | 1 | 60 |
| `core:wooden_sword` | Sword | 1 | 60 |
| `core:stone_pickaxe` | Pickaxe | 2 | 132 |
| `core:stone_axe` | Axe | 2 | 132 |
| `core:stone_shovel` | Shovel | 2 | 132 |
| `core:stone_sword` | Sword | 2 | 132 |
| `core:iron_pickaxe` | Pickaxe | 3 | 251 |

### Break rules

The preferred class only affects speed. The minimum tier gates the drop, and the class must match
when the minimum is above zero.

| Blocks | Class | Minimum tier |
|---|---|---:|
| `core:stone`, `core:ore_copper`, `core:ice`, `core:boulder` | Pickaxe | 1 |
| `core:ore_iron`, `core:furnace` | Pickaxe | 2 |
| `core:bedrock` | Pickaxe | 4 |
| `core:wood`, `core:planks`, `core:workbench`, `core:leaves`, `core:plant` | Axe | 0 |
| `core:dirt`, `core:grass`, `core:sand`, `core:gravel`, `core:clay`, `core:snow` | Shovel | 0 |
| Anything else, including `core:lamp` | Hand | 0 |

A minimum tier of zero always drops, including by hand. Above zero, a tool of the wrong class does
not satisfy the gate: a wooden axe does not mine stone just because it is wooden. Bedrock requires
tier 4, and the highest built-in tool is tier 3, so it never drops.

### Hardness and time

Hardness is content data (`BlockRecord::hardness`). The current table is:

| Block | Hardness (s) | Block | Hardness (s) |
|---|---:|---|---:|
| `core:bedrock` | 100.0 | `core:ore_copper` | 3.0 |
| `core:boulder` | 2.5 | `core:ore_iron` | 3.0 |
| `core:wood` | 2.0 | `core:stone` | 1.5 |
| `core:clay` | 0.6 | `core:grass` | 0.6 |
| `core:gravel` | 0.6 | `core:dirt` | 0.5 |
| `core:ice` | 0.5 | `core:sand` | 0.5 |
| `core:lamp` | 0.3 | `core:leaves` | 0.2 |
| `core:snow` | 0.2 | `core:plant` | 0.0 |

The break time is:

```text
time = max(hardness / speed, 1 / 60)
```

The speed multiplier is `2.0` for tier 1, `4.0` for tier 2, and `6.0` for tier 3 or higher when
the tool class matches the rule. Hand and wrong-class tools use `1.0`.

Examples:

| Target | Tool | Time | Drop |
|---|---|---:|---|
| `core:stone` (1.5) | Wooden pickaxe (2.0) | 0.75 s | Stone |
| `core:stone` (1.5) | Hand (1.0) | 1.5 s | None |
| `core:ore_iron` (3.0) | Stone pickaxe (4.0) | 0.75 s | Iron ore |
| `core:ore_iron` (3.0) | Wooden pickaxe (2.0) | 1.5 s | None |
| `core:wood` (2.0) | Wooden axe (2.0) | 1.0 s | Wood |
| `core:dirt` (0.5) | Wooden shovel (2.0) | 0.25 s | Dirt |
| `core:bedrock` (100.0) | Any built-in tool (at most 6.0) | 16.7 s or more | None |

A break at or below one fixed tick is clamped to `1 / 60 s`, so a zero-hardness block still takes
one tick.

## Drops

The content pack owns the drop names and counts. In the current pack every self-dropping record
drops one of itself; `core:bedrock`, `core:lamp`, `core:water`, and `core:air` drop nothing.

`drops_for(pack, registry, block, tool)`:

1. Returns an empty list when `can_drop` fails.
2. Resolves the block name to its content record.
3. Converts every `(drop name, count)` pair to an item stack.
4. Skips a name the item registry does not know.

A tool below the block's minimum tier still breaks the block, but yields no drop. The block is not
destroyed differently; only the drop list is empty.

## Item entities

A dropped item is an `ItemEntity`:

| Field | Meaning |
|---|---|
| `id: EntityId(u64)` | Stable, monotonic id; never reused. |
| `stack: ItemStack` | Item, count, and durability. |
| `position: Vec3` | Centre of the placeholder cube. |
| `velocity: Vec3` | Metres per second. |
| `age: f32` | Seconds since spawn. |

### Spawn merging

Identical items within `0.75 m` of the spawn point merge before a new entity is created. The merge
is capped by the definition's maximum stack. A full target is skipped; a partial merge leaves the
remainder as a new entity. Tools have a maximum stack of one, so they never merge. The function
returns the existing id when the whole stack merged and a new id otherwise.

Merging is a spawn-time rule. Two drops that drift together after spawning stay separate until one
is picked up; pickup still merges into the inventory.

### Physics

The entity tick uses the same `BlockSource` as the player. Each fixed step:

1. Add gravity (`24 m/s²`) to the vertical velocity.
2. Move on Y, then X, then Z.
3. For each axis, probe the candidate centre and the leading edge at half-extent `0.15 m`.
4. If the probe enters a solid block, cancel that axis. Downward motion snaps the centre to the
   block top plus `0.15 m`; upward motion snaps it to the block bottom minus `0.15 m`.
5. Damp horizontal velocity by `0.9` per tick.

Items do not collide with each other. The placeholder cube is `0.3 m` on an edge, matching the
physics half-extent.

### Pickup and despawn

- Pickup radius is `1.5 m` from the player's feet position.
- The spatial hash is keyed by the floor cell of each entity. Pickup scans every cell within
  `ceil(radius)` on each axis, then applies an exact 3D distance check.
- Each candidate is added to the inventory. The leftover stays on the entity, so a full inventory
  leaves the whole drop on the ground.
- The pickup returns the number of items moved, as `u16` with saturation.
- An entity despawns when `age >= 300 s`.

### Save records

`EntityRecord` stores the id, item name, count, durability, position, velocity, and age. Loading:

- skips an unknown item name;
- skips a duplicate id;
- sets the next id to the highest restored id plus one;
- rebuilds the index and spatial hash.

A player save is limited to **4,096** entities. A larger count is rejected as malformed before any
allocation.

## Interaction

### Breaking

1. The first left click after the cursor is released only captures the cursor.
2. While captured, pressing the left button starts a break.
3. Every frame the app resolves the target and computes the total duration from the held tool.
4. The fixed tick advances the progress by `1 / 60 s`.
5. When progress reaches the duration, the app removes the block through `DemoRuntime::edit`, reads
   the drops with the held tool, spawns item entities at the block centre, and applies one
   durability point to the selected item.
6. Releasing the button, losing focus, or pressing `Escape` cancels the break.

The demo rejects removing air at `y = 0`. The bottom layer is the world floor and stays intact.
The target must be a resident, solid cell; an unloaded column cannot be broken.

### Placement

1. Raycast the same target as breaking.
2. Place into the adjacent cell on the entered face.
3. Reject the cell if it intersects the player's AABB.
4. Require the cell to be resident. Air and replaceable blocks are accepted; any other block is
   rejected.
5. A non-air replaceable block is cleared first, so the placement becomes two edits.
6. A `core:plant` requires a solid block directly below.
7. The selected item must be a `Block` item. A tool, material, or food cannot be placed.
8. `DemoRuntime::edit` must accept the write; then one item is removed from the selected slot.

### Starting kit

`GameState::new` starts with an empty inventory. The wood-to-stone chain is the starting path:
punch a tree, craft planks and sticks in the 2x2 grid, place a workbench, craft the first wooden
pickaxe, then mine stone for the stone tools.

### Hotbar

`1` through `9` select a hotbar slot. `E` opens the inventory screen, which shows the hotbar, main
grid, and offhand; it is documented in [UI](ui.md#inventory). The legacy path still uses `1`
through `5` for its hard-coded block list.

## Persistence

The player save is a sidecar to the terrain store:

| World | Path |
|---|---|
| v1 demo | `world/terrain-demo/player.dat` |
| Legacy | `world/player.dat` |

It is written on `Ctrl+S` and on window close. `Ctrl+S` flushes the terrain journal but keeps the
save worker alive, so later edits still queue. Closing joins the worker and then writes the player
file. Stress scenes and headless runs skip both loading and saving.

The current player format is `VPLY` version 2: position, view, health, stamina, spawn point,
inventory, selected slot, and dropped entities. Version 1 is accepted and upgraded in memory.

On load:

- a missing file is normal;
- a seed mismatch is ignored, because the file belongs to another world;
- a bad magic, unknown version, checksum failure, malformed payload, unknown item, or oversized
  entity count is an error;
- an error moves the file aside to `player.bad` and starts fresh.

The exact byte layout, checksum, and validation order are in
[Save format](save-format.md#player-save-vply).

## Rendering

Dropped items are placeholder cubes, not textured blocks:

- The pass lives in `src/renderer/items.rs` and `src/items.wgsl`.
- Each item is 36 vertices: six faces, two triangles per face.
- Positions are relative to the renderer's origin, so the pass shares the camera uniform.
- The cube is `0.3 m`, depth-tested, and not back-face culled.
- The buffer is rebuilt from the live entity list each frame; the pass is skipped when the list is
  empty.
- Colours come from a small name-based table in `renderer/items.rs`. Stone is grey, grass green,
  dirt brown, wood brown, sand pale, and tools a muted metal or wood tone.

The block outline uses the spare `camera_position.w` lane of the frame uniform for break progress.
At zero the fragment shader uses a pale yellow; at one it uses a hot orange, with a small pulse.
No new bind group or buffer is needed.

There is no hand model, held-item rendering, crack texture, particle effect, or item label yet.
The gameplay HUD (crosshair, hotbar, and interaction prompt) is documented in [UI](ui.md); it
reads the same inventory and never captures gameplay input.

## Invariants and edge cases

- Item ids are in-memory only. The save writes stable names.
- Block ids are content-pack derived and are not written to the player save.
- `GameState` has no `winit` or `wgpu` dependency; the renderer reads its entity list.
- An entity id is never reused; `from_records` restores the high-water mark.
- A full inventory never destroys a drop; the remainder stays on the ground.
- Breaking checks reach, solidity, and residency before starting; moving out of range cancels.
- A block's minimum tier gates drops, and the class must match when the minimum is above zero.
- A break duration is never below one fixed tick.
- A placement consumes the item only after the world edit succeeds.
- A seed mismatch ignores the player save; it does not delete it.
- Corrupt player data is quarantined, never partially applied.
- The player save is capped at 4,096 entities.
- Stress and headless runs never read or write a player file.

## Testing

Gameplay tests live beside the modules:

| Module | Cases |
|---|---|
| `item` | Sorted ids, every placeable block has an item, tool stacks, liquids and air are excluded. |
| `inventory` | Merge then fill, overflow return, remove/take, split, merge/swap, durability break, hotbar clamp. |
| `tools` | Matching-tool speed, wrong-tool speed, stone tier gate, wrong-class gate, bedrock never drops. |
| `drops` | Stone with a pickaxe, stone by hand, every drop resolves to a block item, unknown block. |
| `entities` | Fall and land, pickup, full inventory, despawn, record round trip, spawn merging, distant/different drops. |
| `save` | CRC check value, full round trip, corrupt payload, bad magic, unknown version, oversized entity count, file-system write/load/quarantine. |
| `state` | Empty starting inventory, fixed-tick break, cancel/reset, save/load, seed mismatch, scripted gather-use-save-reopen. |

The scripted loop in `state` covers the release gate's headless half: spawn a stone drop from the
current tool, pick it up, select the hotbar slot, consume one placeable item (the state-side half
of placement), save, reopen, and assert the player and inventory survived.

Manual acceptance uses the demo:

```console
# PowerShell
$env:TERRAIN_DEMO = "1"
$env:ENGINE_STRESS_SCENE = "1"   # only when world/seed is absent
cargo run --bin engine
```

Check hold-to-break on wood, the drop and pickup, the hotbar keys, placement rejection, and
quit/reopen persistence. The full manual pass belongs to `0.0.10.6`.

## Deliberate simplifications

| Simplification | Revisit when |
|---|---|
| Gameplay is wired to the demo only; the legacy path keeps instant break | 0.0.11 makes the demo the default |
| Player movement is frame-driven | Render interpolation lands |
| Tool class and tier are game tables, not content records | The authored pack lands in 0.0.12 |
| No starting kit; the player punches wood to begin | Crafting Phase 1 is implemented in `0.0.10.3` |
| Ground merging happens on spawn and pickup, not continuously | The 0.0.10.2 UI needs merged ground stacks |
| Placeholder item cubes use flat colours | The 0.0.11 art pipeline lands |
| No crafting depth (quality, rarity, affixes, disciplines, discovery, queues, automation) | The crafting Phase 2+ releases |
| No autosave | `0.0.10.6` acceptance decides the cadence |
| No hand or held-item rendering | The 0.0.11 art pass |

## Next checkpoints

- **`0.0.10.2`** added egui, the HUD, the inventory, drag and drop, shift-click, tooltips,
  keyboard navigation, and UI-first input routing.
- **`0.0.10.3`** added crafting Phase 1: the universal grammar, shaped/tagged recipes, the 2x2
  inventory grid, 3x3 workbench, known recipe book, tags, salvage, and the wood-to-stone chain. It
  added `core:planks`, `core:workbench`, and `core:torch`; the furnace is Phase 2.
- **`0.0.10.4`** adds the survival state and `VPLY` version 2, then fall damage, drowning hooks,
  death, respawn, the survival HUD, and pause/death screens.
- **`0.0.10.5`** adds kira audio, the first sounds, the settings screen, and remappable keybinds.
- **`0.0.10.6`** adds save hardening, the full acceptance run, docs, changelog, and the release.

## File reference

| File | What to read it for |
|---|---|
| `src/game/item.rs` | Item definitions, ids, and stack construction. |
| `src/game/inventory.rs` | Slot layout and every stack operation. |
| `src/game/tools.rs` | Break rules, speed, and the drop gate. |
| `src/game/drops.rs` | Content-pack drop resolution. |
| `src/game/entities.rs` | Spawn, merge, physics, pickup, despawn, and records. |
| `src/game/survival.rs` | Health, stamina, spawn point, and stamina tick. |
| `src/game/save.rs` | `VPLY` encoding, validation, atomic write, and quarantine. |
| `src/game/state.rs` | Fixed tick, break progress, and save/load. |
| `src/app/game.rs` | Demo target resolution, break completion, placement, and save paths. |
| `src/renderer/items.rs` | Placeholder cube geometry, colours, and buffer upload. |
| `src/highlight.wgsl` | Break-progress colour in the outline shader. |
