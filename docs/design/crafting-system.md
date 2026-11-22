# Crafting system

**Status:** Proposed. Phase 1 is scoped for `0.0.10.3`; Phases 2-7 are not scheduled.

**Source:** The project's internal Voxel RPG crafting design notes. This page is the engine-facing
adaptation, not a verbatim copy. It separates the long-term RPG design from what the current
roadmap commits to.

**Scope:** Recipe grammar, patterns, tags, stations, disciplines, quality, discovery, the recipe
book, bulk/queue/automation, and the Phase 1 slice. It does not decide the combat, resonance, or
survival systems that the wider design also describes.

## 1. Why this shape

The source document starts from one principle:

> One grammar. Many disciplines. Infinite recipes.

A player who understands one recipe card can read any recipe in the game. The depth comes from
materials, stations, discovery, quality, and mastery, not from a different UI for every station.

The three pillars are:

1. **One recipe grammar** for every recipe, from a torch to a relic.
2. **Eight disciplines**, each with its own materials, stations, and progression.
3. **The world is the recipe book**; recipes are discovered through play, not only unlocked in a
   menu.

The engine adds a fourth constraint: the crafting data must stay data-oriented. Recipes and tags
are loaded and validated once, then resolved by dense ids. No per-craft string comparison, hash
lookup, or allocation in the hot path.

## 2. Universal recipe grammar

Every recipe uses the same fields:

| Field | Meaning | Phase 1 |
|---|---|---|
| `station` | Where the recipe is made | `Inventory` (2x2) or `Workbench` (3x3) |
| `tier` | Minimum station/discipline tier | `T1` only |
| `pattern` | Shaped, tagged, or hybrid | Shaped and tagged |
| `output` | Item, count, and later quality/rarity rules | Item and count |
| `cost` | Time, fuel, catalysts, essence | Instant and free |
| `unlock` | Starting, experimentation, ruin, boss, reverse-engineer | `Start` only |
| `discipline` | Construction, Smithing, Woodworking, etc. | Metadata only |

The Phase 1 Rust model should carry all seven fields even when most values are constant. That
avoids reshaping every recipe when Phases 2-4 arrive.

```rust
struct Recipe {
    name: &'static str,                 // stable, e.g. "core:wooden_pickaxe"
    station: Station,
    tier: Tier,
    pattern: Pattern,
    output: RecipeOutput,
    cost: CraftCost,
    unlock: Unlock,
    discipline: Discipline,
}
```

## 3. Patterns

### Shaped

The arrangement matters. The grid is trimmed to its occupied bounding box, then compared to the
recipe at any offset. A 3x3 grid can make a 2x2 recipe; a 2x2 grid cannot make a 3x3 recipe.

```text
Wooden pickaxe
P P P
  S
  S
```

### Tagged

The arrangement does not matter. The grid is reduced to a multiset of item tags and compared to
the recipe's requirements.

```text
Basic torch
1x stick + 1x fuel
```

Tags such as `fuel`, `wood`, `planks`, `stone`, and `metal` let a recipe accept alternatives. In
Phase 1 the tag table is small and built in; later it becomes authored content.

### Hybrid

A shaped layout whose slots accept tag-flexible ingredients. Deferred until Phase 2/4, but the
model should not assume every slot is a single item.

## 4. Disciplines, tiers, and stations

### Disciplines

| Discipline | Focus | Example stations |
|---|---|---|
| Construction | Blocks, building, decoration | Workbench, Stonecutter |
| Smithing | Metal tools, weapons, armor | Anvil, Forge |
| Woodworking | Wood gear, bows, furniture, boats | Workbench, Lathe |
| Tailoring | Cloth, leather, light armor, bags | Loom, Tannery |
| Alchemy | Potions, salves, poisons | Alchemy Bench |
| Cooking | Food, meals, preservation | Cooking Pot, Smoker |
| Enchanting | Enchanted gear, runes, wards | Enchanter, Runestone |
| Engineering | Mechanisms, traps, automation | Engineering Table, Fabricator |

