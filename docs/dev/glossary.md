# Glossary

Preferred terms, short definitions, and links to the page that owns each topic. **Status** is
current for implemented behaviour, planned with a milestone for scoped work, optional for stretch
work, or deferred when no decision exists. **Avoid** lists terms that are ambiguous or mean
something else; it does not ban the word everywhere.

## World and coordinates

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [voxel](coordinates.md#voxel-coordinates) | Smallest addressable world cell, identified by an integer `IVec3` and occupying a unit cube. | Current | block, when you mean the cell |
| [block](v0.0.4-scope.md) | The type or contents stored in a voxel, such as Air, Stone, Dirt, Grass, Wood, or Leaves. | Planned (v0.0.4) | voxel, when you mean the type |
| [world](architecture.md#coordinates-and-data) | All stored voxels and their state; chunks are the storage unit since v0.0.3. | Current | map, level |
| [chunk](v0.0.3-scope.md) | Fixed 32×32×32 region of voxels. | Current (v0.0.3) | section |
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
| [shader](shaders.md) | WGSL program that runs on the GPU; the current module is `src/shader.wgsl`. | Current | material, when you mean the program |
| [WGSL](shaders.md) | WebGPU Shading Language, the shader language used by wgpu. | Current | GLSL or HLSL, when you mean WGSL |
| [chunk mesh](v0.0.3-scope.md) | Mesh for one chunk, built in chunk-local space. | Current (v0.0.3) | world mesh |
| [face culling](v0.0.3-scope.md) | Omitting a voxel face when the neighbouring voxel is solid. | Current (v0.0.3) | backface culling |
| [backface culling](architecture.md#rendering-resources) | GPU test that discards triangles facing away from the camera; currently disabled. | Current | face culling |
| [dirty chunk](v0.0.3-scope.md) | Chunk whose mesh must be rebuilt after an edit. | Current (v0.0.3) | stale chunk |
| [rebuild](v0.0.3-scope.md) | Regenerate and upload a dirty chunk's mesh. | Current (v0.0.3) | remesh, when you mean the whole edit path |
| [greedy meshing](../specs/v0.0.5.md) | Merging adjacent coplanar same-type faces into larger quads. | Planned (v0.0.5) | merging, when it is ambiguous |
| [LOD](../decisions/0010-two-level-lod-for-chunks.md) | Level of detail; LOD0 full resolution and LOD1 conservative 2×2×2 for distant chunks. | Planned (v0.0.5) | mipmap |
| [frustum culling](../specs/v0.0.5.md) | Skipping chunks outside the camera's view frustum. | Planned (v0.0.5) | occlusion culling |
| [occlusion culling](../specs/v0.0.5.md) | Skipping chunks hidden behind other geometry. | Deferred | frustum culling |
| [async meshing](../specs/v0.0.5.md) | Building meshes on worker threads instead of the main thread. | Planned (v0.0.5) | parallel rendering |
| [worker pool](../decisions/0009-micropool-for-chunk-workers.md) | Threads that run meshing and generation jobs; micropool is selected. | Planned (v0.0.5) | thread pool, when you mean this engine component |
| [revision](../specs/v0.0.5.md) | Version number for a chunk mesh or job; results from older revisions are discarded. | Planned (v0.0.5) | version, when you mean the package version |
| [stress scene](../specs/v0.0.5.md) | Exact 64-chunk in-memory fixture selected by `ENGINE_STRESS_SCENE=1`. | Planned (v0.0.5) | benchmark, when you mean the fixture |
| [upload budget](../specs/v0.0.5.md) | Maximum completed chunk meshes uploaded per frame; fixed at two. | Planned (v0.0.5) | frame budget |
| [draw call](v0.0.3-scope.md) | One draw submission to the GPU. v0.0.3 issues one per non-empty chunk. | Current | pass |
| [frame time](debugging.md#logging) | Time to produce one frame, logged as `frame_ms`. | Current | FPS |
| [FPS](debugging.md#logging) | Frames per second; the reciprocal of frame time. | Current | frame time |

## Gameplay and interaction

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [player](v0.0.4-scope.md) | Walking controller with an AABB, gravity, jumping, and an eye position. | Planned (v0.0.4) | camera, when you mean the player |
| [AABB](v0.0.4-scope.md) | Axis-aligned bounding box. The player is `0.6 m` wide and `1.8 m` tall. | Planned (v0.0.4) | hitbox |
| [swept AABB](v0.0.4-scope.md) | Collision test that moves an AABB along one axis and stops at the first solid block. | Planned (v0.0.4) | raycast |
| [collision](v0.0.4-scope.md) | Resolving player movement against solid voxel AABBs. | Planned (v0.0.4) | physics |
| [raycast](v0.0.4-scope.md) | Casting a ray from the eye to find the first solid voxel and entered face. | Planned (v0.0.4) | raymarch |
| [DDA](v0.0.4-scope.md) | Digital differential analyzer; grid traversal used for voxel raycasts. | Planned (v0.0.4) | raymarching |
| [targeted block](v0.0.4-scope.md) | Solid voxel hit by the view ray and outlined before interaction. | Planned (v0.0.4) | selected block, when you mean the hotbar |
| [highlight](v0.0.4-scope.md) | Wireframe cube drawn around the targeted block. | Planned (v0.0.4) | selection box |
| [hotbar](v0.0.4-scope.md) | Selected block slot; number keys `1`–`5` choose it. There is no inventory UI. | Planned (v0.0.4) | inventory |
| [texture array](../decisions/0007-texture-array-for-block-textures.md) | GPU `texture_2d_array` with one layer per block tile, used instead of a flat atlas. | Planned (v0.0.4) | atlas |
| [tile](textures.md) | One 16x16 RGBA8 block texture stored in a texture-array layer. | Planned (v0.0.4) | atlas tile |
| [texture pack](textures.md) | External set of block tiles; deferred, and must preserve layer indices when introduced. | Deferred | asset pack |
| [Sand](../specs/v0.0.7.md) | Solid desert surface block with ID 8 and texture layer 10. | Planned (v0.0.7) | dirt, when you mean sand |

## Persistence

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [persistence](v0.0.4-scope.md) | Saving and loading chunks so edits survive a restart. | Planned (v0.0.4) | storage, when it is ambiguous |
| [modified chunk](../specs/v0.0.7.md) | Chunk changed by the player and therefore saved; unmodified chunks regenerate from the seed. | Planned (v0.0.7) | dirty chunk, when you mean light/mesh dirty |
| [render distance](../decisions/0013-player-centred-chunk-streaming.md) | Horizontal streaming radius in chunks; configured as `world.render_distance_chunks`. | Planned (v0.0.7) | view distance, when you mean fog |
| [`VCHK`](../decisions/0008-chunk-save-format.md) | Chunk save-file magic; v0.0.7 adds v3 and loads v1–v3. | Planned (v0.0.7) | save file, when you mean the format |
| [format version](versioning.md#out-of-scope) | Version field inside a save format such as `VCHK`, separate from the package version. | Planned (v0.0.4) | package version |

## Lighting and generation

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [seed](../specs/v0.0.7.md) | Deterministic input to world generation. | Planned (v0.0.7) | random, when you mean reproducible |
| [noise](../decisions/0012-fastnoise-lite-and-opensimplex2.md) | Deterministic terrain fields; fastnoise-lite 1.1.1 with OpenSimplex2. | Planned (v0.0.7) | random |
| [biome](../specs/v0.0.7.md) | Terrain and climate region; plains, hills, or desert. | Planned (v0.0.7) | zone |
| [streaming](../decisions/0013-player-centred-chunk-streaming.md) | Loading and unloading chunks around the player as they move. | Planned (v0.0.7) | generation |
| [hysteresis](../decisions/0013-player-centred-chunk-streaming.md) | Extra margin before unloading a chunk, to avoid load/unload thrashing. | Planned (v0.0.7) | delay |
| [SVDAG](../specs/v0.0.7.md) | Sparse voxel directed acyclic graph; optional compression for far or static chunks. | Optional (v0.0.7) | — |
| [raymarching](../specs/v0.0.7.md) | Rendering by marching rays per pixel instead of drawing meshes. | Optional (v0.0.7) | raycasting |
| [sky light](../specs/v0.0.6.md) | Light propagated from the sky through transparent cells. | Planned (v0.0.6) | sunlight, when you mean the light value |
| [block light](../specs/v0.0.6.md) | Light emitted by blocks and propagated through the world. | Planned (v0.0.6) | torch light |
| [ambient occlusion](../decisions/0011-voxel-light-and-baked-ao.md) | Contact shading from nearby geometry, baked into vertex data. | Planned (v0.0.6) | shadows |
| [day/night cycle](../specs/v0.0.6.md) | Ten-minute time-of-day cycle that rotates the sun and changes sky light. | Planned (v0.0.6) | weather |
| [fog](../specs/v0.0.6.md) | Distance haze that hides the render-distance edge. | Planned (v0.0.6) | — |
| [Lamp](../specs/v0.0.6.md) | Solid block that emits block light 14. | Planned (v0.0.6) | light source, when you mean a generic emitter |
| [water](../specs/v0.0.6.md) | Non-solid transparent block with a separate transparent mesh layer. | Planned (v0.0.6) | fluid, when you mean this block |

## Input and camera

| Term | Meaning | Status | Avoid |
|---|---|---|---|
| [fly camera](controls.md) | Current free-flight camera controlled by WASD and mouse look. | Current | player |
| [yaw](coordinates.md#camera-and-direction) | Horizontal camera angle; `0` faces `-Z`, and positive yaw turns toward `+X`. | Current | heading |
| [pitch](coordinates.md#camera-and-direction) | Vertical camera angle; positive looks up and is clamped to ±89°. | Current | elevation |
| [FOV](coordinates.md#camera-and-direction) | Field of view; π/3 vertically. | Current | zoom |
| [view-projection](coordinates.md#camera-and-direction) | Combined view and projection matrix used by the vertex shader. | Current | camera matrix |
| [cursor capture](controls.md#current-capture-and-focus-v002) | Grabbing and hiding the cursor so mouse motion drives the camera. | Current | pointer capture, pointer lock |
| [mouse look](controls.md#current-capture-and-focus-v002) | Rotating the camera from raw mouse motion while captured and focused. | Current | mouse steering |
| [raw mouse motion](controls.md#current-capture-and-focus-v002) | Device motion delta, not the cursor's screen position. | Current | cursor position |
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
| [public API](public-api.md) | The exported surface of the `engine` crate; currently `run`, `RunError`, and `RendererError`. It is unstable before 1.0. | exported internals |
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
| [Tracy / wgpu-profiler](profiling.md) | Planned CPU and GPU profiler integration. | Planned (ADR 0006), not integrated |
| [rayon / micropool](../decisions/0009-micropool-for-chunk-workers.md) | Worker-pool candidates compared by `pool_sim`; micropool is selected, rayon remains benchmark-only. | Current dev dependencies; micropool pending runtime use |
| [mdBook](configuration.md#documentation-and-releases) | Documentation site generator. | Current |
| [Cocogitto](configuration.md#documentation-and-releases) | Conventional Commits and release-tag tooling. | Current |
| [cargo-deny](configuration.md#toolchain-and-build) | Dependency licence, advisory, ban, and source checks. | Current |
| [typos](configuration.md#formatting-and-spelling) | Spell checker used in CI. | Current |
| [Clippy / rustfmt](configuration.md#formatting-and-spelling) | Lints and formatting enforced by CI. | Current |
| [Dependabot](ci.md#dependency-updates) | Weekly grouped Cargo and Actions updates. | Current |
| [GitHub Actions](ci.md#workflow-map) | CI, docs, commit, and release workflows. | Current |
| [RenderDoc / Nsight / PIX](debugging.md#external-tools) | External frame debuggers for pipeline and shader inspection. | External |
