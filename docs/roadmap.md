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
| 0.0.7   | Advanced Horizons     | In progress |

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

**Status:** In progress - deterministic terrain, biomes, caves, and sea level are implemented.

**Goal:** Turn the engine into a game-ready, procedurally generated world.

**Summary:** Optional cutting-edge techniques.

See [v0.0.7 scope](dev/v0.0.7-scope.md) for the concrete deliverables, technical shape, and acceptance checklist.

### Milestones

#### 1. Procedural generation
- [x] Noise functions (Perlin / Simplex / OpenSimplex)
- [x] Terrain, caves, biomes, structures

#### 2. Advanced rendering (optional)
- [ ] Sparse Voxel DAGs for compression
- [ ] GPU compute raymarching without traditional meshes

### Definition of Done
- Fully procedural world that runs smoothly and looks good

### Non-Goals
- None; this is the stretch phase
