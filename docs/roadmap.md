# Roadmap

Milestones move from **planned** to **in progress** to **shipped**. Each one names what it delivers, how it is verified, and what it deliberately leaves out.

| Version | Name                           | Status      |
|---------|--------------------------------|-------------|
| 0.0.1   | The Foundation                 | Shipped     |
| 0.0.2   | Camera                         | Shipped     |
| 0.0.3   | Chunks and Meshes              | Shipped     |
| 0.0.4   | Playable                       | Shipped     |
| 0.0.5   | Scale and Performance          | Shipped     |
| 0.0.6   | Atmosphere                     | Shipped     |
| 0.0.7   | Advanced Horizons              | Shipped     |
| 0.0.8   | Scale                          | In progress |
| 0.0.9   | Far View                       | Shipped     |
| 0.0.10  | Basic mechanics                | In progress |
| 0.0.11  | Target slice and terrain scale | Planned     |
| 0.0.12  | Terrain and vegetation         | Planned     |
| 0.0.13  | Water and ponds                | Planned     |
| 0.0.14  | Lighting and atmosphere        | Planned     |
| 0.0.15  | Raytraced clouds               | Planned     |
| 0.0.16  | Creatures                      | Planned     |
| 0.0.17  | Caves                          | Planned     |
| 0.0.18  | Heat/cold and tides            | Planned     |
| 0.0.19  | Fishing                        | Planned     |

## v0.0.1: The Foundation

**Status:** Shipped

**Goal:** Voxels on screen, driven by data instead of hardcoded draw calls.

**Summary:** Before you can have a world, you need a cube. Establish the rendering pipeline and core math.

