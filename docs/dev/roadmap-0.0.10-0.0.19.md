# Roadmap 0.0.10–0.0.19

**Status:** Planned. This page is the working breakdown; [the roadmap](../roadmap.md) is the
release-status source of truth.

This plan keeps releases on the `0.0.x` line. Each release is one shipped patch version and one
Git tag. The four-part identifiers (`0.0.10.1`, `0.0.10.2`, ...) are **internal planning
checkpoints**: they are reviewable slices inside a release, never Git tags and never published
versions.

The order deliberately differs from the original feature list:

- Terrain scale comes early because micro-voxels change storage, meshing, collision, streaming,
  content density, and memory. Building caves, ponds, creatures, heat, and tides before that
  decision would force all of them to be redone.
- The visual target is a vertical slice, not a separate "global illumination" release. The look in
  the reference is mostly dense props, water shading, shadows, fog/light shafts, palette, and
  post-processing. GI is an optional upgrade once the target slice is measurable.
- Ponds become the hero scene that proves the art pipeline. Fishing stays last because it needs
  water bodies, creatures, items, UI, and persistence.
- Heat/cold and tides share one environment-state milestone. They are coarse simulations over the
  same time/weather/water state and should not duplicate save, UI, or update code.

## Release ladder

| Release | Theme | Player-visible outcome | Depends on |
|---|---|---|---|
| 0.0.10 | Basic mechanics | A complete first playable loop: break, collect, craft, place, survive, save | 0.0.9 |
| 0.0.11 | Target slice and terrain scale | One reference-quality forest pond scene at the chosen voxel scale | 0.0.10 |
| 0.0.12 | Terrain and vegetation | Production terrain plus dense grass, reeds, flowers, lily pads, and trees | 0.0.11 |
| 0.0.13 | Water and ponds | Ponds and rivers with convincing water, aquatic flora, and water life | 0.0.12 |
| 0.0.14 | Lighting and atmosphere | Shadows, AO, fog, light shafts, post-processing, and quality tiers | 0.0.13 |
| 0.0.15 | Raytraced clouds | Volumetric clouds with weather, sun/moon lighting, and cloud shadows | 0.0.14 |
| 0.0.16 | Creatures | Ambient creatures with navigation, animation, ecology, and interaction | 0.0.15 |
| 0.0.17 | Caves | Explorable cave systems with entrances, underground content, and lighting | 0.0.16 |
| 0.0.18 | Heat/cold and tides | Environmental simulation that changes water, weather, players, and creatures | 0.0.17 |
| 0.0.19 | Fishing | Fishing as the capstone activity on ponds and oceans | 0.0.18 |

## Cross-cutting foundations

These are built early and extended by later releases. Each is a small checkpoint inside 0.0.10
unless a release says otherwise.

- **UI.** One immediate-mode or retained UI layer and one input-routing policy; inventory,
  crafting, HUD, menus, and debug screens use it.
- **Audio.** One audio backend and mixer with music, ambience, UI, and positional world buses.
- **Simulation.** A fixed-tick game state separate from the render frame; all gameplay systems
  update on the same clock.
- **Entities.** A small entity/component runtime with stable IDs, spatial lookup, and save/load.
- **Persistence.** One save-format version policy covering player state, inventory, entities,
  environment state, and world edits; every release with new state adds a migration test.
- **Assets.** One authored-asset pipeline for textures, materials, props, and audio; placeholders
  stay valid fallbacks.
- **Measurement.** A fixed hero-scene camera, golden screenshot, frame-time budget, memory budget,
  and headless simulation test for every release.
- **Documentation.** Each release updates the roadmap, changelog, scope page, controls, and
  configuration docs before it can ship.

## How a release ships

Every release uses the same gate:

- [ ] Every checkpoint is either complete or explicitly deferred in this page and the roadmap.
- [ ] The release has a `vX.Y.Z-scope.md` page expanded from the template.
- [ ] New behaviour has headless tests; new rendering has a golden capture.
- [ ] `cargo fmt --all -- --check`, clippy, tests, doc, `typos`, and `mdbook build` pass.
- [ ] Frame-time, memory, and GPU budgets are measured on the hero scene.
- [ ] Save migrations and corruption paths are tested.
- [ ] The changelog, roadmap, controls, and configuration docs are current.
- [ ] Manual acceptance uses the hero scene and one generated world.
- [ ] The release script creates the commit and annotated tag from `main`.