Discipline XP is separate from character level. Phase 1 carries the discipline name as metadata
but does not level or gate on it.

### Tiers

T1-T5 mirror the material tiers. Higher station and discipline tiers unlock higher recipe tiers and
better quality ranges. Only T1 is used in Phase 1.

### Stations

The source lists 17 stations: Campfire, Workbench, Stonecutter, Furnace/Kiln, Anvil, Forge, Loom,
Tannery, Alchemy Bench, Cooking Pot, Smoker, Enchanter, Runestone, Engineering Table, Fabricator,
Altar, and Soul Forge. Stations have upgrade levels, portable versions, and physical structures.

Phase 1 implements only the **Workbench**. The inventory's 2x2 grid handles the smallest recipes.
A placed workbench opens the 3x3 screen. The furnace, campfire, anvil, and later stations are
Phase 2+.

## 5. Materials, tags, and quality

### Tags

Every material carries tags such as `wood`, `planks`, `fuel`, `stone`, `metal`, `cloth`, `hide`, or
`organic`. A recipe can require an item or a tag. The specific item used can later affect quality
and output stats.

Phase 1 tags:

| Tag | Items |
|---|---|
| `wood` | `core:wood` |
| `planks` | `core:planks` |
| `stone` | `core:stone` |
| `fuel` | `core:leaves`, `core:planks`, `core:wood` |

### Quality and rarity

The source's full item model adds rarity, sub-rarity, quality, and affixes. Quality is:

```text
clamp(1, 100,
    1d100
    + material_bonus     // 0..25 by material tier
    + station_bonus      // 0..20 by station tier
    + discipline_bonus   // 0..15 by discipline level
    + catalyst_bonus     // 0..20
)
```

Sub-rarity adds discipline and catalyst bonuses and maps to Ruined / Worn / Sound / Pristine /
Flawless. Crafted and looted items share the same roll system.

This cannot fit the current `ItemStack { item, count, durability }`. The recommended future model
is a side table:

```rust
struct ItemStack {
    item: ItemId,
    count: u16,
    instance: Option<ItemInstanceId>, // gear only
}

struct ItemInstance {
    durability: u16,
    quality: u8,
    rarity: Rarity,
    sub_rarity: SubRarity,
    affixes: Vec<AffixId>,
}
```

Stackable materials stay small. Gear gets an instance record. The player save gains a versioned
instance table. Phase 1 does **not** add quality, rarity, or affixes; it ships normal stacks and
keeps the save format unchanged.

## 6. Discovery and the recipe book

The source has five discovery sources:

| Source | Learns |
|---|---|
| Starting knowledge | Basic recipes: torch, workbench, wooden tools |
| Experimentation | Any matching recipe the player has not learned |
| Ruins and books | Cultural and rare recipes |
| Boss kills | Unique and mythic recipes |
| Reverse-engineering | A looted item's modifier as a craftable recipe |

The recipe book is personal and persistent and shows **known**, **glimpsed**, and **unknown**
recipes, with filters by discipline, tier, and station. It is a progression track, not just a
lookup list.

Phase 1 uses **known only**. Every built-in Phase 1 recipe is known at start. The recipe book can
search and shows craftable/missing states, but it does not persist discovery, glimpsed entries, or
unknown silhouettes. Those arrive with a save version that also stores unlocked recipe ids.

## 7. Crafting flow

1. The player places items in the 2x2 inventory grid or the 3x3 workbench grid.
2. The matcher trims the grid, compares shaped patterns (including optional mirroring), then
   compares tagged multisets.
3. The output slot previews the result.
4. Taking the output consumes the recipe's required count from each ingredient and produces the
   item.
5. Shift-click the output crafts as many as the grid and inventory allow.
6. Closing the screen returns the grid and cursor to the inventory. If the inventory is full, the
   screen stays open rather than deleting items.

For a tagged recipe with counts, the matcher uses the total item count across the grid. The
specific slot arrangement is irrelevant. For a shaped recipe, slot counts matter only as
"occupied" unless the ingredient declares a count.

## 8. Bulk, queues, and automation

The source's later phases add:

