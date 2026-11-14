# Valescent Engine Roadmap

> Status legend: [ ] not started · [~] in progress · [x] done

| Version | Name                  | Status |
|---------|-----------------------|--------|
| 0.0.1   | The Foundation        | [x]    |
| 0.0.2   | Chunks and Meshes     | [ ]    |
| 0.0.3   | Playable              | [ ]    |
| 0.0.4   | Scale and Performance | [ ]    |
| 0.0.5   | Atmosphere            | [ ]    |
| 0.0.6   | Advanced Horizons     | [ ]    |

---

## v0.0.1: The Foundation

**Status:** [x]
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

---

## v0.0.2: Chunks and Meshes

**Status:** [ ]
**Goal:** Replace per-cube draw calls with per-chunk meshes.
**Summary:** Drawing each cube individually bottlenecks CPU and GPU. This is the core architectural shift of a voxel engine.

### Milestones

#### 1. Single world mesh
- [ ] Generate one vertex buffer for all visible faces
- [ ] Upload once, render in a single draw call

#### 2. Chunks
- [ ] Fixed-size chunks (e.g. 32×32×32)
- [ ] Each chunk owns its voxels and its mesh
- [ ] Block change rebuilds only the affected chunk's mesh

#### 3. Face culling
- [ ] Only emit faces exposed to air
- [ ] No faces between two solid blocks

### Definition of Done
- World made of multiple chunks
- Block change triggers only a local mesh rebuild
- Stable frame rate while moving around

### Non-Goals
- Greedy meshing, LOD, async meshing
- Save/load

---

## v0.0.3: Playable

**Status:** [ ]
**Goal:** Walk around, break and place blocks, persist the world.
**Summary:** A world you can move through and interact with feels like a real engine.

### Milestones

#### 1. Player controls and collision
- [ ] First-person camera controller
- [ ] Swept AABB collision against voxel terrain

#### 2. Block interaction
- [ ] Raycast (DDA voxel traversal) to find targeted block
- [ ] Highlight targeted block
- [ ] Place and destroy blocks

#### 3. Persistence
- [ ] Serialize chunks to disk
- [ ] Load chunks from disk

#### 4. Texture atlas
- [ ] Single atlas for all block textures
- [ ] Batch different block types in one chunk mesh

### Definition of Done
- Walk, break, and place blocks
- World persists across engine restarts

### Non-Goals
- Infinite terrain streaming (save/load is its prerequisite only)
- Lighting, LOD

---

## v0.0.4: Scale and Performance

**Status:** [ ]
**Goal:** Large view distances at a smooth frame rate.
**Summary:** Move from toy to real engine.

### Milestones

#### 1. Greedy meshing
- [ ] Merge adjacent coplanar same-type faces into larger quads
- [ ] Measure vertex count before/after (target: large reduction vs naive)

#### 2. Level of Detail
- [ ] Distance-based coarser meshes for far chunks

#### 3. Frustum culling
- [ ] Skip chunks outside the camera frustum

#### 4. Async meshing
- [ ] Chunk generation and meshing on worker threads
- [ ] Main thread never blocks on meshing

### Definition of Done
- Vast draw distance with a smooth frame rate
- Breaking blocks causes no frame hitches

### Non-Goals
- Lighting, water, procedural generation

---

## v0.0.5: Atmosphere

**Status:** [ ]
**Goal:** Make the world look good.
**Summary:** Lighting, fog, and transparency.

### Milestones

#### 1. Voxel lighting
- [ ] Flood-fill (BFS) propagation for sky light and block light
- [ ] Bake light values / ambient occlusion into vertex data

#### 2. Shader lighting and fog
- [ ] Dynamic directional light (sun/moon)
- [ ] Distance fog to hide render-distance edge

#### 3. Water and transparency
- [ ] Separate mesh layer for transparent blocks
- [ ] Water shader (animated normals; reflections/refractions optional)

### Definition of Done
- Day/night cycle
- Ambient occlusion or shadows
- Visually distinct water

### Non-Goals
- Procedural generation, SVDAGs, raymarching

---

## v0.0.6: Advanced Horizons

**Status:** [ ]
**Goal:** Turn the engine into a game-ready, procedurally generated world.
**Summary:** Optional cutting-edge techniques.

### Milestones

#### 1. Procedural generation
- [ ] Noise functions (Perlin / Simplex / OpenSimplex)
- [ ] Terrain, caves, biomes, structures

#### 2. Advanced rendering (optional)
- [ ] Sparse Voxel DAGs for compression
- [ ] GPU compute raymarching without traditional meshes

### Definition of Done
- Fully procedural world that runs smoothly and looks good

### Non-Goals
- None; this is the stretch phase

---