---

## 0.0.10 — Basic mechanics

**Goal:** Turn the terrain demo into a complete first playable loop. The player can break blocks
with the right tool, collect drops, manage an inventory, craft, place blocks, take damage, respawn,
and save/restore the run.

**Definition of Done:**

- A new player can spawn, gather wood/stone, craft a tool, place blocks, and survive a fall.
- Breaking respects block hardness and tool tier; placement rejects occupied or unsupported cells.
- Inventory, hotbar, crafting, health, stamina, and respawn work without the debug overlay.
- The run survives quit/reopen with inventory, player state, and edits intact.
- UI, audio, fixed-tick state, and the save-format bump are documented.

### 0.0.10.1 Interaction and the break/place loop

**Phase A — Target and break progress**

- [ ] Task: Replace instant break with hold-to-break.
  - [ ] Subtask: Add per-block hardness and tool-class tables.
  - [ ] Subtask: Add tool-tier multipliers and a minimum-tier rule.
  - [ ] Subtask: Add break progress, cancel-on-release, and crack/particle feedback.
  - [ ] Subtask: Cover negative coordinates, reach limits, and water/air targets.
- [ ] Task: Keep raycast and highlight stable at high frame rates.
  - [ ] Subtask: Reuse the DDA raycast and clamp to interaction distance.
  - [ ] Subtask: Test moving targets, camera-relative origins, and LOD sections.

**Phase B — Placement rules**

- [ ] Task: Validate placement against the player and world.
  - [ ] Subtask: Reject cells overlapping the player or a creature.
  - [ ] Subtask: Add support rules for torches, plants, and falling blocks.
  - [ ] Subtask: Add replaceable blocks such as tall grass and snow layers.

**Phase C — Drops and pickup**

- [ ] Task: Emit drops from a data-driven table.
  - [ ] Subtask: Add item entities with velocity, gravity, and collision.
  - [ ] Subtask: Add pickup radius, merging, and despawn timers.
  - [ ] Subtask: Persist dropped items across save/load.

### 0.0.10.2 Items, inventory, and hotbar

- [ ] Task: Define the item model.
  - [ ] Subtask: Stable item IDs, stack sizes, max durability, and tags.
  - [ ] Subtask: Block items, tool items, food items, and future placeholders.
- [ ] Task: Build inventory state and operations.
  - [ ] Subtask: Add/remove/split/merge/move stacks with overflow rules.
  - [ ] Subtask: Hotbar selection and offhand slot.
  - [ ] Subtask: Headless tests for every operation and full-inventory case.
- [ ] Task: Build the inventory and hotbar UI.
  - [ ] Subtask: Grid, drag/drop, shift-click, tooltips, and keyboard navigation.
  - [ ] Subtask: Gamepad-safe focus order and rebindable keys.
- [ ] Task: Persist inventory and hotbar state.

### 0.0.10.3 Crafting and tools

- [ ] Task: Add data-driven recipes.
  - [ ] Subtask: Shaped and shapeless recipes; tags and alternatives.
  - [ ] Subtask: Recipe validation and duplicate detection at load.
- [ ] Task: Add crafting surfaces.
  - [ ] Subtask: 2x2 inventory crafting and 3x3 workbench.
  - [ ] Subtask: Recipe book, search, and one-click craft.
- [ ] Task: Add tools and durability.
  - [ ] Subtask: Tool tiers, harvest levels, and durability loss.
  - [ ] Subtask: Repair and item damage display.
- [ ] Task: Add the first progression chain.
  - [ ] Subtask: Wood -> planks -> sticks -> wooden pickaxe.
  - [ ] Subtask: Stone -> stone tools -> furnace and torches.

### 0.0.10.4 Player state and survival

- [ ] Task: Add health and stamina.
  - [ ] Subtask: Damage sources, invulnerability window, and death/respawn.
  - [ ] Subtask: Sprint/jump stamina costs and regeneration.