- batch crafting 1 / 10 / 100 / All,
- per-station queues with progress, pause, reorder, and offline continuation,
- portable stations with lower tier caps,
- Engineering automation (auto-crafter, sorter, smelter array, conveyors, logic gates),
- auto-crafting capped at T1 quality so manual crafting stays meaningful.

These require persistent station state, block entities, timed work on the fixed tick, and a queue
UI. They are **not** in Phase 1 or the current 0.0.10 scope.

## 9. UI adaptation

The source's station screen has a crafting grid, recipe details, ingredient list, quality and
sub-rarity bonuses, `Craft` / `Craft ×10`, and tabs for Recipes / Queue / Catalysts / Upgrade. The
recipe book is a separate screen with discipline tabs and known/glimpsed/unknown states.

Phase 1 uses the existing egui inventory/workbench screen:

- 2x2 or 3x3 grid plus output slot.
- A recipe list with search and craftable/missing state.
- Recipe details: output, ingredients, station, tier, discipline, and unlock source.
- `Craft` and `Craft ×10` actions.
- No queue, catalyst, upgrade, or quality tabs yet.

The recipe book can be a panel in the crafting screen for Phase 1. A separate `R` screen and the
full three-state book arrive with discovery.

## 10. Phase 1 for `0.0.10.3`

Phase 1 is the source's "Basic crafting" prototype, adapted to the current item/block set.

### Recipe model

- All seven grammar fields present.
- `Station::Inventory` and `Station::Workbench`.
- `Tier::T1`, `CraftCost::Instant`, `Unlock::Start`.
- Shaped and tagged patterns.
- Stable recipe names, duplicate detection, ingredient/output validation at load.
- Deterministic first-match order.

### Recipes

| Output | Pattern | Station |
|---|---|---|
| `core:planks` x4 | 1 `wood` tag | 2x2 |
| `core:stick` x4 | 2 planks vertical | 2x2 |
| `core:workbench` | 4 planks square | 2x2 |
| `core:torch` x4 | 1 stick + 1 `fuel` tag | 2x2 |
| `core:wooden_pickaxe` | 3 planks over 2 sticks | 3x3 |
| `core:wooden_axe` | 3 planks in an L + 2 sticks | 3x3 |
| `core:wooden_shovel` | 1 plank + 2 sticks | 3x3 |
| `core:wooden_sword` | 2 planks + 1 stick | 3x3 |
| `core:stone_pickaxe` | 3 stone over 2 sticks | 3x3 |
| `core:stone_axe` | 3 stone in an L + 2 sticks | 3x3 |
| `core:stone_shovel` | 1 stone + 2 sticks | 3x3 |
| `core:stone_sword` | 2 stone + 1 stick | 3x3 |

The workbench is the gate: the 2x2 surface makes planks, sticks, the workbench, and torches; the
3x3 surface makes tools.

### New blocks

| Block | Shape | Collision | Hardness | Tool | Notes |
|---|---|---|---:|---|---|
| `core:planks` | opaque cube | yes | 2.0 | axe | wood placeholder texture |
| `core:workbench` | opaque cube | yes | 2.5 | axe | right-click opens 3x3 |
| `core:torch` | cross-quad | no | 0.0 | hand | emits light, needs solid support |

`core:furnace` and the furnace UI are **Phase 2**, not Phase 1. The source's Phase 2 adds the
Campfire, Furnace, and Anvil; putting a nonfunctional furnace in Phase 1 would create a station
that promises smelting it cannot deliver.

### Salvage

The source's Phase 1 includes salvage. Phase 1 should include a small static salvage table:

- a tool returns one material and one stick,
- a workbench returns two planks,
- a torch returns one stick,
- a block returns its base material at a reduced rate.

Salvage is a station action with no reverse-engineering and no affix recovery. If it makes Phase 1
too large, it can become the first `0.0.10.3` follow-up without changing the recipe model.

### Not included

