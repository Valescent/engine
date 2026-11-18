# Coordinates

Conventions shared by the camera, voxel storage, meshing, and rendering. Sections marked
**planned** come from milestone scopes and are not implemented yet.

| Convention | Status |
|---|---|
| World axes, units, voxel occupancy, camera direction | Current (v0.0.2) |
| Chunk coordinates and chunk-local mesh vertices | Current (v0.0.3) |
| Block targeting and placement | Current (v0.0.4) |
| Large-world precision budget | Deferred |

## Terms

- **World coordinate** — integer `IVec3` identifying one voxel in the world. This is the
  canonical block address.
- **Chunk coordinate** — integer `IVec3` identifying one 32³ chunk.
- **Local coordinate** — integer `IVec3` from `0` to `31` on each axis, relative to one chunk.
- **World space** — floating-point `Vec3` in metres, used by the camera and renderer.
- **Chunk-local space** — floating-point `Vec3` in metres within one chunk.

## World space

- Right-handed, Y-up. `+X` and `+Z` are horizontal; `+Y` is up.
- Use numeric axes only. There are no compass aliases; the camera's default forward is `-Z`.
- One block edge is one metre. World-space positions and mesh vertices are metres.
- The plane `y = 0` is the bottom face of voxel layer `0`.
- World-space positions are `f32` `Vec3`. An integer world coordinate converts with
  `IVec3::as_vec3()`.

## Voxel coordinates

- A voxel coordinate is an integer `IVec3` naming the cell's minimum corner, not its centre.
- Voxel `(x, y, z)` occupies `[x, x + 1] × [y, y + 1] × [z, z + 1]`.
- Its centre is `(x + 0.5, y + 0.5, z + 0.5)`.
- Negative coordinates follow the same rule: `(-1, 0, 0)` occupies
  `[-1, 0] × [0, 1] × [0, 1]`.
- A face normal is an axis-aligned unit vector pointing out of the solid voxel.
- Faces are wound counter-clockwise when viewed from outside the voxel. Backface culling is
  enabled for terrain and water; counter-clockwise remains the front face.
- Adjacent voxels share a face. The shared pair is culled in v0.0.3 and later.

## Chunk coordinates (v0.0.3)

- `CHUNK_SIZE = 32`. Chunks are 32×32×32.
- Chunk coordinate `c` covers world coordinates `c * 32 ..= c * 32 + 31` on each axis.
- The chunk origin is the world coordinate `c * 32`, the minimum corner of its first voxel.
- Missing chunks are air.
- Convert world to chunk and local with Euclidean division, component by component:

  | World | Chunk | Local |
  |---:|---:|---:|
  | -33 | -2 | 31 |
  | -32 | -1 | 0 |
  | -1 | -1 | 31 |
  | 0 | 0 | 0 |
  | 31 | 0 | 31 |
  | 32 | 1 | 0 |

- Use `div_euclid` and `rem_euclid`. Truncating `/` and `%` give the wrong chunk and local
  coordinate for negative worlds.
- Local coordinates are valid only inside their chunk. Do not use them as world keys or in
  `World::is_solid`. On disk they are valid only alongside the chunk coordinate.
- Local coordinate `0` touches the negative neighbour; `31` touches the positive neighbour. An
  edit on either layer can change the neighbour's culled faces, so dirty that neighbour too.
- Meshing resolves face visibility through world coordinates, not local coordinates, so a face
  between solid voxels across a chunk border is culled like any other.

## Mesh space (v0.0.3)

- Chunk meshes are generated in chunk-local space. Vertex positions are in `0..=32` because
  the last voxel's far corner is at local `32`.
- The renderer translates each chunk by its origin when drawing:
  `world_position = chunk_origin + local_position`.
- Chunk meshes do not bake world coordinates into vertices. This keeps mesh coordinates small
  and matches the chunk-local origins recorded in
  [ADR 0004](../decisions/0004-glam-for-math.md).
- The draw path supplies each chunk's origin to the vertex stage. The GPU mechanism is an
  implementation detail, not part of this convention.
- Each non-empty chunk owns a vertex buffer and an index buffer per built LOD, replacing the v0.0.2 world-space buffer.
- Packed vertices store whole-block cell and UV bytes, so a position or UV that is not a whole block
  does not fit the current layout.

## Camera and direction

- The camera position is in world-space metres.
- `yaw = 0` and `pitch = 0` look along `-Z`.
- Positive yaw turns toward `+X`:
  `forward = (sin yaw * cos pitch, sin pitch, -cos yaw * cos pitch)`.
- Positive pitch looks up. Pitch is clamped to ±89° so the up vector never degenerates.
- Angles are radians internally. Yaw wraps to `[-π, π)`.
- The right vector is `(cos yaw, 0, sin yaw)`; at yaw `0` it is `+X`.
- The view matrix is right-handed `look_to_mat4(position, forward, +Y)`.
- Projection is right-handed with a `0..1` depth range
  (`glam::camera::rh::proj::directx`). The field of view is π/3, the near plane is `0.1 m`, and
  the far plane is `(render_distance_chunks + 2) * 32 m` (448 m at the default distance of 12).

## Interaction (v0.0.4)

- Raycast from the eye along the camera forward, with a maximum range of `6 m`.
- A hit identifies the solid voxel by world coordinate and the face normal that was entered.
- Breaking removes the hit voxel. Placing writes to `hit + normal` when that cell is air and
  does not intersect the player AABB.
- The ray uses DDA traversal with X, then Y, then Z tie-breaking. A ray starting inside a solid
  voxel returns no hit.

## Precision and limits

- Integer world coordinates are `i32` per axis. That is the storage limit, not a supported play
  radius.
- World space and mesh vertices are `f32`. Precision degrades far from the origin; the far plane
  follows the render distance and is `448 m` at the default.
- No playable radius or `f32` precision budget is fixed yet.
  [ADR 0004](../decisions/0004-glam-for-math.md) records camera-relative rendering and
  chunk-local origins as the mitigation for large worlds. Camera-relative rendering is not
  implemented or assigned to a milestone.
- Scale only chunk coordinates derived from a valid world coordinate. Scaling arbitrary `IVec3`
  values back to world coordinates can overflow `i32`.