- [ ] Task: Add environmental damage.
  - [ ] Subtask: Fall damage, drowning, and lava/fire hooks.
  - [ ] Subtask: Damage feedback, screen effects, and audio.
- [ ] Task: Add respawn and spawn safety.
  - [ ] Subtask: Bed/spawn-point item and respawn search.
  - [ ] Subtask: Never spawn inside a solid or flooded cell.

### 0.0.10.5 UI, audio, and settings

- [ ] Task: Choose and integrate the UI layer (ADR).
- [ ] Task: Choose and integrate the audio backend (ADR).
- [ ] Task: Build the first HUD.
  - [ ] Subtask: Crosshair, hotbar, health/stamina, selected item, interaction prompt.
  - [ ] Subtask: Loading, pause, death, and repair overlays.
- [ ] Task: Build the first settings screen.
  - [ ] Subtask: Render distance, FOV, sensitivity, volume, vsync, and keybinds.
- [ ] Task: Add the first ambience and interaction sounds.

### 0.0.10.6 Persistence and acceptance

- [ ] Task: Bump the world/player save format.
  - [ ] Subtask: Save inventory, hotbar, health, stamina, spawn point, and dropped items.
  - [ ] Subtask: Add migration and corruption tests.
- [ ] Task: Add the headless gameplay test.
  - [ ] Subtask: Scripted gather -> craft -> place -> damage -> respawn -> save/reopen.
- [ ] Task: Run the manual acceptance loop.
- [ ] Task: Update the changelog, controls, and configuration docs.

**Release gate:** the full loop works, saves, reloads, and is covered by headless tests.

---

## 0.0.11 — Target slice and terrain scale

**Goal:** Prove the visual target on one hero scene and choose the terrain scale before the rest of
the simulation is built. This is the look milestone, not the GI milestone.

**Definition of Done:**

- A fixed hero pond scene exists and is captured from a fixed camera.
- The target voxel scale is chosen by an ADR with measured memory, mesh, and frame costs.
- The scene has dense props, water, fog/light shafts, shadows, a consistent palette, and
  post-processing at the target budget.
- The same scene renders at the chosen scale in a prototype or production path.

### 0.0.11.1 Target experience brief

- [ ] Task: Write the visual target.
  - [ ] Subtask: Reference frames, palette, camera height, FOV, and composition.
  - [ ] Subtask: Target hardware, frame budget, memory budget, and resolution.
  - [ ] Subtask: "Looks right" rules and explicit non-goals.
- [ ] Task: Define the hero scene.
  - [ ] Subtask: Pond shape, shoreline, tree wall, foreground reeds, and lily pads.
  - [ ] Subtask: Time of day, weather, and camera path.
- [ ] Task: Add the golden capture harness.
  - [ ] Subtask: Fixed camera, deterministic seed, `ENGINE_CAPTURE`, and diff threshold.

### 0.0.11.2 Terrain scale ADR

- [ ] Task: Prototype the scale options.
  - [ ] Subtask: 1 m voxels + dense props.
  - [ ] Subtask: 0.5 m voxels.
  - [ ] Subtask: 0.25 m near-field voxels with 0.5/1 m LOD.
- [ ] Task: Measure each option.
  - [ ] Subtask: Memory per column, mesh counts, draw calls, collision cost, and save size.
  - [ ] Subtask: Generation, lighting, and streaming settle time.
- [ ] Task: Decide and record the ADR.
  - [ ] Subtask: State the supported scale, fallback scale, and migration policy.
  - [ ] Subtask: State what micro-voxel content is allowed (terrain only, props only, both).

### 0.0.11.3 Hero pond scene

- [ ] Task: Build a curated scene, not a random spawn.
  - [ ] Subtask: Hand-place the pond, shoreline, trees, reeds, and lily pads.
  - [ ] Subtask: Define the surrounding biome and background hills.
- [ ] Task: Add the scene to the demo.
  - [ ] Subtask: A `ENGINE_SCENE=pond` or equivalent switch.
  - [ ] Subtask: Deterministic rebuild from a seed or scene file.

### 0.0.11.4 Prop and instance renderer

- [ ] Task: Add a GPU-instanced prop path.
  - [ ] Subtask: Instance buffer, per-instance transform, color, wind phase, and LOD.
  - [ ] Subtask: Cross-quad/billboard support for grass and reeds.
  - [ ] Subtask: Frustum and distance culling; target 100k visible instances.