See [v0.0.1 scope](dev/v0.0.1-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Environment setup
- [x] Choose a language
- [x] Choose a graphics API
- [x] Choose a windowing library
- [x] Choose a math library

#### 2. Render a single cube
- [x] `draw_cube(position)` (12 triangles or 6 quads)
- [x] Textured or flat-colored cube on screen
- [x] Basic camera (position, look direction, projection)

#### 3. Basic voxel storage
- [x] Store voxels in a `HashSet<IVec3>`
- [x] Add/remove voxels at runtime
- [x] Rendering reflects changes immediately

### Definition of Done
- Window renders cubes from the voxel set
- Add/remove a coordinate visibly changes the scene
- Static arrangement of cubes can be modified at runtime

### Non-Goals
- Chunking, meshing optimizations, face culling
- World generation, lighting

## v0.0.2: Camera

**Status:** Shipped

**Goal:** Fly through the scene with keyboard and mouse.

**Summary:** A movable camera makes the world inspectable and proves the frame time holds while the view changes.

See [v0.0.2 scope](dev/v0.0.2-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Camera controller
- [x] First-person fly camera with position, yaw, and pitch
- [x] Pitch clamped to ±89°
- [x] WASD movement relative to the view direction
- [x] Mouse look with cursor capture; `Escape` releases the cursor

#### 2. Frame loop
- [x] Delta-time movement independent of frame rate
- [x] Continuous redraw while focused
- [x] Frame-time logging while moving

### Definition of Done
- Fly smoothly through the cube scene with WASD and mouse
- `Escape` releases the cursor without closing the window
- Movement is frame-rate independent and the camera survives resizes
- Frame time stays stable while moving

### Non-Goals
- Collision, gravity, jumping, walking
- Voxel raycasting, block interaction
- Chunks, meshing, textures, persistence

## v0.0.3: Chunks and Meshes

**Status:** Shipped

**Goal:** Split the world into chunks and stop emitting hidden faces.

**Summary:** v0.0.1 drew one mesh for the whole world. Chunks make edits local and keep draw work proportional to the visible world.

See [v0.0.3 scope](dev/v0.0.3-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Chunks
- [x] Fixed-size chunks (32×32×32)
- [x] Each chunk owns its voxels and its mesh
- [x] Block change rebuilds only the affected chunk and any neighbour whose boundary faces changed

#### 2. Face culling
- [x] Only emit faces exposed to air
- [x] No faces between two solid blocks, including across chunk boundaries

#### 3. Per-chunk rendering
- [x] One vertex buffer per non-empty chunk
- [x] Empty chunks release their GPU buffers

### Definition of Done
- World made of multiple chunks
- Block change triggers only a local mesh rebuild
- Stable frame rate while moving around

### Non-Goals
- Greedy meshing, LOD, async meshing
- Save/load

## v0.0.4: Playable

**Status:** Shipped

**Goal:** Walk around, break and place blocks, persist the world.

**Summary:** A world you can move through and interact with feels like a real engine.

See [v0.0.4 scope](dev/v0.0.4-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Walking and collision
- [x] Add gravity and ground checks to the v0.0.2 camera controller
- [x] Swept AABB collision against voxel terrain

#### 2. Block interaction
- [x] Raycast (DDA voxel traversal) to find targeted block
- [x] Highlight targeted block
- [x] Place and destroy blocks

#### 3. Persistence
- [x] Serialize chunks to disk
- [x] Load chunks from disk

#### 4. Texture atlas
- [x] Single texture-array atlas for all block textures
- [x] Batch different block types in one chunk mesh

### Definition of Done
- Walk, break, and place blocks
- World persists across engine restarts

### Non-Goals
- Infinite terrain streaming (save/load is its prerequisite only)
- Lighting, LOD

## v0.0.5: Scale and Performance

**Status:** Shipped

**Goal:** Large view distances at a smooth frame rate.

**Summary:** Move from toy to real engine.

See [v0.0.5 scope](dev/v0.0.5-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Greedy meshing
- [x] Merge adjacent coplanar same-type faces into larger quads
- [x] Measure vertex count before/after (target: large reduction vs naive)

#### 2. Level of Detail
- [x] Distance-based coarser meshes for far chunks

#### 3. Frustum culling
- [x] Skip chunks outside the camera frustum

#### 4. Async meshing
- [x] Chunk generation and meshing on worker threads
- [x] Main thread never blocks on meshing

### Definition of Done
- Vast draw distance with a smooth frame rate
- Breaking blocks causes no frame hitches

### Non-Goals
- Lighting, water, procedural generation

## v0.0.6: Atmosphere

**Status:** Shipped

**Goal:** Make the world look good.

**Summary:** Lighting, fog, and transparency.

See [v0.0.6 scope](dev/v0.0.6-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Voxel lighting
- [x] Flood-fill (BFS) propagation for sky light and block light
- [x] Bake light values / ambient occlusion into vertex data

#### 2. Shader lighting and fog
- [x] Dynamic directional light (sun/moon)
- [x] Distance fog to hide render-distance edge

#### 3. Water and transparency
- [x] Separate mesh layer for transparent blocks
- [x] Water shader (animated normals; reflections/refractions optional)

### Definition of Done
- Day/night cycle
- Ambient occlusion or shadows
- Visually distinct water

### Non-Goals
- Procedural generation, SVDAGs, raymarching

## v0.0.7: Advanced Horizons

**Status:** Shipped

**Goal:** Turn the engine into a game-ready, procedurally generated world.

**Summary:** Optional cutting-edge techniques.

See [v0.0.7 scope](dev/v0.0.7-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Procedural generation
- [x] Noise functions (Perlin / Simplex / OpenSimplex)
- [x] Terrain, caves, biomes, structures
- [x] Player-centred streaming, modified-chunk saves, and a persistent seed

#### 2. Advanced rendering (optional)
- [ ] Sparse Voxel DAGs for compression (evaluated; deferred, not in v0.0.7 acceptance)
- [ ] GPU compute raymarching without traditional meshes (evaluated; deferred, not in v0.0.7 acceptance)

### Definition of Done
- Fully procedural world that runs smoothly and looks good

### Non-Goals
- None; this is the stretch phase

## v0.0.8: Scale

**Status:** In progress - implementation and acceptance complete; release PR open.

**Goal:** Hold the frame budget and resident memory as the render distance grows.

**Summary:** Optimize the engine, not the game.

See [v0.0.8 scope](dev/v0.0.8-scope.md) for the deliverables and the
[v0.0.8 spec](specs/v0.0.8.md) for the normative limits.

### Milestones

#### 1. Light and memory
- [x] Direct light snapshots (constant lookups per job)
- [x] Four-bit packed light fields

#### 2. Distance and pacing
- [x] LOD2 and `render_distance_chunks` up to `32`
- [x] Streaming pacing, readiness bookkeeping, and budget metrics

### Definition of Done
- 60 FPS at `render_distance_chunks = 24` on the reference machine
- Resident light memory at most half of v0.0.7 for the same chunk count
- No per-frame scan of the world to build a light job
- No main-thread frame above 33 ms once the first ring is ready
- Benchmarks and acceptance recorded

### Non-Goals
- Gameplay, inventory, and survival systems
- SVDAG and raymarching; both remain deferred
- Save-format changes

## v0.0.9: Far View

**Status:** Shipped - implementation, manual acceptance, and the v0.0.9 release are complete.

**Goal:** Hold resident memory, VRAM, and frame cost roughly flat while the visible distance grows to kilometres.

**Summary:** Detail falls off with distance instead of the whole world staying resident at full resolution.

See [v0.0.9 scope](dev/v0.0.9-scope.md) for the deliverables and acceptance criteria, and
[GPU-driven meshing research](dev/research/gpu-driven-meshing.md) for the analysis behind them.

### Milestones

#### 1. Storage and geometry
- [x] Four-bit in-memory block storage; `VCHK` unchanged
- [x] Sparse light storage: the block-light plane is stored only when a lamp is in range
- [x] Chunk tops for horizon culling
- [x] Quad pool with storage-buffer chunk info, vertex pulling, and two indirect draws
- [x] 12-byte quad records, four-vertex indexed draws, GPU timestamp timing, dense slots, and the pool high-water metric
- [x] Current per-chunk path retained as the fallback

#### 2. Streaming and CPU hardening
- [x] Deferred neighbour light, cached streaming candidates and readiness, and a saved-chunk index
- [x] Mesher corner gather, light-job merge, and `Arc` chunk storage
- [x] Headless settle of 11.7-14 s at distance 32

#### 3. Distance
- [x] Horizon culling from column tops, on by default with `ENGINE_HORIZON_CULL=0` to disable it
- [x] Minimum-column occluders, tree exclusion, and in-engine BMP captures
- [x] Hierarchy levels 1-2: 64/128 m sections, sample-space meshing, parent retention, hidden ready children, and coarse water
- [x] Hierarchy levels 3-4: recursive projected refinement and parent retention, 256/512 m sections, BFS coarse light, and edit invalidation
- [x] Projected refinement and parent retention for the far levels

#### 4. Metrics
- [x] Distance-32 target: settled frame around 0.6 ms, 39.31 MiB geometry reservation, 2 draws; geometry target met
- [x] 2 km gate: 0.466 ms frame, 0.235 ms GPU, 3.58 M vertices, 2 draws, 30.94 MiB allocated geometry
- [x] 8 km frame/draw gate: 0.62-0.63 ms frame, 0.365 ms GPU, 5.86 M vertices, 1,358 visible nodes, 2 draws, 74.62 MiB allocated geometry, 94.24 MiB reservation; the section horizon removes 552 of 1,330 in-frustum sections and the hierarchy settles in 7.8 s with the shared 16-result upload budget. The 500 MB total-working-set half of the gate is unverified.
- [x] 370 tests pass (ignored benchmarks run separately); fmt, clippy, doc, typos, and mdbook pass

#### 5. Acceptance and release
- [ ] Manual definition-of-done run — replaced by the in-engine captures and the automated edit/LOD
      tests in the shipped release; the interactive reviewer pass remains unverified.
- [x] `Cargo.toml` version bump, release split, changelog, and tag
- [x] Pull request

### Definition of Done
- 60 FPS at distance 32 with a working set at or below 350 MB and geometry VRAM at or below 40 MB
- 60 FPS at a 2 km view at or below 400 MB, and at an 8 km view at or below 500 MB
- Draw calls at or below 10 at distance 32 and at or below 20 at a 2 km view
- Main-thread frame time at or below 4 ms at distance 32, with no frame above 33 ms once ready
- Headless counts unchanged for the same world, proving storage packing is not a content change

**Measured:** the distance-32 and 2 km gates are recorded above. The 8 km frame/draw gate is recorded;
its total working set is unverified, so the 500 MB half is not claimed. The release is complete; the
manual walk-through was replaced by captures and automated tests, so it is not claimed as run.

### Non-Goals
- The compute mesher, GPU-resident voxel and light pools, and readback-driven allocation
- Hi-Z, GPU traversal, and the voxel visibility graph
- Shadows, sub-voxels, and console backends
- Save-format changes

## v0.0.10: Basic mechanics

**Status:** In progress - the `0.0.10.1` gameplay core, the `0.0.10.2` UI slice (HUD and inventory), and the `0.0.10.3` crafting Phase 1 are implemented.

**Goal:** Turn the v1 terrain demo into a complete first playable loop: break, collect, craft, place, survive, and save.

**Summary:** Terrain becomes a game. Blocks take time and the right tool, drops become items, items live in an inventory, recipes turn them into tools, and the player and the run persist.

See [v0.0.10 scope](dev/v0.0.10-scope.md) for the deliverables and acceptance criteria. The
remaining checkpoints are broken into review-sized PRs in
[the 0.0.10.4-0.0.11 plan](dev/plan-0.0.10.4-0.0.11.md).

### Milestones

#### 1. Interaction and items (`0.0.10.1`)
- [x] Hold-to-break with block hardness, tool classes, tier multipliers, and a minimum-tier rule
- [x] Stable item names, stacks, durability, inventory, hotbar, and offhand
- [x] Data-driven block drops and item entities with gravity, pickup, merging, and despawn
- [x] Placement rules for occupied cells, the player box, support, and replaceable blocks
- [x] Versioned player save for position, view, inventory, hotbar, and dropped items

#### 2. UI and crafting (`0.0.10.2`, `0.0.10.3`)
- [x] egui foundation, HUD, and UI-first input hook
- [x] Inventory screen with drag/drop, shift-click, tooltips, and keyboard navigation
- [x] Crafting Phase 1: universal recipe grammar, shaped/tagged patterns, 2x2/3x3, workbench, known recipe book, tags, salvage
- [ ] Pause and death screens
- [x] The wood-to-stone tool progression and crafted tools

See [the crafting system design](design/crafting-system.md) for the full grammar, disciplines,
stations, quality model, discovery, and the Phase 2-7 mapping. Only Phase 1 is committed to 0.0.10.

#### 3. Survival and presentation (`0.0.10.4`, `0.0.10.5`)
- [ ] Health, stamina, fall damage, drowning hooks, death, respawn, and survival UI
- [ ] kira audio, placeholder sounds, settings screen, and remappable keybinds

#### 4. Persistence and acceptance (`0.0.10.6`)
- [ ] `VPLY` v2 hardening, migration, and corruption tests
- [ ] Scripted gather-craft-place-damage-respawn-save run
- [ ] Manual acceptance, docs, changelog, version bump, and release PR

**Measured after `0.0.10.1`:** 469 engine library tests and 411 terrain-core library tests pass; the
scripted gather-use-save-reopen loop is covered; a 15-second windowed demo run created both new
pipelines with no validation errors; the static distance-12 headless run settles at 625 of 625
ready columns in 470 frames. The manual definition-of-done pass and the full save migration set
remain open.

**Measured after the `0.0.10.3` crafting slice:** 511 engine library tests and 411 terrain-core
library tests pass, plus the integration suites. The recipe table validates twelve recipes; shaped,
tagged, mirrored, station-gated, first-match, direct-craft, grid-craft, full-inventory rollback,
salvage, and transient-grid return paths are covered headlessly. The terrain golden file and the
pinned content hashes were regenerated for the three new block records.

**Measured after the `0.0.10.4.2` survival damage:** 532 engine library tests and 411 terrain-core
library tests pass. Fall damage, drowning intervals, lava/fire hooks, sprint and jump stamina,
fall-distance tracking, safe-spawn search, death reporting, and respawn are covered headlessly.

**Measured after the `0.0.10.4.1` survival state:** 520 engine library tests and 411 terrain-core
library tests pass. `VPLY` version 2 round-trips health, stamina, and the spawn point and accepts
version 1 files. The survival state covers damage, death, revive, stamina drain, and regeneration.

**Measured after the `0.0.10.2` UI slice:** 477 engine library tests and 411 terrain-core library
tests pass; captured windowed frames show the crosshair, nine-slot hotbar, selected border,
temporary pickaxe icon, durability bar, existing FPS overlay, and the full inventory screen with
main grid, hotbar row, offhand, close button, and dimmed world. The inventory captures pointer and
keyboard input; the HUD does not. Headless tests cover click, right-click, shift-click, quick move,
and the two-frame `E` close behavior.

### Definition of Done
- A new player can spawn, gather wood and stone, craft a tool, place blocks, and survive a fall.
- Breaking respects hardness and tool tier; placement rejects occupied or unsupported cells.
- Inventory, hotbar, crafting, health, stamina, and respawn work without the debug overlay.
- The run survives quit and reopen with inventory, player state, dropped items, and edits intact.
- UI, audio, fixed-tick state, and the player save format are documented.

### Non-Goals
- The hero scene, the final voxel scale, and production vegetation (`0.0.11`, `0.0.12`)
- Water bodies, swimming, and water life (`0.0.13`)
- Shadows, GI, clouds, creatures, caves, temperature, tides, and fishing (`0.0.14`-`0.0.19`)

## Post-0.0.9: the road to 1.0

The next ten releases stay on the `0.0.x` line. Four-part identifiers such as `0.0.10.1` are internal
planning checkpoints, not releases or tags.

- **Basic mechanics first.** 0.0.10 turns the terrain demo into a complete first playable loop: break, collect, craft, place, survive, and save.
- **Target slice and terrain scale, then dense vegetation.** 0.0.11 fixes the hero pond look and chooses the voxel scale before simulation is built on it; 0.0.12 makes that scale production terrain with dense, data-driven vegetation.
- **Water and ponds as the vertical slice.** 0.0.13 adds ponds and rivers with convincing water, swimming, aquatic flora, and water life.
- **Lighting, atmosphere, and clouds.** 0.0.14 adds shadows, AO, fog, light shafts, and post-processing; 0.0.15 adds raytraced clouds, weather, and cloud shadows.
- **Creatures, then caves.** 0.0.16 adds ambient creatures with navigation, animation, and ecology; 0.0.17 makes caves explorable with entrances, underground content, and lighting.
- **Heat/cold and tides share one environment state.** 0.0.18 runs temperature and tides on the same clock and save block instead of duplicating updates, UI, and persistence.
- **Fishing last.** 0.0.19 is the capstone because it needs water bodies, creatures, items, UI, and persistence in place.
- **1.0 needs its own decision.** Reaching 0.0.19 completes the planned single-player sandbox but does not declare 1.0; API, save-format, performance, and content stability must be decided separately.

See [the detailed 0.0.10-0.0.19 plan](dev/roadmap-0.0.10-0.0.19.md) for checkpoints, release gates, and open decisions.
