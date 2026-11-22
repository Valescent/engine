# Glossary

Preferred terms, short definitions, and links to the page that owns each topic. **Status** is
current for implemented behaviour, planned with a milestone for scoped work, optional for stretch
work, or deferred when no decision exists. **Avoid** lists terms that are ambiguous or mean
something else; it does not ban the word everywhere.

## World and coordinates

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [voxel](coordinates.md#voxel-coordinates) | Smallest addressable world cell, identified by an integer `IVec3` and occupying a unit cube. | Current | block, when you mean the cell |
| [block](v0.0.4-scope.md) | The type or contents stored in a voxel, such as Air, Stone, Dirt, Grass, Wood, or Leaves. | Current (v0.0.4) | voxel, when you mean the type |
| [world](architecture.md#coordinates-and-data) | All stored voxels and their state; chunks are the storage unit since v0.0.3. | Current | map, dimension |
| [chunk](v0.0.3-scope.md) | Fixed 32×32×32 region of voxels; the level-0 section. | Current (v0.0.3) | section, when you mean a coarse level |
| [section](../design/lod-hierarchy.md#2-level-model) | Derived `32³` sample grid at a hierarchy level's spacing. Level 0 is a chunk; levels 1-4 are coarse rendering data. | Current (v0.0.9) | chunk, when you mean a coarse level |
| [level](../design/lod-hierarchy.md#2-level-model) | Hierarchy depth from 0 to 4; a section at level `n` covers `32 × 2^n` blocks, so level 1 spans 64 m and level 4 spans 512 m. | Current (v0.0.9) | LOD, when you mean the section level |
| [ChunkCoord](../specs/v0.0.3.md) | Newtype around `IVec3` used as an ordered chunk key; `IVec3` itself is not `Ord`. | Current (v0.0.3) | — |
| [world coordinate](coordinates.md#voxel-coordinates) | Integer `IVec3` address of a voxel in the world. | Current | global coordinate |
| [chunk coordinate](coordinates.md#chunk-coordinates-v003) | Integer `IVec3` index of a chunk. Chunk `c` covers world coordinates `c * 32 ..= c * 32 + 31`. | Current (v0.0.3) | chunk position |
| [local coordinate](coordinates.md#chunk-coordinates-v003) | Integer coordinate from `0` to `31` on each axis, relative to one chunk. | Current (v0.0.3) | relative coordinate |
| [world space](coordinates.md#world-space) | Floating-point `Vec3` position in metres. | Current | world coordinates, when you mean floats |
| [position](coordinates.md#world-space) | Floating-point location in world space; not an integer voxel address. | Current | coordinate, when you mean floats |
| [chunk origin](coordinates.md#chunk-coordinates-v003) | World coordinate `c * 32` at a chunk's minimum corner. | Current (v0.0.3) | chunk center |
| [chunk-local space](coordinates.md#mesh-space-v003) | Floating-point space inside one chunk, from `0` to `32` metres. | Current (v0.0.3) | local space, when it is ambiguous |
| [chunk border](v0.0.3-scope.md) | Local coordinate `0` or `31` on an axis; edits there can affect a neighbour chunk. | Current (v0.0.3) | — |
| [precision budget](coordinates.md#precision-and-limits) | Accepted `f32` error or playable radius before camera-relative rendering is needed. | Deferred | chunk size |

## Rendering and performance

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [mesh](architecture.md#modules) | Vertex data for voxel surfaces. | Current | model |
| [meshing](v0.0.3-scope.md) | Generating a mesh from voxel data. | Current | rendering |
| [shader](shaders.md) | WGSL program that runs on the GPU; terrain uses `src/shader.wgsl`, and pooled geometry uses `src/quad.wgsl`. | Current | material, when you mean the program |
| [WGSL](shaders.md) | WebGPU Shading Language, the shader language used by wgpu. | Current | GLSL or HLSL, when you mean WGSL |
| [chunk mesh](v0.0.3-scope.md) | Mesh for one chunk, built in chunk-local space. | Current (v0.0.3) | world mesh |
| [face culling](v0.0.3-scope.md) | Omitting a voxel face when the neighbouring voxel is solid. | Current (v0.0.3) | backface culling |
| [backface culling](architecture.md#rendering-resources) | GPU test that discards triangles facing away from the camera; enabled for terrain and water. | Current (v0.0.8) | face culling |
| [dirty chunk](v0.0.3-scope.md) | Chunk whose mesh must be rebuilt after an edit. | Current (v0.0.3) | stale chunk |
| [rebuild](v0.0.3-scope.md) | Regenerate and upload a dirty chunk's mesh. | Current (v0.0.3) | remesh, when you mean the whole edit path |
| [greedy meshing](../specs/v0.0.5.md) | Merging adjacent coplanar same-type faces into larger quads. | Current (v0.0.5) | merging, when it is ambiguous |
| [LOD](../specs/v0.0.8.md) | Level of detail; the per-chunk fallback has LOD0 full resolution, LOD1 conservative 2×2×2, and LOD2 4×4×4 for distant chunks. The hierarchy replaces distant chunks with coarse sections. | Current (v0.0.8) | mipmap |
| [LOD section](../decisions/0014-seed-native-lod-hierarchy.md) | Derived `32³` sample section at a spacing of `2^level`; level 1 covers `64³` blocks and level 4 covers `512³`. | Current (v0.0.9) | chunk, when you mean a coarse section |
| [seed-native LOD](../decisions/0014-seed-native-lod-hierarchy.md) | Generating a coarse section directly from the seed at its own spacing, without full-resolution children. | Current (v0.0.9) | mipmap |
| [node hierarchy](../decisions/0014-seed-native-lod-hierarchy.md) | Parent-first LOD tree. A parent stays drawn until all in-world children are ready, then swaps atomically. | Current (v0.0.9) | quadtree |
| [detail radius](../design/lod-hierarchy.md#43-detail-policy) | L0 streaming radius in chunks when the hierarchy is on; `ENGINE_DETAIL_CHUNKS`, default `16` (512 m). | Current (v0.0.9) | render distance, when you mean the L0 ring |
| [hierarchy target](../design/lod-hierarchy.md#62-node-selection) | Exclusive section draw list: no parent and descendant are selected together; retained fallbacks cover nodes that are still building. | Current (v0.0.9) | selection, when it is ambiguous |
| [hidden child](architecture.md#rendering-resources) | Ready coarse child kept uploaded but not drawn while its retained parent is drawn. | Current (v0.0.9) | culled child |
| [frustum culling](../specs/v0.0.5.md) | Skipping chunks and sections outside the camera's view frustum. | Current (v0.0.5) | occlusion culling |
| [occlusion culling](../specs/v0.0.5.md) | Skipping geometry hidden behind other geometry. The conservative horizon pass covers column-top occlusion for L0 chunks and uploaded hierarchy sections. | Deferred beyond horizon (v0.0.9) | frustum culling |
| [horizon culling](architecture.md#rendering-resources) | Conservative occlusion test using minimum column tops. On by default; `ENGINE_HORIZON_CULL=0` disables it. | Current (v0.0.9) | occlusion culling, when you mean the general case |
| [async meshing](../specs/v0.0.5.md) | Building meshes on worker threads instead of the main thread. | Current (v0.0.5) | parallel rendering |
| [worker pool](../decisions/0009-micropool-for-chunk-workers.md) | Threads that run meshing and generation jobs; micropool is selected. | Current (v0.0.5) | thread pool, when you mean this engine component |
| [revision](../specs/v0.0.5.md) | Version number for a chunk mesh or job; results from older revisions are discarded. | Current (v0.0.5) | version, when you mean the package version |
| [stress scene](../specs/v0.0.5.md) | Exact 64-chunk in-memory fixture selected by `ENGINE_STRESS_SCENE=1`. | Current (v0.0.5) | benchmark, when you mean the fixture |
| [upload budget](../specs/v0.0.5.md) | Maximum completed chunk meshes uploaded per frame; fixed at two. | Current (v0.0.5) | frame budget |
| [draw call](architecture.md#rendering-resources) | One draw submission to the GPU. The quad path normally issues two indirect draws (opaque and water), with additional non-indexed draws for meshes above 16,383 quads; the per-chunk fallback issues one per visible mesh. | Current (v0.0.9) | pass |
| [frame time](debugging.md#logging) | Time to produce one frame, logged as `frame_ms`. | Current | FPS |
| [FPS](debugging.md#logging) | Frames per second; the reciprocal of frame time. | Current | frame time |
| [debug overlay](debugging.md#profiling) | On-screen FPS and frame time, toggled with `F3` and refreshed once per second. | Current | HUD, when you mean the debug text |

## Gameplay and interaction

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [player](v0.0.4-scope.md) | Walking controller with an AABB, gravity, jumping, and an eye position. | Current (v0.0.4) | camera, when you mean the player |
| [AABB](v0.0.4-scope.md) | Axis-aligned bounding box. The player is `0.6 m` wide and `1.8 m` tall. | Current (v0.0.4) | hitbox |
| [swept AABB](v0.0.4-scope.md) | Collision test that moves an AABB along one axis and stops at the first solid block. | Current (v0.0.4) | raycast |
| [collision](v0.0.4-scope.md) | Resolving player movement against solid voxel AABBs. | Current (v0.0.4) | physics |
| [raycast](v0.0.4-scope.md) | Casting a ray from the eye to find the first solid voxel and entered face. | Current (v0.0.4) | raymarch |
| [DDA](v0.0.4-scope.md) | Digital differential analyzer; grid traversal used for voxel raycasts. | Current (v0.0.4) | raymarching |
| [targeted block](v0.0.4-scope.md) | Solid voxel hit by the view ray and outlined before interaction. | Current (v0.0.4) | selected block, when you mean the hotbar |
| [highlight](v0.0.4-scope.md) | Wireframe cube drawn around the targeted block. | Current (v0.0.4) | selection box |
| [hotbar](gameplay.md#inventory) | Selected item slot. The demo has nine slots selected with `1`-`9`; the legacy path has five hard-coded block slots selected with `1`-`5`. The demo inventory screen shows all 37 slots. | Current (0.0.10.2) | inventory, when you mean the selected row |
| [inventory screen](ui.md#inventory) | The `E` screen with 27 main slots, 9 hotbar slots, and the offhand; supports drag/drop, shift-click, and keyboard navigation. | Current (0.0.10.2) | backpack |
| [HUD](ui.md) | The egui crosshair, nine-slot hotbar, item/durability display, and interaction prompt. Non-interactive in `0.0.10.2`. | Current (0.0.10.2) | overlay, when you mean the debug text |
| [item](gameplay.md#items) | A stable `namespace:name` identity with a kind, stack limit, and optional durability. | Current (0.0.10.1) | block, when you mean an inventory item |
| [item id](gameplay.md#identity) | In-memory `u16` index derived by sorting item names at load; never saved. | Current (0.0.10.1) | UUID |
| [stack](gameplay.md#stacks-and-durability) | An item id, count, and durability in one inventory slot or dropped entity. | Current (0.0.10.1) | item, when you mean a count |
| [inventory](gameplay.md#inventory) | Nine hotbar slots, twenty-seven main slots, and one offhand slot with stack operations. | Current (0.0.10.1) | hotbar, when you mean all slots |
| [tool class](gameplay.md#break-rules) | The block family a tool matches: hand, pickaxe, axe, shovel, or sword. | Current (0.0.10.1) | tool type |
| [tool tier](gameplay.md#break-rules) | The harvest level of a tool (wood 1, stone 2, iron 3); also gates drops. | Current (0.0.10.1) | level |
| [hardness](gameplay.md#hardness-and-time) | Content-pack resistance in seconds before the tool speed multiplier. | Current (0.0.10.1) | durability |
| [break progress](gameplay.md#breaking) | Accumulated fixed-tick time toward a block's break duration, shown by the outline colour. | Current (0.0.10.1) | damage |
| [drop](gameplay.md#drops) | The item stack a broken block yields; empty when the tool tier or class fails. | Current (0.0.10.1) | loot |
| [item entity](gameplay.md#item-entities) | A dropped item with a stable id, position, velocity, and age, simulated on the fixed tick. | Current (0.0.10.1) | drop, when you mean the entity |
| [pickup](gameplay.md#pickup-and-despawn) | Moving item entities within `1.5 m` into the inventory, leaving overflow on the ground. | Current (0.0.10.1) | collect |
| [player save](save-format.md#player-save-vply) | The `VPLY` sidecar for position, view, inventory, hotbar, and dropped items. | Current (0.0.10.1) | world save |
| [fixed tick](gameplay.md#frame-and-tick) | The 60 Hz gameplay step for break progress, entity physics, pickup, and despawn. Player movement is still frame-driven. | Current (0.0.10.1) | frame |
| [starting kit](gameplay.md#starting-kit) | The single wooden pickaxe granted on a new demo run until crafting lands. | Current (0.0.10.1) | loadout |
| [recipe grammar](../design/crafting-system.md#2-universal-recipe-grammar) | The shared `station · tier · pattern · output · cost · unlock · discipline` recipe shape. | Proposed; Phase 1 scoped | recipe format |
| [shaped recipe](../design/crafting-system.md#3-patterns) | A recipe whose ingredient arrangement matters; the grid is trimmed and matched at any offset. | Proposed; Phase 1 scoped | pattern |
| [tagged recipe](../design/crafting-system.md#3-patterns) | A recipe matched by an ingredient multiset and tags rather than arrangement. | Proposed; Phase 1 scoped | shapeless, when you mean the source term |
| [item tag](../design/crafting-system.md#5-materials-tags-and-quality) | A label such as `wood`, `planks`, `stone`, or `fuel` that a recipe can require instead of one item. | Proposed; Phase 1 scoped | item group |
| [station](../design/crafting-system.md#4-disciplines-tiers-and-stations) | A physical crafting surface; Phase 1 uses the inventory 2x2 grid and the placed workbench. | Proposed; Phase 1 scoped | bench |
| [discipline](../design/crafting-system.md#4-disciplines-tiers-and-stations) | A crafting domain such as Construction, Smithing, or Woodworking. Phase 1 stores it as metadata only. | Proposed; Phase 4 | profession |
| [crafting tier](../design/crafting-system.md#4-disciplines-tiers-and-stations) | The T1-T5 station/material level; Phase 1 uses T1 only. | Proposed; Phase 1 scoped | level |
| [quality](../design/crafting-system.md#5-materials-tags-and-quality) | A 1-100 per-item-instance roll influenced by materials, station, discipline, and catalysts. | Proposed; Phase 2 | durability |
| [recipe book](../design/crafting-system.md#6-discovery-and-the-recipe-book) | The known/glimpsed/unknown recipe view. Phase 1 is known-only. | Proposed; Phase 1 scoped | wiki |
| [salvage](../design/crafting-system.md#10-phase-1-for-00103) | Breaking a crafted item into a reduced set of base materials. | Proposed; Phase 1 scoped | recycle |
| [item instance](../design/crafting-system.md#5-materials-tags-and-quality) | The future side-table record for durability, quality, rarity, and affixes on gear. | Proposed; Phase 2 | item, when you mean an instance |
| [texture array](../decisions/0007-texture-array-for-block-textures.md) | GPU `texture_2d_array` with one layer per block tile, used instead of a flat atlas. | Current (v0.0.4) | atlas |
| [tile](textures.md) | One 16x16 RGBA8 block texture stored in a texture-array layer. | Current (v0.0.4) | atlas tile |
| [texture pack](textures.md) | External set of block tiles; deferred, and must preserve layer indices when introduced. | Deferred | asset pack |
| [Sand](../specs/v0.0.7.md) | Solid desert surface block with ID 8 and texture layer 10. | Current (v0.0.7) | dirt, when you mean sand |

## Persistence

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [persistence](v0.0.4-scope.md) | Saving and loading chunks so edits survive a restart. | Current (v0.0.4) | storage, when it is ambiguous |
| [modified chunk](../specs/v0.0.7.md) | Chunk changed by the player and therefore saved; unmodified chunks regenerate from the seed. | Current (v0.0.7) | dirty chunk, when you mean light/mesh dirty |
| [render distance](../decisions/0013-player-centred-chunk-streaming.md) | Horizontal streaming radius in chunks; configured as `world.render_distance_chunks`. | Current (v0.0.8) | view distance, when you mean fog |
| [`VCHK`](../decisions/0008-chunk-save-format.md) | Chunk save-file magic; v1 ships in v0.0.4, and v0.0.7 adds v3 and loads v1-v3. | Current (v0.0.7) | save file, when you mean the format |
| [format version](versioning.md#out-of-scope) | Version field inside a save format such as `VCHK`, separate from the package version. | Current (v0.0.4) | package version |

## Lighting and generation

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [seed](../specs/v0.0.7.md) | Deterministic input to world generation. | Current (v0.0.7) | random, when you mean reproducible |
| [noise](../decisions/0012-fastnoise-lite-and-opensimplex2.md) | Deterministic terrain fields; fastnoise-lite 1.1.1 with OpenSimplex2. | Current (v0.0.7) | random |
| [biome](../specs/v0.0.7.md) | Terrain and climate region; plains, hills, or desert. | Current (v0.0.7) | zone |
| [streaming](../decisions/0013-player-centred-chunk-streaming.md) | Loading and unloading chunks around the player as they move. | Current (v0.0.7) | generation |
| [hysteresis](../decisions/0013-player-centred-chunk-streaming.md) | Extra margin before unloading a chunk, to avoid load/unload thrashing. | Current (v0.0.7) | delay |
| [SVDAG](../specs/v0.0.7.md) | Sparse voxel directed acyclic graph; optional compression for far or static chunks. | Optional (v0.0.7) | — |
| [raymarching](../specs/v0.0.7.md) | Rendering by marching rays per pixel instead of drawing meshes. | Optional (v0.0.7) | raycasting |
| [sky light](../specs/v0.0.6.md) | Light propagated from the sky through transparent cells. | Current (v0.0.6) | sunlight, when you mean the light value |
| [block light](../specs/v0.0.6.md) | Light emitted by blocks and propagated through the world. | Current (v0.0.6) | torch light |
| [ambient occlusion](../decisions/0011-voxel-light-and-baked-ao.md) | Contact shading from nearby geometry, baked into vertex data. | Current (v0.0.6) | shadows |
| [day/night cycle](../specs/v0.0.6.md) | Ten-minute time-of-day cycle that rotates the sun and changes sky light. | Current (v0.0.6) | weather |
| [fog](../specs/v0.0.6.md) | Distance haze that hides the render-distance edge. | Current (v0.0.6) | — |
| [Lamp](../specs/v0.0.6.md) | Solid block that emits block light 14. | Current (v0.0.6) | light source, when you mean a generic emitter |
| [water](../specs/v0.0.6.md) | Non-solid transparent block with a separate transparent mesh layer. | Current (v0.0.6) | fluid, when you mean this block |

## Input and camera

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [fly camera](controls.md) | Historical free-flight camera controlled by WASD and mouse look. | Historical (v0.0.2-v0.0.3) | player |
| [yaw](coordinates.md#camera-and-direction) | Horizontal camera angle; `0` faces `-Z`, and positive yaw turns toward `+X`. | Current | heading |
| [pitch](coordinates.md#camera-and-direction) | Vertical camera angle; positive looks up and is clamped to ±89°. | Current | elevation |
| [FOV](coordinates.md#camera-and-direction) | Field of view; π/3 vertically. | Current | zoom |
| [view-projection](coordinates.md#camera-and-direction) | Combined view and projection matrix used by the vertex shader. | Current | camera matrix |
| [cursor capture](controls.md#current-capture-and-focus-v004) | Grabbing and hiding the cursor so mouse motion drives the camera. | Current | pointer capture, pointer lock |
| [mouse look](controls.md#current-capture-and-focus-v004) | Rotating the camera from raw mouse motion while captured and focused. | Current | mouse steering |
| [raw mouse motion](controls.md#current-capture-and-focus-v004) | Device motion delta, not the cursor's screen position. | Current | cursor position |
| [physical key code](controls.md#keybindings) | Key position reported by the platform, independent of keyboard layout. | Current | scan code |
| [delta time](architecture.md#frame-flow) | Elapsed time between frames, clamped to `100 ms`. | Current | tick |

## Project and release process

| Term | Meaning | Avoid |
|---|---|---|
| [roadmap](../roadmap.md) | Versioned milestones and their current status. | schedule |
| [milestone](../roadmap.md) | One versioned delivery unit in the roadmap. | sprint |
| [scope page](milestone-template.md) | Page that expands a milestone into deliverables, technical shape, and acceptance criteria. | spec, when you mean a milestone scope |
| [Definition of Done](milestone-template.md) | Observable criteria that prove a milestone is complete. | acceptance checklist, when you mean the criteria |
| [deliberate simplification](milestone-template.md) | Accepted shortcut with a stated condition for revisiting it. | temporary hack |
| [non-goal](../roadmap.md) | Work a milestone explicitly excludes. | out of scope, when you mean a roadmap non-goal |
| [ADR](../decisions/index.md) | Architecture decision record; one decision, alternatives, and consequences. | design doc |
| [changelog](versioning.md#changelog) | User-visible changes grouped by release. | commit log |
| [Unreleased](versioning.md#changelog) | Changelog section for changes not yet released. | pending |
| [SemVer](versioning.md#semantic-versioning) | Semantic Versioning 2.0.0. | version numbers, when you mean the policy |
| [public API](public-api.md) | The exported surface of the `engine` crate; currently `run`, `RunError`, `RendererError`, `SaveError`, and `ChunkCoord`. It is unstable before 1.0. | exported internals |
| [Keep a Changelog](versioning.md#changelog) | Changelog format used by `CHANGELOG.md`. | release notes, when you mean the file format |
| [MSRV](versioning.md#out-of-scope) | Minimum supported Rust version; currently `1.99.1`. | toolchain, when you mean the policy |
| [pre-alpha](../index.md) | Early project status with no stability promise. | beta |
| [release](release-checklist.md) | Versioned tag and matching GitHub release. | deployment |
| [annotated tag](versioning.md#tags-and-release-commits) | Git tag with metadata, created as `vX.Y.Z`. | lightweight tag |
| [source-available](../index.md) | PolyForm Shield licensing; readable and modifiable, but not OSI open source. | open source |
| [CI](ci.md#workflow-map) | GitHub Actions checks for tests, docs, commits, and releases. | build, when you mean the whole workflow |

## Tools

| Term | Meaning | Status |
|---|---|---|
| [Rust](../decisions/0001-rust-for-engine-core.md) | Systems language used for the engine. | Current |
| [Cargo](setup.md) | Rust build tool, package manager, and test runner. | Current |
| [rustup](setup.md) | Toolchain installer that reads `rust-toolchain.toml`. | Current |
| [wgpu](../decisions/0002-wgpu-for-graphics.md) | Cross-platform graphics API abstraction used by the renderer. | Current |
| [winit](../decisions/0003-winit-for-windowing.md) | Windowing and event-loop library. | Current |
| [WGSL](../decisions/0002-wgpu-for-graphics.md) | Shading language used in `src/shader.wgsl`. | Current |
| [glam](../decisions/0004-glam-for-math.md) | Vector, matrix, quaternion, and integer math library. | Current |
| [bytemuck](../decisions/0005-bytemuck-for-gpu-layout.md) | Plain-old-data casting used for GPU buffer layouts. | Current |
| [thiserror](conventions.md#errors) | Derive macro for typed library errors. | Current |
| [tracing](debugging.md#logging) | Structured logging and diagnostics. | Current |
| pollster | Small executor used to block on `wgpu` futures. | Current |
| [Tracy / wgpu-profiler](profiling.md) | CPU spans, memory events, GPU timer zones, and metric plots behind the `tracy` feature. | Current |
| [rayon / micropool](../decisions/0009-micropool-for-chunk-workers.md) | Worker-pool candidates compared by `pool_sim`; micropool is the runtime default, rayon is the optional `rayon-pool` A/B build. | Current (v0.0.5) |
| [mdBook](configuration.md#documentation-and-releases) | Documentation site generator. | Current |
| [Cocogitto](configuration.md#documentation-and-releases) | Conventional Commits and release-tag tooling. | Current |
| [cargo-deny](configuration.md#toolchain-and-build) | Dependency licence, advisory, ban, and source checks. | Current |
| [typos](configuration.md#formatting-and-spelling) | Spell checker used in CI. | Current |
| [Clippy / rustfmt](configuration.md#formatting-and-spelling) | Lints and formatting enforced by CI. | Current |
| [Dependabot](ci.md#dependency-updates) | Weekly grouped Cargo and Actions updates. | Current |
| [GitHub Actions](ci.md#workflow-map) | CI, docs, commit, and release workflows. | Current |
| [RenderDoc / Nsight / PIX](debugging.md#external-tools) | External frame debuggers for pipeline and shader inspection. | External |