- [ ] Task: Add density-driven placement.
  - [ ] Subtask: Biome/height/slope masks.
  - [ ] Subtask: Poisson or jittered-grid placement with deterministic seeds.
  - [ ] Subtask: Water-edge and shade rules.
- [ ] Task: Add wind and interaction.
  - [ ] Subtask: Vertex wind and player push.
  - [ ] Subtask: LOD fade and impostors.

### 0.0.11.5 Materials and art pipeline

- [ ] Task: Replace placeholder tiles for the hero scene.
  - [ ] Subtask: Author grass, dirt, stone, bark, leaves, water-edge, and lily-pad materials.
  - [ ] Subtask: Define atlas/mipmap/sampling policy and update ADR-0015/0016 if needed.
- [ ] Task: Add palette and tint controls.
  - [ ] Subtask: Biome palette, season hooks, and per-instance tint.
- [ ] Task: Add the material validation tool.
  - [ ] Subtask: Missing layer, bad dimensions, and duplicate names fail CI.

### 0.0.11.6 Lighting, water, and post baseline

- [ ] Task: Add the cheap look first.
  - [ ] Subtask: Sun shadows, AO, height fog, light shafts, bloom, tonemapping, and grade.
  - [ ] Subtask: Depth-coloured water with animated normals and shoreline blend.
- [ ] Task: Establish the quality tiers.
  - [ ] Subtask: Low/medium/high/ultra and a headless "reference" preset.
- [ ] Task: Record the first visual gate.
  - [ ] Subtask: Screenshot diff, frame time, GPU time, and memory.

**Release gate:** the hero scene looks like the target at the chosen scale, and the scale decision
is recorded with measurements.

---

## 0.0.12 — Terrain and vegetation

**Goal:** Make the chosen terrain scale and dense vegetation production systems, not a prototype.
The hero scene becomes representative of generated worlds.

**Definition of Done:**

- Terrain generation, storage, meshing, collision, LOD, and saves work at the chosen scale.
- Grass, reeds, flowers, lily pads, and trees are placed by data-driven biome rules.
- A generated world can produce a scene close to the hero pond; the golden capture diff holds.
- Memory and frame budgets hold at the default render distance.

### 0.0.12.1 Terrain scale conversion

- [ ] Task: Convert storage and meshing.
  - [ ] Subtask: Section/chunk data at the new scale; deterministic hashes.
  - [ ] Subtask: Greedy meshing, AO, light, and skirts at the new scale.
  - [ ] Subtask: Raycast, collision, and placement snapping.
- [ ] Task: Convert LOD and streaming.
  - [ ] Subtask: LOD0/LOD1/LOD2 scale mapping and coarse water.
  - [ ] Subtask: Section origins, neighborhood, and edit invalidation.
- [ ] Task: Bump and migrate the save format.
  - [ ] Subtask: Old 1 m worlds are refused or migrated by explicit policy.
  - [ ] Subtask: Golden hashes and deterministic generation fixtures.

### 0.0.12.2 Terrain generation final pass

- [ ] Task: Improve macro terrain.
  - [ ] Subtask: Erosion, cliffs, beaches, riverbeds, and shore shaping.
  - [ ] Subtask: Biome blending and transition rules.
  - [ ] Subtask: Spawn selection and build-safe flats.
- [ ] Task: Improve micro detail.
  - [ ] Subtask: Surface roughness, overhangs, and small ledges.
  - [ ] Subtask: Cave entrances and pond basins reserved for later milestones.

### 0.0.12.3 Vegetation system

- [ ] Task: Data-driven vegetation rules.
  - [ ] Subtask: Biome, height, slope, light, water, and noise masks.
  - [ ] Subtask: Density, clustering, spacing, and deterministic seeds.
- [ ] Task: Content set.
  - [ ] Subtask: Grass clumps, reeds, flowers, ferns, lily pads, and fallen logs.
  - [ ] Subtask: Tree shapes, leaf clusters, and canopy gaps.