- quality, rarity, sub-rarity, affixes, catalysts, essence,
- discipline XP and station upgrades,
- experimentation, ruins, bosses, reverse-engineering,
- recipe-book glimpsed/unknown persistence,
- bulk crafting, queues, portable stations, automation,
- furnace and smelting,
- hybrid patterns,
- a separate `R` recipe-book screen.

## 11. Later phases and release mapping

The source's prototype order is:

| Phase | Content | Current mapping |
|---|---|---|
| 1 | Workbench, 12 recipes, shaped/tagged, known recipe book, salvage | `0.0.10.3` |
| 2 | Campfire, Furnace, Anvil; station tiers; material tags and quality inheritance | Post-0.0.19 crafting-depth release |
| 3 | Experimentation, ruins, reverse-engineering | Post-0.0.19 crafting-depth release |
| 4 | Eight disciplines with recipe pools and XP | Post-0.0.19 crafting-depth release |
| 5 | Batch crafting, queues, portable stations | Post-0.0.19 bulk/queue release |
| 6 | Engineering automation | Post-0.0.19 automation release |
| 7 | Resonance-locked recipes and endgame crafting | After Resonance exists |

The current `0.0.10`-`0.0.19` roadmap has no room for Phases 2-7. Committing to them requires one
of:

1. new `0.0.x` releases after `0.0.19` (for example `0.0.20` crafting depth, `0.0.21` bulk and
   queues, `0.0.22` automation and endgame),
2. a revised roadmap that inserts crafting depth and shifts later features, or
3. a smaller long-term crafting scope.

Phase 2 also needs the item-instance side table and a player-save version bump. That is the
gate before quality, rarity, affixes, and discipline-gated recipes.

## 12. Engine fit and tests

Crafting belongs in the headless gameplay layer:

| Path | Responsibility |
|---|---|
| `src/game/recipe.rs` | Grammar fields, patterns, tags, validation, matching, recipe book queries |
| `src/game/crafting.rs` | Grid, output preview, consumption, salvage, transactional direct craft |
| `src/game/item.rs` | Item ids, stack limits, future instance ids |
| `src/ui/crafting.rs` | Shared 2x2/3x3 grid, output slot, recipe list and details |
| `src/ui/inventory.rs` | Adds the 2x2 grid and recipe panel |
| `src/ui/workbench.rs` | 3x3 screen and workbench open/close |
| `crates/terrain-core/src/content/record.rs` | New block records for planks, workbench, torch |

Headless tests should cover:

- grammar validation, duplicate names, unknown ingredients/outputs, empty patterns,
- shaped matching at offsets, trimming, mirroring, and the 2x2/3x3 size gate,
- tagged multiset matching and alternatives,
- first-match determinism,
- output consumption, shift-craft, full-inventory rollback,
- closing with grid/cursor items,
- salvage returns,
- the Phase 1 recipe chain from wood to stone tools,
- new block ids and the shifted water id,
- player-save round-trip with the new item names.

## 13. Anti-patterns to keep out

The source explicitly rejects:

- too many stations,
- recipes nobody can find,
- no bulk crafting,
- crafting always beating loot or loot always beating crafting,
- fully deterministic or fully random crafting,
- useless low-tier recipes,
- no material-quality effect,
- infinite quality scaling,
- auto-crafting producing the best items,
- recipe bloat.

Phase 1 should stay small and legible. Twelve recipes, one station, and one tag table are enough
to prove the grammar.

## 14. Open decisions

- Item instances: defer quality/rarity until Phase 2, or add a side table now?
- Furnace: Phase 2 station, or a nonfunctional Phase 1 block?
- Salvage: include in `0.0.10.3` or make it the first follow-up?
- Recipe data: in-code validated tables or an authored data file?
- Recipe book: panel in the crafting screen or a separate `R` screen?
- Torch fuel: `stick + fuel` tag, or add coal/charcoal and a real fuel chain?
- Roadmap: new post-0.0.19 crafting releases, or a revised roadmap that inserts them?

See [the 0.0.10-0.0.19 plan](../dev/roadmap-0.0.10-0.0.19.md), [the v0.0.10 scope](../dev/v0.0.10-scope.md),
and [the UI reference](../dev/ui.md) for the current commitments.
