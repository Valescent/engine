# Roadmap

Milestones move from **planned** to **in progress** to **shipped**. Each one names what it delivers, how it is verified, and what it deliberately leaves out.

| Version | Name                  | Status   |
|---------|-----------------------|----------|
| 0.0.1   | The Foundation        | Shipped  |
| 0.0.2   | Camera                | Shipped  |
| 0.0.3   | Chunks and Meshes     | Shipped  |
| 0.0.4   | Playable              | Shipped  |
| 0.0.5   | Scale and Performance | Shipped  |
| 0.0.6   | Atmosphere            | Shipped  |
| 0.0.7   | Advanced Horizons     | Shipped  |
| 0.0.8   | Scale                 | In progress |
| 0.0.9   | Far View              | In progress |

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

**Status:** In progress - implementation and acceptance complete; release PR open.

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
- [x] 2 km gate: 0.466 ms frame, 0.235 ms GPU, 3.58 M vertices, 2 draws, 30.94 MB allocated
- [x] 8 km gate: 0.577 ms frame, 0.323 ms GPU, 5.15 M vertices, 1,259 visible nodes, 2 draws, 74.62 MB allocated, 94.24 MB reservation; the section horizon removes 646 of 1,330 in-frustum sections
- [x] 325 tests pass (ignored benchmarks run separately); fmt, clippy, doc, typos, and mdbook pass

#### 5. Acceptance and release
- [ ] Manual definition-of-done run
- [ ] `Cargo.toml` version bump, release split, changelog, and tag
- [ ] Pull request

### Definition of Done
- 60 FPS at distance 32 with a working set at or below 350 MB and geometry VRAM at or below 40 MB
- 60 FPS at a 2 km view at or below 400 MB, and at an 8 km view at or below 500 MB
- Draw calls at or below 10 at distance 32 and at or below 20 at a 2 km view
- Main-thread frame time at or below 4 ms at distance 32, with no frame above 33 ms once ready
- Headless counts unchanged for the same world, proving storage packing is not a content change

**Measured:** the distance-32, 2 km, and 8 km gates are recorded above. The manual run, release
decision, and version bump remain.

### Non-Goals
- The compute mesher, GPU-resident voxel and light pools, and readback-driven allocation
- Hi-Z, GPU traversal, and the voxel visibility graph
- Shadows, sub-voxels, and console backends
- Save-format changes