- [ ] Task: Rendering.
  - [ ] Subtask: GPU instancing, cross-quads, wind, shadows, and LOD.
  - [ ] Subtask: Distance culling and impostors; target 100k+ instances.
- [ ] Task: Interaction and persistence.
  - [ ] Subtask: Break/place rules, drops, trampling, and regrowth hooks.

### 0.0.12.4 Terrain and vegetation tooling

- [ ] Task: Add a scene/scatter preview tool.
- [ ] Task: Add deterministic generation tests.
- [ ] Task: Add memory and density budgets per biome.

**Release gate:** generated terrain consistently produces the target density and palette at the
chosen scale, and the hero-scene diff still passes.

---

## 0.0.13 — Water and ponds

**Goal:** Ponds and rivers look and behave like water, with aquatic plants and life. This is the
vertical-slice release that makes the reference scene feel alive.

**Definition of Done:**

- Ponds and rivers are placed by world generation and have stable water levels and shorelines.
- Water shows depth colour, normal animation, reflections, refraction, foam, and a soft shoreline.
- The player can swim, sink, and surface; water blocks and flow are deterministic.
- Lily pads, reeds, algae, and at least two water creatures exist and persist.

### 0.0.13.1 Pond and river generation

- [ ] Task: Classify water bodies.
  - [ ] Subtask: Pond, lake, river, ocean, and wetland tags.
  - [ ] Subtask: Watershed and spill-level calculation.
- [ ] Task: Place ponds and rivers.
  - [ ] Subtask: Basin carving, shoreline smoothing, and bed materials.
  - [ ] Subtask: Inflow/outflow and waterfall edges.
- [ ] Task: Validate generation.
  - [ ] Subtask: No floating water, no dry basins, and no overlapping bodies.
  - [ ] Subtask: Seed and order invariance.

### 0.0.13.2 Water rendering

- [ ] Task: Add depth-based water colour and absorption.
- [ ] Task: Add normal animation and foam.
  - [ ] Subtask: Two-layer scrolling normals and wind response.
  - [ ] Subtask: Shoreline foam and waterfall foam.
- [ ] Task: Add reflections and refraction.
  - [ ] Subtask: Planar or screen-space reflection baseline.
  - [ ] Subtask: Refraction with depth rejection and underwater fog.
- [ ] Task: Add underwater rendering.
  - [ ] Subtask: Underwater tint, caustics hook, and surface from below.

### 0.0.13.3 Water physics and gameplay

- [ ] Task: Add swimming, buoyancy, and drag.
- [ ] Task: Add flow and current.
- [ ] Task: Add water-sensitive block rules.
- [ ] Task: Add drowning and underwater interaction.

### 0.0.13.4 Aquatic flora and life

- [ ] Task: Place aquatic plants.
  - [ ] Subtask: Lily pads, reeds, cattails, algae, and submerged grass.
- [ ] Task: Add water creatures.
  - [ ] Subtask: Fish, insects, and amphibians with simple navigation.
  - [ ] Subtask: Spawn tables, population caps, and despawn.
- [ ] Task: Add ecology hooks.
  - [ ] Subtask: Food chain, water quality, and day/night behavior.

**Release gate:** the hero pond is fully interactive, the water pass has a golden capture, and
swimming plus water life are covered by headless tests.

---

## 0.0.14 — Lighting and atmosphere

**Goal:** Make the world read like the reference without committing to full GI. Shadows, AO, fog,
light shafts, and post-processing carry most of the look; GI remains an optional upgrade.

**Definition of Done:**

- Sun/moon shadows, contact shadows, AO, height fog, light shafts, bloom, tonemapping, and colour
  grading work on the hero scene.
- Indoor/cave and underwater lighting have explicit rules.
- Quality tiers meet frame and memory budgets.
- A GI ADR states whether and when probes, SDF, screen-space, or ray-traced GI will replace the
  approximation.

### 0.0.14.1 Lighting architecture

- [ ] Task: Write the lighting ADR.
  - [ ] Subtask: Compare baked AO, probes, SDF GI, screen-space GI, and hardware RT.
  - [ ] Subtask: Pick the shipped path and an upgrade path.
- [ ] Task: Define lighting units and exposure.
  - [ ] Subtask: Sun, sky, ambient, emissive, and fog units.
  - [ ] Subtask: Tonemapping and exposure policy.

### 0.0.14.2 Shadows

- [ ] Task: Add sun/moon shadow maps.
  - [ ] Subtask: Cascades, stable projections, and peter-panning controls.
  - [ ] Subtask: Alpha/cross-quad shadows for foliage.
- [ ] Task: Add contact shadows or SSAO for small details.
- [ ] Task: Integrate shadows with LOD and far sections.

### 0.0.14.3 Atmosphere

- [ ] Task: Add height fog and aerial perspective.
- [ ] Task: Add volumetric light shafts.
  - [ ] Subtask: Sun/moon shafts through canopy and caves.
- [ ] Task: Add sky gradient, sun disk, and stars.
- [ ] Task: Add post-processing.
  - [ ] Subtask: Bloom, tonemapping, colour grade, vignette, and TAA hooks.

### 0.0.14.4 Quality and fallbacks

- [ ] Task: Add low/medium/high/ultra paths.
- [ ] Task: Add the `ENGINE_LEGACY_RENDER`-style fallback for unsupported GPUs.
- [ ] Task: Measure and record the visual-perf ladder.

**Release gate:** the hero scene passes the visual diff at the default quality tier, and the
fallback path still renders a coherent world.

---

## 0.0.15 — Raytraced clouds

**Goal:** Add volumetric clouds with weather, sun/moon lighting, and cloud shadows on the world.

**Definition of Done:**

- A raymarched or froxel cloud layer covers the sky with believable coverage, density, and motion.
- Sun/moon lighting, silver linings, and cloud shadows respond to time and weather.
- Clouds composite correctly with fog, water, terrain, and the far LOD.
- Quality tiers keep the frame budget; clouds can be disabled without breaking the scene.

### 0.0.15.1 Cloud model and weather

- [ ] Task: Define cloud types and parameters.
  - [ ] Subtask: Coverage, density, height, wind, and type masks.
  - [ ] Subtask: Weather presets: clear, scattered, overcast, storm.
- [ ] Task: Add deterministic weather state.
  - [ ] Subtask: Time-of-day, wind, and front movement.
  - [ ] Subtask: Save/restore weather state.

### 0.0.15.2 Volumetric renderer

- [ ] Task: Implement the cloud raymarch.
  - [ ] Subtask: Froxel or half-resolution raymarch with temporal reprojection.
  - [ ] Subtask: Depth-aware upsample and compositing.
- [ ] Task: Implement lighting.
  - [ ] Subtask: Sun/moon multiple-scattering approximation.
  - [ ] Subtask: Ambient sky contribution and silver linings.
- [ ] Task: Implement cloud shadows.
  - [ ] Subtask: Projected shadow map or analytical coverage on terrain/water.

### 0.0.15.3 Weather and atmosphere integration

- [ ] Task: Tie clouds to fog, wind, and precipitation hooks.
- [ ] Task: Add cloud-driven lighting changes.
- [ ] Task: Add the cloud debug view and quality controls.

**Release gate:** clouds look correct from the hero camera and a flying camera, cloud shadows
move with weather, and the default tier holds the budget.

---

## 0.0.16 — Creatures

**Goal:** Add ambient creatures that navigate, animate, react to the player, and persist.

**Definition of Done:**

- At least three creature types with distinct behavior, navigation, animation, and spawn rules.
- Creatures path over terrain, avoid water/cliffs, and react to light, players, and damage.
- Population caps, despawn, and save/restore work without leaking entities.
- The entity runtime is shared with future NPCs and fishing.

### 0.0.16.1 Entity runtime

- [ ] Task: Add stable entity IDs, components, and a fixed-tick update.
- [ ] Task: Add a spatial index and activation tiers.
  - [ ] Subtask: Near simulation, mid simulation, far despawn.
- [ ] Task: Add entity save/load and migration hooks.

### 0.0.16.2 Navigation

- [ ] Task: Add voxel A* or flow-field navigation.
  - [ ] Subtask: Walk, jump, fall, swim, and door/gap rules.
  - [ ] Subtask: Path smoothing and stuck recovery.
- [ ] Task: Add local avoidance and crowd separation.
- [ ] Task: Add home range, wander, and flee/seek behaviors.

### 0.0.16.3 Animation and rendering

- [ ] Task: Add skeletal or procedural voxel animation.
- [ ] Task: Add instanced rendering, LOD, and culling.
- [ ] Task: Add shadows, water interaction, and footprint hooks.

### 0.0.16.4 Spawning and ecology

- [ ] Task: Add biome/time/light spawn tables.
- [ ] Task: Add population caps and despawn rules.
- [ ] Task: Add simple food-chain hooks for 0.0.18.

### 0.0.16.5 Interaction

- [ ] Task: Add health, damage, drops, and breeding/food hooks.
- [ ] Task: Add player avoidance, panic, and attack rules.
- [ ] Task: Add the creature debug overlay and spawn commands.

**Release gate:** creatures survive save/reload, stay within population and frame budgets, and
path through the hero scene without falling into ponds or through terrain.

---

## 0.0.17 — Caves

**Goal:** Make caves a real place to explore: entrances, underground biomes, hazards, lighting,
and vertical streaming.

**Definition of Done:**

- Generated worlds contain connected cave systems with visible entrances.
- Caves have distinct materials, ores, water, flora, and ambient audio.
- Cave lighting and fog work from both the surface and underground.
- The player can navigate, mine, and survive underground; saves and LOD cover vertical space.

### 0.0.17.1 Cave generation

- [ ] Task: Add 3D cave carving.
  - [ ] Subtask: Noise and worm caves, domes, tunnels, and shafts.
  - [ ] Subtask: Connectivity and entrance placement.
- [ ] Task: Add underground biomes.
  - [ ] Subtask: Depth bands, rock types, ores, and moisture.
- [ ] Task: Validate generation.
  - [ ] Subtask: No isolated spawn traps, no surface holes without entrances.
  - [ ] Subtask: Deterministic seeds and golden fixtures.

### 0.0.17.2 Cave content

- [ ] Task: Add cave blocks and deposits.
  - [ ] Subtask: Ores, crystals, mushrooms, moss, and water pools.
- [ ] Task: Add underground flora and props.
- [ ] Task: Add hazards.
  - [ ] Subtask: Falling, drowning, lava, and unstable ceilings hook.

### 0.0.17.3 Cave rendering and lighting

- [ ] Task: Add cave darkness and player light.
- [ ] Task: Integrate caves with shadows, fog, and optional GI.
- [ ] Task: Add cave ambience and audio zones.

### 0.0.17.4 Streaming and navigation

- [ ] Task: Add vertical streaming and cave LOD.
- [ ] Task: Extend collision and raycast for ceilings and overhangs.
- [ ] Task: Add cave pathfinding and spawn safety.

**Release gate:** a player can enter a cave, explore a connected section, mine an ore, and return
to the surface without a save/load or streaming failure.

---

## 0.0.18 — Heat/cold and tides

**Goal:** Add one shared environment simulation for temperature and tides. Both run on coarse
grids and drive visuals, player status, creatures, and water.

**Definition of Done:**

- Temperature varies by biome, altitude, time, weather, water, and heat sources.
- The player has body temperature, wetness, insulation, and hypothermia/heatstroke states.
- Tides raise and lower coastal water on a lunar cycle and update shorelines and currents.
- Creatures and plants react to temperature and tide; state survives save/reload.

### 0.0.18.1 Shared environment state

- [ ] Task: Add the environment clock and interpolation.
  - [ ] Subtask: Time of day, weather, season, moon phase, and tide phase.
  - [ ] Subtask: Fixed-tick update and render interpolation.
- [ ] Task: Add the environment save block and migration.

### 0.0.18.2 Temperature

- [ ] Task: Build the coarse temperature field.
  - [ ] Subtask: Biome base, altitude lapse, sun, shade, water, wind, and weather.
  - [ ] Subtask: Heat sources: lava, fire, torches, and geothermal.
- [ ] Task: Add player status.
  - [ ] Subtask: Body temperature, wetness, insulation, and acclimation.
  - [ ] Subtask: Hypothermia/heatstroke effects and UI.
- [ ] Task: Add world effects.
  - [ ] Subtask: Snow/ice, freezing/thawing water, and steam hooks.
- [ ] Task: Add creature/plant reactions.

### 0.0.18.3 Tides and currents

- [ ] Task: Add the tide model.
  - [ ] Subtask: Lunar cycle, amplitude, local modifiers, and storm surge.
- [ ] Task: Update water bodies.
  - [ ] Subtask: Raise/lower coastal water, flood/ebb shorelines, and tidal pools.
  - [ ] Subtask: Currents and flow directions near shore.
- [ ] Task: Update rendering and gameplay.
  - [ ] Subtask: Wet sand, foam lines, reflections, and underwater changes.
  - [ ] Subtask: Swimming, fishing, and creature behavior hooks.

### 0.0.18.4 Environment UI and tools

- [ ] Task: Add temperature/tide debug views and overlays.
- [ ] Task: Add tuning commands and golden environment states.
- [ ] Task: Add save/load and time-skip tests.

**Release gate:** a coastal hero scene shows visible tide change and temperature effects over a
full cycle, and both survive save/reload without blocking the main thread.

---

## 0.0.19 — Fishing

**Goal:** Make fishing the capstone activity: gear, casting, bobber physics, fish behavior, loot,
and progression on ponds and oceans.

**Definition of Done:**

- The player can craft a rod, cast into a valid water body, wait, hook, reel, and land fish.
- Bobber physics, line tension, fish stamina, and escape rules feel responsive.
- Fish species depend on water type, depth, time, weather, and temperature.
- Catches, records, and gear persist; fishing works in ponds, rivers, and oceans.

### 0.0.19.1 Gear and progression

- [ ] Task: Add rods, reels, lines, hooks, bait, and lures.
- [ ] Task: Add recipes and durability/repair.
- [ ] Task: Add fishing skill/records and unlock hooks.

### 0.0.19.2 Cast, bobber, and reel

- [ ] Task: Add cast charge and trajectory.
- [ ] Task: Add bobber physics, line rendering, and water detection.
- [ ] Task: Add bite windows, hook timing, tension, and reel controls.
- [ ] Task: Add failure states: snag, break, escape, and out-of-water.

### 0.0.19.3 Fish and water bodies

- [ ] Task: Add fish species and behavior.
  - [ ] Subtask: Habitat, depth, time, weather, and temperature preferences.
  - [ ] Subtask: Bite, fight, stamina, and escape curves.
- [ ] Task: Add water-body lookup and rarity tables.
- [ ] Task: Add loot: fish, junk, treasure, and records.

### 0.0.19.4 UI, animation, and audio

- [ ] Task: Add the fishing HUD and tension meter.
- [ ] Task: Add rod/bobber/fish animations.
- [ ] Task: Add cast, splash, reel, and catch audio.

### 0.0.19.5 Persistence and acceptance

- [ ] Task: Save catches, records, gear, and bait.
- [ ] Task: Add headless fishing tests and a deterministic catch table.
- [ ] Task: Add the hero-scene fishing acceptance run.

**Release gate:** fishing works end-to-end in a pond, a river, and the ocean, and catches survive
save/reload.

---

## Open decisions

These need an ADR or a short decision note before the owning release starts:

- **UI stack.** Immediate mode or retained; keyboard/gamepad/mouse routing.
- **Audio backend.** Mixer, positional audio, streaming music, and platform support.
- **Entity runtime.** Custom minimal ECS or a crate; activation tiers and save format.
- **Terrain scale.** 1 m, 0.5 m, or dual-scale micro-voxels; memory and content budget.
- **Lighting path.** Shadows + AO + fog baseline; when GI becomes worth its cost.
- **Cloud technique.** Raymarch vs froxel, temporal requirements, and quality tiers.
- **Water model.** Height-field water vs voxel water; flow and tide coupling.
- **Environment save block.** Versioning, migration, and authority for time/weather/tides.
- **Target hardware.** Minimum GPU, CPU, memory, and resolution for the hero scene.
- **Multiplayer.** Explicitly out of scope unless a later decision adds it.

## Relationship to 1.0

This plan reaches a complete single-player sandbox with the target look and the listed simulation
systems. It does **not** by itself declare 1.0. A separate decision is required after 0.0.19 to
decide whether the public API, save formats, performance, and content set are stable enough for
1.0.
